//! A playlist's page, the Mac's `PlaylistView`: laid out as the queue is,
//! grouped under record headings or flat, its rows drawn by the queue's own
//! row parts, with the playlist's header above and its verbs in the window's
//! toolbar.
//!
//! Every edit goes to the engine and comes back as the library moving, which
//! reads the playlist again; nothing here keeps a copy that could disagree.
//! Each entry is drawn as a `QueueItem` named by its entry id, so the queue's
//! row layout, drop targets and reorder rules apply unchanged.

use std::collections::HashSet;

use koan_ffi::{EntryStatus, Playlist, PlaylistEntry, QueueItem, TrackSet};
use koan_kit::app::*;

use super::Lib;
use super::actions;
use super::chrome::count;
use super::covers::{Art, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use super::drag::{Playable, PlayableDrag, PlaylistDrag};
use crate::components::sheet::{Confirm, confirm, name};
use crate::components::{
    Badge, Cover, CoverArt, EmptyState, INSERTION, IconToggle, KoanContextMenu, Menu, MenuEntry,
    MenuPanel, MenuRow, Mosaic, Page, PageHeader, PlayButton, RowKind, RowList, RowListState,
    RowMetrics, RowOffset, ShuffleButton, TableRow, ToolbarButton, ToolbarGroup, ToolbarMenu,
    ToolbarSelection,
};
use crate::covers::{self, CoversHandle};
use crate::engine::command;
use crate::mirror::Mirror;
use crate::navigator::Route;
use crate::paint::Paint;
use crate::platform::actions::{ClearSelection, Delete, Redo, Undo};
use crate::platform::drop;
use crate::queue::rows::{self, Row};
use crate::queue::{self as queue_rows, Extras, Look, Numbering};
use crate::theme::{self, Icon};

/// The entries as the rows draw them, laid out off the main thread.
struct Laid {
    items: Vec<QueueItem>,
    rows: Vec<Row>,
    disc_records: HashSet<i64>,
}

fn lay_out(entries: Vec<PlaylistEntry>, grouped: bool) -> Laid {
    let items: Vec<QueueItem> = entries.into_iter().map(listed).collect();
    let rows = rows::build(&items, grouped);
    let disc_records = rows::disc_records(&items);
    Laid {
        items,
        rows,
        disc_records,
    }
}

/// An entry as a row draws it: named by its entry id, waiting for nothing.
fn listed(entry: PlaylistEntry) -> QueueItem {
    let t = entry.track;
    QueueItem {
        queue_item_id: entry.entry_id.to_string(),
        track_id: Some(t.id),
        album_id: t.album_id,
        artist_id: t.artist_id,
        album_artist_id: t.album_artist_id,
        title: t.title,
        artist: t.artist_name,
        album_artist: t.album_artist_name,
        artist_credit: t.artist_credit,
        album_artist_credit: t.album_artist_credit,
        credit_differs: t.credit_differs,
        run: entry.run,
        album: t.album_title,
        year: None,
        codec: t.codec,
        track_number: t.track_number.map(i64::from),
        disc: t.disc.map(i64::from),
        duration_ms: t.duration_ms.and_then(|d| u64::try_from(d).ok()),
        status: EntryStatus::Queued,
        playlist_entry_id: Some(entry.entry_id),
        failure_reason: None,
        on_server: t.on_server,
        on_disk: t.on_disk,
    }
}

pub struct PlaylistPage {
    lib: Lib,
    id: i64,
    /// `None` until read, and after the playlist is deleted.
    playlist: Option<Playlist>,
    loaded: bool,
    items: Vec<QueueItem>,
    rows: Vec<Row>,
    disc_records: HashSet<i64>,
    /// Up to four records, whose sleeves make the header's.
    faces: Vec<i64>,
    list: RowListState,
    /// Picked entries, by id. A heading reads as picked when its run is.
    selected: HashSet<String>,
    /// Where a shift-click extends from: an entry id.
    anchor: Option<String>,
    /// The row a drag over the list would land before.
    drop_at: Option<usize>,
    /// The row a secondary click landed on, for the list's one menu.
    menu_row: Option<usize>,
    /// The entry playing, when the queue plays one of this playlist's.
    playing: Option<i64>,
    /// What the last export had to say.
    note: Option<SharedString>,
    focus: FocusHandle,
    /// The keyboard's place in the list, by row.
    cursor: crate::components::ListCursor,
    /// The list holds the keyboard, as of the last frame.
    keyed: bool,
    _loading: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl PlaylistPage {
    pub fn new(lib: Lib, id: i64, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>();
        let (library, playback) = (mirror.library.clone(), mirror.playback.clone());
        let row_covers = cx.global::<CoversHandle>().0.clone();
        let subscriptions = vec![
            // An edit here, a sync, another device: the playlist is read again.
            cx.observe(&library, |this, _, cx| this.load(cx)),
            cx.observe(&playback, |this, playback, cx| {
                let playing = playback
                    .read(cx)
                    .0
                    .entry
                    .as_ref()
                    .and_then(|e| e.playlist_entry_id);
                if playing != this.playing {
                    this.playing = playing;
                    cx.notify();
                }
            }),
            cx.observe(&row_covers, |_, _, cx| cx.notify()),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.observe(&lib.feed, |_, _, cx| cx.notify()),
            // The playing row and the header's play are in the accent.
            theme::observe_accent(cx),
        ];
        let playing = playback
            .read(cx)
            .0
            .entry
            .as_ref()
            .and_then(|e| e.playlist_entry_id);
        let mut page = Self {
            lib,
            id,
            playlist: None,
            loaded: false,
            items: Vec::new(),
            rows: Vec::new(),
            disc_records: HashSet::new(),
            faces: Vec::new(),
            list: RowListState::new(),
            selected: HashSet::new(),
            anchor: None,
            drop_at: None,
            menu_row: None,
            playing,
            note: None,
            focus: cx.focus_handle().tab_stop(true),
            cursor: Default::default(),
            keyed: false,
            _loading: None,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page
    }

    /// Reads the playlist and its entries; a newer read replaces one in
    /// flight. The rows are laid out on the background executor.
    fn load(&mut self, cx: &mut Context<Self>) {
        let (engine, id) = (self.lib.engine.clone(), self.id);
        self._loading = Some(cx.spawn(async move |this, cx| {
            let playlist = match engine.clone().playlists().await {
                Ok(all) => all.into_iter().find(|p| p.id == id),
                Err(e) => {
                    log::warn!("playlist {id}: {e}");
                    return;
                }
            };
            let entries = match playlist {
                Some(_) => match engine.clone().playlist_tracks(id).await {
                    Ok(entries) => entries,
                    Err(e) => {
                        log::warn!("playlist {id} entries: {e}");
                        return;
                    }
                },
                None => Vec::new(),
            };
            let faces = engine
                .clone()
                .playlist_cover_album_ids(id)
                .await
                .unwrap_or_default();
            let grouped = playlist.as_ref().and_then(|p| p.grouped).unwrap_or(false);
            let laid = cx
                .background_spawn(async move { lay_out(entries, grouped) })
                .await;
            this.update(cx, |this, cx| {
                this.playlist = playlist;
                this.loaded = true;
                this.faces = faces;
                this.set_laid(laid);
                cx.notify();
            })
            .ok();
        }));
    }

    /// New rows in place of the old, keeping the place in the list and
    /// dropping picks of entries that went.
    fn set_laid(&mut self, laid: Laid) {
        let top = self.list.top();
        self.items = laid.items;
        self.rows = laid.rows;
        self.disc_records = laid.disc_records;
        let present: HashSet<&str> = self
            .items
            .iter()
            .map(|i| i.queue_item_id.as_str())
            .collect();
        self.selected.retain(|id| present.contains(id.as_str()));
        if self
            .anchor
            .as_ref()
            .is_some_and(|a| !present.contains(a.as_str()))
        {
            self.anchor = None;
        }
        self.list.reset(self.rows.len(), RowKind::Text);
        self.list.scroll_to(RowOffset {
            item_ix: top.item_ix.min(self.rows.len().saturating_sub(1)),
            offset_in_item: top.offset_in_item,
        });
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

    fn name(&self) -> String {
        self.playlist
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }

    fn row_items(&self, row: usize) -> impl Iterator<Item = &QueueItem> {
        self.rows
            .get(row)
            .map_or(0..0, |r| r.items())
            .map(|ix| &self.items[ix])
    }

    fn is_selected(&self, row: usize) -> bool {
        !self.selected.is_empty()
            && self
                .row_items(row)
                .all(|i| self.selected.contains(&i.queue_item_id))
    }

    /// The picked entries, in playlist order.
    fn selected_items(&self) -> Vec<&QueueItem> {
        self.items
            .iter()
            .filter(|i| self.selected.contains(&i.queue_item_id))
            .collect()
    }

    /// What a row's verbs act on: the pick when the row is in it, the row
    /// alone otherwise.
    fn acting_on(&self, row: usize) -> Vec<&QueueItem> {
        if self.is_selected(row) {
            self.selected_items()
        } else {
            self.row_items(row).collect()
        }
    }

    fn click(&mut self, row: usize, event: &ClickEvent, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .row_items(row)
            .map(|i| i.queue_item_id.clone())
            .collect();
        let Some(first) = ids.first().cloned() else {
            return;
        };
        let modifiers = event.modifiers();
        if event.click_count() >= 2 {
            self.play_from(first.parse().ok(), false, cx);
        } else if modifiers.shift {
            let at = |id: &str| self.items.iter().position(|i| i.queue_item_id == id);
            let from = self.anchor.as_deref().and_then(at);
            let to = self.rows[row].items();
            let range = match from {
                Some(from) => from.min(to.start)..from.max(to.end - 1) + 1,
                None => to,
            };
            self.selected = self.items[range]
                .iter()
                .map(|i| i.queue_item_id.clone())
                .collect();
        } else if modifiers.secondary(cx) {
            if ids.iter().all(|id| self.selected.contains(id)) {
                for id in &ids {
                    self.selected.remove(id);
                }
            } else {
                self.selected.extend(ids);
            }
            self.anchor = Some(first);
        } else {
            self.selected = ids.into_iter().collect();
            self.anchor = Some(first);
        }
        cx.notify();
    }

    fn play_from(&self, entry: Option<i64>, shuffled: bool, cx: &App) {
        let id = self.id;
        command(cx, "play playlist", move |e| {
            e.play_playlist(id, entry, shuffled)
        });
    }

    fn set_grouped(&mut self, grouped: bool, cx: &mut Context<Self>) {
        if let Some(p) = self.playlist.as_mut() {
            p.grouped = Some(grouped);
        }
        let laid = Laid {
            rows: rows::build(&self.items, grouped),
            items: std::mem::take(&mut self.items),
            disc_records: std::mem::take(&mut self.disc_records),
        };
        self.set_laid(laid);
        cx.notify();
        let id = self.id;
        command(cx, "group playlist", move |e| {
            e.set_playlist_grouped(id, Some(grouped))
        });
    }

    /// Moves or adds what was dropped before row `dest`: rows of this
    /// playlist move, anything else is added there.
    fn dropped(&mut self, dragged: &PlayableDrag, dest: usize, cx: &mut Context<Self>) {
        self.drop_at = None;
        cx.notify();
        if !self.fillable() {
            return;
        }
        let id = self.id;
        match dragged.origin.as_ref().filter(|o| o.playlist == id) {
            Some(origin) => {
                let moving: Vec<String> = origin.entries.iter().map(i64::to_string).collect();
                let Some((target, after)) =
                    rows::drop_target(&self.rows, dest, &moving, &self.items)
                else {
                    return;
                };
                let order: Vec<String> =
                    self.items.iter().map(|i| i.queue_item_id.clone()).collect();
                let moved = rows::reordered(&order, &moving, &target, after);
                if moved == order {
                    return;
                }
                let ids: Vec<i64> = moved.iter().filter_map(|id| id.parse().ok()).collect();
                command(cx, "reorder playlist", move |e| e.reorder_playlist(id, ids));
            }
            None => {
                let at = rows::position(&self.rows, dest, self.items.len()) as u32;
                let sets = dragged.sets.to_vec();
                command(cx, "add to playlist", move |e| {
                    e.insert_into_playlist(id, sets, at)
                });
            }
        }
    }

    /// The payload for dragging `items` out: their tracks, marked as this
    /// playlist's rows.
    fn drag_of(&self, row: usize) -> PlayableDrag {
        let items = self.acting_on(row);
        let tracks: Vec<Playable> = items
            .iter()
            .filter_map(|i| i.track_id.map(Playable::Track))
            .collect();
        let entries = items.iter().filter_map(|i| i.playlist_entry_id).collect();
        let label = match items.as_slice() {
            [one] => SharedString::from(one.title.clone()),
            many => count(many.len() as u64, "track", "tracks").into(),
        };
        let album = items.first().and_then(|i| i.album_id);
        PlayableDrag::new(&tracks, label)
            .with_sleeve(album)
            .from_playlist(self.id, entries)
    }

    /// Asks before taking entries out, naming them.
    fn ask_remove(&self, entries: Vec<(i64, String)>, window: &mut Window, cx: &mut App) {
        if entries.is_empty() || !self.fillable() {
            return;
        }
        let id = self.id;
        let n = entries.len() as u64;
        let mut lines: Vec<SharedString> = vec![format!("From “{}”:", self.name()).into()];
        lines.extend(entries.iter().take(8).map(|(_, t)| t.clone().into()));
        if entries.len() > 8 {
            lines.push(format!("and {} more", entries.len() - 8).into());
        }
        let ids: Vec<i64> = entries.into_iter().map(|(e, _)| e).collect();
        confirm(
            Confirm {
                title: format!("remove {}", count(n, "track", "tracks")).into(),
                verb: "remove".into(),
                destructive: true,
                lines,
                note: Some("The tracks stay in the library. Undo puts them back.".into()),
            },
            window,
            cx,
            move |cx| {
                let ids = ids.clone();
                command(cx, "remove from playlist", move |e| {
                    e.remove_from_playlist(id, ids)
                });
            },
        );
    }

    fn ask_remove_selected(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        let entries = described(&self.selected_items());
        self.ask_remove(entries, window, cx);
    }

    fn ask_shuffle(&self, window: &mut Window, cx: &mut App) {
        let id = self.id;
        confirm(
            Confirm {
                title: "shuffle the order".into(),
                verb: "shuffle".into(),
                destructive: false,
                lines: vec![
                    format!(
                        "The {} in “{}” are put in a random order, for good.",
                        count(self.items.len() as u64, "track", "tracks"),
                        self.name()
                    )
                    .into(),
                ],
                note: Some("Undo puts the last order back.".into()),
            },
            window,
            cx,
            move |cx| command(cx, "shuffle playlist", move |e| e.shuffle_playlist(id)),
        );
    }

    fn ask_rename(&self, window: &mut Window, cx: &mut App) {
        if let Some(playlist) = &self.playlist {
            ask_rename(playlist, window, cx);
        }
    }

    fn ask_delete(&self, window: &mut Window, cx: &mut App) {
        if let Some(playlist) = &self.playlist {
            ask_delete(playlist, self.lib.nav.read(cx).navigator(), window, cx);
        }
    }

    /// Writes the playlist out as an M3U8, saying under the header how it
    /// went.
    fn export(&mut self, cx: &mut Context<Self>) {
        let page = cx.weak_entity();
        export(
            self.lib.engine.clone(),
            self.id,
            self.name(),
            cx,
            move |note, cx| {
                page.update(cx, |this, cx| {
                    this.note = Some(note.into());
                    cx.notify();
                })
                .ok();
            },
        );
    }
}

/// Asks for a playlist's new name, saying what changes, then renames it.
pub fn ask_rename(playlist: &Playlist, window: &mut Window, cx: &mut App) {
    let id = playlist.id;
    let old = playlist.name.clone();
    let synced = playlist.remote_id.is_some();
    name(
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
        move |name, cx| command(cx, "rename playlist", move |e| e.rename_playlist(id, name)),
    );
}

/// Asks before deleting a playlist, saying what goes and what stays. The
/// queue shows in its place when it was the page showing.
pub fn ask_delete(
    playlist: &Playlist,
    nav: Entity<crate::navigator::Navigator>,
    window: &mut Window,
    cx: &mut App,
) {
    let id = playlist.id;
    let mut lines: Vec<SharedString> = vec![
        format!(
            "“{}” and its {} go.",
            playlist.name,
            count(playlist.track_count.into(), "entry", "entries")
        )
        .into(),
    ];
    if playlist.remote_id.is_some() {
        lines.push("It is deleted on the server too, for every device.".into());
    }
    if playlist.from_file {
        lines.push("Its file in the library stays, and a rescan reads it again.".into());
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
            nav.update(cx, |nav, cx| {
                if nav.current() == Route::Playlist(id) && nav.go(Route::Queue) {
                    cx.notify();
                }
            });
            command(cx, "delete playlist", move |e| e.delete_playlist(id));
        },
    );
}

/// Writes a playlist out as an M3U8 where the person chooses, and hands
/// `said` what could not go in: a track never downloaded has no file to
/// name.
pub fn export(
    engine: std::sync::Arc<koan_ffi::KoanEngine>,
    id: i64,
    name: String,
    cx: &mut App,
    said: impl FnOnce(String, &mut App) + 'static,
) {
    let file = format!("{name}.m3u8");
    let home = dirs::home_dir().unwrap_or_default();
    let chosen = cx.prompt_for_new_path(&home, Some(&file));
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(path))) = chosen.await else {
            return;
        };
        let note = match engine
            .export_playlist(id, path.to_string_lossy().into_owned())
            .await
        {
            Ok(summary) if summary.skipped > 0 => format!(
                "Exported {}; {} not downloaded, so they have no file to name.",
                count(summary.written.into(), "track", "tracks"),
                summary.skipped
            ),
            Ok(summary) => format!(
                "Exported {}.",
                count(summary.written.into(), "track", "tracks")
            ),
            Err(e) => format!("Could not write the playlist: {e}"),
        };
        cx.update(|cx| said(note, cx));
    })
    .detach();
}

