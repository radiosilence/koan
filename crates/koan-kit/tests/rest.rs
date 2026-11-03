//! What draws at rest (#1741): a page of controls nobody is touching puts
//! no shader quad in the scene, so a frame presented for something else
//! (the playing bars, a sleeve arriving) pays for none of them.

use gpui::{
    Context, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, point, px,
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
