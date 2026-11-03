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

/// Keeps Tab and Shift-Tab among a sheet's own controls while it is up: a
/// sheet drawn by its owner rather than presented through `present`.
pub fn trap_focus(panel: Stateful<Div>, focus: &FocusHandle) -> impl IntoElement {
    use gpui_kit::base::FocusTrapElement as _;
    panel.focus_trap("sheet-trap", focus)
}

/// Tab and Shift-Tab: the next control in the window's order, kept inside
/// the innermost sheet, question or menu holding the keyboard, as the
/// window's root keeps it. For an app that binds Tab in a context of its
/// own, which the root's binding never reaches.
pub fn step_focus(forward: bool, window: &mut Window, cx: &mut App) {
    let advance = |window: &mut Window, cx: &mut App| {
        if forward {
            window.focus_next(cx)
        } else {
            window.focus_prev(cx)
        }
    };
    let Some(container) = gpui_kit::base::active_focus_trap(window, cx) else {
        advance(window, cx);
        return;
    };
    advance(window, cx);
    let mut visited = Vec::new();
    while !container.contains_focused(window, cx) {
        let focused = window.focused(cx);
        if visited.contains(&focused) {
            // Nothing in it takes Tab: the container keeps the keyboard.
            container.focus(window, cx);
            break;
        }
        visited.push(focused);
        advance(window, cx);
    }
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
    /// The sheet's keys, so the window's bare keys (Space, the arrows) do
    /// not reach the page under it, and what had them before it showed.
    sheet_focus: Option<(FocusHandle, Option<FocusHandle>)>,
    /// Given back on the next draw after the sheet closed, where closing
    /// had no window to give it back in.
    restore: Option<(FocusHandle, Option<FocusHandle>)>,
    /// The sheet is busy with something it must see through (organise
    /// moving files): Escape leaves it be.
    held: bool,
}

/// Shows `view` as the window's sheet, over the shell and under the
/// question host, replacing any sheet showing. The view draws its own
/// `panel`; the host draws the scrim and the placement, and holds the
/// keyboard while it shows, as a question does.
pub fn present(view: impl Into<AnyView>, window: &mut Window, cx: &mut App) {
    let view = view.into();
    let before = window.focused(cx);
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    asking(cx).update(cx, |a, cx| {
        // A sheet replacing another gives the keyboard back, when it goes,
        // to what had it before the first.
        let before = match a.sheet_focus.take() {
            Some((_, earlier)) => earlier,
            None => before,
        };
        a.sheet = Some(view);
        a.sheet_focus = Some((focus, before));
        a.restore = None;
        a.held = false;
        cx.notify();
    });
}

/// While `held`, the sheet presented is busy with something it must see
/// through, such as files on the move: Escape leaves it showing, as its
/// disabled cancel does. Closing it lets go.
pub fn hold(held: bool, cx: &mut App) {
    asking(cx).update(cx, |a, _| a.held = held);
}

/// Takes the sheet away. The keyboard goes back to what had it before, on
/// the window's next draw.
pub fn close(cx: &mut App) {
    asking(cx).update(cx, |a, cx| {
        a.held = false;
        if a.sheet.take().is_some() {
            a.restore = a.sheet_focus.take();
            cx.notify();
        }
    });
}

/// Where an overlay stands among the window's layers, lowest first. Every
/// layer is drawn after the window's content and the playing bars painted
/// over it, so nothing on a page shows through a panel or a sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// A panel over a page: a track's details.
    Panel = 1,
    /// A sheet over the window: organise, the picker, the shortcuts, the
    /// artwork viewer.
    Sheet = 2,
    /// A question asked over everything, sheets included.
    Question = 3,
}

/// `element` drawn on `layer`, covering its parent: the parent lays it out,
/// and it is painted once the window's content is.
pub fn layered(layer: Layer, element: impl IntoElement) -> impl IntoElement {
    deferred(div().absolute().inset_0().child(element)).with_priority(layer as usize)
}

