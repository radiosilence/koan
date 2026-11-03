//! koan-kit's components as a screen reader meets them: each reports its
//! role, its name and its state to the accessibility tree GPUI builds, so a
//! page made from the kit needs no code of its own to be read.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::accesskit::{Action, ActionRequest, NodeId, TreeId};
use gpui_kit::{
    AnyWindowHandle, Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window,
    div, px,
};
use koan_kit::components::{
    Badge, Banner, Button, ButtonKind, EmptyState, IconToggle, NavNote, NavRow, PageHeader,
    PlayButton, ProgressRow, RowKind, RowList, RowListState, SectionHeading, Segmented,
    ShuffleButton, Slider, Stepper, TableRow, Toggle, Toolbar, spoken,
};
use koan_kit::theme::{self, Icon, Tone};
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
            // Two badges saying the same in rows with no id of their own,
            // told apart by their keys, as the EQ's layer roles are.
            .child(div().child(Badge::new("tuning").key(("role", 0usize))))
            .child(div().child(Badge::new("tuning").key(("role", 1usize))))
            // A slider its owner did not name: its id, read as words.
            .child(Slider::new("wash-tint", 0.5).on_change(|_, _, _| {}))
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
    let window: AnyWindowHandle = cx.add_window(|_, _| page).into();
    attach(window, cx);
    (window, queued, seek)
}

/// A screen reader attaching: GPUI builds the tree only while one is.
fn attach(window: AnyWindowHandle, cx: &mut TestAppContext) {
    window
        .update(cx, |_, window, _| window.set_a11y_active_for_test(true))
        .expect("the window");
    cx.run_until_parked();
}

fn tree(window: AnyWindowHandle, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |_, window, _| window.debug_a11y_tree_json())
        .expect("the window")
        .expect("a tree while accessibility is on")
}

/// Every node of the tree, as its semantics.
fn nodes(tree: &str) -> Vec<Value> {
    let tree: Value = serde_json::from_str(tree).expect("JSON");
    tree["nodes"]
        .as_object()
        .expect("nodes")
        .values()
        .cloned()
        .collect()
}

/// The one node a screen reader knows by `label` in `role`: its AccessKit
/// id and its semantics. Two such, or none, fail.
fn node(tree: &str, label: &str, role: &str) -> (NodeId, Value) {
    let found: Vec<Value> = nodes(tree)
        .into_iter()
        .filter(|node| node["aria"]["label"] == label && node["aria"]["role"] == role)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "{} nodes named {label} in role {role} in:\n{tree}",
        found.len()
    );
    let node = &found[0];
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
    for (name, role) in [
        ("Hounds of Love", "Heading"),
        ("shuffle Hounds of Love", "Button"),
        ("albums", "Heading"),
        ("queue", "Button"),
        ("queue", "Link"),
        ("forget this device", "Button"),
        ("fade on pause", "Switch"),
        ("group by album", "Switch"),
        ("seek", "Slider"),
        ("wash tint", "Slider"),
        ("rate", "SpinButton"),
        ("look", "RadioGroup"),
        ("kōan", "RadioButton"),
        ("scanning the library", "ProgressIndicator"),
        ("no albums", "Group"),
        ("hi-res", "Label"),
    ] {
        node(&tree, name, role);
    }
    // The two "tuning" badges are two nodes, not one id met twice.
    let tunings = nodes(&tree)
        .into_iter()
        .filter(|node| node["aria"]["label"] == "tuning")
        .count();
    assert_eq!(tunings, 2);
}

#[gpui_kit::test]
fn components_report_their_state(cx: &mut TestAppContext) {
    let (window, _, _) = open(cx);
    let tree = tree(window, cx);
    assert_eq!(node(&tree, "fade on pause", "Switch").1["toggled"], "True");
    assert_eq!(
        node(&tree, "group by album", "Switch").1["toggled"],
        "False"
    );
    assert_eq!(node(&tree, "kōan", "RadioButton").1["toggled"], "True");
    assert_eq!(node(&tree, "system", "RadioButton").1["toggled"], "False");
    let (_, seek) = node(&tree, "seek", "Slider");
    assert_eq!(seek["numeric_value"], 0.25);
    assert_eq!(seek["value"], "1:00");
    assert_eq!(node(&tree, "remove", "Button").1["disabled"], true);
    assert!(node(&tree, "queue", "Button").1.get("disabled").is_none());
}

#[gpui_kit::test]
fn components_answer_a_screen_reader_s_actions(cx: &mut TestAppContext) {
    let (window, queued, seek) = open(cx);
    let tree = tree(window, cx);
    send(window, cx, Action::Click, node(&tree, "queue", "Button").0);
    assert_eq!(queued.get(), 1);
    send(
        window,
        cx,
        Action::Increment,
        node(&tree, "seek", "Slider").0,
    );
    assert!((seek.get() - 0.30).abs() < 1e-4, "seek at {}", seek.get());
}

/// A queue as the app draws one: a record's heading over its tracks, each
/// row one node, in a list that draws only what is on screen; the toolbar
/// over it and what the app says on its own.
struct Queue {
    list: RowListState,
    revealed: Rc<Cell<Option<usize>>>,
}

impl Render for Queue {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let revealed = self.revealed.clone();
        div()
            .size(px(800.))
            .flex()
            .flex_col()
            .child(Toolbar::new())
            .child(Banner::new(
                Icon::WarningFilled,
                "the server can't be reached",
                Tone::Bad,
            ))
            .child(NavNote::new("Waiting for the TV to wake", Tone::Muted))
            .child(
                div().h(px(400.)).child(
                    RowList::new(&self.list, move |ix, _, _| {
                        let revealed = revealed.clone();
                        let row = if ix == 0 {
                            TableRow::new(("row", ix), RowKind::Heading).spoken(spoken([
                                "Hounds of Love",
                                "Kate Bush",
                                "",
                                "1985",
                            ]))
                        } else {
                            TableRow::new(("row", ix), RowKind::Text)
                                .spoken(spoken([format!("track {ix}"), "4:01".into()]))
                        };
                        row.place(ix, 3)
                            .on_reveal(move |_, _| revealed.set(Some(ix)))
                            .into_any_element()
                    })
                    .named("queue"),
                ),
            )
    }
}

#[gpui_kit::test]
fn rows_lists_and_announcements(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let list = RowListState::new();
    list.reset(3, RowKind::Text);
    let revealed = Rc::new(Cell::new(None));
    let queue = Queue {
        list,
        revealed: revealed.clone(),
    };
    let window: AnyWindowHandle = cx.add_window(|_, _| queue).into();
    attach(window, cx);
    let tree = tree(window, cx);

    node(&tree, "queue", "List");
    node(&tree, "Hounds of Love, Kate Bush, 1985", "Heading");
    let (row, track) = node(&tree, "track 2, 4:01", "ListItem");
    assert_eq!(track["position_in_set"], 3);
    assert_eq!(track["size_of_set"], 3);
    assert!(
        nodes(&tree)
            .iter()
            .any(|node| node["aria"]["role"] == "Toolbar"),
        "no toolbar in:\n{tree}"
    );

    let (_, banner) = node(&tree, "the server can't be reached", "Alert");
    assert_eq!(banner["live"], "Assertive");
    assert_eq!(banner["value"], "the server can't be reached");
    let (_, note) = node(&tree, "Waiting for the TV to wake", "Status");
    assert_eq!(note["live"], "Polite");

    send(window, cx, Action::ScrollIntoView, row);
    assert_eq!(revealed.get(), Some(2));
}
