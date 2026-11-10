//! "Add to playlist…": a sheet over the window listing every playlist that
//! takes tracks, each with its sleeve and size, narrowed by a filter as typed.
//! One for every menu and selection bar, phone and desktop alike: a submenu
//! cannot hold a few thousand playlists, and a finger cannot steer one.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use futures::StreamExt as _;
use koan_kit::app::*;
use koan_runtime::Playlist;

use super::Lib;
use super::actions::{After, add_to_playlist};
use super::chrome::count;
use super::covers::{Art, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use super::drag::{Playable, sources};
use crate::components::sheet as ask;
use crate::components::{
    Button, ButtonKind, EmptyState, FieldState, FilterField, InputEvent, Mosaic, NavRow,
    RowMetrics, Scrollbar, UniformListScrollHandle, uniform_list,
};
use crate::paint::Paint;
use crate::theme::{Icon, Space, Tone};

/// How many playlists' sleeves are read at once.
const SLEEVE_READS: usize = 8;

/// Opens the sheet for `items`; `after` runs once they have gone somewhere.
pub fn open(lib: &Lib, items: Arc<[Playable]>, after: After, window: &mut Window, cx: &mut App) {
    let sheet = cx.new(|cx| PickPlaylist::new(lib.clone(), items, after, window, cx));
    ask::present(sheet, window, cx);
}

pub struct PickPlaylist {
    lib: Lib,
    items: Arc<[Playable]>,
    after: After,
    filter: Entity<FieldState>,
    /// Every playlist that takes tracks, once read.
    playlists: Option<Vec<Playlist>>,
    /// Indices into `playlists` the filter keeps, in order.
    shown: Vec<usize>,
    /// Up to four records per playlist, read as its row first comes on screen.
    faces: HashMap<i64, Vec<i64>>,
    asked: HashSet<i64>,
    /// The sleeves not yet in memory at the last draw: one landing redraws.
    pending: HashSet<Key>,
    scroll: UniformListScrollHandle,
    _subscriptions: [Subscription; 3],
}

impl PickPlaylist {
    fn new(
        lib: Lib,
        items: Arc<[Playable]>,
        after: After,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| FieldState::new(window, cx).placeholder("filter playlists"));
        // A pointer's person types straight away; a phone keeps its keyboard
        // down until the field is tapped, as iOS pickers do.
        if !crate::theme::Look::of(window, cx).touch {
            let handle = filter.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        let id = cx.entity_id();
        let subscriptions = [
            cx.subscribe_in(
                &filter,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => {
                            this.refilter(cx);
                            this.scroll
                                .scroll_to_item(0, crate::components::ScrollStrategy::Top);
                        }
                        // Return takes the first that matches.
                        InputEvent::PressEnter { .. } => {
                            if let Some(id) = this.first_shown() {
                                this.choose(id, window, cx);
                            }
                        }
                        _ => {}
                    }
                },
            ),
            cx.observe(&lib.covers, |this, covers, cx| {
                let covers = covers.read(cx);
                if this.pending.iter().any(|key| covers.has(*key)) {
                    cx.notify();
                }
            }),
            cx.on_release(move |this, cx| {
                this.lib
                    .covers
                    .update(cx, |covers, cx| covers.release(id, cx));
            }),
        ];
        let engine = lib.engine.clone();
        cx.spawn(async move |this, cx| {
            let read = engine.playlists().await;
            this.update(cx, |this, cx| {
                match read {
                    Ok(playlists) => {
                        this.playlists =
                            Some(playlists.into_iter().filter(|p| !p.readonly).collect());
                    }
                    Err(e) => {
                        log::warn!("playlists: {e}");
                        this.playlists = Some(Vec::new());
                    }
                }
                this.refilter(cx);
            })
            .ok();
        })
        .detach();
        Self {
            lib,
            items,
            after,
            filter,
            playlists: None,
            shown: Vec::new(),
            faces: HashMap::new(),
            asked: HashSet::new(),
            pending: HashSet::new(),
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        self.shown = self
            .playlists
            .iter()
            .flatten()
            .enumerate()
            .filter(|(_, p)| query.is_empty() || p.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        cx.notify();
    }

    fn first_shown(&self) -> Option<i64> {
        let playlists = self.playlists.as_ref()?;
        self.shown.first().map(|i| playlists[*i].id)
    }

    fn choose(&mut self, id: i64, _: &mut Window, cx: &mut Context<Self>) {
        add_to_playlist(&self.lib, id, &self.items, cx);
        self.done(cx);
    }

    fn new_playlist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let navigator = self.lib.nav.read(cx).navigator();
        let sets = sources(&self.items);
        ask::close(cx);
        super::playlist::new_playlist(self.lib.engine.clone(), navigator, sets, window, cx);
        if let Some(after) = &self.after {
            after(cx);
        }
    }

    fn done(&self, cx: &mut App) {
        ask::close(cx);
        if let Some(after) = &self.after {
            after(cx);
        }
    }

    /// Reads the sleeves' records for the playlists in `ids` not yet asked.
    fn read_faces(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        let ids: Vec<i64> = ids
            .into_iter()
            .filter(|id| self.asked.insert(*id))
            .collect();
        if ids.is_empty() {
            return;
        }
        let engine = self.lib.engine.clone();
        cx.spawn(async move |this, cx| {
            let read: Vec<_> = futures::stream::iter(ids.into_iter().map(|id| {
                let engine = engine.clone();
                async move { (id, engine.playlist_cover_album_ids(id).await) }
            }))
            .buffered(SLEEVE_READS)
            .collect()
            .await;
            this.update(cx, |this, cx| {
                for (id, albums) in read {
                    match albums {
                        Ok(albums) => {
                            this.faces.insert(id, albums);
                        }
                        Err(e) => log::warn!("playlist {id} sleeve: {e}"),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The rows in `range` of what the filter keeps, their sleeves asked of
    /// the art cache as this list's one request.
    fn rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(playlists) = self.playlists.as_ref() else {
            return Vec::new();
        };
        let visible: Vec<&Playlist> = self.shown[range.clone()]
            .iter()
            .map(|i| &playlists[*i])
            .collect();
        let ids: Vec<i64> = visible.iter().map(|p| p.id).collect();
        let look = crate::theme::Look::of(window, cx);
        let side = RowMetrics::nav_art(&look);
        let scale = window.scale_factor();
        let keys: Vec<Vec<Key>> = ids
            .iter()
            .map(|id| {
                self.faces
                    .get(id)
                    .into_iter()
                    .flatten()
                    .map(|album| Key::new(Art::Album(*album), side, scale))
                    .collect()
            })
            .collect();
        let owner = cx.entity_id();
        let (sleeves, more, pending) = self.lib.covers.update(cx, |c, cx| {
            c.request(
                owner,
                keys.iter().flatten().map(|key| (*key, Priority::Visible)),
                cx,
            );
            let more = c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx);
            let sleeves: Vec<Vec<_>> = keys
                .iter()
                .map(|keys| keys.iter().map(|key| c.sleeve(*key)).collect())
                .collect();
            let pending = keys.iter().flatten().copied().filter(|k| !c.has(*k));
            (sleeves, more, pending.collect())
        });
        self.pending = pending;
        if more {
            window.request_animation_frame();
        }
        let rows = visible
            .iter()
            .zip(sleeves)
            .map(|(playlist, sleeve)| {
                let id = playlist.id;
                // As wide as the list: a list's items size to their content.
                div()
                    .w_full()
                    .child(
                        NavRow::data(("pick-playlist", id as u64), playlist.name.clone())
                            .art(Mosaic::new(
                                sleeve.into_iter().map(Into::into).collect(),
                                side,
                            ))
                            .detail(count(playlist.track_count.into(), "track", "tracks"))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.choose(id, window, cx)),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        self.read_faces(ids, cx);
        rows
    }
}

impl Render for PickPlaylist {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let look = &p.look;
        let cancel = Button::new("pick-playlist-cancel", ButtonKind::Standard)
            .label("cancel")
            .on_click(|_, _, cx| ask::close(cx));
        let new = NavRow::new("pick-playlist-new", "new playlist…")
            .icon(Icon::Add)
            .on_click(cx.listener(|this, _, window, cx| this.new_playlist(window, cx)));
        let list = match &self.playlists {
            None => div().flex_1().into_any_element(),
            Some(all) if self.shown.is_empty() => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(if all.is_empty() {
                    EmptyState::new(Icon::Playlist, "no playlists yet")
                } else {
                    EmptyState::new(Icon::Playlist, "no playlists match")
                })
                .into_any_element(),
            Some(_) => div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    uniform_list(
                        "pick-playlist-list",
                        self.shown.len(),
                        cx.processor(|this, range: Range<usize>, window, cx| {
                            this.rows(range, window, cx)
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .child(Scrollbar::vertical(&self.scroll))
                .into_any_element(),
        };
        let total = self.playlists.as_ref().map_or(0, Vec::len);
        // Tall enough to scroll through many, whatever the filter keeps.
        ask::tall_panel("add to playlist", look, window)
            .child(ask::head(
                "add to playlist",
                vec![cancel.into_any_element()],
                look,
            ))
            .child(
                div()
                    .flex_none()
                    .px(look.space(Space::L))
                    .py(look.space(Space::M))
                    .child(
                        FilterField::new(&self.filter)
                            .label("filter playlists")
                            .fill(),
                    ),
            )
            .child(new)
            .when(total > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px(look.space(Space::L))
                        .pt(look.space(Space::S))
                        .pb(look.space(Space::Xs))
                        .child(ask::note(
                            count(self.shown.len() as u64, "playlist", "playlists"),
                            Tone::Muted,
                            look,
                        )),
                )
            })
            .child(list)
    }
}