/// Entries with how a question names them: title and artist.
fn described(items: &[&QueueItem]) -> Vec<(i64, String)> {
    items
        .iter()
        .filter_map(|i| Some((i.playlist_entry_id?, format!("{} — {}", i.title, i.artist))))
        .collect()
}

/// Asks for a name for a new playlist holding what `sets` name (nothing, for
/// an empty one), then makes it and opens it. The tracks are resolved first,
/// so the question can say how many it will hold.
pub fn new_playlist(
    engine: std::sync::Arc<koan_ffi::KoanEngine>,
    nav: Entity<crate::navigator::Navigator>,
    sets: Vec<TrackSet>,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let tracks = if sets.is_empty() {
            Vec::new()
        } else {
            match engine.clone().track_ids_of(sets).await {
                Ok(tracks) => tracks,
                Err(e) => return log::warn!("new playlist: {e}"),
            }
        };
        let held = tracks.len() as u64;
        cx.update_window(handle, |_, window, cx| {
            name(
                "new playlist",
                "create",
                "",
                move |typed| {
                    let holding = if held == 0 {
                        "nothing yet".to_owned()
                    } else {
                        count(held, "track", "tracks")
                    };
                    if typed.is_empty() {
                        vec![format!("Name it. It will hold {holding}.").into()]
                    } else {
                        vec![format!("“{typed}”, holding {holding}.").into()]
                    }
                },
                window,
                cx,
                move |name, cx| {
                    let (engine, nav, tracks) = (engine.clone(), nav.clone(), tracks.clone());
                    cx.spawn(
                        async move |cx| match engine.create_playlist(name, tracks).await {
                            Ok(playlist) => cx.update(|cx| {
                                nav.update(cx, |nav, cx| {
                                    if nav.go(Route::Playlist(playlist.id)) {
                                        cx.notify();
                                    }
                                })
                            }),
                            Err(e) => log::warn!("new playlist: {e}"),
                        },
                    )
                    .detach();
                },
            )
        })
        .ok();
    })
    .detach();
}

