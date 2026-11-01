//! A playlist, laid out as the queue is: a heading over each record's run
//! when grouped, a flat list of tracks with their sleeves when not (the
//! default, since a playlist is a sequence someone chose). The choice is kept
//! per playlist on this machine.
//!
//! Rows are entries, not tracks: a playlist can hold one song twice, and each
//! copy is picked, moved and removed on its own. The playing row is the entry
//! the queue item came from, so only that copy lights.
//!
//! Dragging rows within the list moves them; anything dragged from the
//! library (or a row of another playlist) is added where it lands. A smart
//! playlist or one read from a file keeps its contents and its order.

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Playlist, PlaylistEntry, RunRole};

use super::{Confirm, Picks, confirm, count, listed};
use crate::components::{Button, ButtonKind, KoanContextMenu, MenuEntry};
use crate::library::covers::{Art, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::{Playable, PlayableDrag};
use crate::library::nav::Page;
use crate::library::rows::{self, RowPart, RowState, TrackList};
use crate::library::tokens::{Paint, Type, Typeset};
use crate::library::{Dismiss, Lib, SelectAll, actions, chrome, tile};
use crate::mirror::{Mirror, Playback};
use crate::platform::actions::Delete;
use crate::platform::drop;
use crate::theme::Space;

const MOSAIC: f32 = 132.0;
const HEADING_SLEEVE: f32 = 44.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A record's heading over the `len` entries from `first`.
    Heading { first: usize, len: usize },
    /// One entry; `grouped` when a heading above it names its record.
    Entry { ix: usize, grouped: bool },
}

impl Row {
    pub fn entries(self) -> Range<usize> {
        match self {
            Row::Heading { first, len } => first..first + len,
            Row::Entry { ix, .. } => ix..ix + 1,
        }
    }
}

/// The rows for `runs`, the entries' places among the playlist's records.
/// Flat, every entry is a row of its own.
pub fn build(runs: &[RunRole], grouped: bool) -> Vec<Row> {
    if !grouped {
        return (0..runs.len())
            .map(|ix| Row::Entry { ix, grouped: false })
            .collect();
    }
    let mut rows = Vec::with_capacity(runs.len() + runs.len() / 8);
    let mut ix = 0;
    while ix < runs.len() {
        if runs[ix] == RunRole::Opens {
            let len = 1 + runs[ix + 1..]
                .iter()
                .take_while(|r| **r == RunRole::Continues)
                .count();
            rows.push(Row::Heading { first: ix, len });
            rows.extend((ix..ix + len).map(|ix| Row::Entry { ix, grouped: true }));
            ix += len;
        } else {
            rows.push(Row::Entry { ix, grouped: false });
            ix += 1;
        }
    }
    rows
}

/// The entries in their new order once `moving` is put before row `dest`:
/// before the first entry at or after it that is not itself moving, or at
/// the end. The moving entries keep their order among themselves.
pub fn moved(entries: &[i64], rows: &[Row], dest: usize, moving: &[i64]) -> Vec<i64> {
    let moving_set: HashSet<i64> = moving.iter().copied().collect();
    let anchor = rows
        .get(dest..)
        .unwrap_or_default()
        .iter()
        .flat_map(|r| r.entries())
        .map(|ix| entries[ix])
        .find(|id| !moving_set.contains(id));
    let lifted: Vec<i64> = entries
        .iter()
        .copied()
        .filter(|id| moving_set.contains(id))
        .collect();
    let mut order: Vec<i64> = entries
        .iter()
        .copied()
        .filter(|id| !moving_set.contains(id))
        .collect();
    let at = anchor
        .and_then(|a| order.iter().position(|id| *id == a))
        .unwrap_or(order.len());
    order.splice(at..at, lifted);
    order
}

/// Where something from outside dropped before row `dest` lands, as a
/// position among the entries.
pub fn position(rows: &[Row], dest: usize, len: usize) -> usize {
    rows.get(dest).map_or(len, |r| r.entries().start)
}

