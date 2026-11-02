//! A sheet: the square panel over the window, its head with a title and the
//! actions trailing, its scrolling body; and the one place a question is
//! asked before a write (`confirm`, `name`), drawn over the whole window by
//! `overlay`. A menu that asks is gone the moment it is used, so the
//! question lives here instead.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Button, ButtonKind, TextField};
use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole, label};

/// A sheet's panel: square, `bg`, a hairline `rule`, 560 wide.
pub fn panel(look: &Look) -> Stateful<Div> {
    // A dialog to a screen reader. The sheet's head names it.
    super::accessible(div().id("sheet-panel"), gpui_kit::Role::Dialog, None)
        .flex()
        .flex_col()
        .w(px(560.0))
        .max_h(relative(0.85))
        .bg(look.palette.bg.hsla())
        .border(look.hairline)
        .border_color(look.palette.rule.hsla())
}

/// A sheet that holds a list to pick from (the picker): wider than a
/// question's, and tall from the start, so the list growing as it is
/// typed into moves nothing.
pub fn list_panel(look: &Look) -> Stateful<Div> {
    panel(look).w(px(680.0)).h(relative(0.8)).max_h(px(640.0))
}

/// A wide sheet's panel, 880 wide and as tall as it may be: one whose body
/// ends in a table of paths that fills the rest, as organise's preview does.
pub fn wide_panel(look: &Look) -> Stateful<Div> {
    panel(look).w(px(880.0)).h(relative(0.85))
}

/// A sheet's head: its title in the app's words, and its actions trailing.
pub fn head(title: impl AsRef<str>, actions: Vec<AnyElement>, look: &Look) -> impl IntoElement {
    head_data(label(title), actions, look)
}

/// A sheet's head whose title is data, a profile's name, which keeps its
/// case.
pub fn head_data(
    title: impl Into<SharedString>,
    actions: Vec<AnyElement>,
    look: &Look,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(look.space(Space::M))
        .px(look.space(Space::L))
        .py(look.space(Space::M))
        .border_b(look.hairline)
        .border_color(look.palette.rule.hsla())
        .child(
            div()
                .text_role(TypeRole::Title, look)
                .text_color(look.palette.strong.hsla())
                .child(title.into()),
        )
        .child(div().flex().gap(look.space(Space::M)).children(actions))
}

/// A sheet's scrolling body.
pub fn body(id: impl Into<ElementId>, look: &Look) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_col()
        .gap(look.space(Space::S))
        .px(look.space(Space::L))
        .pb(look.space(Space::L))
        // Shrinks below its content inside the panel's height, and scrolls.
        .min_h_0()
        .overflow_y_scroll()
}

/// What a sheet sits on: the whole window dimmed, taking every click, the
/// panel hung near the top. The owner listens to it for a click outside the
/// panel, which the panel stops.
pub fn scrim(id: impl Into<ElementId>, look: &Look) -> Stateful<Div> {
    div()
        .id(id.into())
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .items_start()
        .pt(look.space(Space::Xxl))
        .bg(look.veil(0.6))
        .occlude()
}

/// A sheet whose head is a search: the field across the top, with a hairline
/// under it.
pub fn search_head(input: &Entity<InputState>, look: &Look) -> impl IntoElement {
    div()
        .px(look.space(Space::L))
        .py(look.space(Space::M))
        .border_b(look.hairline)
        .border_color(look.palette.rule.hsla())
        .child(super::FilterField::new(input).fill())
}

/// A band under the head or over the foot: a sheet's segmented choice.
pub fn band(content: impl IntoElement, look: &Look) -> impl IntoElement {
    div()
        .px(look.space(Space::L))
        .py(look.space(Space::S))
        .border_b(look.hairline)
        .border_color(look.palette.rule.hsla())
        .child(content)
}

/// A sheet's foot: what it says of its state leading, in `fine`, `muted`,
/// and its verbs trailing.
pub fn foot(leading: Vec<AnyElement>, actions: Vec<AnyElement>, look: &Look) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(look.space(Space::S))
        .px(look.space(Space::L))
        .py(look.space(Space::S))
        .border_t(look.hairline)
        .border_color(look.palette.rule.hsla())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(look.space(Space::S))
                .text_role(TypeRole::Fine, look)
                .text_color(look.palette.muted.hsla())
                .children(leading),
        )
        .children(actions)
}

/// A line of `fine` text in a tone, for footers and notes.
pub fn note(text: impl Into<SharedString>, tone: Tone, look: &Look) -> Div {
    div()
        .text_role(TypeRole::Fine, look)
        .text_color(look.text_color(tone))
        .child(text.into())
}

