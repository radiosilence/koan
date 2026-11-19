use gpui::{
    AppContext as _, Context, Entity, EntityInputHandler as _, IntoElement, Modifiers,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div, point,
    px,
};

use super::*;
// `gpui::*` brings GPUI's `test` attribute, which would expand its own
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
#[gpui::test]
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
#[gpui::test]
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
#[gpui::test]
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
#[gpui::test]
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
#[gpui::test]
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
#[gpui::test]
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
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 2,
    });
    assert_eq!(selected(&host, cx), 6..10, "beta");
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 3,
        first_mouse: false,
    });
    assert_eq!(selected(&host, cx), 0..16, "the line");
}

/// A single line leaves Up to its owner, which a list it leads takes;
/// pasted newlines become spaces.
#[gpui::test]
fn a_single_line_leaves_up_to_its_owner(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.dispatch_action(MoveUp);
    assert_eq!(host.read_with(cx, |h, _| h.ups), 1);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("a\nb".into()));
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
#[gpui::test]
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

/// A masked field asks for a secure keyboard, and what a field holds is told
/// to the platform.
#[gpui::test]
fn a_field_tells_the_platform_what_it_holds(cx: &mut TestAppContext) {
    use gpui::{EntityInputHandler as _, TextContentType};
    let (host, cx) = host(cx, 1);
    let f = field(&host, cx);
    let config = |f: &Entity<FieldState>, cx: &mut VisualTestContext| {
        cx.update(|window, cx| f.update(cx, |f, cx| f.text_input_configuration(window, cx)))
    };
    let plain = config(&f, cx);
    assert!(!plain.secure);
    assert_eq!(plain.content_type, TextContentType::Unspecified);
    f.update(cx, |f, _| {
        f.masked = true;
        f.set_content(TextContentType::CurrentPassword);
    });
    let secret = config(&f, cx);
    assert!(secret.secure);
    assert_eq!(secret.content_type, TextContentType::CurrentPassword);
}

/// A masked field draws bullets and keeps its text off the clipboard.
#[gpui::test]
fn a_masked_field_keeps_its_text(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    let f = field(&host, cx);
    f.update(cx, |f, _| f.masked = true);
    cx.simulate_input("secret");
    assert_eq!(f.read_with(cx, |f, _| f.drawn()), "••••••");
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
    cx.dispatch_action(SelectAll);
    cx.dispatch_action(Copy);
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("before")
    );
}

/// `set_value` says no change and starts the history afresh.
#[gpui::test]
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
#[gpui::test]
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

/// A Japanese input method: kana composed from romaji, marked as it grows,
/// then the kanji chosen committed in its place.
#[gpui::test]
fn a_japanese_input_method_composes_kana_into_kanji(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    let f = field(&host, cx);
    cx.update(|window, cx| {
        f.update(cx, |f, cx| {
            f.replace_and_mark_text_in_range(None, "k", None, window, cx);
            f.replace_and_mark_text_in_range(None, "か", None, window, cx);
            f.replace_and_mark_text_in_range(None, "かん", Some(2..2), window, cx);
        })
    });
    assert_eq!(value(&host, cx), "かん");
    assert_eq!(f.read_with(cx, |f, _| f.marked.clone()), Some(0..6));
    assert_eq!(
        selected(&host, cx),
        6..6,
        "the input method's caret, in bytes"
    );
    let marked = cx.update(|window, cx| f.update(cx, |f, cx| f.marked_text_range(window, cx)));
    assert_eq!(marked, Some(0..2), "and in UTF-16 to the input method");
    cx.update(|window, cx| f.update(cx, |f, cx| f.replace_text_in_range(None, "漢", window, cx)));
    assert_eq!(value(&host, cx), "漢");
    assert_eq!(selected(&host, cx), 3..3);
    cx.simulate_input("字");
    assert_eq!(value(&host, cx), "漢字");
}

/// The caret moves, and Backspace deletes, a character as a reader sees
/// one: an emoji family, a flag, a letter with its combining accent.
#[gpui::test]
fn the_caret_steps_over_whole_characters(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    let family = "👨\u{200d}👩\u{200d}👧";
    let accented = "e\u{301}";
    cx.simulate_input(&format!("a{family}🇯🇵{accented}"));
    cx.dispatch_action(Backspace);
    assert_eq!(value(&host, cx), format!("a{family}🇯🇵"));
    cx.dispatch_action(MoveLeft);
    let after = 1 + family.len();
    assert_eq!(selected(&host, cx), after..after);
    cx.dispatch_action(MoveLeft);
    assert_eq!(selected(&host, cx), 1..1);
    cx.dispatch_action(Delete);
    assert_eq!(value(&host, cx), "a🇯🇵");
}

