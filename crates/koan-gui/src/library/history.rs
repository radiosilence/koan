//! What has been listened to, most recent first, under a heading per day:
//! the Mac's `HistoryView`. A list of plays, not of tracks: a record played
//! three times is three rows, and nothing regroups them. Rows are the
//! library's track rows, the time where a record's page puts the number,
//! and are picked by play, so one play of a track can be forgotten and its
//! others kept. Forgetting and clearing ask first and reach every device on
//! the account through the engine.

use std::collections::HashSet;

use chrono::{Local, TimeZone};
use koan_ffi::PlayHistoryEntry;
use koan_kit::app::*;

use super::covers::{Art, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::rows::{self, Reveal, RowPart, RowState, TrackList};
use super::tokens::Paint;
use super::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::components::sheet::{Confirm, confirm};
use crate::components::{
    EmptyState, KoanContextMenu, Menu, MenuEntry, MenuPanel, MenuRow, Page as PageFrame,
    PageHeader, RowKind, RowList, RowListState, SectionHeading, ToolbarButton, ToolbarGroup,
    ToolbarMenu, ToolbarSelection,
};
use crate::engine::command;
use crate::mirror::Mirror;
use crate::theme::Icon;

/// One line of the list: a day's heading, or a play by its index.
#[derive(Clone)]
enum Line {
    Day(SharedString),
    Play { ix: usize, time: SharedString },
}

/// The plays, newest first, laid out under their days. Off the main thread:
/// a long history is thousands of dates to format.
fn lay_out(entries: &[PlayHistoryEntry], now: i64) -> Vec<Line> {
    let today = Local.timestamp_opt(now, 0).single().map(|d| d.date_naive());
    let mut lines = Vec::with_capacity(entries.len() + entries.len() / 10 + 1);
    let mut last = None;
    for (ix, entry) in entries.iter().enumerate() {
        let Some(at) = Local.timestamp_opt(entry.played_at, 0).single() else {
            continue;
        };
        let day = at.date_naive();
        if last != Some(day) {
            last = Some(day);
            let label = match today.map(|t| (t - day).num_days()) {
                Some(0) => "today".to_owned(),
                Some(1) => "yesterday".to_owned(),
                _ => at.format("%A %-d %B %Y").to_string(),
            };
            lines.push(Line::Day(label.into()));
        }
        lines.push(Line::Play {
            ix,
            time: at.format("%H:%M").to_string().into(),
        });
    }
    lines
}

pub struct HistoryPage {
    lib: Lib,
    entries: Vec<PlayHistoryEntry>,
    lines: Vec<Line>,
    loaded: bool,
    list: RowListState,
    /// Picked plays, by entry id.
    selected: HashSet<i64>,
    /// Where a shift-click extends from: an index into `entries`.
    anchor: Option<usize>,
    /// What the list's context menu acts on, for the library's verbs.
    menu: Option<actions::MenuTarget>,
    /// The plays the last right-click acts on.
    menu_plays: Vec<i64>,
    /// The keyboard's place among the plays.
    cursor: crate::components::ListCursor,
    /// The list holds the keyboard, as of the last frame.
    keyed: bool,
    focus: FocusHandle,
    search: Entity<chrome::SearchField>,
    _loading: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl HistoryPage {
    pub fn new(lib: Lib, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        let history = cx.global::<Mirror>().history.clone();
        let subscriptions = vec![
            crate::theme::observe_accent(cx),
            // A play landing, a forget here or on another device.
            cx.observe(&history, |this, _, cx| this.load(cx)),
            cx.observe(&lib.browse, |this, _, cx| this.load(cx)),
            cx.observe(&lib.feed, |_, _, cx| cx.notify()),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.on_release(move |this, cx| this.lib.covers.update(cx, |c, cx| c.release(id, cx))),
        ];
        let search =
            cx.new(|cx| chrome::SearchField::new(lib.browse.clone(), "filter history", window, cx));
        let mut page = Self {
            lib,
            entries: Vec::new(),
            lines: Vec::new(),
            loaded: false,
            list: RowListState::new(),
            selected: HashSet::new(),
            anchor: None,
            menu: None,
            menu_plays: Vec::new(),
            cursor: Default::default(),
            keyed: false,
            focus: cx.focus_handle(),
            search,
            _loading: None,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page
    }

    /// Reads the history again, narrowed by the shared filter; a newer read
    /// replaces one in flight.
    fn load(&mut self, cx: &mut Context<Self>) {
        let engine = self.lib.engine.clone();
        let search = self.lib.browse.read(cx).search();
        self._loading = Some(cx.spawn(async move |this, cx| {
            let entries = match engine.play_history(search).await {
                Ok(entries) => entries,
                Err(e) => {
                    log::warn!("history: {e}");
                    return;
                }
            };
            let now = chrono::Utc::now().timestamp();
            let (entries, lines) = cx
                .background_spawn(async move {
                    let lines = lay_out(&entries, now);
                    (entries, lines)
                })
                .await;
            this.update(cx, |this, cx| {
                let present: HashSet<i64> = entries.iter().map(|e| e.id).collect();
                this.selected.retain(|id| present.contains(id));
                this.anchor = this.anchor.filter(|a| *a < entries.len());
                let top = this.list.top();
                this.entries = entries;
                this.lines = lines;
                this.loaded = true;
                this.list.reset(this.lines.len(), RowKind::Art);
                this.list.scroll_to(crate::components::RowOffset {
                    item_ix: top.item_ix.min(this.lines.len().saturating_sub(1)),
                    offset_in_item: top.offset_in_item,
                });
                cx.notify();
            })
            .ok();
        }));
    }

    /// The picked plays, newest first.
    fn picked(&self) -> Vec<&PlayHistoryEntry> {
        self.entries
            .iter()
            .filter(|e| self.selected.contains(&e.id))
            .collect()
    }

    /// The plays a row's verbs act on: the pick when the row is in it.
    fn acting_on(&self, ix: usize) -> Vec<&PlayHistoryEntry> {
        let entry = &self.entries[ix];
        if self.selected.contains(&entry.id) {
            self.picked()
        } else {
            vec![entry]
        }
    }

    /// Asks before forgetting `plays`, naming them; the tracks stay.
    fn ask_forget(&self, plays: Vec<(i64, String)>, window: &mut Window, cx: &mut App) {
        if plays.is_empty() {
            return;
        }
        let n = plays.len() as u64;
        let mut lines: Vec<SharedString> = plays
            .iter()
            .take(8)
            .map(|(_, t)| t.clone().into())
            .collect();
        if plays.len() > 8 {
            lines.push(format!("and {} more", plays.len() - 8).into());
        }
        let ids: Vec<i64> = plays.into_iter().map(|(id, _)| id).collect();
        confirm(
            Confirm {
                title: format!("forget {}", chrome::count(n, "play", "plays")).into(),
                verb: "forget".into(),
                destructive: true,
                lines,
                note: Some(
                    "The tracks stay in the library. Signed in to a kōan server, they are forgotten on every device."
                        .into(),
                ),
            },
            window,
            cx,
            move |cx| {
                let ids = ids.clone();
                command(cx, "forget plays", move |e| e.delete_plays(ids));
            },
        );
    }

    fn forget_picked(&mut self, _: &Forget, window: &mut Window, cx: &mut Context<Self>) {
        let plays = described(&self.picked());
        self.ask_forget(plays, window, cx);
    }

    fn ask_clear(&self, window: &mut Window, cx: &mut App) {
        let n = self.entries.len() as u64;
        confirm(
            Confirm {
                title: "clear history".into(),
                verb: "clear".into(),
                destructive: true,
                lines: vec![
                    format!(
                        "Every play is forgotten: {}.",
                        chrome::count(n, "play", "plays")
                    )
                    .into(),
                ],
                note: Some("This cannot be undone.".into()),
            },
            window,
            cx,
            |cx| command(cx, "clear history", |e| e.clear_play_history()),
        );
    }

    /// The covers on screen, for the art cache.
    fn aim(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let top = self.list.top().item_ix;
        let scale = window.scale_factor();
        let ranked: Vec<(Key, Priority)> = self
            .lines
            .iter()
            .enumerate()
            .skip(top.saturating_sub(10))
            .take(60)
            .filter_map(|(i, line)| match line {
                Line::Play { ix, .. } => {
                    let album = self.entries[*ix].track.album_id?;
                    let priority = if i >= top && i < top + 30 {
                        Priority::Visible
                    } else {
                        Priority::Ahead(i.abs_diff(top) as f32)
                    };
                    Some((
                        Key::new(Art::Album(album), Pixels::from(rows::SLEEVE), scale),
                        priority,
                    ))
                }
                Line::Day(_) => None,
            })
            .collect();
        let owner = cx.entity_id();
        self.lib
            .covers
            .update(cx, |covers, _| covers.request(owner, ranked));
    }

    fn line(
        &mut self,
        p: &Paint,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.lines.get(i).cloned() {
            Some(Line::Day(day)) => div()
                .px(crate::components::RowMetrics::page_margin(&p.look))
                .child(SectionHeading::new(day.as_ref()))
                .into_any_element(),
            Some(Line::Play { ix, time }) => {
                let entry = &self.entries[ix];
                let track = entry.track.clone();
                let selected = self.selected.contains(&entry.id);
                let elsewhere = entry.source != "local";
                let item = Playable::Track(track.id);
                let drag = if selected && self.selected.len() > 1 {
                    let picked: Vec<Playable> = self
                        .picked()
                        .iter()
                        .map(|e| Playable::Track(e.track.id))
                        .collect();
                    PlayableDrag::new(
                        &picked,
                        chrome::count(picked.len() as u64, "track", "tracks"),
                    )
                } else {
                    PlayableDrag::new(&[item], track.title.clone()).with_sleeve(track.album_id)
                };
                let sleeve = track.album_id.map(|album| {
                    let key = Key::new(
                        Art::Album(album),
                        Pixels::from(rows::SLEEVE),
                        window.scale_factor(),
                    );
                    self.lib.covers.update(cx, |c, _| c.sleeve(key))
                });
                let feed = self.lib.feed.read(cx);
                let state = RowState {
                    lead: time,
                    lead_width: rows::lead_width(p, 99_999, window),
                    current: false,
                    favourite: feed.is_favourite(item),
                    selected,
                    gathered: true,
                    transfer: feed.transfers.get(&track.id).copied(),
                    drag,
                    kind: RowKind::Art,
                    note: elsewhere.then_some((Icon::Renderer, "Scrobbled by another client")),
                    cursor: self.keyed && self.cursor.at == Some(ix),
                    place: Some((ix, self.entries.len())),
                    reveal: p.a11y.then(|| Reveal::Rows(self.list.clone(), i)),
                };
                let row = rows::track_row(p, &track, ix, state, sleeve, None, cx);
                let plays: Vec<i64> = self.acting_on(ix).iter().map(|e| e.id).collect();
                div()
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, _| this.menu_plays = plays.clone()),
                    )
                    .child(row)
                    .into_any_element()
            }
            None => div().into_any_element(),
        }
    }
}

impl super::keys::KeyedList for HistoryPage {
    fn cursor(&mut self) -> &mut crate::components::ListCursor {
        &mut self.cursor
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn pick(&mut self, from: usize, to: usize, _: &mut Context<Self>) {
        self.selected = self.entries[from..=to].iter().map(|e| e.id).collect();
        self.anchor = Some(from);
    }

    fn toggle(&mut self, ix: usize, _: &mut Context<Self>) {
        let id = self.entries[ix].id;
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
    }

    /// The play's line, past the day headings above it.
    fn reveal(&mut self, ix: usize, _: &mut Context<Self>) {
        let line = self
            .lines
            .iter()
            .position(|l| matches!(l, Line::Play { ix: at, .. } if *at == ix));
        if let Some(line) = line {
            self.list.reveal(line);
        }
    }

    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        actions::play_list(&self.lib, self.row_tracks(), ix, cx);
    }

    fn menu_at(&mut self, ix: Option<usize>, _: &mut Context<Self>) {
        let Some(ix) = ix else {
            self.menu = None;
            self.menu_plays.clear();
            return;
        };
        let (plays, items, album) = {
            let plays = self.acting_on(ix);
            let album = match plays.as_slice() {
                [one] => one.track.album_id,
                _ => None,
            };
            let items: Vec<Playable> = plays.iter().map(|e| Playable::Track(e.track.id)).collect();
            (plays.iter().map(|e| e.id).collect(), items, album)
        };
        self.menu_plays = plays;
        self.menu = Some(actions::MenuTarget {
            items: items.into(),
            album,
            after: None,
        });
    }
}

koan_kit::app::actions!(history, [Forget]);

/// The page's keys: Delete and Backspace forget the picked plays.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Forget, Some("History")),
        KeyBinding::new("delete", Forget, Some("History")),
    ]);
}

