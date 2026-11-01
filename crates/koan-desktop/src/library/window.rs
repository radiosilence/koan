//! The library in a window of its own: the three browsers as tabs, the page
//! the navigator is on below them. A stand-in for the app's shell, which
//! hosts these pages when it lands.

use gpui_kit::*;

use super::albums::AlbumGrid;
use super::nav::Page;
use super::tokens::{self, Type, Typeset, space};
use super::{GoBack, GoForward, Lib};

pub struct LibraryWindow {
    lib: Lib,
    /// The page on screen, built when the navigator arrives at it. Pages are
    /// rebuilt on each visit and put their scroll back from the navigator.
    page: Option<(Page, AnyView)>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryWindow {
    pub fn new(lib: Lib, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&lib.nav, |_, _, cx| cx.notify())];
        Self {
            lib,
            page: None,
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
            other => cx.new(|_| Unbuilt(other)).into(),
        };
        self.page = Some((current, view.clone()));
        view
    }
}

/// A page this branch does not draw yet.
struct Unbuilt(Page);

impl Render for Unbuilt {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .role(Type::Control, tokens::MUTED)
            .child(format!("{:?}", self.0))
    }
}

impl Render for LibraryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page(window, cx);
        let current = self.lib.nav.read(cx).current();
        let tab = |label: &'static str, target: Page, nav: Entity<super::nav::Nav>| {
            let selected = current == target;
            div()
                .id(label)
                .cursor_pointer()
                .pb(px(5.0))
                .role(
                    Type::Control,
                    if selected { tokens::INK } else { tokens::MUTED },
                )
                .when(selected, |d| d.border_b_1().border_color(tokens::ACCENT))
                .hover(|s| s.text_color(tokens::INK))
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
            .size_full()
            .flex()
            .flex_col()
            .bg(tokens::BG)
            .font_family(tokens::FONT)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(18.0))
                    .px(px(super::tile::MARGIN))
                    .pt(space::s12())
                    .child(
                        div()
                            .id("back")
                            .cursor_pointer()
                            .role(Type::Control, tokens::MUTED)
                            .hover(|s| s.text_color(tokens::INK))
                            .child("‹")
                            .on_click({
                                let nav = nav.clone();
                                move |_, _, cx| nav.update(cx, |n, cx| n.back(cx))
                            }),
                    )
                    .child(tab("albums", Page::Albums, nav.clone()))
                    .child(tab("artists", Page::Artists, nav.clone()))
                    .child(tab("tracks", Page::Tracks, nav.clone())),
            )
            .child(div().flex_1().min_h_0().child(page))
    }
}
