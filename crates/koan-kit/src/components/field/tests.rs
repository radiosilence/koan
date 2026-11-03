use gpui_kit::{
    AppContext as _, Context, Entity, EntityInputHandler as _, IntoElement, Modifiers,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div, point,
    px,
};

use super::*;
// `gpui_kit::*` brings GPUI's `test` attribute, which would expand its own
// `#[test]` into itself.
use core::prelude::v1::test;

/// A page holding a field, counting the Up its field leaves to it.
struct Host {
    field: Entity<FieldState>,
    ups: usize,
    events: Vec<InputEvent>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(240.0))
            .on_action(cx.listener(|this, _: &MoveUp, _, _| this.ups += 1))
            .child(TextField::new(&self.field))
    }
}

fn host(cx: &mut TestAppContext, rows: usize) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let (host, cx) = cx.add_window_view(|window, cx| {
        let field = cx.new(|cx| FieldState::new(window, cx).rows(rows));
        cx.subscribe(&field, |host: &mut Host, _, event: &InputEvent, _| {
            host.events.push(event.clone())
        })
        .detach();
        Host {
            field,
            ups: 0,
            events: Vec::new(),
        }
    });
    let field = host.read_with(cx, |h, _| h.field.clone());
    cx.update(|window, cx| {
        let focus = field.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    cx.run_until_parked();
    (host, cx)
}

fn field(host: &Entity<Host>, cx: &mut VisualTestContext) -> Entity<FieldState> {
    host.read_with(cx, |h, _| h.field.clone())
}

fn value(host: &Entity<Host>, cx: &mut VisualTestContext) -> String {
    let field = field(host, cx);
    field.read_with(cx, |f, _| f.value().to_string())
}

fn selected(host: &Entity<Host>, cx: &mut VisualTestContext) -> std::ops::Range<usize> {
    let field = field(host, cx);
    field.read_with(cx, |f, _| f.selected.clone())
}

/// Typing goes in at the caret, and Backspace, Delete and the arrows edit
/// around it; each change is said.
#[gpui_kit::test]
fn typing_and_editing(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("hello world");
    assert_eq!(value(&host, cx), "hello world");
    cx.dispatch_action(Backspace);
    assert_eq!(value(&host, cx), "hello worl");
    cx.dispatch_action(MoveWordLeft);
    cx.dispatch_action(Delete);
    assert_eq!(value(&host, cx), "hello orl");
    cx.dispatch_action(MoveHome);
    cx.simulate_input("» ");
    assert_eq!(value(&host, cx), "» hello orl");
    let changes = host.read_with(cx, |h, _| {
        h.events
            .iter()
            .filter(|e| **e == InputEvent::Change)
            .count()
    });
    assert!(changes >= 4, "{changes} changes said");
}

/// Typing undoes as one step, as on the Mac; redo puts it back.
#[gpui_kit::test]
fn typing_undoes_as_one_step(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("abc");
    cx.dispatch_action(SelectAll);
    cx.dispatch_action(Backspace);
    assert_eq!(value(&host, cx), "");
    cx.dispatch_action(Undo);
    assert_eq!(value(&host, cx), "abc");
    cx.dispatch_action(Undo);
    assert_eq!(value(&host, cx), "");
    cx.dispatch_action(Redo);
    assert_eq!(value(&host, cx), "abc");
}

/// An input method composes in place, its text marked, and commits as one
/// step of the history.
#[gpui_kit::test]
fn an_input_method_composes_and_commits(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("x ");
    let f = field(&host, cx);
    cx.update(|window, cx| {
        f.update(cx, |f, cx| {
            f.replace_and_mark_text_in_range(None, "n", None, window, cx);
            f.replace_and_mark_text_in_range(None, "ni", None, window, cx);
        })
    });
    let marked = f.read_with(cx, |f, _| f.marked.clone());
    assert_eq!(value(&host, cx), "x ni");
    assert_eq!(marked, Some(2..4));
    cx.update(|window, cx| f.update(cx, |f, cx| f.replace_text_in_range(None, "你", window, cx)));
    assert_eq!(value(&host, cx), "x 你");
    assert_eq!(f.read_with(cx, |f, _| f.marked.clone()), None);
    assert_eq!(
        selected(&host, cx),
        5..5,
        "the caret after the committed word"
    );
    cx.dispatch_action(Undo);
    assert_eq!(value(&host, cx), "x ", "the composition undoes as one step");
}

/// The input method's offsets are UTF-16, the text's UTF-8: a character
/// beyond the basic plane counts two.
#[gpui_kit::test]
fn input_method_offsets_are_utf16(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("a😀b");
    let f = field(&host, cx);
    let (length, range) = cx.update(|window, cx| {
        f.update(cx, |f, cx| {
            let length = f.text_length_utf16(window, cx);
            f.set_selected_text_range(3..4, window, cx);
            (length, f.selected.clone())
        })
    });
    assert_eq!(length, Some(4));
    assert_eq!(range, 5..6, "the b after the emoji's four bytes");
}

/// Shift with the arrows selects from where the caret was, either way;
/// Select All takes the whole text, and typing replaces a selection.
#[gpui_kit::test]
fn selecting_with_the_keys(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("one two three");
    cx.dispatch_action(SelectWordLeft);
    assert_eq!(selected(&host, cx), 8..13);
    cx.dispatch_action(SelectLeft);
    assert_eq!(selected(&host, cx), 7..13);
    cx.dispatch_action(MoveRight);
    assert_eq!(
        selected(&host, cx),
        13..13,
        "Right goes to the selection's end"
    );
    cx.dispatch_action(SelectHome);
    assert_eq!(selected(&host, cx), 0..13);
    cx.simulate_input("four");
    assert_eq!(value(&host, cx), "four");
}

/// A double click selects the word under it, a triple click the line.
#[gpui_kit::test]
fn double_and_triple_clicks(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("alpha beta gamma");
    cx.run_until_parked();
    let f = field(&host, cx);
    let (bounds, x) = f.read_with(cx, |f, _| {
        let line = &f.lines[0].shaped;
        (f.bounds.expect("drawn"), line.x_for_index(8))
    });
    let at = point(bounds.left() + x, bounds.center().y);
    cx.simulate_event(gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui_kit::MouseUpEvent {
        button: gpui_kit::MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 2,
    });
    assert_eq!(selected(&host, cx), 6..10, "beta");
    cx.simulate_event(gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 3,
        first_mouse: false,
    });
    assert_eq!(selected(&host, cx), 0..16, "the line");
}