/// Plays with how a question names them: title and artist.
fn described(plays: &[&PlayHistoryEntry]) -> Vec<(i64, String)> {
    plays
        .iter()
        .map(|e| (e.id, format!("{} — {}", e.track.title, e.track.artist_name)))
        .collect()
}

impl actions::Menus for HistoryPage {
    fn lib(&self) -> &Lib {
        &self.lib
    }

    fn menu_target(&mut self) -> &mut Option<actions::MenuTarget> {
        &mut self.menu
    }

    fn menu_target_ref(&self) -> &Option<actions::MenuTarget> {
        &self.menu
    }
}

impl TrackList for HistoryPage {
    fn row_tracks(&self) -> Vec<i64> {
        self.entries.iter().map(|e| e.track.id).collect()
    }

    fn clicked(
        &mut self,
        index: usize,
        track: i64,
        part: RowPart,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        match part {
            RowPart::Row => {
                // The keyboard carries on from the row clicked.
                self.cursor.at = Some(index);
                self.cursor.anchor = Some(index);
                let id = self.entries[index].id;
                let m = event.modifiers();
                if event.click_count() >= 2 && !m.shift && !m.secondary(cx) {
                    actions::play_list(&self.lib, self.row_tracks(), index, cx);
                } else if m.shift {
                    let from = self.anchor.unwrap_or(index);
                    self.selected = self.entries[from.min(index)..=from.max(index)]
                        .iter()
                        .map(|e| e.id)
                        .collect();
                } else if m.secondary(cx) {
                    if !self.selected.remove(&id) {
                        self.selected.insert(id);
                    }
                    self.anchor = Some(index);
                } else {
                    self.selected = HashSet::from([id]);
                    self.anchor = Some(index);
                }
                cx.notify();
            }
            RowPart::Artist(id) => self
                .lib
                .nav
                .update(cx, |n, cx| n.open(Page::Artist(id), cx)),
            RowPart::Album(id) => self.lib.nav.update(cx, |n, cx| n.open(Page::Album(id), cx)),
            RowPart::Heart => self
                .lib
                .feed
                .update(cx, |f, cx| f.toggle_favourite(Playable::Track(track), cx)),
        }
    }
}

