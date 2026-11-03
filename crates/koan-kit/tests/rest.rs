//! What draws at rest (#1741): a page of controls nobody is touching puts
//! no shader quad in the scene, so a frame presented for something else
//! (the playing bars, a sleeve arriving) pays for none of them.

use gpui::{
    AppContext, Context, Entity, IntoElement, Modifiers, ParentElement, Render, StyleRefinement,
    Styled, TestAppContext, VisualTestContext, Window, div, point, px,
};
use koan_kit::components::{ListRow, ToolbarButton, lit_row};
use koan_kit::theme::{self, Icon};

struct Page;

impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Below an empty strip: the test window's pointer starts at its
        // origin, and a control under it would be lit, not at rest.
        div()
            .size_full()
            .pt(px(40.))
            .flex()
            .flex_col()
            .child(div().flex().children(
                (0..10).map(|i| ToolbarButton::new(("button", i as u64), Icon::Sort, "sort")),
            ))
            .children((0..20).map(|i| {
                lit_row(("lit", i as u64), window, cx)
                    .h(px(24.))
                    .child(format!("row {i}"))
            }))
            .children((0..20).map(|i| ListRow::new(("list", i as u64)).title(format!("item {i}"))))
    }
}

fn shaders(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, _| window.painted_shader_count())
}

#[gpui::test]
fn controls_at_rest_draw_no_shader(cx: &mut TestAppContext) {
    cx.update(|cx| {
        koan_kit::app::platform::init(cx);
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Page);
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    assert_eq!(shaders(cx), 0, "controls nobody touches drew shader quads");

    // Under the pointer a button lights, which is a shader quad.
    cx.simulate_mouse_move(point(px(10.), px(50.)), None, Modifiers::default());
    cx.run_until_parked();
    assert!(shaders(cx) > 0, "the button under the pointer drew no fill");
}

/// A control the pointer leaves fades out over `Motion::LEAVE`, its fill
/// mounted until the fade has landed, and then nothing is drawn for it.
#[gpui::test]
fn a_fill_fades_out_on_leave_then_is_gone(cx: &mut TestAppContext) {
    cx.update(|cx| {
        koan_kit::app::platform::init(cx);
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Page);
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.simulate_mouse_move(point(px(10.), px(50.)), None, Modifiers::default());
    cx.run_until_parked();
    let lit = shaders(cx);
    assert!(lit > 0, "the button under the pointer drew no fill");

    // Off every control: the fill is still there, fading.
    cx.simulate_mouse_move(point(px(10.), px(10.)), None, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        shaders(cx),
        lit,
        "the fill went at once instead of fading out"
    );

    // Past the fade, the next frame draws nothing for it.
    std::thread::sleep(theme::Motion::LEAVE + std::time::Duration::from_millis(60));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(shaders(cx), 0, "the faded fill is still drawn");
}

/// The same page inside a cached view, as the app draws its pages: the
/// pointer's state is read inside it, so its changes have to reach it
/// through the cache.
struct Cached(Entity<Page>);

impl Render for Cached {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

#[gpui::test]
fn a_fill_inside_a_cached_view_fades_out_on_leave(cx: &mut TestAppContext) {
    cx.update(|cx| {
        koan_kit::app::platform::init(cx);
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, cx| Cached(cx.new(|_| Page)));
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    assert_eq!(shaders(cx), 0);
    cx.simulate_mouse_move(point(px(10.), px(50.)), None, Modifiers::default());
    cx.run_until_parked();
    let lit = shaders(cx);
    assert!(
        lit > 0,
        "inside a cached view, the button under the pointer drew no fill"
    );
    cx.simulate_mouse_move(point(px(10.), px(10.)), None, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        shaders(cx),
        lit,
        "inside a cached view, the fill went at once instead of fading out"
    );
    std::thread::sleep(theme::Motion::LEAVE + std::time::Duration::from_millis(60));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(
        shaders(cx),
        0,
        "inside a cached view, the faded fill is still drawn"
    );
}
