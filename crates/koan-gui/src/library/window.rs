//! The library in a window of its own: the three browsers as tabs, the page
//! the navigator is on below them. A stand-in for the app's shell, which
//! hosts these pages when it lands.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::albums::AlbumGrid;
use super::nav::Page;
use super::tokens::{Paint, Type, Typeset};
use super::{GoBack, GoForward, Lib};
use crate::theme::Space;

pub struct LibraryWindow {
    lib: Lib,
    /// The page on screen, built when the navigator arrives at it. Pages are
    /// rebuilt on each visit and put their scroll back from the navigator.
    page: Option<(Page, AnyView)>,
    info: Option<(i64, Entity<super::info::TrackInfo>)>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryWindow {
    pub fn new(lib: Lib, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&lib.nav, |_, _, cx| cx.notify())];
        Self {
            lib,
            page: None,
            info: None,
            _subscriptions: subscriptions,
        }
    }

    fn page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyView {
        let current = self.lib.nav.read(cx).current();
        if let Some((page, view)) = &self.page
            && *page == current
        {
            return view.clone();
        }
        let lib = self.lib.clone();
        let view: AnyView = match current {
            Page::Albums => cx.new(|cx| AlbumGrid::new(lib, window, cx)).into(),
            Page::Artists => cx
                .new(|cx| super::artists::ArtistBrowser::new(lib, window, cx))
                .into(),
            Page::Album(id) => cx
                .new(|cx| super::album_page::AlbumPage::new(lib, id, window, cx))
                .into(),
            Page::Artist(id) => cx
                .new(|cx| super::artist_page::ArtistPage::new(lib, id, window, cx))
                .into(),
            Page::Tracks => cx
                .new(|cx| super::tracks::TrackBrowser::new(lib, window, cx))
                .into(),
        };
        self.page = Some((current, view.clone()));
        view
    }
}

impl Render for LibraryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let page = self.page(window, cx);
        let wanted = self.lib.nav.read(cx).info;
        if self.info.as_ref().map(|(id, _)| *id) != wanted {
            let lib = self.lib.clone();
            self.info = wanted.map(|id| {
                (
                    id,
                    cx.new(|cx| super::info::TrackInfo::new(lib, id, window, cx)),
                )
            });
        }
        let current = self.lib.nav.read(cx).current();
        let tab = |label: &'static str, target: Page, nav: Entity<super::nav::Nav>| {
            let selected = current == target;
            div()
                .id(label)
                .cursor_pointer()
                .pb(px(5.0))
                .typeset(p, Type::Control, if selected { p.ink } else { p.muted })
                .when(selected, |d| d.border_b_1().border_color(p.accent))
                .hover(|s| s.text_color(p.ink))
                .child(label)
                .on_click(move |_, _, cx| nav.update(cx, |nav, cx| nav.open(target, cx)))
        };
        let nav = self.lib.nav.clone();
        div()
            .key_context("Library")
            .on_action(
                cx.listener(|this, _: &GoBack, _, cx| this.lib.nav.update(cx, |n, cx| n.back(cx))),
            )
            .on_action(cx.listener(|this, _: &GoForward, _, cx| {
                this.lib.nav.update(cx, |n, cx| n.forward(cx))
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .font_family(crate::theme::FONT_FAMILY)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(18.0))
                    .px(px(super::tile::MARGIN))
                    .pt(p.space(Space::M))
                    .child(
                        div()
                            .id("back")
                            .cursor_pointer()
                            .typeset(p, Type::Control, p.muted)
                            .hover(|s| s.text_color(p.ink))
                            .child(
                                crate::theme::KoanIcon::new(crate::theme::Icon::Back)
                                    .role(Type::Control)
                                    .color(p.muted),
                            )
                            .on_click({
                                let nav = nav.clone();
                                move |_, _, cx| nav.update(cx, |n, cx| n.back(cx))
                            }),
                    )
                    .child(tab("albums", Page::Albums, nav.clone()))
                    .child(tab("artists", Page::Artists, nav.clone()))
                    .child(tab("tracks", Page::Tracks, nav.clone())),
            )
            // Each page replays its last frame unless it notified, so the
            // tabs and the overlay above it never redraw a 5,000-record grid.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(page.cached(StyleRefinement::default().size_full())),
            )
            .when_some(self.info.as_ref(), |d, (_, info)| d.child(info.clone()))
    }
}
