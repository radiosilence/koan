//! koan-kit's components as a screen reader meets them: each reports its
//! role, its name and its state to the accessibility tree GPUI builds, so a
//! page made from the kit needs no code of its own to be read.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::accesskit::{Action, ActionRequest, NodeId, TreeId};
use gpui_kit::{
    AnyWindowHandle, AppContext, Context, IntoElement, ParentElement, Render, Styled,
    TestAppContext, Window, div, px,
};
use koan_kit::components::{
    Badge, Button, ButtonKind, EmptyState, IconToggle, NavRow, PageHeader, PlayButton, ProgressRow,
    SectionHeading, Segmented, ShuffleButton, Slider, Stepper, Toggle,
};
use koan_kit::theme::{self, Icon};
use serde_json::Value;

#[derive(Default)]
struct Page {
    queued: Rc<Cell<u32>>,
    seek: Rc<Cell<f32>>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let queued = self.queued.clone();
        let seek = self.seek.clone();
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
                    .on_click(move |_, _, _| queued.set(queued.get() + 1)),
            )
            .child(
                Button::new("remove", ButtonKind::Compact)
                    .label("remove")
                    .disabled(true)
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
                    .step(0.05)
                    .on_change(move |value, _, _| seek.set(*value)),
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

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Rc<Cell<u32>>, Rc<Cell<f32>>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let page = Page::default();
    let (queued, seek) = (page.queued.clone(), page.seek.clone());
    let window = cx.add_window(|_, _| page);
    cx.run_until_parked();
    (window.into(), queued, seek)
}

fn tree(window: AnyWindowHandle, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |_, window, _| window.debug_a11y_tree_json())
        .expect("the window")
        .expect("a tree while accessibility is on")
}

/// The node a screen reader knows by `label`: its AccessKit id and its
/// semantics.
fn node(tree: &str, label: &str) -> (NodeId, Value) {
    let tree: Value = serde_json::from_str(tree).expect("JSON");
    let node = tree["nodes"]
        .as_object()
        .expect("nodes")
        .values()
        .find(|node| node["aria"]["label"] == label)
        .unwrap_or_else(|| panic!("no node named {label}"));
    let id = node["accesskit_id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a number");
    (NodeId(id), node["aria"].clone())
}

fn send(window: AnyWindowHandle, cx: &mut TestAppContext, action: Action, target: NodeId) {
    window
        .update(cx, |_, window, cx| {
            window.dispatch_a11y_action(
                ActionRequest {
                    action,
                    target_tree: TreeId::ROOT,
                    target_node: target,
                    data: None,
                },
                cx,
            )
        })
        .expect("the window");
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_kit_s_components_are_in_the_accessibility_tree(cx: &mut TestAppContext) {
    let (window, _, _) = open(cx);
    let tree = tree(window, cx);
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
}

#[gpui_kit::test]
fn components_report_their_state(cx: &mut TestAppContext) {
    let (window, _, _) = open(cx);
    let tree = tree(window, cx);
    assert_eq!(node(&tree, "fade on pause").1["toggled"], "True");
    assert_eq!(node(&tree, "group by album").1["toggled"], "False");
    assert_eq!(node(&tree, "kōan").1["toggled"], "True");
    assert_eq!(node(&tree, "system").1["toggled"], "False");
    let (_, seek) = node(&tree, "seek");
    assert_eq!(seek["numeric_value"], 0.25);
    assert_eq!(seek["value"], "1:00");
    assert_eq!(node(&tree, "remove").1["disabled"], true);
    assert!(node(&tree, "queue").1.get("disabled").is_none());
}

#[gpui_kit::test]
fn components_answer_a_screen_reader_s_actions(cx: &mut TestAppContext) {
    let (window, queued, seek) = open(cx);
    let tree = tree(window, cx);
    send(window, cx, Action::Click, node(&tree, "queue").0);
    assert_eq!(queued.get(), 1);
    send(window, cx, Action::Increment, node(&tree, "seek").0);
    assert!((seek.get() - 0.30).abs() < 1e-4, "seek at {}", seek.get());
}
