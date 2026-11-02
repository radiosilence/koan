//! The sidebar: the queue, the library's sections and the playlists.

use std::collections::HashMap;

use koan_ffi::Playlist;
use koan_kit::app::*;
// virtualised lists and their scroll state, which GPUI owns and koan-kit's rows sit in.
use koan_kit::raw::ScrollHandle;

use crate::components::{
    CoverArt, DropTarget, INSERTION, Mosaic, NavHeading, NavNote, NavRow, RowMetrics,
};
use crate::engine::{command, engine};
use crate::library::chrome::count;
use crate::library::covers::{Art, Covers, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::{PlayableDrag, PlaylistDrag};
use crate::mirror::Mirror;
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::platform::drop::{self, Imports};
use crate::theme::{self, Icon, Space, Tone};

const LIBRARY: [(Route, &str, Icon); 7] = [
    (Route::Albums, "albums", Icon::Album),
    (Route::Artists, "artists", Icon::Artist),
    (Route::Tracks, "tracks", Icon::Track),
    (Route::Favourites, "favourites", Icon::Favourite),
    (
        Route::RecentlyPlayed,
        "recently played",
        Icon::RecentlyPlayed,
    ),
    (Route::History, "history", Icon::History),
    (Route::Downloads, "downloads", Icon::Downloads),
];

pub struct Sidebar {
    nav: Entity<Navigator>,
    /// What this device plays from and how much the library holds.
    status: Entity<crate::sidebar_status::SidebarStatus>,
    playlists: Vec<Playlist>,
    /// Up to four records per playlist, whose sleeves make its own.
    faces: HashMap<i64, Vec<i64>>,
    loading: Option<Task<()>>,
    /// The library's art cache, once the library has loaded.
    covers: Option<(Entity<Covers>, [Subscription; 2])>,
    /// Dropped files being added, and what the last addition had to say.
    importing: usize,
    import_problem: Option<String>,
    scroll: ScrollHandle,
    /// Where a playlist dragged among the playlists would land: before the
    /// playlist at this index, or after the last at the count.
    drop_playlist: Option<usize>,
    _subscriptions: [Subscription; 4],
}

impl Sidebar {
    pub fn new(nav: &Entity<Navigator>, cx: &mut Context<Self>) -> Self {
        let library = cx.global::<Mirror>().library.clone();
        let mut sidebar = Self {
            status: cx.new(crate::sidebar_status::SidebarStatus::new),
            nav: nav.clone(),
            playlists: Vec::new(),
            faces: HashMap::new(),
            loading: None,
            covers: None,
            importing: 0,
            drop_playlist: None,
            import_problem: None,
            scroll: ScrollHandle::new(),
            _subscriptions: [
                cx.observe(&library, |this, _, cx| this.reload(cx)),
                // The selected row follows the page.
                cx.observe(nav, |_, _, cx| cx.notify()),
                // The selected row is in the accent, and this view is cached.
                theme::observe_accent(cx),
                // A global, which a cached view hears of only by observing.
                cx.observe_global::<Imports>(|this, cx| {
                    let running = cx.global::<Imports>().running;
                    if running > 0 && this.importing == 0 {
                        this.import_problem = None;
                    }
                    this.importing = running;
                    if let Some(problem) = Imports::take_problem(cx) {
                        this.import_problem = Some(problem);
                    }
                    cx.notify();
                }),
            ],
        };
        sidebar.reload(cx);
        sidebar
    }

    /// Draws the playlists' sleeves from the library's art cache.
    pub fn set_covers(&mut self, covers: Entity<Covers>, cx: &mut Context<Self>) {
        let id = cx.entity_id();
        self.covers = Some((
            covers.clone(),
            [
                cx.observe(&covers, |_, _, cx| cx.notify()),
                cx.on_release(move |this, cx| {
                    if let Some((covers, _)) = &this.covers {
                        covers.update(cx, |c, _| c.release(id));
                    }
                }),
            ],
        ));
        cx.notify();
    }

    /// Asks for the playlists again, with the records on each one's sleeve;
    /// a newer ask replaces one in flight.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let engine = engine(cx);
        self.loading = Some(cx.spawn(
            async move |this, cx| match engine.clone().playlists().await {
                Ok(playlists) => {
                    let mut faces = HashMap::new();
                    for playlist in &playlists {
                        match engine.clone().playlist_cover_album_ids(playlist.id).await {
                            Ok(albums) => {
                                faces.insert(playlist.id, albums);
                            }
                            Err(e) => log::warn!("playlist {} sleeve: {e}", playlist.id),
                        }
                    }
                    this.update(cx, |this, cx| {
                        this.faces = faces;
                        let gone: Vec<i64> = this
                            .playlists
                            .iter()
                            .filter(|old| !playlists.iter().any(|p| p.id == old.id))
                            .map(|old| old.id)
                            .collect();
                        this.playlists = playlists;
                        // A deleted playlist is not somewhere Back can return to.
                        if !gone.is_empty() {
                            this.nav.update(cx, |nav, cx| {
                                for id in gone {
                                    nav.forget(Route::Playlist(id));
                                }
                                cx.notify();
                            });
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => log::warn!("playlists: {e}"),
            },
        ));
    }
}

impl Sidebar {
    /// Moves playlist `id` to sit before the playlist now at `at`, or last.
    fn move_playlist(&mut self, id: i64, at: usize, cx: &mut Context<Self>) {
        let Some(from) = self.playlists.iter().position(|p| p.id == id) else {
            return;
        };
        let moved = self.playlists.remove(from);
        let at = if from < at { at - 1 } else { at }.min(self.playlists.len());
        self.playlists.insert(at, moved);
        let ids: Vec<i64> = self.playlists.iter().map(|p| p.id).collect();
        command(cx, "reorder playlists", move |e| e.reorder_playlists(ids));
        cx.notify();
    }
}

fn go(
    page: Route,
    cx: &mut Context<Sidebar>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |this, _: &ClickEvent, _, cx| {
        this.nav.update(cx, |nav, cx| {
            nav.choose(page);
            cx.notify();
        });
    })
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let current = Some(self.nav.read(cx).current());
        let row = |page: Route,
                   id: ElementId,
                   label: &'static str,
                   icon: Icon,
                   cx: &mut Context<Self>| {
            NavRow::new(id, label)
                .icon(icon)
                .selected(current == Some(page))
                .on_click(go(page, cx))
        };
        let queue = Route::Queue;
        let sleeves = self.sleeves(window, cx);
        // The rows scroll and keep their height when the window is short;
        // the lines at the foot stay put beneath them.
        let body = div()
            .id("sidebar-rows")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .pt(p.space(Space::S))
            .pb(p.space(Space::S))
            // Files, and what is dragged from the library, dropped on the
            // queue's row are queued.
            .child(
                row(queue, "nav-queue".into(), "queue", Icon::QueueSection, cx)
                    .drop_highlight::<ExternalPaths>(&p.look)
                    .drop_highlight::<PlayableDrag>(&p.look)
                    .drop_highlight::<PlaylistDrag>(&p.look)
                    .on_drop(cx.listener(|this, dragged: &PlaylistDrag, _, cx| {
                        this.drop_playlist = None;
                        let sets = dragged.as_library().sets.to_vec();
                        command(cx, "queue a playlist", move |e| {
                            e.queue(sets, koan_ffi::Placement::Append)
                        });
                    }))
                    .on_drop(cx.listener(|_, paths: &ExternalPaths, _, cx| {
                        drop::queue_files(&engine(cx), paths.paths().to_vec(), cx)
                    }))
                    .on_drop(cx.listener(|_, dragged: &PlayableDrag, _, cx| {
                        let sets = dragged.sets.to_vec();
                        command(cx, "queue from the library", move |e| {
                            e.queue(sets, koan_ffi::Placement::Append)
                        });
                    })),
            )
            .child(NavHeading::new("library"))
            .children(LIBRARY.map(|(page, label, icon)| {
                row(page, ElementId::Name(label.into()), label, icon, cx)
            }))
            .when(!self.playlists.is_empty(), |d| {
                d.child(NavHeading::new("playlists"))
            })
            .children({
                let total = self.playlists.len();
                let accent = p.accent;
                self.playlists
                    .iter()
                    .zip(sleeves)
                    .enumerate()
                    .map(|(index, (playlist, sleeve))| {
                        let page = Route::Playlist(playlist.id);
                        let drag = PlaylistDrag {
                            id: playlist.id,
                            name: playlist.name.clone().into(),
                            sleeve: self
                                .faces
                                .get(&playlist.id)
                                .and_then(|f| f.first().copied()),
                        };
                        let line_above = self.drop_playlist == Some(index);
                        let line_below = index + 1 == total && self.drop_playlist == Some(total);
                        let face = NavRow::data(
                            ("nav-playlist", playlist.id as u64),
                            playlist.name.clone(),
                        )
                        .art(Mosaic::new(
                            sleeve.into_iter().map(CoverArt::from).collect(),
                            RowMetrics::nav_art(&p.look),
                        ))
                        .detail(count(playlist.track_count.into(), "track", "tracks"))
                        .selected(current == Some(page))
                        .on_click(go(page, cx))
                        // Dragged among the playlists it moves; dropped on
                        // the queue, its tracks are queued.
                        .on_drag(drag, |drag: &PlaylistDrag, at, _, cx| drag.preview(at, cx))
                        .drop_highlight::<ExternalPaths>(&p.look)
                        .drop_highlight::<PlayableDrag>(&p.look)
                        // Files, and what is dragged from the library, dropped on a
                        // playlist's row are added to it.
                        .on_drop(cx.listener({
                            let id = playlist.id;
                            move |_, paths: &ExternalPaths, _, cx| {
                                drop::add_files_to_playlist(
                                    &engine(cx),
                                    id,
                                    paths.paths().to_vec(),
                                    cx,
                                )
                            }
                        }))
                        .on_drop(cx.listener({
                            let id = playlist.id;
                            move |_, dragged: &PlayableDrag, _, cx| {
                                let sets = dragged.sets.to_vec();
                                command(cx, "add to playlist", move |e| {
                                    e.add_to_playlist(id, sets)
                                });
                            }
                        }))
                        .on_drop(cx.listener(
                            move |this, dragged: &PlaylistDrag, _, cx| {
                                let at = this.drop_playlist.take().unwrap_or(index);
                                this.move_playlist(dragged.id, at, cx);
                            },
                        ));
                        let line = |top: bool| {
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .h(INSERTION)
                                .bg(accent)
                                .map(|d| if top { d.top_0() } else { d.bottom_0() })
                        };
                        div()
                            .relative()
                            .on_drag_move::<PlaylistDrag>(cx.listener(
                                move |this, event: &DragMoveEvent<PlaylistDrag>, _, cx| {
                                    if !event.bounds.contains(&event.event.position) {
                                        return;
                                    }
                                    let lower = event.event.position.y > event.bounds.center().y;
                                    let at = index + usize::from(lower);
                                    if this.drop_playlist != Some(at) {
                                        this.drop_playlist = Some(at);
                                        cx.notify();
                                    }
                                },
                            ))
                            .child(face)
                            .when(line_above, |d| d.child(line(true)))
                            .when(line_below, |d| d.child(line(false)))
                    })
                    .collect::<Vec<_>>()
            });
        div()
            .id("sidebar")
            // A playlist drag that leaves the sidebar, or ends, takes its
            // insertion line with it.
            .on_drag_move::<PlaylistDrag>(cx.listener(
                |this, event: &DragMoveEvent<PlaylistDrag>, _, cx| {
                    if this.drop_playlist.is_some() && !event.bounds.contains(&event.event.position)
                    {
                        this.drop_playlist = None;
                        cx.notify();
                    }
                },
            ))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    // GPUI's scrollbar over a virtualised list, which koan-kit's ScrollArea cannot wrap.
                    .child(koan_kit::raw::base::Scrollbar::vertical(&self.scroll)),
            )
            .child(self.status.clone())
            // Scans, rebuilds and syncs running; nothing while idle.
            .child(
                cx.global::<crate::library::activity::ActivityHandle>()
                    .0
                    .clone(),
            )
            .when(self.importing > 0, |d| {
                d.child(NavNote::new("adding dropped files", Tone::Muted))
            })
            .children(
                self.import_problem
                    .clone()
                    .map(|problem| NavNote::new(problem, Tone::Bad)),
            )
    }
}

impl Sidebar {
    /// Each playlist's sleeve, in order: what to draw for each of its
    /// records, asked of the art cache as one request. Before the library
    /// loads, every playlist shows its records as waiting.
    fn sleeves(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<Vec<Sleeve>> {
        let Some((covers, _)) = self.covers.as_ref() else {
            return self
                .playlists
                .iter()
                .map(|playlist| {
                    vec![Sleeve::Waiting; self.faces.get(&playlist.id).map_or(0, Vec::len)]
                })
                .collect();
        };
        let scale = window.scale_factor();
        let side = RowMetrics::nav_art(&crate::theme::Look::of(window, cx));
        let owner = cx.entity_id();
        let keys: Vec<Vec<Key>> = self
            .playlists
            .iter()
            .map(|playlist| {
                self.faces
                    .get(&playlist.id)
                    .into_iter()
                    .flatten()
                    .map(|album| Key::new(Art::Album(*album), side, scale))
                    .collect()
            })
            .collect();
        let (sleeves, more) = covers.update(cx, |c, cx| {
            c.request(
                owner,
                keys.iter().flatten().map(|key| (*key, Priority::Visible)),
            );
            let more = c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx);
            let sleeves = keys
                .iter()
                .map(|keys| keys.iter().map(|key| c.sleeve(*key)).collect())
                .collect();
            (sleeves, more)
        });
        if more {
            window.request_animation_frame();
        }
        sleeves
    }
}
