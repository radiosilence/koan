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

#[gpui::test]
fn a_fill_fades_out_after_the_pointer_leaves(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Page);
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.simulate_mouse_move(point(px(10.), px(50.)), None, Modifiers::default());
    cx.run_until_parked();
    assert!(shaders(cx) > 0, "the button under the pointer drew no fill");

    cx.simulate_mouse_move(point(px(500.), px(700.)), None, Modifiers::default());
    assert!(
        shaders(cx) > 0,
        "the fill was gone the frame the pointer left, with no fade"
    );
}

struct Switches {
    on: bool,
}

impl Render for Switches {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        div().size_full().pt(px(40.)).child(
            koan_kit::components::Toggle::new("switch", "Switch", self.on).on_change(
                move |on, _, cx| {
                    let on = *on;
                    view.update(cx, |this, cx| {
                        this.on = on;
                        cx.notify();
                    })
                },
            ),
        )
    }
}

#[gpui::test]
fn a_toggle_turned_by_a_click_draws_its_tick_in(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Switches { on: false });
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.simulate_click(point(px(10.), px(50.)), Modifiers::default());
    assert!(shaders(cx) > 0, "the tick did not draw itself in");
}
