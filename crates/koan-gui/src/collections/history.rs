//! What was played, newest first, under a heading per day: a list of plays,
//! not of tracks, so a record played three times is three runs of rows. The
//! day is the only grouping, since that is how people reach for it ("what
//! was that on Tuesday").
//!
//! Plays can be forgotten, picked ones or all of them, here and on every
//! device of the account; the tracks are untouched. Nothing reorders.

use std::ops::Range;
use std::rc::Rc;

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Placement, PlayHistoryEntry};

use super::{Confirm, Picks, confirm, count, listed};
use crate::components::{KoanContextMenu, MenuEntry};
use crate::library::covers::{Art, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::{Playable, PlayableDrag};
use crate::library::nav::Page;
use crate::library::rows::{self, RowPart, RowState, TrackList};
use crate::library::tokens::{Paint, Type, Typeset};
use crate::library::{Dismiss, Lib, SelectAll, actions, chrome, tile};
use crate::mirror::Mirror;
use crate::platform::actions::Delete;
use crate::theme::{Icon, Space};

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Day(SharedString),
    Play(usize),
}

fn local(at: i64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(at, 0).single()
}

/// A day's heading: today and yesterday by name, other days by date.
pub fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    match (today - day).num_days() {
        0 => "today".into(),
        1 => "yesterday".into(),
        _ => day.format("%A %-d %B %Y").to_string(),
    }
}

/// The plays under a heading per run of one day. The list is newest first,
/// so one pass finds the runs.
pub fn cut(days: &[Option<NaiveDate>], today: NaiveDate) -> Vec<Item> {
    let mut items = Vec::with_capacity(days.len() + days.len() / 16);
    let mut last: Option<Option<NaiveDate>> = None;
    for (ix, day) in days.iter().enumerate() {
        if last != Some(*day) {
            let label = day.map_or_else(|| "undated".to_owned(), |d| day_label(d, today));
            items.push(Item::Day(label.into()));
            last = Some(*day);
        }
        items.push(Item::Play(ix));
    }
    items
}

pub struct HistoryPage {
    lib: Lib,
    plays: Rc<Vec<PlayHistoryEntry>>,
    /// Each play's time of day, made once per read.
    times: Rc<Vec<SharedString>>,
    order: Rc<Vec<i64>>,
    items: Rc<Vec<Item>>,
    loaded: Option<(u64, u64)>,
    request: u64,
    _load: Option<Task<()>>,
    list: ListState,
    visible: Range<usize>,
    picks: Picks,
    menu: Option<actions::MenuTarget>,
    menu_plays: Vec<i64>,
    focus: FocusHandle,
    history: Entity<crate::mirror::Slice<u64>>,
    rewinds: u64,
    _subscriptions: Vec<Subscription>,
}

