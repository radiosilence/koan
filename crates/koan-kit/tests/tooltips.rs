//! Every glyph-only control says what it does when the pointer rests on it,
//! wherever it sits: a toolbar's buttons as much as the transport's.

use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui::{
    AppContext, Context, Entity, IntoElement, Modifiers, ParentElement, Render, StyleRefinement,
    Styled, TestAppContext, Window, div, point, px,
};
use koan_kit::components::{TOOLTIPS_DRAWN, Toolbar, ToolbarButton};
use koan_kit::theme::{self, Icon};

struct Page;

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(800.)).flex().flex_col().child(
            Toolbar::new()
                .leading(ToolbarButton::new("back", Icon::Back, "back").help("back (⌘[)"))
                .trailing(ToolbarButton::new("lyrics", Icon::Lyrics, "lyrics")),
        )
    }
}

/// The app's nesting: the page a cached view inside the window's root.
struct Root(Entity<Page>);

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

fn hover_finds_a_tooltip(cx: &mut gpui::VisualTestContext) -> bool {
    cx.run_until_parked();
    let before = TOOLTIPS_DRAWN.load(Ordering::Relaxed);
    (0..200).step_by(4).any(|x| {
        cx.simulate_mouse_move(point(px(x as f32), px(20.)), None, Modifiers::none());
        cx.executor().advance_clock(Duration::from_millis(700));
        cx.run_until_parked();
        TOOLTIPS_DRAWN.load(Ordering::Relaxed) > before
    })
}

#[gpui::test]
fn a_toolbar_button_in_a_cached_view_shows_its_tooltip(cx: &mut TestAppContext) {
    cx.update(|cx| {
        koan_kit::app::platform::init(cx);
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, cx| Root(cx.new(|_| Page)));
    assert!(
        hover_finds_a_tooltip(cx),
        "no tooltip over a cached toolbar"
    );
}

#[gpui::test]
fn a_toolbar_button_shows_its_tooltip(cx: &mut TestAppContext) {
    cx.update(|cx| {
        koan_kit::app::platform::init(cx);
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Page);
    cx.run_until_parked();
    let before = TOOLTIPS_DRAWN.load(Ordering::Relaxed);
    // Along the toolbar's middle until a button's tooltip shows.
    let shown = (0..200).step_by(4).any(|x| {
        cx.simulate_mouse_move(point(px(x as f32), px(20.)), None, Modifiers::none());
        cx.executor().advance_clock(Duration::from_millis(700));
        cx.run_until_parked();
        TOOLTIPS_DRAWN.load(Ordering::Relaxed) > before
    });
    assert!(shown, "no tooltip over the toolbar's leading buttons");
}