/// What a question shows for what is typed, a line each.
type Preview = Rc<dyn Fn(&str) -> Vec<SharedString>>;
/// What a question does once answered, with what was typed.
type Run = Rc<dyn Fn(String, &mut App)>;

/// A question asked before a write: what will change, and the verb that
/// changes it. With a name field, the preview follows what is typed.
struct Ask {
    title: SharedString,
    verb: SharedString,
    destructive: bool,
    /// What the write will change, a line each.
    preview: Preview,
    /// Said under the preview: what the write leaves alone, or how to undo
    /// it.
    note: Option<SharedString>,
    name: Option<Entity<InputState>>,
    run: Run,
    /// The sheet's own keys, for a question without a field; with one, the
    /// field has them and answers Return itself.
    focus: Option<FocusHandle>,
    /// What had the keyboard when the question was asked, which has it back
    /// once the question is answered or put away: a sheet that asked (the
    /// picker) goes on taking its keys.
    before: Option<FocusHandle>,
    _enter: Option<Subscription>,
}

/// The one place a question is asked, over the whole window: a menu that
/// asks is gone the moment it is used, so the question cannot live there.
#[derive(Default)]
pub struct Asking {
    ask: Option<Ask>,
    /// A sheet of the app's own, a view that lives while it shows (organise's
    /// preview), under any question it asks.
    sheet: Option<AnyView>,
}

/// Shows `view` as the window's sheet, over the shell and under the
/// question host, replacing any sheet showing. The view draws its own
/// `panel`; the host draws the scrim and the placement.
pub fn present(view: impl Into<AnyView>, cx: &mut App) {
    let view = view.into();
    asking(cx).update(cx, |a, cx| {
        a.sheet = Some(view);
        cx.notify();
    });
}

/// Takes the sheet away.
pub fn close(cx: &mut App) {
    asking(cx).update(cx, |a, cx| {
        if a.sheet.take().is_some() {
            cx.notify();
        }
    });
}

/// The sheet presented, placed over the window, for the shell to draw under
/// `overlay`.
pub fn presented(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let view = asking(cx).read(cx).sheet.clone()?;
    let look = Look::of(window, cx);
    Some(
        div()
            .id("sheet")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(look.space(Space::Xxl))
            .bg(look.veil(0.6))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(view)
            .into_any_element(),
    )
}

struct AskingHandle(Entity<Asking>);

impl Global for AskingHandle {}

/// The window's question host, made the first time it is wanted.
pub fn asking(cx: &mut App) -> Entity<Asking> {
    if let Some(handle) = cx.try_global::<AskingHandle>() {
        return handle.0.clone();
    }
    let entity = cx.new(|_| Asking::default());
    cx.set_global(AskingHandle(entity.clone()));
    entity
}

/// What a confirmation shows and does.
pub struct Confirm {
    pub title: SharedString,
    pub verb: SharedString,
    pub destructive: bool,
    pub lines: Vec<SharedString>,
    pub note: Option<SharedString>,
}

/// Asks before `run`: the lines are what it will change.
pub fn confirm(c: Confirm, window: &mut Window, cx: &mut App, run: impl Fn(&mut App) + 'static) {
    let lines = c.lines;
    let before = window.focused(cx);
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    show(
        Ask {
            title: c.title,
            verb: c.verb,
            destructive: c.destructive,
            preview: Rc::new(move |_| lines.clone()),
            note: c.note,
            name: None,
            run: Rc::new(move |_, cx| run(cx)),
            focus: Some(focus),
            before,
            _enter: None,
        },
        cx,
    );
}

/// Asks for a name, saying what it will do with what is typed, before
/// `run` gets it trimmed and not empty.
pub fn name(
    title: impl Into<SharedString>,
    verb: impl Into<SharedString>,
    initial: &str,
    preview: impl Fn(&str) -> Vec<SharedString> + 'static,
    window: &mut Window,
    cx: &mut App,
    run: impl Fn(String, &mut App) + 'static,
) {
    let initial = initial.to_owned();
    let before = window.focused(cx);
    let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
    let focus = input.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
    let focus = None;
    let run: Run = Rc::new(run);
    let enter = {
        let run = run.clone();
        window.subscribe(&input, cx, move |input, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let typed = input.read(cx).value().trim().to_owned();
                if !typed.is_empty() {
                    dismiss(window, cx);
                    run(typed, cx);
                }
            }
        })
    };
    show(
        Ask {
            title: title.into(),
            verb: verb.into(),
            destructive: false,
            preview: Rc::new(preview),
            note: None,
            name: Some(input.clone()),
            run,
            focus,
            before,
            _enter: Some(enter),
        },
        cx,
    );
    // The preview follows the field.
    let host = asking(cx);
    cx.observe(&input, move |_, cx| host.update(cx, |_, cx| cx.notify()))
        .detach();
}