impl PlaylistPage {
    fn header(&mut self, p: &Paint, window: &mut Window, cx: &mut Context<Self>) -> PageHeader {
        let side = RowMetrics::header_art(&p.look);
        let scale = window.scale_factor();
        let owner = cx.entity_id();
        let keys: Vec<Key> = self
            .faces
            .iter()
            .map(|album| Key::new(Art::Album(*album), side, scale))
            .collect();
        let (arts, more) = self.lib.covers.update(cx, |c, cx| {
            c.request(owner, keys.iter().map(|k| (*k, Priority::Visible)));
            let more = c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx);
            let arts: Vec<CoverArt> = keys.iter().map(|k| CoverArt::from(c.sleeve(*k))).collect();
            (arts, more)
        });
        if more {
            window.request_animation_frame();
        }
        let total: u64 = self.items.iter().filter_map(|i| i.duration_ms).sum();
        let mut detail = count(self.items.len() as u64, "track", "tracks");
        if total > 0 {
            detail = format!("{detail} · {}", crate::time::clock(total));
        }
        let empty = self.items.is_empty();
        let (play, shuffle) = (cx.entity(), cx.entity());
        let playlist = self.playlist.as_ref();
        PageHeader::data(self.name())
            .art(if arts.is_empty() {
                Cover::new(CoverArt::Missing, side).into_any_element()
            } else {
                Mosaic::new(arts, side).into_any_element()
            })
            .lead(
                PlayButton::new("playlist-play", "play").on_click(move |_, _, cx| {
                    play.read(cx).play_from(None, false, cx);
                }),
            )
            .lead(
                ShuffleButton::new("playlist-shuffle", self.name())
                    .disabled(empty)
                    .on_click(move |_, _, cx| shuffle.read(cx).play_from(None, true, cx)),
            )
            .detail(detail)
            .when(playlist.is_some_and(|p| p.smart), |h| {
                h.badge(Badge::new("smart"))
            })
            .when(playlist.is_some_and(|p| p.from_file), |h| {
                h.badge(Badge::new("from a file"))
            })
            .when(
                playlist.is_some_and(|p| p.readonly && !p.smart && !p.from_file),
                |h| h.badge(Badge::new("read-only")),
            )
            .when(playlist.is_some_and(|p| p.remote_id.is_none()), |h| {
                h.badge(Badge::new("this device"))
            })
            .when_some(self.note.clone(), |h, note| h.body(note))
    }

    /// The playlist's controls in the window's toolbar, the Mac's: the pick
    /// and its verbs, grouping, undo and redo, and the rest under ⋯.
    pub fn controls(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        let fillable = self.fillable();
        let picked = self.selected.len();
        let mut controls = ToolbarGroup::new();
        if picked > 0 {
            let (clear, remove) = (view.clone(), view.clone());
            let mut selection = ToolbarSelection::new(format!("{picked} selected")).action(
                "playlist-deselect",
                "clear",
                move |_, _, cx| {
                    clear.update(cx, |this, cx| {
                        this.selected.clear();
                        cx.notify();
                    })
                },
            );
            if fillable {
                selection = selection.action("playlist-remove", "remove", move |_, window, cx| {
                    remove.update(cx, |this, cx| {
                        let entries = described(&this.selected_items());
                        this.ask_remove(entries, window, cx);
                    });
                });
            }
            controls = controls.child(selection).spacer();
        }
        let id = self.id;
        let regroup = view.clone();
        let mut controls = controls.child(
            IconToggle::new(
                "playlist-group",
                Icon::Album,
                "group by album",
                self.grouped(),
            )
            .on_change(move |on, _, cx| {
                let on = *on;
                regroup.update(cx, |this, cx| this.set_grouped(on, cx))
            }),
        );
        if fillable {
            controls = controls
                .child(
                    ToolbarButton::new("playlist-undo", Icon::Undo, "undo")
                        .help(crate::platform::actions::help("undo", &Undo, cx))
                        .on_click(move |_, _, cx| command(cx, "undo", move |e| e.undo(Some(id)))),
                )
                .child(
                    ToolbarButton::new("playlist-redo", Icon::Redo, "redo")
                        .help(crate::platform::actions::help("redo", &Redo, cx))
                        .on_click(move |_, _, cx| command(cx, "redo", move |e| e.redo(Some(id)))),
                );
        }
        controls.child(self.more(view)).into_any_element()
    }

    /// The playlist's ⋯: queueing it, and changing or writing it out.
    fn more(&self, view: Entity<Self>) -> impl IntoElement {
        let id = self.id;
        let fillable = self.fillable();
        let renamable = self.playlist.as_ref().is_some_and(|p| !p.from_file);
        let shufflable = fillable && self.items.len() > 1;
        ToolbarMenu::new(
            "playlist-more",
            ToolbarButton::new("playlist-more-trigger", Icon::More, "more"),
            move |dismiss, _, _| {
                let queue = |placement: fn() -> koan_ffi::Placement| {
                    let close = dismiss.clone();
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        close(window, cx);
                        command(cx, "queue playlist", move |e| {
                            e.queue(vec![TrackSet::Playlist { id }], placement())
                        });
                    }
                };
                let (shuffle, rename, export, delete) =
                    (view.clone(), view.clone(), view.clone(), view.clone());
                let (c1, c2, c3, c4) = (
                    dismiss.clone(),
                    dismiss.clone(),
                    dismiss.clone(),
                    dismiss.clone(),
                );
                MenuPanel::new()
                    .child(
                        MenuRow::new("playlist-next", "play next")
                            .on_click(queue(|| koan_ffi::Placement::Next)),
                    )
                    .child(
                        MenuRow::new("playlist-queue", "add to queue")
                            .on_click(queue(|| koan_ffi::Placement::Append)),
                    )
                    .child(
                        MenuRow::new("playlist-shuffle-order", "shuffle the order…")
                            .disabled(!shufflable)
                            .on_click(move |_, window, cx| {
                                c1(window, cx);
                                shuffle.update(cx, |this, cx| this.ask_shuffle(window, cx));
                            }),
                    )
                    .child(
                        MenuRow::new("playlist-rename", "rename…")
                            .disabled(!renamable)
                            .on_click(move |_, window, cx| {
                                c2(window, cx);
                                rename.update(cx, |this, cx| this.ask_rename(window, cx));
                            }),
                    )
                    .child(MenuRow::new("playlist-export", "export as M3U8…").on_click(
                        move |_, window, cx| {
                            c3(window, cx);
                            export.update(cx, |this, cx| this.export(cx));
                        },
                    ))
                    .child(MenuRow::new("playlist-delete", "delete…").on_click(
                        move |_, window, cx| {
                            c4(window, cx);
                            delete.update(cx, |this, cx| this.ask_delete(window, cx));
                        },
                    ))
            },
        )
    }

    fn render_row(
        &mut self,
        row: usize,
        p: &Paint,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.rows.len();
        if row >= count {
            return div().into_any_element();
        }
        let selected = self.is_selected(row);
        let insertion = p.accent;
        let p = &p.picked(selected);
        let r = self.rows[row];
        let album = match r {
            Row::Heading { first, .. } => Some(self.items[first].album_id),
            Row::Track { ix, grouped: false } => Some(self.items[ix].album_id),
            Row::Track { grouped: true, .. } => None,
        };
        let sleeve = album.map(|album| {
            let side = if matches!(r, Row::Heading { .. }) {
                RowMetrics::heading_sleeve(&p.look)
            } else {
                RowMetrics::sleeve(&p.look)
            };
            let art = covers::art(album, side, cx);
            Cover::new(art, side)
                .id(("sleeve", row as u64))
                .into_any_element()
        });
        let feed = self.lib.feed.clone();
        let nav = self.lib.nav.read(cx).navigator();
        let (row_frame, _) = match r {
            Row::Heading { first, len } => {
                let run = &self.items[first..first + len];
                let id = SharedString::from(format!("h:{}", run[0].queue_item_id));
                let look = Look {
                    p,
                    nav: &nav,
                    row: &id,
                    picked: selected,
                    waiting: false,
                };
                let frame = queue_rows::heading(
                    TableRow::new(ElementId::Name(id.clone()), RowKind::Heading),
                    &run[0],
                    queue_rows::detail(run),
                    sleeve.unwrap_or_else(|| div().into_any_element()),
                    &look,
                );
                (frame, id)
            }
            Row::Track { ix, grouped } => {
                let kind = if grouped { RowKind::Text } else { RowKind::Art };
                let base = &self.items[ix];
                let id = SharedString::from(base.queue_item_id.clone());
                let look = Look {
                    p,
                    nav: &nav,
                    row: &id,
                    picked: selected,
                    waiting: false,
                };
                let playing =
                    base.playlist_entry_id.is_some() && base.playlist_entry_id == self.playing;
                let shown;
                let item = if playing {
                    shown = QueueItem {
                        status: EntryStatus::Playing,
                        ..base.clone()
                    };
                    &shown
                } else {
                    base
                };
                let favourite = item
                    .track_id
                    .map(|t| feed.read(cx).is_favourite(Playable::Track(t)));
                let numbering = Numbering {
                    by_disc: item
                        .album_id
                        .is_some_and(|a| self.disc_records.contains(&a)),
                    wide: !self.disc_records.is_empty(),
                };
                let frame = queue_rows::track(
                    TableRow::new(ElementId::Name(id.clone()), kind),
                    item,
                    grouped,
                    sleeve,
                    numbering,
                    Extras {
                        favourite,
                        feed: Some(feed.clone()),
                        availability: crate::transfers::of_queue_row(
                            item.status,
                            item.track_id,
                            item.on_server,
                            item.on_disk,
                            item.track_id
                                .and_then(|t| feed.read(cx).transfers.get(&t).copied()),
                        ),
                    },
                    &look,
                );
                (frame, id)
            }
        };
        let dragging = cx.has_active_drag();
        let line_above = dragging && self.drop_at == Some(row);
        let line_below = dragging && row + 1 == count && self.drop_at == Some(count);
        let fillable = self.fillable();
        let drag = self.drag_of(row);
        let aim = move |this: &mut Self,
                        position: Point<Pixels>,
                        bounds: Bounds<Pixels>,
                        cx: &mut Context<Self>| {
            if !this.fillable() || !bounds.contains(&position) {
                return;
            }
            let lower = position.y > bounds.center().y;
            let at = rows::insertion_point(row, lower);
            if this.drop_at != Some(at) {
                this.drop_at = Some(at);
                cx.notify();
            }
        };
        let list = self.list.clone();
        row_frame
            .selected(selected)
            .cursor(self.keyed && self.cursor.at == Some(row))
            .when(p.a11y, |frame| {
                frame
                    .place(row, count)
                    .on_reveal(move |_, _| list.reveal_around(row))
            })
            .when(line_above, |d| {
                d.border_t(INSERTION).border_color(insertion)
            })
            .when(line_below, |d| {
                d.border_b(INSERTION).border_color(insertion)
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.menu_row = Some(row)),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                // The keyboard carries on from the row clicked.
                this.cursor.at = Some(row);
                this.cursor.anchor = Some(row);
                this.click(row, event, cx);
            }))
            .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
            .when(fillable, |d| {
                d.on_drag_move::<PlayableDrag>(cx.listener(
                    move |this, e: &DragMoveEvent<PlayableDrag>, _, cx| {
                        aim(this, e.event.position, e.bounds, cx)
                    },
                ))
                .on_drag_move::<PlaylistDrag>(cx.listener(
                    move |this, e: &DragMoveEvent<PlaylistDrag>, _, cx| {
                        aim(this, e.event.position, e.bounds, cx)
                    },
                ))
                .on_drop(cx.listener(move |this, dragged: &PlayableDrag, _, cx| {
                    let dest = this.drop_at.unwrap_or(row);
                    this.dropped(dragged, dest, cx);
                }))
                .on_drop(cx.listener(
                    move |this, dragged: &PlaylistDrag, _, cx| {
                        let dest = this.drop_at.unwrap_or(row);
                        this.dropped(&dragged.as_library(), dest, cx);
                    },
                ))
            })
            .into_any_element()
    }
}

