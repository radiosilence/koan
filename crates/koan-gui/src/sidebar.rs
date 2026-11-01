//! The sidebar: the queue, the library's sections and the playlists.

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::Playlist;

use crate::engine::{command, engine};
use crate::library::drag::PlayableDrag;
use crate::mirror::Mirror;
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::platform::drop::{self, Imports};
use crate::theme::{self, KoanStyled, Space, TypeRole};

pub const WIDTH: Pixels = px(220.);

const LIBRARY: [(Route, &str); 8] = [
    (Route::Albums, "albums"),
    (Route::Artists, "artists"),
    (Route::Tracks, "tracks"),
    (Route::Favourites, "favourites"),
    (Route::RecentlyPlayed, "recently played"),
    (Route::OnDevice, "downloaded"),
    (Route::History, "history"),
    (Route::Downloads, "downloads"),
];

pub struct Sidebar {
    nav: Entity<Navigator>,
    playlists: Vec<Playlist>,
    loading: Option<Task<()>>,
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
            loading: None,
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

    /// Asks for the playlists again; a newer ask replaces one in flight.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let fetch = engine(cx).playlists();
        self.loading = Some(cx.spawn(async move |this, cx| match fetch.await {
            Ok(playlists) => {
                this.update(cx, |this, cx| {
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
        }));
    }
}

fn nav_row(
    id: impl Into<ElementId>,
    label: SharedString,
    selected: bool,
    p: &Paint,
) -> Stateful<Div> {
    let ink = p.ink;
    div()
        .id(id)
        .h(px(28.))
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
        .text_color(if selected { p.accent_text } else { p.muted })
        .when(!selected, move |d| d.hover(move |s| s.text_color(ink)))
        .child(div().truncate().child(label))
}

fn heading(label: &'static str, p: &Paint) -> Div {
    div()
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
        let row = |page: Route, id: ElementId, label: SharedString, cx: &mut Context<Self>| {
            nav_row(id, label, current == Some(page), &p).on_click(go(page, cx))
        };
        let queue = Route::Queue;
        div()
            .relative()
            .w(WIDTH)
            .h_full()
            .flex_shrink_0()
            .child(
                div()
                    .id("sidebar")
                    .w(WIDTH)
                    .bg(p.bg)
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .pt(p.space(Space::S))
                    // Files, and what is dragged from the library, dropped on the
                    // queue's row are queued.
                    .child(
                        row(queue, "nav-queue".into(), "queue".into(), cx)
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
                    .children(LIBRARY.map(|(page, label)| {
                        row(page, ElementId::Name(label.into()), label.into(), cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .justify_between()
                            .pr(p.space(Space::M))
                            .child(heading("playlists", &p))
                            .child(
                                div()
                                    .id("new-playlist")
                                    .cursor_pointer()
                                    .text_role(TypeRole::Fine, &p.look)
                                    .text_color(p.muted)
                                    .hover({
                                        let ink = p.ink;
                                        move |s| s.text_color(ink)
                                    })
                                    .pb(p.space(Space::Xs))
                                    .child("new")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let nav = this.nav.clone();
                                        crate::collections::new_playlist(
                                            engine(cx),
                                            Vec::new(),
                                            window,
                                            cx,
                                            move |id, cx| {
                                                nav.update(cx, |nav, cx| {
                                                    nav.go(Route::Playlist(id));
                                                    cx.notify();
                                                })
                                            },
                                        );
                                    })),
                            ),
                    )
                    .children(self.playlists.iter().map(|playlist| {
                        let page = Route::Playlist(playlist.id);
                        nav_row(
                            ("nav-playlist", playlist.id as u64),
                            playlist.name.clone().into(),
                            current == Some(page),
                            &p,
                        )
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
                                if dragged.is_from(id) {
                                    return;
                                }
                                let sets = dragged.sets.to_vec();
                                command(cx, "add to playlist", move |e| {
                                    e.add_to_playlist(id, sets)
                                });
                            }
                        }))
                    }))
                    .child(div().flex_1())
                    .when(self.importing > 0, |d| {
                        d.child(foot_line("adding dropped files", p.muted, &p))
                    })
                    .children(
                        self.import_problem
                            .clone()
                            .map(|problem| foot_line(problem, p.bad, &p)),
                    ),
            )
            .child(gpui_kit::base::Scrollbar::vertical(&self.scroll))
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