fn show(ask: Ask, cx: &mut App) {
    asking(cx).update(cx, |a, cx| {
        a.ask = Some(ask);
        cx.notify();
    });
}

fn dismiss(window: &mut Window, cx: &mut App) {
    let before = asking(cx).update(cx, |a, cx| {
        cx.notify();
        a.ask.take().and_then(|ask| ask.before)
    });
    if let Some(before) = before {
        window.focus(&before, cx);
    }
}

/// The question being asked, over the window, for the shell to draw.
pub fn overlay(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let host = asking(cx);
    let a = host.read(cx);
    let ask = a.ask.as_ref()?;
    let look = Look::of(window, cx);
    let typed = ask
        .name
        .as_ref()
        .map(|i| i.read(cx).value().trim().to_owned());
    let lines = (ask.preview)(typed.as_deref().unwrap_or(""));
    let blocked = typed.as_ref().is_some_and(String::is_empty);
    let go = {
        let (run, name) = (ask.run.clone(), ask.name.clone());
        move |window: &mut Window, cx: &mut App| {
            let typed = name
                .as_ref()
                .map(|i| i.read(cx).value().trim().to_owned())
                .unwrap_or_default();
            if name.is_some() && typed.is_empty() {
                return;
            }
            dismiss(window, cx);
            run(typed, cx);
        }
    };
    let go = Rc::new(go);
    let named = ask.name.is_some();
    let cancel = Button::new("ask-cancel", ButtonKind::Standard)
        .label("cancel")
        .on_click(|_, window, cx| dismiss(window, cx));
    let verb = {
        let go = go.clone();
        Button::new("ask-go", ButtonKind::Standard)
            .label(ask.verb.as_ref())
            .destructive(ask.destructive)
            .disabled(blocked)
            .on_click(move |_, window, cx| go(window, cx))
    };
    let rows: Vec<AnyElement> = lines
        .into_iter()
        .map(|line| note(line, Tone::Ink, &look).into_any_element())
        .collect();
    Some(
        scrim("ask", &look)
            .key_context("Ask")
            .when_some(ask.focus.as_ref(), |d, focus| d.track_focus(focus))
            .on_key_down(
                move |e: &KeyDownEvent, window, cx| match e.keystroke.key.as_str() {
                    "escape" => dismiss(window, cx),
                    "enter" if !named => go(window, cx),
                    _ => {}
                },
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                panel(&look)
                    .child(head(
                        ask.title.as_ref(),
                        vec![cancel.into_any_element(), verb.into_any_element()],
                        &look,
                    ))
                    .child(
                        body("ask-body", &look)
                            .pt(look.space(Space::M))
                            .when_some(ask.name.as_ref(), |d, input| d.child(TextField::new(input)))
                            .children(rows)
                            .when_some(ask.note.clone(), |d, n| {
                                d.child(note(n, Tone::Muted, &look))
                            }),
                    ),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use core::prelude::v1::test;

    struct Asker {
        focus: FocusHandle,
    }

    impl Render for Asker {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .children(overlay(window, cx))
        }
    }

    /// A question put away with Escape gives the keyboard back to what
    /// asked, so a sheet that asked goes on taking its keys.
    #[gpui_kit::test]
    fn the_keyboard_goes_back_to_what_asked(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (asker, cx) = cx.add_window_view(|_, cx| Asker {
            focus: cx.focus_handle(),
        });
        let cx: &mut VisualTestContext = cx;
        let focus = asker.read_with(cx, |a, _| a.focus.clone());
        cx.update(|window, cx| {
            window.focus(&focus, cx);
            confirm(
                Confirm {
                    title: "replace the queue".into(),
                    verb: "replace".into(),
                    destructive: true,
                    lines: vec!["3 tracks come off the queue.".into()],
                    note: None,
                },
                window,
                cx,
                |_| {},
            );
        });
        let host = cx.update(|_, cx| asking(cx));
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert!(!cx.update(|window, _| focus.is_focused(window)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.update(|_, cx| host.read(cx).ask.is_none()));
        assert!(cx.update(|window, _| focus.is_focused(window)));
    }
}