/// The list's one context menu: play from the row, the library's verbs for
/// the tracks, then what can be done to the entries here.
fn row_menu(
    menu: Menu,
    view: &Entity<PlaylistPage>,
    window: &mut Window,
    cx: &mut Context<Menu>,
) -> Menu {
    let (lib, id, fillable, tracks, album, first, entries) = {
        let this = view.read(cx);
        let Some(row) = this.menu_row.filter(|&row| row < this.rows.len()) else {
            return menu;
        };
        let items = this.acting_on(row);
        let tracks: std::sync::Arc<[Playable]> = items
            .iter()
            .filter_map(|i| i.track_id.map(Playable::Track))
            .collect();
        let album = (items.len() == 1).then(|| items[0].album_id).flatten();
        let first = items.first().and_then(|i| i.playlist_entry_id);
        (
            this.lib.clone(),
            this.id,
            this.fillable(),
            tracks,
            album,
            first,
            described(&items),
        )
    };
    let menu = menu.item(MenuEntry::new("play from here").icon(Icon::Play).on_click(
        move |_, _, cx| {
            command(cx, "play playlist", move |e| {
                e.play_playlist(id, first, false)
            })
        },
    ));
    let menu = actions::menu(menu.separator(), &lib, tracks, album, None, window, cx);
    if !fillable {
        return menu;
    }
    let n = entries.len();
    let view = view.downgrade();
    menu.separator().item(
        MenuEntry::new(if n == 1 {
            "remove from playlist…".to_string()
        } else {
            format!("remove {n} from playlist…")
        })
        .icon(Icon::Remove)
        .on_click(move |_, window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.ask_remove(entries.clone(), window, cx));
            }
        }),
    )
}

