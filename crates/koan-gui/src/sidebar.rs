//! The sidebar: the queue, the library's sections and the playlists.

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::Playlist;

use crate::engine::engine;
use crate::mirror::Mirror;
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::theme::{self, KoanStyled, Space, TypeRole};

pub const WIDTH: Pixels = px(220.);

const LIBRARY: [(Route, &str); 7] = [
    (Route::Albums, "albums"),
    (Route::Artists, "artists"),
    (Route::Tracks, "tracks"),
    (Route::Favourites, "favourites"),
    (Route::RecentlyPlayed, "recently played"),
    (Route::History, "history"),
    (Route::Downloads, "downloads"),
];

pub struct Sidebar {
    nav: Entity<Navigator>,
    playlists: Vec<Playlist>,
    loading: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

impl Sidebar {
    pub fn new(nav: &Entity<Navigator>, cx: &mut Context<Self>) -> Self {
        let library = cx.global::<Mirror>().library.clone();
        let mut sidebar = Self {
            nav: nav.clone(),
            playlists: Vec::new(),
            loading: None,
            _subscriptions: [
                cx.observe(&library, |this, _, cx| this.reload(cx)),
                // The selected row follows the page.
                cx.observe(nav, |_, _, cx| cx.notify()),
                // The selected row is in the accent, and this view is cached.
                theme::observe_accent(cx),
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
            .id("sidebar")
            .w(WIDTH)
            .bg(p.bg)
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .pt(p.space(Space::S))
            .child(row(queue, "nav-queue".into(), "queue".into(), cx))
            .child(heading("library", &p))
            .children(
                LIBRARY.map(|(page, label)| {
                    row(page, ElementId::Name(label.into()), label.into(), cx)
                }),
            )
            .when(!self.playlists.is_empty(), |d| {
                d.child(heading("playlists", &p))
            })
            .children(self.playlists.iter().map(|playlist| {
                let page = Route::Playlist(playlist.id);
                nav_row(
                    ("nav-playlist", playlist.id as u64),
                    playlist.name.clone().into(),
                    current == Some(page),
                    &p,
                )
                .on_click(go(page, cx))
            }))
    }
}
