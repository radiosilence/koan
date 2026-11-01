//! The sidebar: the queue, the library's sections and the playlists.

use std::collections::HashMap;

use koan_ffi::Playlist;
use koan_kit::raw::prelude::*;
use koan_kit::raw::*;

use crate::engine::{command, engine};
use crate::library::chrome::count;
use crate::library::covers::{Art, Covers, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::PlayableDrag;
use crate::mirror::Mirror;
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::platform::drop::{self, Imports};
use crate::theme::{self, Icon, KoanIcon, KoanStyled, Space, TypeRole};

pub const WIDTH: Pixels = px(220.);
const ROW: Pixels = px(28.);
const PLAYLIST_ROW: Pixels = px(40.);
const PLAYLIST_SLEEVE: Pixels = px(24.);

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
    _subscriptions: [Subscription; 4],
}

impl Sidebar {
    pub fn new(nav: &Entity<Navigator>, cx: &mut Context<Self>) -> Self {
        let library = cx.global::<Mirror>().library.clone();
        let mut sidebar = Self {
            nav: nav.clone(),
            playlists: Vec::new(),
            faces: HashMap::new(),
            loading: None,
            covers: None,
            importing: 0,
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

/// A place in the sidebar: its glyph when icons are on, then its label.
fn nav_row(
    id: impl Into<ElementId>,
    label: SharedString,
    icon: Icon,
    selected: bool,
    p: &Paint,
) -> Stateful<Div> {
    let ink = p.ink;
    let colour = if selected { p.accent_text } else { p.muted };
    div()
        .id(id)
        .h(ROW)
        .flex_shrink_0()
        .flex()
        .items_center()
        .pl(p.space(Space::L))
        .pr(p.space(Space::S))
        .border_l_2()
        .border_color(if selected {
            p.accent
        } else {
            transparent_black()
        })
        .cursor_pointer()
        .text_role(TypeRole::Body, &p.look)
        .gap(p.space(Space::S))
        .text_color(colour)
        .when(!selected, move |d| d.hover(move |s| s.text_color(ink)))
        .when(p.look.icons, |d| {
            d.child(KoanIcon::new(icon).role(TypeRole::Body).color(colour))
        })
        .child(div().truncate().child(label))
}

fn heading(label: &'static str, p: &Paint) -> Div {
    div()
        .flex_shrink_0()
        .pt(p.space(Space::L))
        .pb(p.space(Space::Xs))
        .pl(p.space(Space::L))
        .text_role(TypeRole::Fine, &p.look)
        .text_color(p.ink)
        .child(label)
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
                   label: SharedString,
                   icon: Icon,
                   cx: &mut Context<Self>| {
            nav_row(id, label, icon, current == Some(page), &p).on_click(go(page, cx))
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
                row(
                    queue,
                    "nav-queue".into(),
                    "queue".into(),
                    Icon::QueueSection,
                    cx,
                )
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
            .child(heading("library", &p))
            .children(LIBRARY.map(|(page, label, icon)| {
                row(page, ElementId::Name(label.into()), label.into(), icon, cx)
            }))
            .when(!self.playlists.is_empty(), |d| {
                d.child(heading("playlists", &p))
            })
            .children(
                self.playlists
                    .iter()
                    .zip(sleeves)
                    .map(|(playlist, sleeve)| {
                        let page = Route::Playlist(playlist.id);
                        playlist_row(playlist, sleeve, current == Some(page), &p)
                            .on_click(go(page, cx))
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
                    }),
            );
        div()
            .id("sidebar")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .child(body)
                    .child(koan_kit::raw::base::Scrollbar::vertical(&self.scroll)),
            )
            // Scans, rebuilds and syncs running; nothing while idle.
            .child(
                cx.global::<crate::library::activity::ActivityHandle>()
                    .0
                    .clone(),
            )
            .when(self.importing > 0, |d| {
                d.child(foot_line("adding dropped files", p.muted, &p))
            })
            .children(
                self.import_problem
                    .clone()
                    .map(|problem| foot_line(problem, p.bad, &p)),
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
        let owner = cx.entity_id();
        let keys: Vec<Vec<Key>> = self
            .playlists
            .iter()
            .map(|playlist| {
                self.faces
                    .get(&playlist.id)
                    .into_iter()
                    .flatten()
                    .map(|album| Key::new(Art::Album(*album), PLAYLIST_SLEEVE, scale))
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

/// A playlist's row: its sleeve, its name and how many tracks it holds.
fn playlist_row(
    playlist: &Playlist,
    sleeve: Vec<Sleeve>,
    selected: bool,
    p: &Paint,
) -> Stateful<Div> {
    let ink = p.ink;
    div()
        .id(("nav-playlist", playlist.id as u64))
        .h(PLAYLIST_ROW)
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(p.space(Space::S))
        .pl(p.space(Space::L))
        .pr(p.space(Space::S))
        .border_l_2()
        .border_color(if selected {
            p.accent
        } else {
            transparent_black()
        })
        .cursor_pointer()
        .text_color(if selected { p.accent_text } else { p.muted })
        .when(!selected, move |d| d.hover(move |s| s.text_color(ink)))
        .child(face(sleeve, p))
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_role(TypeRole::Body, &p.look)
                        .child(playlist.name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_role(TypeRole::Meta, &p.look)
                        .text_color(p.muted)
                        .child(count(playlist.track_count.into(), "track", "tracks")),
                ),
        )
}

/// A playlist's sleeve: its first four records, square and edge to edge.
/// One fills the square, two split it down the middle, three put the third
/// across the foot; an empty playlist is a plain square of `rule`.
fn face(sleeves: Vec<Sleeve>, p: &Paint) -> Div {
    let side = PLAYLIST_SLEEVE;
    let half = side / 2.;
    let surface = p.surface;
    let tile = |sleeve: &Sleeve, w: Pixels, h: Pixels| {
        let field = div()
            .w(w)
            .h(h)
            .flex_shrink_0()
            .bg(surface)
            .overflow_hidden();
        match sleeve {
            Sleeve::Ready(image) => field.child(
                img(ImageSource::Render(image.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            ),
            Sleeve::Colour(colour) => field.bg(*colour),
            Sleeve::Waiting | Sleeve::None => field,
        }
    };
    let square = div().size(side).flex_shrink_0().overflow_hidden();
    match sleeves.as_slice() {
        [] => square.bg(p.rule),
        [one] => square.child(tile(one, side, side)),
        [a, b] => square
            .flex()
            .child(tile(a, half, side))
            .child(tile(b, half, side)),
        [a, b, c] => square
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .child(tile(a, half, half))
                    .child(tile(b, half, half)),
            )
            .child(tile(c, side, half)),
        [a, b, c, d, ..] => square
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .child(tile(a, half, half))
                    .child(tile(b, half, half)),
            )
            .child(
                div()
                    .flex()
                    .child(tile(c, half, half))
                    .child(tile(d, half, half)),
            ),
    }
}

/// A line at the sidebar's foot: what koan is doing, or what went wrong.
fn foot_line(text: impl Into<SharedString>, color: Hsla, p: &Paint) -> Div {
    div()
        .px(p.space(Space::L))
        .py(p.space(Space::Xs))
        .text_role(TypeRole::Fine, &p.look)
        .text_color(color)
        .child(text.into())
}