impl Render for PlaylistPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        #[cfg(feature = "profiler")]
        crate::measure::TOP.store(
            self.list.top().item_ix,
            std::sync::atomic::Ordering::Relaxed,
        );
        if self.drop_at.is_some() && !cx.has_active_drag() {
            self.drop_at = None;
        }
        if self.loaded && self.playlist.is_none() {
            return div()
                .size_full()
                .child(
                    EmptyState::new(Icon::Playlist, "playlist not found")
                        .detail("It was deleted, here or on another device."),
                )
                .into_any_element();
        }
        let count = self.rows.len();
        let fillable = self.fillable();
        let view = cx.entity();
        let header = self.header(&p, window, cx);
        let body = if count == 0 {
            EmptyState::new(Icon::Playlist, "nothing in this playlist")
                .detail(if fillable {
                    "Drag tracks, records or playlists here."
                } else {
                    "What it holds is decided elsewhere."
                })
                .into_any_element()
        } else {
            let rows_view = view.clone();
            RowList::new(&self.list, move |row, window, cx| {
                rows_view.update(cx, |this, cx| this.render_row(row, &p, window, cx))
            })
            .named(self.name())
            .into_any_element()
        };
        self.keyed = self.focus.is_focused(window);
        self.cursor.clamp(self.rows.len());
        super::keys::keyed(
            div()
                .id("playlist")
                .key_context("Library List")
                .track_focus(&self.focus),
            cx,
        )
        .on_action(cx.listener(|this, _: &super::SelectAll, _, cx| {
            this.selected = this.items.iter().map(|i| i.queue_item_id.clone()).collect();
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &super::Dismiss, _, cx| {
            this.selected.clear();
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &ClearSelection, _, cx| {
            this.selected.clear();
            cx.notify();
        }))
        .when(fillable, |d| {
            let id = self.id;
            d.on_action(cx.listener(Self::ask_remove_selected))
                .on_action(move |_: &Undo, _, cx| command(cx, "undo", move |e| e.undo(Some(id))))
                .on_action(move |_: &Redo, _, cx| command(cx, "redo", move |e| e.redo(Some(id))))
                // Below the last row: the end of the playlist.
                .on_drop(cx.listener(move |this, dragged: &PlayableDrag, _, cx| {
                    let dest = this.drop_at.unwrap_or(count);
                    this.dropped(dragged, dest, cx);
                }))
                .on_drop(cx.listener(move |this, dragged: &PlaylistDrag, _, cx| {
                    let dest = this.drop_at.unwrap_or(count);
                    this.dropped(&dragged.as_library(), dest, cx);
                }))
                // Files from outside: indexed where they lie and appended.
                .on_drop(cx.listener(move |_, paths: &ExternalPaths, _, cx| {
                    drop::add_files_to_playlist(
                        &crate::engine::engine(cx),
                        id,
                        paths.paths().to_vec(),
                        cx,
                    )
                }))
                // A drag that leaves the list takes its insertion line with it.
                .on_drag_move::<PlayableDrag>(cx.listener(
                    |this, e: &DragMoveEvent<PlayableDrag>, _, cx| {
                        if this.drop_at.is_some() && !e.bounds.contains(&e.event.position) {
                            this.drop_at = None;
                            cx.notify();
                        }
                    },
                ))
        })
        .size_full()
        .flex()
        .flex_col()
        .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
            if event.button == MouseButton::Right {
                this.menu_row = None;
            }
        }))
        .child(Page::new(header).body(body))
        .koan_context_menu("playlist-menu", window, cx, {
            let view = view.clone();
            move |menu, window, cx| row_menu(menu, &view, window, cx)
        })
        .into_any_element()
    }
}

