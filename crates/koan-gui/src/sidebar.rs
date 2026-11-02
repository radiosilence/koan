//! The sidebar: the queue, the library's sections and the playlists.

use std::collections::HashMap;

use koan_ffi::Playlist;
use koan_kit::app::*;

use crate::components::{
    Button, ButtonKind, CoverArt, DropTarget, INSERTION, ListCursor, Mosaic, NavHeading, NavNote,
    NavRow, RowMetrics, ScrollArea, list_actions,
};
use crate::engine::{command, engine};
use crate::library::chrome::count;
use crate::library::covers::{Art, Covers, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::{PlayableDrag, PlaylistDrag};
use crate::library::playlist::new_playlist;
use crate::mirror::Mirror;
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::platform::drop::{self, Imports};
use crate::theme::{self, Icon, Space, Tone};

const LIBRARY: [(Route, &str, Icon); 8] = [
    (Route::Albums, "albums", Icon::Album),
    (Route::Artists, "artists", Icon::Artist),
    (Route::Tracks, "tracks", Icon::Track),
    (Route::Favourites, "favourites", Icon::Favourite),
    (
        Route::RecentlyPlayed,
        "recently played",
        Icon::RecentlyPlayed,
    ),
    (Route::OnDevice, "downloaded", Icon::OnDevice),
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
    /// The faces not yet in memory at the last render: a sleeve landing
    /// redraws the sidebar only when it is one of these.
    pending: std::collections::HashSet<Key>,
    /// Dropped files being added, and what the last addition had to say.
    importing: usize,
    import_problem: Option<String>,
    /// Where a playlist dragged among the playlists would land: before the
    /// playlist at this index, or after the last at the count.
    drop_playlist: Option<usize>,
    /// Reorders made here: a read of the playlists that one overtook is
    /// read again rather than putting the old order back.
    reorders: u64,
    /// The search field at the sidebar's head, once the library is open,
    /// and whether it holds a query, which lists the results page.
    search: Option<(Entity<crate::search::field::SearchField>, Subscription)>,
    searching: bool,
    /// The sidebar as one stop in the window's Tab order, its rows reached
    /// with the arrows.
    focus: FocusHandle,
    cursor: ListCursor,
    _subscriptions: [Subscription; 5],
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
            pending: Default::default(),
            importing: 0,
            drop_playlist: None,
            reorders: 0,
            search: None,
            searching: false,
            focus: cx.focus_handle(),
            cursor: ListCursor::default(),
            import_problem: None,
            _subscriptions: [
                cx.observe(&library, |this, _, cx| this.reload(cx)),
                // The selected row follows the page.
                cx.observe(nav, |_, _, cx| cx.notify()),
                // The selected row is in the accent, and this view is cached.
                theme::observe_accent(cx),
                // A global, which a cached view hears of only by observing.
                // What became of music moved, or a command another device
                // did not take.
                cx.observe_global::<crate::notice::Notice>(|_, cx| cx.notify()),
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

    /// The search field, over the library's search and art.
    pub fn set_search(
        &mut self,
        search: Entity<crate::search::Search>,
        covers: Entity<Covers>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let watch = cx.observe(&search, |this: &mut Self, search, cx| {
            let searching = !search.read(cx).query().is_empty();
            if searching != this.searching {
                this.searching = searching;
                cx.notify();
            }
        });
        let field = cx.new(|cx| crate::search::field::SearchField::new(search, covers, window, cx));
        self.search = Some((field, watch));
        cx.notify();
    }

    /// ⌘K and `/`: the keyboard to the search field.
    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((field, _)) = &self.search {
            field.update(cx, |f, cx| f.focus(window, cx));
        }
    }

    /// Draws the playlists' sleeves from the library's art cache.
    pub fn set_covers(&mut self, covers: Entity<Covers>, cx: &mut Context<Self>) {
        let id = cx.entity_id();
        self.covers = Some((
            covers.clone(),
            [
                cx.observe(&covers, |this, covers, cx| {
                    let covers = covers.read(cx);
                    if this.pending.iter().any(|key| covers.has(*key)) {
                        cx.notify();
                    }
                }),
                cx.on_release(move |this, cx| {
                    if let Some((covers, _)) = &this.covers {
                        covers.update(cx, |c, cx| c.release(id, cx));
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
        let reorders = self.reorders;
        self.loading = Some(cx.spawn(
            async move |this, cx| match engine.clone().playlists().await {
                Ok(playlists) => {
                    // Every playlist's sleeve read together, not one after
                    // another.
                    let read = futures::future::join_all(playlists.iter().map(|playlist| {
                        let (engine, id) = (engine.clone(), playlist.id);
                        async move { (id, engine.playlist_cover_album_ids(id).await) }
                    }))
                    .await;
                    this.update(cx, |this, cx| {
                        if this.reorders != reorders {
                            this.reload(cx);
                            return;
                        }
                        // A sleeve that could not be read keeps the one it had,
                        // rather than reading as an empty playlist.
                        let mut faces = HashMap::new();
                        for (id, albums) in read {
                            match albums {
                                Ok(albums) => {
                                    faces.insert(id, albums);
                                }
                                Err(e) => {
                                    log::warn!("playlist {id} sleeve: {e}");
                                    if let Some(kept) = this.faces.get(&id) {
                                        faces.insert(id, kept.clone());
                                    }
                                }
                            }
                        }
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
        // Dropped where it already was: nothing to write.
        if at == from || at == from + 1 {
            return;
        }
        self.reorders += 1;
        let moved = self.playlists.remove(from);
        let at = if from < at { at - 1 } else { at }.min(self.playlists.len());
        self.playlists.insert(at, moved);
        let ids: Vec<i64> = self.playlists.iter().map(|p| p.id).collect();
        command(cx, "reorder playlists", move |e| e.reorder_playlists(ids));
        cx.notify();
    }
}

impl Sidebar {
    /// The places the sidebar lists, in its order: what the arrows walk.
    fn places(&self) -> Vec<Route> {
        std::iter::once(Route::Queue)
            .chain(self.searching.then_some(Route::Search))
            .chain(LIBRARY.iter().map(|(page, _, _)| *page))
            .chain(self.playlists.iter().map(|p| Route::Playlist(p.id)))
            .collect()
    }

    /// The arrows: from the place showing when the keyboard first arrives.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let places = self.places();
        if self.cursor.at.is_none() {
            let current = self.nav.read(cx).current();
            self.cursor.at = places.iter().position(|p| *p == current);
        }
        self.cursor.step(by, places.len(), false);
        cx.notify();
    }

    fn open_cursor(&mut self, cx: &mut Context<Self>) {
        let places = self.places();
        if let Some(page) = self.cursor.at.and_then(|i| places.get(i)).copied() {
            self.nav.update(cx, |nav, cx| {
                nav.choose(page);
                cx.notify();
            });
        }
    }
}

impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
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
        // The row under the keyboard's cursor, while the sidebar holds it.
        let keyed = self.focus.is_focused(window).then(|| {
            let places = self.places();
            self.cursor.at.and_then(|i| places.get(i).copied())
        });
        let keyed = keyed.flatten();
        // A drag that ended anywhere, or was given up, leaves no line.
        let dragging = cx.has_active_drag();
        if !dragging {
            self.drop_playlist = None;
        }
        let row = |page: Route,
                   id: ElementId,
                   label: &'static str,
                   icon: Icon,
                   cx: &mut Context<Self>| {
            NavRow::new(id, label)
                .icon(icon)
                .selected(current == Some(page))
                .cursor(keyed == Some(page))
                .on_click(go(page, cx))
        };
        let queue = Route::Queue;
        let sleeves = self.sleeves(window, cx);
        // The rows scroll and keep their height when the window is short;
        // the lines at the foot stay put beneath them.
        let body = div()
            .flex()
            .flex_col()
            .pt(p.space(Space::S))
            .pb(p.space(Space::S))
            // Files, and what is dragged from the library, dropped on the
            // queue's row are queued.
            .child(
                row(queue, "nav-queue".into(), "queue", Icon::QueueSection, cx)
                    .drop_highlight::<ExternalPaths>(&p.look)
                    .drop_highlight::<PlayableDrag>(&p.look)
                    .drop_highlight::<PlaylistDrag>(&p.look)
                    .accept(cx.listener(|this, dragged: &PlaylistDrag, _, cx| {
                        this.drop_playlist = None;
                        let sets = dragged.as_library().sets.to_vec();
                        command(cx, "queue a playlist", move |e| {
                            e.queue(sets, koan_ffi::Placement::Append)
                        });
                    }))
                    .accept(cx.listener(|_, paths: &ExternalPaths, _, cx| {
                        drop::queue_files(&engine(cx), paths.paths().to_vec(), cx)
                    }))
                    .accept(cx.listener(|_, dragged: &PlayableDrag, _, cx| {
                        let sets = dragged.sets.to_vec();
                        command(cx, "queue from the library", move |e| {
                            e.queue(sets, koan_ffi::Placement::Append)
                        });
                    })),
            )
            .when(self.searching, |d| {
                d.child(row(
                    Route::Search,
                    "nav-results".into(),
                    "results",
                    Icon::Search,
                    cx,
                ))
            })
            .child(NavHeading::new("library"))
            .children(LIBRARY.map(|(page, label, icon)| {
                row(page, ElementId::Name(label.into()), label, icon, cx)
            }))
            // "new", and a drop on the heading makes a playlist of what was
            // dropped.
            .child(
                div()
                    .id("nav-playlists")
                    .drop_highlight::<PlayableDrag>(&p.look)
                    .on_drop(cx.listener(|this, dragged: &PlayableDrag, window, cx| {
                        let sets = dragged.sets.to_vec();
                        new_playlist(engine(cx), this.nav.clone(), sets, window, cx);
                    }))
                    .child(
                        NavHeading::new("playlists").trailing(
                            Button::new("nav-new-playlist", ButtonKind::Text)
                                .label("new")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    new_playlist(
                                        engine(cx),
                                        this.nav.clone(),
                                        Vec::new(),
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                    ),
            )
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
                        let line_above = dragging && self.drop_playlist == Some(index);
                        let line_below =
                            dragging && index + 1 == total && self.drop_playlist == Some(total);
                        let face = NavRow::data(
                            ("nav-playlist", playlist.id as u64),
                            playlist.name.clone(),
                        )
                        .art(Mosaic::new(
                            sleeve.into_iter().map(CoverArt::from).collect(),
                            RowMetrics::nav_art(&p.look),
                        ))
                        .cursor(keyed == Some(page))
                        .detail(count(playlist.track_count.into(), "track", "tracks"))
                        .selected(current == Some(page))
                        .on_click(go(page, cx))
                        // Dragged among the playlists it moves; dropped on
                        // the queue, its tracks are queued.
                        .on_drag(drag, |drag: &PlaylistDrag, at, _, cx| drag.preview(at, cx))
                        // Files, and what is dragged from the library, dropped on a
                        // playlist's row are added to it; a smart or file-backed
                        // playlist takes nothing, does not light for it, and says
                        // why when something is dropped on it anyway.
                        .when(playlist.readonly, |row| {
                            let why: SharedString = if playlist.smart {
                                "A smart playlist is filled by its rules, and can't be added to"
                                    .into()
                            } else if playlist.from_file {
                                "A playlist read from a file can't be added to here".into()
                            } else {
                                "This playlist is read-only, and can't be added to".into()
                            };
                            let refused = why.clone();
                            row.on_drop(move |_: &PlayableDrag, _, cx| {
                                crate::notice::show(refused.clone(), Tone::Muted, cx)
                            })
                            .on_drop(
                                move |_: &ExternalPaths, _, cx| {
                                    crate::notice::show(why.clone(), Tone::Muted, cx)
                                },
                            )
                        })
                        .when(!playlist.readonly, |row| {
                            row.drop_highlight::<ExternalPaths>(&p.look)
                                .drop_highlight::<PlayableDrag>(&p.look)
                                .accept(cx.listener({
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
                                .accept(cx.listener({
                                    let id = playlist.id;
                                    move |_, dragged: &PlayableDrag, _, cx| {
                                        if dragged.is_from(id) {
                                            return;
                                        }
                                        let sets = dragged.sets.to_vec();
                                        command(cx, "add to playlist", move |e| {
                                            e.add_to_playlist(id, sets)
                                        });
                                    }
                                }))
                        })
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
            .track_focus(&self.focus)
            .key_context("Sidebar List")
            .on_action(cx.listener(|this, _: &list_actions::Up, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &list_actions::Down, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &list_actions::First, _, cx| {
                let len = this.places().len();
                this.cursor.to(0, len, false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &list_actions::Last, _, cx| {
                let len = this.places().len();
                this.cursor.to(usize::MAX, len, false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &list_actions::Activate, _, cx| this.open_cursor(cx)))
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
            .children(self.search.as_ref().map(|(field, _)| field.clone()))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(ScrollArea::new("sidebar-rows", body)),
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
            .children(
                cx.try_global::<crate::notice::Notice>()
                    .and_then(|n| n.text.clone())
                    .map(|(text, tone)| NavNote::new(text, tone)),
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
        // Below whatever a page shows: the sidebar's faces are small and
        // there are few, but a page's sleeves on screen come first.
        let (sleeves, more, pending) = covers.update(cx, |c, cx| {
            c.request(
                owner,
                keys.iter()
                    .flatten()
                    .map(|key| (*key, Priority::Ahead(0.0))),
            );
            let more = c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx);
            let sleeves = keys
                .iter()
                .map(|keys| keys.iter().map(|key| c.sleeve(*key)).collect())
                .collect();
            let pending = keys
                .iter()
                .flatten()
                .filter(|k| !c.has(**k))
                .copied()
                .collect();
            (sleeves, more, pending)
        });
        self.pending = pending;
        if more {
            window.request_animation_frame();
        }
        sleeves
    }
}