/// Words are Unicode's: an elided French article is part of its word, and
/// Greek stops where its words do.
#[gpui::test]
fn words_are_unicode_words(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("l'été καλό");
    cx.dispatch_action(MoveWordLeft);
    let at = "l'été ".len();
    assert_eq!(selected(&host, cx), at..at);
    cx.dispatch_action(MoveWordLeft);
    assert_eq!(selected(&host, cx), 0..0, "l'été is one word");
    cx.dispatch_action(SelectWordRight);
    assert_eq!(selected(&host, cx), 0.."l'été".len());
}

/// Hebrew, right to left, goes in, the arrows move through it by its
/// letters, and it is selected and deleted whole.
#[gpui::test]
fn right_to_left_text_edits_sanely(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("שלום");
    cx.run_until_parked();
    assert_eq!(value(&host, cx), "שלום");
    cx.dispatch_action(MoveLeft);
    assert_eq!(selected(&host, cx), 6..6, "one letter, two bytes");
    cx.dispatch_action(SelectAll);
    assert_eq!(selected(&host, cx), 0..8);
    cx.dispatch_action(Backspace);
    assert_eq!(value(&host, cx), "");
}

/// Ten thousand characters typed one at a time, and a long paste, stay
/// whole, the paste undoes alone, and the caret ends at the end.
#[gpui::test]
fn long_text_types_and_pastes(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    let f = field(&host, cx);
    cx.update(|window, cx| {
        f.update(cx, |f, cx| {
            for i in 0..10_000u32 {
                let c = char::from(b'a' + (i % 26) as u8).to_string();
                f.replace_text_in_range(None, &c, window, cx);
            }
        })
    });
    cx.run_until_parked();
    assert_eq!(value(&host, cx).len(), 10_000);
    let pasted = "kōan ".repeat(20_000);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(pasted.clone()));
    cx.dispatch_action(Paste);
    cx.run_until_parked();
    let text = value(&host, cx);
    assert_eq!(text.len(), 10_000 + pasted.len());
    assert_eq!(selected(&host, cx), text.len()..text.len());
    cx.dispatch_action(Undo);
    assert_eq!(value(&host, cx).len(), 10_000, "the paste undoes alone");
}

/// The owner's highlighter colours the text as it stands, after typing
/// and after the owner sets it.
#[gpui::test]
fn a_highlighter_follows_the_text(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    let f = field(&host, cx);
    f.update(cx, |f, _| {
        f.highlighter = Some(std::rc::Rc::new(|text: &str| {
            text.find('%')
                .map(|at| vec![(at..text.len(), crate::theme::Tone::Accent)])
                .unwrap_or_default()
        }))
    });
    cx.simulate_input("a/%b");
    assert_eq!(
        f.read_with(cx, |f, _| f.spans.clone()),
        [(2..4, crate::theme::Tone::Accent)]
    );
    cx.update(|window, cx| f.update(cx, |f, cx| f.set_value("plain", window, cx)));
    assert!(f.read_with(cx, |f, _| f.spans.is_empty()));
}

fn press(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>, clicks: usize) {
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: clicks,
        first_mouse: false,
    });
}

fn drag_to(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
    cx.simulate_event(gpui::MouseMoveEvent {
        position: at,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: Modifiers::default(),
    });
}

/// A drag that leaves the field still selects: past the trailing edge to
/// the end of the line, scrolled there, and back past the leading edge to
/// its start; a word drag likewise extends by words.
#[gpui::test]
fn a_drag_past_the_edges_still_selects(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    let text = "word ".repeat(40);
    cx.simulate_input(&text);
    cx.dispatch_action(MoveHome);
    cx.run_until_parked();
    let f = field(&host, cx);
    let bounds = f.read_with(cx, |f, _| f.bounds.expect("drawn"));
    let middle = point(bounds.left() + px(60.0), bounds.center().y);
    press(cx, middle, 1);
    let from = selected(&host, cx).start;
    assert!(from > 0);
    // Past the edge, it selects to the character under the pointer, out
    // of view, and scrolls to it.
    let edge = f.read_with(cx, |f, _| {
        f.offset_at(point(bounds.right(), bounds.center().y))
    });
    drag_to(cx, point(bounds.right() + px(200.0), bounds.center().y));
    cx.run_until_parked();
    assert!(
        selected(&host, cx).end > edge,
        "beyond what showed at the edge"
    );
    assert!(
        f.read_with(cx, |f, _| f.scroll.x) > px(0.0),
        "scrolled to it"
    );
    drag_to(cx, point(bounds.right() + px(4000.0), bounds.center().y));
    cx.run_until_parked();
    assert_eq!(
        selected(&host, cx).end,
        text.len(),
        "to the end, past the edge"
    );
    drag_to(cx, point(bounds.left() - px(4000.0), bounds.center().y));
    cx.run_until_parked();
    assert_eq!(selected(&host, cx), 0..from, "back to the start");
}