impl super::keys::KeyedList for PlaylistPage {
    fn cursor(&mut self) -> &mut crate::components::ListCursor {
        &mut self.cursor
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    fn pick(&mut self, from: usize, to: usize, _: &mut Context<Self>) {
        let entries = self.rows[from].items().start..self.rows[to].items().end;
        self.selected = self.items[entries]
            .iter()
            .map(|i| i.queue_item_id.clone())
            .collect();
        if from == to {
            let first = self.row_items(from).next().map(|i| i.queue_item_id.clone());
            self.anchor = first;
        }
    }

    fn toggle(&mut self, ix: usize, _: &mut Context<Self>) {
        let ids: Vec<String> = self
            .row_items(ix)
            .map(|i| i.queue_item_id.clone())
            .collect();
        if ids.iter().all(|id| self.selected.contains(id)) {
            for id in &ids {
                self.selected.remove(id);
            }
        } else {
            self.selected.extend(ids);
        }
    }

    fn reveal(&mut self, ix: usize, _: &mut Context<Self>) {
        self.list.reveal(ix);
    }

    /// Return plays the playlist from the row, as a double click does.
    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        let entry = self
            .row_items(ix)
            .next()
            .and_then(|i| i.queue_item_id.parse().ok());
        self.play_from(entry, false, cx);
    }

    fn menu_at(&mut self, ix: Option<usize>, _: &mut Context<Self>) {
        self.menu_row = ix;
    }
}

impl Focusable for PlaylistPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_listed_under_its_own_id_and_waits_for_nothing() {
        let track = koan_ffi::Track {
            id: 7,
            title: "Song".into(),
            artist_name: "Artist".into(),
            artist_credit: Vec::new(),
            album_artist_name: "Artist".into(),
            album_artist_id: None,
            album_artist_credit: Vec::new(),
            credit_differs: false,
            album_title: "Record".into(),
            album_id: Some(3),
            artist_id: None,
            disc: Some(2),
            track_number: Some(4),
            duration_ms: Some(1000),
            codec: Some("flac".into()),
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
        let item = listed(PlaylistEntry {
            entry_id: 42,
            track,
            run: koan_ffi::RunRole::Single,
        });
        assert_eq!(item.queue_item_id, "42");
        assert_eq!(item.playlist_entry_id, Some(42));
        assert_eq!((item.disc, item.track_number), (Some(2), Some(4)));
        assert_eq!(item.status, EntryStatus::Queued);
    }
}