impl HistoryPage {
    pub fn new(lib: Lib, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.entity_id();
        let history = cx.global::<Mirror>().history.clone();
        let subscriptions = vec![
            crate::theme::observe_accent(cx),
            cx.observe(&lib.feed, |this, _, cx| {
                this.load(cx);
                cx.notify();
            }),
            cx.observe(&history, |this, _, cx| this.load(cx)),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.observe(&lib.nav, |this, nav, cx| {
                let rewinds = nav.read(cx).rewinds(Page::History, cx);
                if rewinds != this.rewinds {
                    this.rewinds = rewinds;
                    this.list.scroll_to(ListOffset::default());
                    cx.notify();
                }
            }),
            cx.on_release(move |this, cx| {
                let top = this.list.logical_scroll_top();
                this.lib.nav.update(cx, |nav, _| {
                    nav.remember_offset(Page::History, top.offset_in_item)
                });
                this.lib.covers.update(cx, |c, _| c.release(owner));
            }),
        ];
        let list = ListState::new(0, ListAlignment::Top, px(400.0));
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |event: &ListScrollEvent, _, cx| {
            weak.update(cx, |this, _| this.visible = event.visible_range.clone())
                .ok();
        });
        let rewinds = lib.nav.read(cx).rewinds(Page::History, cx);
        let mut this = Self {
            lib,
            plays: Rc::default(),
            times: Rc::default(),
            order: Rc::default(),
            items: Rc::default(),
            loaded: None,
            request: 0,
            _load: None,
            list,
            visible: 0..0,
            picks: Picks::default(),
            menu: None,
            menu_plays: Vec::new(),
            focus: cx.focus_handle(),
            history,
            rewinds,
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    /// Reads the plays again when one is recorded or forgotten, or the
    /// library lost a track.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.lib.feed.read(cx).library_version else {
            return;
        };
        let wanted = (library, self.history.read(cx).0);
        if self.loaded == Some(wanted) {
            return;
        }
        self.loaded = Some(wanted);
        self.request += 1;
        let request = self.request;
        let engine = self.lib.engine.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let plays = match engine.play_history(None).await {
                Ok(plays) => plays,
                Err(e) => {
                    log::warn!("history: {e}");
                    return;
                }
            };
            // Dates are worked out here, off the frame.
            let cut = cx
                .background_executor()
                .spawn(async move {
                    let today = Local::now().date_naive();
                    let at: Vec<Option<DateTime<Local>>> =
                        plays.iter().map(|p| local(p.played_at)).collect();
                    let days: Vec<Option<NaiveDate>> =
                        at.iter().map(|t| t.map(|t| t.date_naive())).collect();
                    let times: Vec<SharedString> = at
                        .iter()
                        .map(|t| {
                            t.map(|t| t.format("%H:%M").to_string())
                                .unwrap_or_default()
                                .into()
                        })
                        .collect();
                    let items = cut(&days, today);
                    (plays, times, items)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                let (plays, times, items) = cut;
                let first = this.items.is_empty();
                let top = this.list.logical_scroll_top();
                this.order = Rc::new(plays.iter().map(|p| p.id).collect());
                this.picks.retain(&this.order);
                this.plays = Rc::new(plays);
                this.times = Rc::new(times);
                this.items = Rc::new(items);
                this.list.reset(this.items.len());
                let landing = if first {
                    ListOffset {
                        item_ix: 0,
                        offset_in_item: this
                            .lib
                            .nav
                            .read(cx)
                            .offset(Page::History)
                            .unwrap_or_default(),
                    }
                } else {
                    top
                };
                this.list.scroll_to(landing);
                cx.notify();
            })
            .ok();
        }));
    }

    fn ask_forget(&self, ids: Vec<i64>, window: &mut Window, cx: &mut App) {
        if ids.is_empty() {
            return;
        }
        let wanted: std::collections::HashSet<i64> = ids.iter().copied().collect();
        let names = self
            .plays
            .iter()
            .zip(self.times.iter())
            .filter(|(p, _)| wanted.contains(&p.id))
            .map(|(p, t)| format!("{t} {} — {}", p.track.title, p.track.artist_name));
        let engine = self.lib.engine.clone();
        confirm(
            Confirm {
                title: format!("forget {}", count(ids.len(), "play", "plays")).into(),
                verb: "forget".into(),
                destructive: true,
                lines: listed(names, 8),
                note: Some(
                    "Forgotten here and on every device of the account. The tracks stay.".into(),
                ),
            },
            window,
            cx,
            move |cx| {
                let (engine, ids) = (engine.clone(), ids.clone());
                cx.spawn(async move |_| {
                    if let Err(e) = engine.delete_plays(ids).await {
                        log::warn!("forget plays: {e}");
                    }
                })
                .detach();
            },
        );
    }

    fn ask_clear(&self, window: &mut Window, cx: &mut App) {
        let engine = self.lib.engine.clone();
        confirm(
            Confirm {
                title: "clear history".into(),
                verb: "clear".into(),
                destructive: true,
                lines: vec![
                    format!(
                        "Every one of the {} is forgotten, here and on every device of the account.",
                        count(self.plays.len(), "play", "plays")
                    )
                    .into(),
                ],
                note: Some("The tracks stay. This cannot be undone.".into()),
            },
            window,
            cx,
            move |cx| {
                let engine = engine.clone();
                cx.spawn(async move |_| {
                    if let Err(e) = engine.clear_play_history().await {
                        log::warn!("clear history: {e}");
                    }
                })
                .detach();
            },
        );
    }

    fn tracks_of(&self, ids: &[i64]) -> Vec<Playable> {
        let wanted: std::collections::HashSet<i64> = ids.iter().copied().collect();
        self.plays
            .iter()
            .filter(|p| wanted.contains(&p.id))
            .map(|p| Playable::Track(p.track.id))
            .collect()
    }

    fn aim(&mut self, scale: f32, cx: &mut Context<Self>) {
        let visible = self.visible.clone();
        let reach = visible.len().max(10);
        let from = visible.start.saturating_sub(reach);
        let to = (visible.end + reach).min(self.items.len());
        let ranked: Vec<(Key, Priority)> = (from..to)
            .filter_map(|i| {
                let Item::Play(ix) = self.items[i] else {
                    return None;
                };
                let album = self.plays.get(ix)?.track.album_id?;
                let priority = if visible.contains(&i) {
                    Priority::Visible
                } else {
                    Priority::Ahead(i.abs_diff(visible.start).min(i.abs_diff(visible.end)) as f32)
                };
                Some((
                    Key::new(Art::Album(album), px(rows::SLEEVE), scale),
                    priority,
                ))
            })
            .collect();
        let owner = cx.entity_id();
        self.lib
            .covers
            .update(cx, |covers, _| covers.request(owner, ranked));
    }

    fn item(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = &Paint::of(window, cx);
        match self.items.get(i).cloned() {
            Some(Item::Day(label)) => div()
                .px(px(tile::MARGIN))
                .pt(p.space(Space::L))
                .pb(p.space(Space::S))
                .typeset(p, Type::Fine, p.ink)
                .child(label)
                .into_any_element(),
            Some(Item::Play(ix)) => self.play_row(p, i, ix, window, cx),
            None => div().into_any_element(),
        }
    }

    fn play_row(
        &mut self,
        p: &Paint,
        item: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(play) = self.plays.get(ix) else {
            return div().into_any_element();
        };
        let (id, track) = (play.id, play.track.clone());
        let elsewhere = play.source != "local";
        let acting = self.picks.acting_on(id, &self.order);
        let tracks = self.tracks_of(&acting);
        let label: SharedString = if tracks.len() == 1 {
            track.title.clone().into()
        } else {
            count(tracks.len(), "track", "tracks").into()
        };
        let sleeve = track.album_id.map(|album| {
            let key = Key::new(Art::Album(album), px(rows::SLEEVE), window.scale_factor());
            self.lib.covers.update(cx, |c, _| c.sleeve(key))
        });
        let feed = self.lib.feed.read(cx);
        let state = RowState {
            lead: self.times[ix].clone(),
            lead_width: time_width(p, window),
            current: false,
            playing: false,
            favourite: feed.is_favourite(Playable::Track(track.id)),
            selected: self.picks.contains(id),
            gathered: true,
            transfer: feed.transfers.get(&track.id).copied(),
            drag: PlayableDrag::new(&tracks, label),
            height: rows::art_height(p),
        };
        let row = rows::track_row(p, &track, item, state, sleeve, None, cx);
        div()
            .id(("play", id as u64))
            .relative()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.menu_plays = acting.clone()),
            )
            .child(row)
            // A play another client scrobbled in did not happen here.
            .when(elsewhere, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(4.0))
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .child(
                            crate::theme::KoanIcon::new(Icon::Renderer)
                                .role(Type::Fine)
                                .color(p.muted),
                        ),
                )
            })
            .into_any_element()
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let order = self.order.clone();
        self.picks.all(&order);
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.picks.clear();
        cx.notify();
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        let picked = self.picks.in_order(&self.order);
        self.ask_forget(picked, window, cx);
    }
}