impl HistoryPage {
    /// The page's controls in the window's toolbar: the pick and forgetting
    /// it, the filter, and clearing everything under ⋯.
    pub fn controls(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        let mut group = ToolbarGroup::new();
        if !self.selected.is_empty() {
            let (clear, forget) = (view.clone(), view.clone());
            group = group
                .child(
                    ToolbarSelection::new(chrome::count(
                        self.selected.len() as u64,
                        "play",
                        "plays",
                    ))
                    .action("history-deselect", "clear", move |_, _, cx| {
                        clear.update(cx, |this, cx| {
                            this.selected.clear();
                            cx.notify();
                        })
                    })
                    .action(
                        "history-forget",
                        "forget",
                        move |_, window, cx| {
                            forget.update(cx, |this, cx| {
                                let plays = described(&this.picked());
                                this.ask_forget(plays, window, cx)
                            })
                        },
                    ),
                )
                .spacer();
        }
        let empty = self.entries.is_empty();
        group
            .child(self.search.clone())
            .child(ToolbarMenu::new(
                "history-more",
                ToolbarButton::new("history-more-trigger", Icon::More, "more"),
                move |dismiss, _, _| {
                    let view = view.clone();
                    MenuPanel::new().child(
                        MenuRow::new("history-clear", "clear history…")
                            .disabled(empty)
                            .on_click(move |_, window, cx| {
                                dismiss(window, cx);
                                view.update(cx, |this, cx| this.ask_clear(window, cx));
                            }),
                    )
                },
            ))
            .into_any_element()
    }
}