pub struct PlaylistPage {
    lib: Lib,
    id: i64,
    playlist: Option<Playlist>,
    entries: Rc<Vec<PlaylistEntry>>,
    /// The entries' ids in order, for picks and moves.
    order: Rc<Vec<i64>>,
    rows: Rc<Vec<Row>>,
    covers: Vec<i64>,
    undo_shuffles: u32,
    loaded: Option<u64>,
    request: u64,
    _load: Option<Task<()>>,
    list: ListState,
    visible: Range<usize>,
    picks: Picks,
    /// What the list's menu acts on: the tracks for the library's verbs, the
    /// entries for removal.
    menu: Option<actions::MenuTarget>,
    menu_entries: Vec<i64>,
    /// The row a drag over the list would land before.
    drop_at: Option<usize>,
    /// What the last export or edit had to say.
    note: Option<SharedString>,
    playback: Entity<Playback>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl PlaylistPage {
    pub fn new(lib: Lib, id: i64, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.entity_id();
        let playback = cx.global::<Mirror>().playback.clone();
        let subscriptions = vec![
            crate::theme::observe_accent(cx),
            cx.observe(&lib.feed, |this, _, cx| {
                this.load(cx);
                cx.notify();
            }),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            // The playing entry lights.
            cx.observe(&playback, |_, _, cx| cx.notify()),
            cx.on_release(move |this, cx| {
                let offset = this.list.logical_scroll_top();
                this.lib.nav.update(cx, |nav, _| {
                    nav.remember_anchor(
                        Page::Playlist(this.id),
                        crate::library::nav::Anchor {
                            at: crate::library::nav::Anchored::Header,
                            offset: offset.offset_in_item,
                            item_ix: offset.item_ix,
                        },
                    )
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
        let mut this = Self {
            lib,
            id,
            playlist: None,
            entries: Rc::default(),
            order: Rc::default(),
            rows: Rc::default(),
            covers: Vec::new(),
            undo_shuffles: 0,
            loaded: None,
            request: 0,
            _load: None,
            list,
            visible: 0..0,
            picks: Picks::default(),
            menu: None,
            menu_entries: Vec::new(),
            drop_at: None,
            note: None,
            playback,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    fn grouped(&self) -> bool {
        self.playlist
            .as_ref()
            .and_then(|p| p.grouped)
            .unwrap_or(false)
    }

    /// Takes edits to what it holds: rules or a file decide a read-only
    /// playlist's.
    fn fillable(&self) -> bool {
        self.playlist.as_ref().is_some_and(|p| !p.readonly)
    }

    /// Reads the playlist again when the library moved: an edit here, a
    /// sync, another device.
    fn load(&mut self, cx: &mut Context<Self>) {
        let version = self.lib.feed.read(cx).library_version;
        if version.is_none() || version == self.loaded {
            return;
        }
        self.loaded = version;
        self.request += 1;
        let request = self.request;
        let (engine, id) = (self.lib.engine.clone(), self.id);
        self._load = Some(cx.spawn(async move |this, cx| {
            let playlist = engine
                .clone()
                .playlists()
                .await
                .ok()
                .and_then(|all| all.into_iter().find(|p| p.id == id));
            let entries = engine.clone().playlist_tracks(id).await;
            let covers = engine
                .clone()
                .playlist_cover_album_ids(id)
                .await
                .unwrap_or_default();
            let undo_shuffles = engine.playlist_undo_shuffles(id).await;
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                let entries = match entries {
                    Ok(entries) => entries,
                    Err(e) => {
                        log::warn!("playlist {id}: {e}");
                        return;
                    }
                };
                let first = this.playlist.is_none();
                this.playlist = playlist;
                this.covers = covers;
                this.undo_shuffles = undo_shuffles;
                this.set_entries(entries);
                if first && let Some(anchor) = this.lib.nav.read(cx).anchor(Page::Playlist(id)) {
                    this.list.scroll_to(ListOffset {
                        item_ix: anchor.item_ix.min(this.rows.len().saturating_sub(1)),
                        offset_in_item: anchor.offset,
                    });
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Lays the rows out again, keeping the row at the top where it was.
    fn set_entries(&mut self, entries: Vec<PlaylistEntry>) {
        let order: Vec<i64> = entries.iter().map(|e| e.entry_id).collect();
        self.picks.retain(&order);
        self.order = Rc::new(order);
        self.entries = Rc::new(entries);
        self.relayout();
    }

    fn relayout(&mut self) {
        let runs: Vec<RunRole> = self.entries.iter().map(|e| e.run).collect();
        let top = self.list.logical_scroll_top();
        self.rows = Rc::new(build(&runs, self.grouped()));
        self.list.reset(self.rows.len());
        self.list.scroll_to(ListOffset {
            item_ix: top.item_ix.min(self.rows.len().saturating_sub(1)),
            offset_in_item: top.offset_in_item,
        });
    }

    fn row_entries(&self, row: usize) -> Vec<i64> {
        self.rows
            .get(row)
            .map(|r| r.entries().map(|ix| self.order[ix]).collect())
            .unwrap_or_default()
    }

    fn tracks_of(&self, entries: &[i64]) -> Vec<i64> {
        let wanted: HashSet<i64> = entries.iter().copied().collect();
        self.entries
            .iter()
            .filter(|e| wanted.contains(&e.entry_id))
            .map(|e| e.track.id)
            .collect()
    }

    /// The payload for dragging `entries` out: their tracks, marked as this
    /// playlist's rows.
    fn drag_of(&self, entries: Vec<i64>) -> PlayableDrag {
        let tracks: Vec<Playable> = self
            .tracks_of(&entries)
            .into_iter()
            .map(Playable::Track)
            .collect();
        let label = match tracks.len() {
            1 => self
                .entries
                .iter()
                .find(|e| e.entry_id == entries[0])
                .map(|e| SharedString::from(e.track.title.clone()))
                .unwrap_or_default(),
            n => count(n, "track", "tracks").into(),
        };
        PlayableDrag::new(&tracks, label).from_playlist(self.id, entries)
    }

    fn play_from(&self, entry: i64, cx: &mut App) {
        let (engine, id) = (self.lib.engine.clone(), self.id);
        cx.spawn(async move |_| {
            if let Err(e) = engine.play_playlist(id, Some(entry), false).await {
                log::warn!("play playlist {id}: {e}");
            }
        })
        .detach();
    }

    fn play(&self, shuffled: bool, cx: &mut App) {
        let (engine, id) = (self.lib.engine.clone(), self.id);
        cx.spawn(async move |_| {
            if let Err(e) = engine.play_playlist(id, None, shuffled).await {
                log::warn!("play playlist {id}: {e}");
            }
        })
        .detach();
    }

    /// Asks before taking `entries` out, naming them.
    fn ask_remove(&self, entries: Vec<i64>, window: &mut Window, cx: &mut App) {
        if entries.is_empty() || !self.fillable() {
            return;
        }
        let wanted: HashSet<i64> = entries.iter().copied().collect();
        let names = self
            .entries
            .iter()
            .filter(|e| wanted.contains(&e.entry_id))
            .map(|e| format!("{} — {}", e.track.title, e.track.artist_name));
        let name = self.name();
        let (engine, id) = (self.lib.engine.clone(), self.id);
        confirm(
            Confirm {
                title: format!("remove {}", count(entries.len(), "track", "tracks")).into(),
                verb: "remove".into(),
                destructive: true,
                lines: std::iter::once(SharedString::from(format!("from “{name}”:")))
                    .chain(listed(names, 8))
                    .collect(),
                note: Some("The tracks stay in the library. Undo puts them back.".into()),
            },
            window,
            cx,
            move |cx| {
                let (engine, entries) = (engine.clone(), entries.clone());
                cx.spawn(async move |_| {
                    if let Err(e) = engine.remove_from_playlist(id, entries).await {
                        log::warn!("remove from playlist {id}: {e}");
                    }
                })
                .detach();
            },
        );
    }

    fn name(&self) -> String {
        self.playlist
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }

    fn ask_shuffle(&self, window: &mut Window, cx: &mut App) {
        let (engine, id) = (self.lib.engine.clone(), self.id);
        confirm(
            Confirm {
                title: "shuffle the order".into(),
                verb: "shuffle".into(),
                destructive: false,
                lines: vec![
                    format!(
                        "The {} in “{}” are put in a random order, for good.",
                        count(self.entries.len(), "track", "tracks"),
                        self.name()
                    )
                    .into(),
                ],
                note: Some("Undo shuffle puts the last order back.".into()),
            },
            window,
            cx,
            move |cx| {
                let engine = engine.clone();
                cx.spawn(async move |_| {
                    if let Err(e) = engine.shuffle_playlist(id).await {
                        log::warn!("shuffle playlist {id}: {e}");
                    }
                })
                .detach();
            },
        );
    }

    fn ask_rename(&self, window: &mut Window, cx: &mut App) {
        let Some(playlist) = self.playlist.clone() else {
            return;
        };
        let (engine, id) = (self.lib.engine.clone(), self.id);
        let old = playlist.name.clone();
        let synced = playlist.remote_id.is_some();
        super::name(
            "rename playlist",
            "rename",
            &playlist.name,
            move |typed| {
                let mut lines: Vec<SharedString> = if typed.is_empty() || typed == old {
                    vec!["Type the new name.".into()]
                } else {
                    vec![format!("“{old}” becomes “{typed}”.").into()]
                };
                if synced {
                    lines.push("It is renamed on the server too.".into());
                }
                lines
            },
            window,
            cx,
            move |name, cx| {
                let engine = engine.clone();
                cx.spawn(async move |_| {
                    if let Err(e) = engine.rename_playlist(id, name).await {
                        log::warn!("rename playlist {id}: {e}");
                    }
                })
                .detach();
            },
        );
    }

    fn ask_delete(&self, window: &mut Window, cx: &mut App) {
        let Some(playlist) = self.playlist.clone() else {
            return;
        };
        let (engine, id, nav) = (self.lib.engine.clone(), self.id, self.lib.nav.clone());
        let mut lines: Vec<SharedString> = vec![
            format!(
                "“{}” and its {} go.",
                playlist.name,
                count(playlist.track_count as usize, "entry", "entries")
            )
            .into(),
        ];
        if playlist.remote_id.is_some() {
            lines.push("It is deleted on the server too, for every device.".into());
        }
        confirm(
            Confirm {
                title: "delete playlist".into(),
                verb: "delete".into(),
                destructive: true,
                lines,
                note: Some("The tracks stay in the library. This cannot be undone.".into()),
            },
            window,
            cx,
            move |cx| {
                let engine = engine.clone();
                nav.update(cx, |nav, cx| nav.open(Page::Queue, cx));
                cx.spawn(async move |_| {
                    if let Err(e) = engine.delete_playlist(id).await {
                        log::warn!("delete playlist {id}: {e}");
                    }
                })
                .detach();
            },
        );
    }

    /// Writes the playlist out as an M3U8 where the person chooses, saying
    /// what could not go in: a track never downloaded has no file to name.
    fn export(&mut self, cx: &mut Context<Self>) {
        let name = format!("{}.m3u8", self.name());
        let home = dirs::home_dir().unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&home, Some(&name));
        let (engine, id) = (self.lib.engine.clone(), self.id);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let note = match engine
                .export_playlist(id, path.to_string_lossy().into_owned())
                .await
            {
                Ok(summary) if summary.skipped > 0 => format!(
                    "Exported {}; {} not downloaded, so they have no file to point at.",
                    count(summary.written as usize, "track", "tracks"),
                    summary.skipped
                ),
                Ok(summary) => format!(
                    "Exported {}.",
                    count(summary.written as usize, "track", "tracks")
                ),
                Err(e) => format!("Could not write the playlist: {e}"),
            };
            this.update(cx, |this, cx| {
                this.note = Some(note.into());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_grouped(&mut self, grouped: bool, cx: &mut Context<Self>) {
        if let Some(p) = self.playlist.as_mut() {
            p.grouped = Some(grouped);
        }
        self.relayout();
        cx.notify();
        let (engine, id) = (self.lib.engine.clone(), self.id);
        cx.spawn(async move |_, _| {
            if let Err(e) = engine.set_playlist_grouped(id, Some(grouped)).await {
                log::warn!("group playlist {id}: {e}");
            }
        })
        .detach();
    }

    /// Moves or adds what was dropped before row `dest`.
    fn dropped(&mut self, dragged: &PlayableDrag, dest: usize, cx: &mut Context<Self>) {
        self.drop_at = None;
        cx.notify();
        if !self.fillable() {
            return;
        }
        let (engine, id) = (self.lib.engine.clone(), self.id);
        if let Some(origin) = dragged.origin.as_ref().filter(|o| o.playlist == id) {
            let order = moved(&self.order, &self.rows, dest, &origin.entries);
            if order == *self.order {
                return;
            }
            // Applied here first, so the rows stay where they were dropped
            // and a second move is made against the first.
            let at: std::collections::HashMap<i64, usize> = self
                .order
                .iter()
                .enumerate()
                .map(|(ix, id)| (*id, ix))
                .collect();
            let entries: Vec<PlaylistEntry> = order
                .iter()
                .filter_map(|e| at.get(e).map(|ix| self.entries[*ix].clone()))
                .collect();
            self.set_entries(entries);
            cx.spawn(async move |_, _| {
                if let Err(e) = engine.reorder_playlist(id, order).await {
                    log::warn!("reorder playlist {id}: {e}");
                }
            })
            .detach();
        } else {
            let at = position(&self.rows, dest, self.entries.len()) as u32;
            let sets = dragged.sets.to_vec();
            cx.spawn(async move |_, _| {
                if let Err(e) = engine.insert_into_playlist(id, sets, at).await {
                    log::warn!("add to playlist {id}: {e}");
                }
            })
            .detach();
        }
    }

    fn aim(&mut self, scale: f32, cx: &mut Context<Self>) {
        let visible = self.visible.clone();
        let reach = visible.len().max(10);
        let from = visible.start.saturating_sub(reach);
        let to = (visible.end + reach).min(self.rows.len());
        let mut ranked: Vec<(Key, Priority)> = self
            .covers
            .iter()
            .take(4)
            .map(|a| {
                (
                    Key::new(Art::Album(*a), px(MOSAIC / 2.0), scale),
                    Priority::Visible,
                )
            })
            .collect();
        for row in from..to {
            let (album, side) = match self.rows[row] {
                Row::Heading { first, .. } => (self.entries[first].track.album_id, HEADING_SLEEVE),
                Row::Entry { ix, grouped: false } => {
                    (self.entries[ix].track.album_id, rows::SLEEVE)
                }
                Row::Entry { .. } => (None, 0.0),
            };
            let Some(album) = album else { continue };
            let priority = if visible.contains(&row) {
                Priority::Visible
            } else {
                Priority::Ahead(row.abs_diff(visible.start).min(row.abs_diff(visible.end)) as f32)
            };
            ranked.push((Key::new(Art::Album(album), px(side), scale), priority));
        }
        let owner = cx.entity_id();
        self.lib
            .covers
            .update(cx, |covers, _| covers.request(owner, ranked));
    }

    fn sleeve(&self, album: i64, side: f32, scale: f32, cx: &mut App) -> Sleeve {
        let key = Key::new(Art::Album(album), px(side), scale);
        self.lib.covers.update(cx, |c, _| c.sleeve(key))
    }

    fn row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = &Paint::of(window, cx);
        let Some(row) = self.rows.get(ix).copied() else {
            return div().into_any_element();
        };
        let scale = window.scale_factor();
        let first = self.order[row.entries().start];
        let selected = row.entries().all(|e| self.picks.contains(self.order[e]));
        let acting = self.picks.acting_on(first, &self.order);
        let acting = match row {
            Row::Heading { .. } if !selected => self.row_entries(ix),
            _ => acting,
        };
        let drag = self.drag_of(acting.clone());
        let line = match row {
            Row::Heading { first, .. } => self.heading(p, first, ix, drag, scale, cx),
            Row::Entry { ix: entry, grouped } => {
                let e = &self.entries[entry];
                let track = e.track.clone();
                let current = self
                    .playback
                    .read(cx)
                    .0
                    .entry
                    .as_ref()
                    .and_then(|q| q.playlist_entry_id)
                    == Some(e.entry_id);
                let lead = if grouped {
                    track
                        .track_number
                        .map(|n| n.to_string())
                        .unwrap_or_default()
                } else {
                    (entry + 1).to_string()
                };
                let sleeve = (!grouped)
                    .then_some(track.album_id)
                    .flatten()
                    .map(|a| self.sleeve(a, rows::SLEEVE, scale, cx));
                let feed = self.lib.feed.read(cx);
                let state = RowState {
                    lead: lead.into(),
                    lead_width: rows::lead_width(p, self.entries.len(), window),
                    current,
                    playing: feed.playing,
                    favourite: feed.is_favourite(Playable::Track(track.id)),
                    selected,
                    gathered: !grouped,
                    transfer: feed.transfers.get(&track.id).copied(),
                    drag,
                    height: if !grouped {
                        rows::art_height(p)
                    } else if track.credit_differs {
                        rows::text_height(p)
                    } else {
                        rows::height(p)
                    },
                };
                rows::track_row(p, &track, ix, state, sleeve, None, cx)
            }
        };
        let dragging = cx.has_active_drag();
        let count = self.rows.len();
        let line_above = dragging && self.drop_at == Some(ix);
        let line_below = dragging && ix + 1 == count && self.drop_at == Some(count);
        let fillable = self.fillable();
        // Each row under its entry's id, so two copies of one track are two
        // rows to hover, press and drag.
        div()
            .id((
                "entry",
                first as u64
                    | if matches!(row, Row::Heading { .. }) {
                        1 << 63
                    } else {
                        0
                    },
            ))
            .when(line_above, |d| d.border_t_1().border_color(p.accent))
            .when(line_below, |d| d.border_b_1().border_color(p.accent))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.menu_entries = acting.clone()),
            )
            .when(fillable, |d| {
                d.on_drag_move::<PlayableDrag>(cx.listener(
                    move |this, event: &DragMoveEvent<PlayableDrag>, _, cx| {
                        if !event.bounds.contains(&event.event.position) {
                            return;
                        }
                        let lower = event.event.position.y > event.bounds.center().y;
                        let at = ix + usize::from(lower);
                        if this.drop_at != Some(at) {
                            this.drop_at = Some(at);
                            cx.notify();
                        }
                    },
                ))
                .on_drop(cx.listener(
                    move |this, dragged: &PlayableDrag, _, cx| {
                        let dest = this.drop_at.unwrap_or(ix);
                        this.dropped(dragged, dest, cx);
                    },
                ))
            })
            .child(line)
            .into_any_element()
    }

    fn heading(
        &mut self,
        p: &Paint,
        first: usize,
        row: usize,
        drag: PlayableDrag,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let track = self.entries[first].track.clone();
        let sleeve = track
            .album_id
            .map(|a| self.sleeve(a, HEADING_SLEEVE, scale, cx));
        let entries = self.row_entries(row);
        let selected = entries.iter().all(|e| self.picks.contains(*e));
        let target = actions::MenuTarget {
            items: drag.items.clone(),
            album: track.album_id,
            after: None,
        };
        let album = track.album_id;
        let nav = self.lib.nav.clone();
        div()
            .id("heading")
            .flex()
            .items_center()
            .gap(p.space(Space::M))
            .px(px(tile::MARGIN))
            .pt(p.space(Space::M))
            .pb(p.space(Space::Xs))
            .when(selected, |d| d.bg(p.surface))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                let m = event.modifiers();
                if event.click_count() >= 2 {
                    if let Some(first) = entries.first() {
                        this.play_from(*first, cx);
                    }
                } else if m.secondary(cx) {
                    for e in &entries {
                        this.picks.click(*e, event, &[], cx);
                    }
                } else {
                    this.picks.clear();
                    for e in &entries {
                        this.picks.toggle(*e);
                    }
                }
                cx.notify();
            }))
            .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
            .on_mouse_down(MouseButton::Right, actions::mark(cx.weak_entity(), target))
            .child(sleeve_box(p, sleeve, HEADING_SLEEVE))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .id("heading-album")
                            .truncate()
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .typeset(p, Type::Body, p.strong)
                            .child(track.album_title.clone())
                            .on_click(move |_, _, cx| {
                                if let Some(album) = album {
                                    cx.stop_propagation();
                                    nav.update(cx, |nav, cx| nav.open(Page::Album(album), cx));
                                }
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .typeset(p, Type::Meta, p.muted)
                            .child(track.album_artist_name.clone()),
                    ),
            )
            .into_any_element()
    }

    fn header(&mut self, p: &Paint, scale: f32, cx: &mut Context<Self>) -> Div {
        let mosaic = self.mosaic(p, scale, cx);
        let name = self.name();
        let mut detail = vec![count(self.entries.len(), "track", "tracks")];
        let total: i64 = self
            .entries
            .iter()
            .filter_map(|e| e.track.duration_ms)
            .sum();
        if total > 0 {
            detail.push(crate::time::clock(total as u64));
        }
        if let Some(p) = &self.playlist {
            if p.smart {
                detail.push("smart".into());
            } else if p.from_file {
                detail.push("from a file".into());
            } else if p.readonly {
                detail.push("read-only".into());
            }
            // Edits here reach the server, and the server's reach here.
            if p.remote_id.is_some() {
                detail.push("synced".into());
            }
        }
        let fillable = self.fillable();
        let grouped = self.grouped();
        let from_file = self.playlist.as_ref().is_some_and(|p| p.from_file);
        let entity = cx.entity();
        let play = {
            let e = entity.clone();
            Button::new("playlist-play", ButtonKind::Prominent)
                .label("play")
                .disabled(self.entries.is_empty())
                .on_click(move |_, _, cx| e.update(cx, |this, cx| this.play(false, cx)))
        };
        let text = |id: &'static str, label: &str| chrome::text_button(id, label);
        let id = self.id;
        let lib = self.lib.clone();
        let mut verbs = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(p.space(Space::L))
            .child(play)
            .child(text("playlist-shuffle-play", "shuffle").on_click({
                let e = entity.clone();
                move |_, _, cx| e.update(cx, |this, cx| this.play(true, cx))
            }))
            .child(text("playlist-next", "play next").on_click({
                let lib = lib.clone();
                move |_, _, cx| {
                    queue(&lib, id, koan_ffi::Placement::Next, cx);
                }
            }))
            .child(text("playlist-queue", "add to queue").on_click({
                let lib = lib.clone();
                move |_, _, cx| queue(&lib, id, koan_ffi::Placement::Append, cx)
            }));
        if fillable && self.entries.len() > 1 {
            verbs = verbs.child(text("playlist-reorder", "shuffle order").on_click({
                let e = entity.clone();
                move |_, window, cx| e.update(cx, |this, cx| this.ask_shuffle(window, cx))
            }));
        }
        if self.undo_shuffles > 0 {
            let engine = self.lib.engine.clone();
            verbs = verbs.child(text("playlist-unshuffle", "undo shuffle").on_click(
                move |_, _, cx| {
                    let engine = engine.clone();
                    cx.spawn(async move |_| {
                        if let Err(e) = engine.undo(Some(id)).await {
                            log::warn!("undo playlist {id}: {e}");
                        }
                    })
                    .detach();
                },
            ));
        }
        let manage = div()
            .flex()
            .items_center()
            .gap(p.space(Space::L))
            .child(
                text(
                    "playlist-grouping",
                    if grouped { "flat" } else { "group by record" },
                )
                .on_click({
                    let e = entity.clone();
                    move |_, _, cx| e.update(cx, |this, cx| this.set_grouped(!grouped, cx))
                }),
            )
            .when(!from_file, |d| {
                d.child(text("playlist-rename", "rename").on_click({
                    let e = entity.clone();
                    move |_, window, cx| e.update(cx, |this, cx| this.ask_rename(window, cx))
                }))
            })
            .child(text("playlist-export", "export").on_click({
                let e = entity.clone();
                move |_, _, cx| e.update(cx, |this, cx| this.export(cx))
            }))
            .child(text("playlist-delete", "delete").on_click({
                let e = entity.clone();
                move |_, window, cx| e.update(cx, |this, cx| this.ask_delete(window, cx))
            }));
        let picked = self.picks.len();
        div()
            .flex()
            .items_start()
            .gap(p.space(Space::L))
            .px(px(tile::MARGIN))
            .pt(p.space(Space::L))
            .pb(p.space(Space::M))
            .child(mosaic)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(p.space(Space::S))
                    .child(
                        div()
                            .typeset(p, Type::Title, p.strong)
                            .line_clamp(2)
                            .child(name),
                    )
                    .child(
                        div()
                            .typeset(p, Type::Meta, p.muted)
                            .child(detail.join(" · ")),
                    )
                    .child(verbs)
                    .child(manage)
                    .when(picked > 0 && fillable, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(p.space(Space::L))
                                .typeset(p, Type::Fine, p.muted)
                                .child(format!("{picked} selected"))
                                .child(text("playlist-remove-picked", "remove").on_click({
                                    let e = entity.clone();
                                    move |_, window, cx| {
                                        e.update(cx, |this, cx| {
                                            let picked = this.picks.in_order(&this.order);
                                            this.ask_remove(picked, window, cx)
                                        })
                                    }
                                }))
                                .child(text("playlist-clear-picked", "clear").on_click({
                                    let e = entity.clone();
                                    move |_, _, cx| {
                                        e.update(cx, |this, cx| {
                                            this.picks.clear();
                                            cx.notify();
                                        })
                                    }
                                })),
                        )
                    })
                    .when_some(self.note.clone(), |d, note| {
                        d.child(div().typeset(p, Type::Fine, p.muted).child(note))
                    }),
            )
    }

    /// Up to four of the playlist's records, two by two; one fills the
    /// square alone.
    fn mosaic(&self, p: &Paint, scale: f32, cx: &mut App) -> Div {
        let side = px(MOSAIC);
        let covers: Vec<i64> = self.covers.iter().take(4).copied().collect();
        let mut square = div().size(side).flex_none().bg(p.surface).overflow_hidden();
        if covers.len() < 4 {
            if let Some(a) = covers.first() {
                square = square.child(sleeve_box(
                    p,
                    Some(self.sleeve(*a, MOSAIC, scale, cx)),
                    MOSAIC,
                ));
            }
            return square;
        }
        square.flex().flex_wrap().children(covers.iter().map(|a| {
            sleeve_box(
                p,
                Some(self.sleeve(*a, MOSAIC / 2.0, scale, cx)),
                MOSAIC / 2.0,
            )
        }))
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
        self.ask_remove(picked, window, cx);
    }
}

fn queue(lib: &Lib, id: i64, placement: koan_ffi::Placement, cx: &mut App) {
    let engine = lib.engine.clone();
    cx.spawn(async move |_| {
        if let Err(e) = engine
            .queue(vec![koan_ffi::TrackSet::Playlist { id }], placement)
            .await
        {
            log::warn!("queue playlist {id}: {e}");
        }
    })
    .detach();
}

/// A sleeve at `side`, or its colour while it decodes.
fn sleeve_box(p: &Paint, sleeve: Option<Sleeve>, side: f32) -> Div {
    let mut art = div().size(px(side)).flex_none().bg(p.surface);
    match sleeve {
        Some(Sleeve::Ready(image)) => art = art.child(img(ImageSource::Render(image)).size_full()),
        Some(Sleeve::Colour(colour)) => art = art.bg(colour),
        _ => {}
    }
    art
}

impl actions::Menus for PlaylistPage {
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

impl TrackList for PlaylistPage {
    fn clicked(
        &mut self,
        row: usize,
        track: i64,
        part: RowPart,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(entry) = self.row_entries(row).first().copied() else {
            return;
        };
        match part {
            RowPart::Row => {
                let order = self.order.clone();
                if self.picks.click(entry, event, &order, cx) {
                    self.play_from(entry, cx);
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

impl Render for PlaylistPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let scale = window.scale_factor();
        self.aim(scale, cx);
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        if self.drop_at.is_some() && !cx.has_active_drag() {
            self.drop_at = None;
        }
        let header = self.header(p, scale, cx);
        let count = self.rows.len();
        let fillable = self.fillable();
        let body = if count == 0 && self.loaded.is_some() {
            let detail = match &self.playlist {
                Some(pl) if pl.smart => "nothing in the library matches its rules yet.",
                Some(pl) if pl.readonly => "none of its tracks are in the library.",
                _ => "drag records, artists or tracks onto it, or onto its row in the sidebar.",
            };
            chrome::empty_state(crate::theme::Icon::Playlist, "nothing in here yet", detail)
                .into_any_element()
        } else {
            let view = cx.weak_entity();
            list(self.list.clone(), move |ix, window, cx| {
                view.update(cx, |this, cx| this.row(ix, window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .size_full()
            .into_any_element()
        };
        let weak = cx.weak_entity();
        let lib = self.lib.clone();
        let list_area = div()
            .id("playlist-list")
            .relative()
            .flex_1()
            .min_h_0()
            .capture_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, _, _| {
                if e.button == MouseButton::Right {
                    this.menu = None;
                    this.menu_entries.clear();
                }
            }))
            // Past the last row, or onto an empty playlist: the end.
            .when(fillable, |d| {
                d.on_drop(cx.listener(move |this, dragged: &PlayableDrag, _, cx| {
                    let dest = this.drop_at.unwrap_or(count);
                    this.dropped(dragged, dest, cx);
                }))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    drop::add_files_to_playlist(
                        &this.lib.engine,
                        this.id,
                        paths.paths().to_vec(),
                        cx,
                    )
                }))
                .on_drag_move::<PlayableDrag>(cx.listener(
                    |this, event: &DragMoveEvent<PlayableDrag>, _, cx| {
                        if this.drop_at.is_some() && !event.bounds.contains(&event.event.position) {
                            this.drop_at = None;
                            cx.notify();
                        }
                    },
                ))
            })
            .child(body)
            .child(gpui_kit::base::Scrollbar::vertical(&self.list))
            .koan_context_menu("playlist-menu", window, cx, move |m, window, cx| {
                let Some(view) = weak.upgrade() else { return m };
                let (target, entries, fillable) = {
                    let page = view.read(cx);
                    (
                        page.menu.clone(),
                        page.menu_entries.clone(),
                        page.fillable(),
                    )
                };
                let Some(t) = target else { return m };
                let mut m = actions::menu(m, &lib, t.items, t.album, None, window, cx);
                if fillable && !entries.is_empty() {
                    let view = view.clone();
                    m = m.separator().item(
                        MenuEntry::new("remove from playlist")
                            .destructive()
                            .on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| {
                                    this.ask_remove(entries.clone(), window, cx)
                                })
                            }),
                    );
                }
                m
            });
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
            .child(list_area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn runs() -> Vec<RunRole> {
        vec![
            RunRole::Opens,
            RunRole::Continues,
            RunRole::Single,
            RunRole::Opens,
            RunRole::Continues,
        ]
    }

    #[test]
    fn grouped_runs_get_headings_and_flat_rows_do_not() {
        let rows = build(&runs(), true);
        assert_eq!(rows[0], Row::Heading { first: 0, len: 2 });
        assert_eq!(
            rows[3],
            Row::Entry {
                ix: 2,
                grouped: false
            }
        );
        assert_eq!(rows[4], Row::Heading { first: 3, len: 2 });
        assert_eq!(rows.len(), 7);
        assert_eq!(build(&runs(), false).len(), 5);
    }

    #[test]
    fn a_move_lands_before_the_first_row_that_stays() {
        let entries = [10, 11, 12, 13, 14];
        let rows = build(&runs(), false);
        // 14 dropped above 11.
        assert_eq!(moved(&entries, &rows, 1, &[14]), vec![10, 14, 11, 12, 13]);
        // 10 and 11 dropped onto themselves: no change.
        assert_eq!(moved(&entries, &rows, 1, &[10, 11]), entries.to_vec());
        // Past the end.
        assert_eq!(moved(&entries, &rows, 5, &[10]), vec![11, 12, 13, 14, 10]);
    }

    #[test]
    fn a_drop_under_a_heading_lands_at_the_top_of_its_record() {
        let rows = build(&runs(), true);
        assert_eq!(position(&rows, 4, 5), 3);
        assert_eq!(position(&rows, 5, 5), 3);
        assert_eq!(position(&rows, 7, 5), 5);
    }
}
