//! koan-kit's components as a screen reader meets them: each reports its
//! role, its name and its state to the accessibility tree GPUI builds, so a
//! page made from the kit needs no code of its own to be read.

use gpui_kit::{
    AppContext, Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div,
    px,
};
use koan_kit::components::{
    Badge, Button, ButtonKind, EmptyState, IconToggle, NavRow, PageHeader, PlayButton, ProgressRow,
    SectionHeading, Segmented, ShuffleButton, Slider, Stepper, Toggle,
};
use koan_kit::theme::{self, Icon};

struct Page;

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(800.))
            .flex()
            .flex_col()
            .child(
                PageHeader::data("Hounds of Love")
                    .lead(PlayButton::new("play", "play").on_click(|_, _, _| {}))
                    .lead(ShuffleButton::new("shuffle", "Hounds of Love").on_click(|_, _, _| {}))
                    .badge(Badge::new("hi-res")),
            )
            .child(SectionHeading::new("albums"))
            .child(
                Button::new("queue", ButtonKind::Compact)
                    .label("queue")
                    .on_click(|_, _, _| {}),
            )
            .child(Button::icon_only(
                "forget",
                Icon::Clear,
                "forget this device",
            ))
            .child(Toggle::new("fade", "fade on pause", true).on_change(|_, _, _| {}))
            .child(IconToggle::new(
                "group",
                Icon::Album,
                "group by album",
                false,
            ))
            .child(
                Slider::new("seek", 0.25)
                    .named("seek")
                    .ends("1:00", "4:00")
                    .on_change(|_, _, _| {}),
            )
            .child(Stepper::new("rate", "rate", 3, 1..=9).on_change(|_, _, _| {}))
            .child(Segmented::new("look", ["kōan", "system"], 0).on_change(|_, _, _| {}))
            .child(
                NavRow::new("nav-queue", "queue")
                    .selected(true)
                    .on_click(|_, _, _| {}),
            )
            .child(ProgressRow::new("scan", "scanning the library").fraction(Some(0.5)))
            .child(
                div()
                    .h(px(200.))
                    .child(EmptyState::new(Icon::Album, "no albums")),
            )
    }
}

fn tree(cx: &mut TestAppContext) -> String {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let window = cx.add_window(|_, _| Page);
    cx.run_until_parked();
    window
        .update(cx, |_, window, _| window.debug_a11y_tree_json())
        .expect("the window")
        .expect("a tree while accessibility is on")
}

#[gpui_kit::test]
fn the_kit_s_components_are_in_the_accessibility_tree(cx: &mut TestAppContext) {
    let tree = tree(cx);
    let has = |what: &str| assert!(tree.contains(what), "{what} missing from:\n{tree}");
    for role in [
        "Heading",
        "Button",
        "Switch",
        "Slider",
        "SpinButton",
        "RadioGroup",
        "RadioButton",
        "Link",
        "ProgressIndicator",
        "Label",
        "Group",
    ] {
        has(role);
    }
    for name in [
        "Hounds of Love",
        "shuffle Hounds of Love",
        "albums",
        "forget this device",
        "fade on pause",
        "group by album",
        "seek",
        "scanning the library",
        "no albums",
        "hi-res",
    ] {
        has(name);
    }
    // State: the slider's place and the time it reads as, the toggle on.
    has("1:00");
    has("0.25");
}