impl Render for HistoryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        self.aim(window, cx);
        let narrowed = self.lib.browse.read(cx).search().is_some();
        let header = PageHeader::new("history").detail(chrome::count(
            self.entries.len() as u64,
            "play",
            "plays",
        ));
        let body = if !self.loaded {
            div().into_any_element()
        } else if self.entries.is_empty() {
            EmptyState::new(
                Icon::History,
                if narrowed {
                    "no matches"
                } else {
                    "nothing played yet"
                },
            )
            .detail(if narrowed {
                "Try a different filter."
            } else {
                "Everything you play lands here, most recent first."
            })
            .into_any_element()
        } else {
            let view = cx.entity();
            RowList::new(&self.list, move |i, window, cx| {
                view.update(cx, |this, cx| {
                    let p = Paint::of(window, cx);
                    this.line(&p, i, window, cx)
                })
            })
            .into_any_element()
        };
        let view = cx.entity();
        self.keyed = self.focus.is_focused(window);
        super::keys::keyed(
            div()
                .id("history")
                .key_context("Library History List")
                .track_focus(&self.focus),
            cx,
        )
        .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
            this.selected = this.entries.iter().map(|e| e.id).collect();
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
            this.selected.clear();
            cx.notify();
        }))
        .on_action(cx.listener(Self::forget_picked))
        .size_full()
        .flex()
        .flex_col()
        // One menu for the list, over the row the right-click landed on:
        // cleared as the press arrives, set by the row it reaches.
        .capture_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, _, _| {
            if e.button == MouseButton::Right {
                this.menu = None;
                this.menu_plays.clear();
            }
        }))
        .child(PageFrame::new(header).body(body))
        .koan_context_menu("history-menu", window, cx, move |menu, window, cx| {
            row_menu(menu, &view, window, cx)
        })
    }
}