/// The lead column, as wide as a time of day in the face it is drawn in.
fn time_width(p: &Paint, window: &Window) -> Pixels {
    let text = window.text_system();
    let font = text.resolve_font(&font(crate::theme::FONT_FAMILY));
    let size = p.look.font_size(Type::Meta);
    let digit = text
        .advance(font, size, '0')
        .map(|size| size.width)
        .unwrap_or(size * 0.6);
    digit * 5.0 + px(4.0)
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
    fn clicked(
        &mut self,
        item: usize,
        track: i64,
        part: RowPart,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(Item::Play(ix)) = self.items.get(item).cloned() else {
            return;
        };
        let id = self.plays[ix].id;
        match part {
            RowPart::Row => {
                let order = self.order.clone();
                if self.picks.click(id, event, &order, cx) {
                    actions::queue(
                        &self.lib,
                        &[Playable::Track(track)],
                        Placement::Replace {
                            start_at: None,
                            shuffled: false,
                        },
                        cx,
                    )
                    .detach();
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

impl Render for HistoryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        self.aim(window.scale_factor(), cx);
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        let entity = cx.entity();
        let header = chrome::page_header(
            p,
            "history",
            count(self.plays.len(), "play", "plays"),
            div()
                .flex()
                .items_center()
                .gap(p.space(Space::L))
                .when(!self.picks.is_empty(), |d| {
                    let e = entity.clone();
                    d.child(
                        chrome::text_button(
                            "history-forget",
                            &format!("forget {}", count(self.picks.len(), "play", "plays")),
                        )
                        .on_click(move |_, window, cx| {
                            e.update(cx, |this, cx| {
                                let picked = this.picks.in_order(&this.order);
                                this.ask_forget(picked, window, cx)
                            })
                        }),
                    )
                })
                .when(!self.plays.is_empty(), |d| {
                    let e = entity.clone();
                    d.child(chrome::text_button("history-clear", "clear…").on_click(
                        move |_, window, cx| e.update(cx, |this, cx| this.ask_clear(window, cx)),
                    ))
                }),
        );
        let body = if self.plays.is_empty() && self.loaded.is_some() {
            chrome::empty_state(
                Icon::History,
                "nothing played yet",
                "everything you play lands here, most recent first.",
            )
            .into_any_element()
        } else {
            let view = cx.weak_entity();
            list(self.list.clone(), move |ix, window, cx| {
                view.update(cx, |this, cx| this.item(ix, window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .size_full()
            .into_any_element()
        };
        let weak = cx.weak_entity();
        let lib = self.lib.clone();
        div()
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::delete))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .typeset(p, Type::Body, p.ink)
            .child(header)
            .child(
                div()
                    .id("history-list")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .pt(px(10.0))
                    .capture_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, _, _| {
                        if e.button == MouseButton::Right {
                            this.menu = None;
                            this.menu_plays.clear();
                        }
                    }))
                    .child(body)
                    .child(gpui_kit::base::Scrollbar::vertical(&self.list))
                    .koan_context_menu("history-menu", window, cx, move |m, window, cx| {
                        let Some(view) = weak.upgrade() else { return m };
                        let (target, plays) = {
                            let page = view.read(cx);
                            (page.menu.clone(), page.menu_plays.clone())
                        };
                        let Some(t) = target else { return m };
                        let m = actions::menu(m, &lib, t.items, t.album, None, window, cx);
                        if plays.is_empty() {
                            return m;
                        }
                        m.separator().item(
                            MenuEntry::new("remove from history")
                                .destructive()
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |this, cx| {
                                        this.ask_forget(plays.clone(), window, cx)
                                    })
                                }),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn plays_are_cut_into_days_newest_first() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let yesterday = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let older = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let items = cut(
            &[Some(today), Some(today), Some(yesterday), Some(older)],
            today,
        );
        assert_eq!(
            items,
            vec![
                Item::Day("today".into()),
                Item::Play(0),
                Item::Play(1),
                Item::Day("yesterday".into()),
                Item::Play(2),
                Item::Day("Monday 26 October 2026".into()),
                Item::Play(3),
            ]
        );
    }
}