/// The lines of the last frame can point inside a character of text set
/// since: moving down a line, or a click, before the next frame lands on
/// a character's edge and does not panic.
#[gpui::test]
fn moves_before_a_redraw_land_on_characters(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 4);
    cx.simulate_input("abcdef\nabcdef");
    cx.run_until_parked();
    let f = field(&host, cx);
    let bounds = f.read_with(cx, |f, _| f.bounds.expect("drawn"));
    cx.update(|window, cx| {
        f.update(cx, |f, cx| {
            f.set_value("ééé\nüüü", window, cx);
            f.selected = 2..2;
            f.down(&MoveDown, window, cx);
            assert!(f.text.is_char_boundary(f.cursor()));
            let at = f.offset_at(point(bounds.left() + px(25.0), bounds.top() + px(2.0)));
            assert!(f.text.is_char_boundary(at));
        })
    });
    cx.run_until_parked();
    assert_eq!(value(&host, cx), "ééé\nüüü");
}

/// Only a focused field blinks: one emptied while unfocused, as a clear
/// mark or a screen reader does, shows no caret and starts no blink.
#[gpui::test]
fn an_unfocused_field_shows_no_caret(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("abc");
    cx.update(|window, cx| window.blur(cx));
    cx.run_until_parked();
    let f = field(&host, cx);
    cx.update(|window, cx| f.update(cx, |f, cx| f.clean(window, cx)));
    cx.run_until_parked();
    assert_eq!(f.read_with(cx, |f, _| f.caret.get(0)), 0.0);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(f.read_with(cx, |f, _| f.caret.get(0)), 0.0);
}

/// The platform's text services are told a password's bullets, never its
/// characters.
#[gpui::test]
fn a_masked_field_hands_out_bullets(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    let f = field(&host, cx);
    f.update(cx, |f, _| f.masked = true);
    cx.simulate_input("pä😀d");
    let (told, actual) = cx.update(|window, cx| {
        f.update(cx, |f, cx| {
            let mut actual = None;
            let told = f.text_for_range(0..5, &mut actual, window, cx);
            (told, actual)
        })
    });
    // As long as the range it answers, in UTF-16 units.
    assert_eq!(actual, Some(0..5));
    assert_eq!(told.as_deref(), Some("•••••"));
}

/// A click while an input method composes ends the composition where it
/// stands; typing after goes in at the click.
#[gpui::test]
fn a_click_ends_a_composition(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, 1);
    cx.simulate_input("abc ");
    cx.run_until_parked();
    let f = field(&host, cx);
    cx.update(|window, cx| {
        f.update(cx, |f, cx| {
            f.replace_and_mark_text_in_range(None, "に", None, window, cx)
        })
    });
    cx.run_until_parked();
    let bounds = f.read_with(cx, |f, _| f.bounds.expect("drawn"));
    press(cx, point(bounds.left() + px(1.0), bounds.center().y), 1);
    assert_eq!(f.read_with(cx, |f, _| f.marked.clone()), None);
    assert_eq!(selected(&host, cx), 0..0);
    cx.simulate_input("x");
    assert_eq!(value(&host, cx), "xabc に");
}

/// A page holding a field in a cached view, counting its renders.
struct Cached {
    page: Entity<Page>,
}

struct Page {
    field: Entity<FieldState>,
    renders: usize,
}

impl Render for Cached {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.page
                .clone()
                .cached(gpui::StyleRefinement::default().size_full()),
        )
    }
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div().w(px(240.0)).child(TextField::new(&self.field))
    }
}

/// Text from the platform's input handler, with no key event around it (as
/// iOS's text input sends it), redraws the cached view the field is in.
#[gpui::test]
fn text_from_the_platform_redraws_the_view_holding_the_field(cx: &mut TestAppContext) {
    cx.update(crate::theme::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let field = cx.new(|cx| FieldState::new(window, cx));
        Cached {
            page: cx.new(|_| Page { field, renders: 0 }),
        }
    });
    cx.run_until_parked();
    let page = root.read_with(cx, |root, _| root.page.clone());
    let field = page.read_with(cx, |page, _| page.field.clone());
    let before = page.read_with(cx, |page, _| page.renders);
    cx.update(|window, cx| {
        field.update(cx, |field, cx| {
            field.replace_text_in_range(None, "k", window, cx)
        })
    });
    cx.run_until_parked();
    assert!(page.read_with(cx, |page, _| page.renders) > before);
}