/// The list's one context menu: the library's verbs for the tracks, then
/// forgetting the plays the right-click landed on.
fn row_menu(
    menu: Menu,
    view: &Entity<HistoryPage>,
    window: &mut Window,
    cx: &mut Context<Menu>,
) -> Menu {
    let (lib, target, plays) = {
        let this = view.read(cx);
        let wanted: HashSet<i64> = this.menu_plays.iter().copied().collect();
        let plays = described(
            &this
                .entries
                .iter()
                .filter(|e| wanted.contains(&e.id))
                .collect::<Vec<_>>(),
        );
        (this.lib.clone(), this.menu.clone(), plays)
    };
    let Some(target) = target else {
        return menu;
    };
    let menu = actions::menu(menu, &lib, target.items, target.album, None, window, cx);
    if plays.is_empty() {
        return menu;
    }
    let n = plays.len();
    let view = view.clone();
    menu.separator().item(
        MenuEntry::new(if n == 1 {
            "remove from history…".to_owned()
        } else {
            format!("remove {n} plays from history…")
        })
        .icon(Icon::Remove)
        .on_click(move |_, window, cx| {
            view.update(cx, |this, cx| this.ask_forget(plays.clone(), window, cx))
        }),
    )
}

impl Focusable for HistoryPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_fall_under_their_days_newest_first() {
        let track = |id| koan_ffi::Track {
            id,
            title: "Song".into(),
            artist_name: "Artist".into(),
            artist_credit: Vec::new(),
            album_artist_name: "Artist".into(),
            album_artist_id: None,
            album_artist_credit: Vec::new(),
            credit_differs: false,
            album_title: "Record".into(),
            album_id: None,
            artist_id: None,
            disc: None,
            track_number: None,
            duration_ms: None,
            codec: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "local".into(),
            on_server: false,
            on_disk: true,
            path: None,
            is_favourite: false,
        };
        let now = Local::now().timestamp();
        let entry = |id, played_at| PlayHistoryEntry {
            id,
            track: track(id),
            played_at,
            listened_ms: None,
            source: "local".into(),
        };
        let day = 86_400;
        let lines = lay_out(
            &[entry(1, now), entry(2, now - 1), entry(3, now - 2 * day)],
            now,
        );
        let shape: Vec<String> = lines
            .iter()
            .map(|l| match l {
                Line::Day(d) => format!("day {d}"),
                Line::Play { ix, .. } => format!("play {ix}"),
            })
            .collect();
        assert_eq!(shape[0], "day today");
        assert_eq!(&shape[1..3], ["play 0", "play 1"]);
        assert!(shape[3].starts_with("day ") && shape[3] != "day yesterday");
        assert_eq!(shape[4], "play 2");
    }
}