/// A single line leaves Up to its owner, which a list it leads takes;
/// pasted newlines become spaces.
#[gpui_kit::test]
fn a_single_line_leaves_up_to_its_owner(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.dispatch_action(MoveUp);
    assert_eq!(host.read_with(cx, |h, _| h.ups), 1);
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("a\nb".into()));
    cx.dispatch_action(Paste);
    assert_eq!(value(&host, cx), "a b");
    cx.dispatch_action(Enter);
    assert!(
        host.read_with(cx, |h, _| h.events.contains(&InputEvent::PressEnter {
            secondary: false,
            shift: false
        })),
        "Return is said to the owner"
    );
}

/// Several lines take Return as a new line and move between lines with
/// the arrows, keeping the distance across.
#[gpui_kit::test]
fn several_lines(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 4);
    cx.simulate_input("abcd");
    cx.dispatch_action(Enter);
    cx.simulate_input("ef");
    assert_eq!(value(&host, cx), "abcd\nef");
    cx.run_until_parked();
    cx.dispatch_action(MoveUp);
    assert_eq!(
        host.read_with(cx, |h, _| h.ups),
        0,
        "a multi-line field keeps Up"
    );
    assert_eq!(selected(&host, cx), 2..2);
    cx.dispatch_action(MoveDown);
    assert_eq!(selected(&host, cx), 7..7);
}

/// A masked field draws bullets and keeps its text off the clipboard.
#[gpui_kit::test]
fn a_masked_field_keeps_its_text(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    let f = field(&host, cx);
    f.update(cx, |f, _| f.masked = true);
    cx.simulate_input("secret");
    assert_eq!(f.read_with(cx, |f, _| f.drawn()), "••••••");
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("before".into()));
    cx.dispatch_action(SelectAll);
    cx.dispatch_action(Copy);
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("before")
    );
}

/// `set_value` says no change and starts the history afresh.
#[gpui_kit::test]
fn set_value_is_the_owners(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("typed");
    let f = field(&host, cx);
    cx.update(|window, cx| f.update(cx, |f, cx| f.set_value("set", window, cx)));
    let changes_before = host.read_with(cx, |h, _| h.events.len());
    assert_eq!(value(&host, cx), "set");
    cx.dispatch_action(Undo);
    assert_eq!(value(&host, cx), "set", "nothing before the owner's value");
    assert_eq!(host.read_with(cx, |h, _| h.events.len()), changes_before);
}

/// A long line scrolls to keep the caret in view, and back at Home.
#[gpui_kit::test]
fn a_long_line_scrolls_to_the_caret(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input(&"word ".repeat(40));
    cx.run_until_parked();
    let f = field(&host, cx);
    let scrolled = f.read_with(cx, |f, _| f.scroll.x);
    assert!(scrolled > px(0.0), "scrolled {scrolled:?}");
    cx.dispatch_action(MoveHome);
    cx.run_until_parked();
    assert_eq!(f.read_with(cx, |f, _| f.scroll.x), px(0.0));
}