/// The sheet presented, placed over the window, for the shell to draw under
/// `overlay`.
pub fn presented(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let asking = asking(cx);
    if let Some((sheet, before)) = asking.update(cx, |a, _| a.restore.take())
        // Unless something in the sheet's last moments took it elsewhere.
        && (sheet.contains_focused(window, cx) || window.focused(cx).is_none())
        && let Some(before) = before
    {
        window.focus(&before, cx);
    }
    let (view, focus) = {
        let a = asking.read(cx);
        (
            a.sheet.clone()?,
            a.sheet_focus.as_ref().map(|(f, _)| f.clone()),
        )
    };
    let look = Look::of(window, cx);
    let scrim = div()
        .id("sheet")
        // The window's bare keys are bound outside `Ask`.
        .key_context("Ask")
        .when_some(focus.as_ref(), |d, focus| d.track_focus(focus))
        // Escape puts it away as its cancel does, once nothing inside it
        // (a field, a menu) has taken the key for its own.
        .on_key_down({
            let host = asking.clone();
            move |e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    if !host.read(cx).held {
                        close(cx);
                    }
                }
            }
        })
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .items_start()
        .pt(look.space(Space::Xxl))
        .bg(look.veil(0.6))
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(view);
    Some(trapped(scrim, focus.as_ref()))
}

/// A scrim holding the keyboard keeps Tab inside it.
fn trapped(scrim: Stateful<Div>, focus: Option<&FocusHandle>) -> AnyElement {
    use gpui_kit::base::FocusTrapElement as _;
    match focus {
        Some(focus) => scrim.focus_trap("sheet-trap", focus).into_any_element(),
        None => scrim.into_any_element(),
    }
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
    let focus = ask.focus.clone();
    let scrim = scrim("ask", &look)
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
        );
    Some(trapped(scrim, focus.as_ref()))
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

    struct Presenter {
        focus: FocusHandle,
    }

    impl Render for Presenter {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .children(presented(window, cx))
        }
    }

    struct Panel;

    impl Render for Panel {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// A sheet presented over the window goes with Escape, as its cancel
    /// does, and gives the keyboard back to what presented it.
    #[gpui_kit::test]
    fn a_presented_sheet_goes_with_escape(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (presenter, cx) = cx.add_window_view(|_, cx| Presenter {
            focus: cx.focus_handle(),
        });
        let cx: &mut VisualTestContext = cx;
        let focus = presenter.read_with(cx, |p, _| p.focus.clone());
        cx.update(|window, cx| {
            window.focus(&focus, cx);
            let panel = cx.new(|_| Panel);
            present(panel, window, cx);
        });
        let host = cx.update(|_, cx| asking(cx));
        cx.update(|_, cx| presenter.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert!(cx.update(|_, cx| host.read(cx).sheet.is_some()));
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| presenter.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert!(cx.update(|_, cx| host.read(cx).sheet.is_none()));
        assert!(cx.update(|window, _| focus.is_focused(window)));
    }

    /// A sheet busy with something it must see through stays through
    /// Escape, and goes with it again once it lets go.
    #[gpui_kit::test]
    fn a_held_sheet_stays_through_escape(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (presenter, cx) = cx.add_window_view(|_, cx| Presenter {
            focus: cx.focus_handle(),
        });
        let cx: &mut VisualTestContext = cx;
        let focus = presenter.read_with(cx, |p, _| p.focus.clone());
        cx.update(|window, cx| {
            window.focus(&focus, cx);
            let panel = cx.new(|_| Panel);
            present(panel, window, cx);
            hold(true, cx);
        });
        let host = cx.update(|_, cx| asking(cx));
        let redraw = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| presenter.update(cx, |_, cx| cx.notify()));
            cx.run_until_parked();
        };
        redraw(cx);
        cx.simulate_keystrokes("escape");
        redraw(cx);
        assert!(
            cx.update(|_, cx| host.read(cx).sheet.is_some()),
            "held, it stays"
        );
        cx.update(|_, cx| hold(false, cx));
        cx.simulate_keystrokes("escape");
        redraw(cx);
        assert!(
            cx.update(|_, cx| host.read(cx).sheet.is_none()),
            "let go, it goes"
        );
    }
}
