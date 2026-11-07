//! A sheet: the square panel over the window, its head with a title and the
//! actions trailing, its scrolling body; and the one place a question is
//! asked before a write (`confirm`, `name`), drawn over the whole window by
//! `overlay`. A menu that asks is gone the moment it is used, so the
//! question lives here instead.

use std::rc::Rc;
use std::time::Instant;

use crate::components::field::{FieldState as InputState, InputEvent};
use gpui::prelude::*;
use gpui::*;

use super::{Button, ButtonKind, Fading, Rise, TextField, Veil};
use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole, label};

/// A sheet's panel: square, `bg`, a hairline `rule`, 560 wide. A dialog to
/// a screen reader, named `name` in the app's words: what its head says.
/// As tall as its content up to the room the window leaves it, where its
/// `body` scrolls; a view drawn in it makes `content` its root, or its body
/// cannot shrink.
pub fn panel(name: impl AsRef<str>, look: &Look) -> Stateful<Div> {
    super::accessible(
        div().id("sheet-panel"),
        gpui::Role::Dialog,
        Some(label(name)),
    )
    .flex()
    .flex_col()
    .w(px(560.0))
    .max_w_full()
    .max_h(relative(0.85))
    // Shrinks within what holds it, its body scrolling.
    .min_h_0()
    .bg(look.palette.bg.hsla())
    .border(look.hairline)
    .border_color(look.palette.rule.hsla())
}

/// Keeps Tab and Shift-Tab among a sheet's own controls while it is up: a
/// sheet drawn by its owner rather than presented through `present`.
pub fn trap_focus(panel: Stateful<Div>, focus: &FocusHandle, cx: &mut App) -> impl IntoElement {
    use super::focus_trap::FocusTrap as _;
    panel.focus_trap(focus, cx)
}

/// Tab and Shift-Tab: the next control in the window's order, kept inside
/// the innermost sheet, question or menu holding the keyboard, as the
/// window's root keeps it. For an app that binds Tab in a context of its
/// own, which the root's binding never reaches.
pub fn step_focus(forward: bool, window: &mut Window, cx: &mut App) {
    super::focus_trap::step(forward, window, cx);
}

/// A panel taking all the room a phone leaves a sheet, inside the safe
/// area and clear of the keyboard: a sheet that is a screen on a phone (a
/// list to choose from, a flow of steps), its body filling what its content
/// does not. Content-sized on the desktop.
pub fn fill(panel: Stateful<Div>, look: &Look, window: &Window) -> Stateful<Div> {
    if !look.touch {
        return panel;
    }
    let room = room(look, window);
    panel.h(room).max_h(room)
}

/// The height a sheet's panel may take on a phone: the window less the
/// system's insets and the keyboard, a step off each edge.
pub fn room(look: &Look, window: &Window) -> Pixels {
    let insets = super::system_insets(super::Clear::Keyboard, window);
    window.viewport_size().height - insets.top - insets.bottom - look.space(Space::L) * 2.
}

/// The root of a view drawn in a panel: a column that shrinks to the
/// panel's height, so its `body` scrolls, and grows to fill a `fill` panel.
pub fn content() -> Div {
    div().flex().flex_col().flex_grow().flex_shrink().min_h_0()
}

/// A wide sheet's panel, 880 wide and as tall as it may be: one whose body
/// ends in a table of paths that fills the rest, as organise's preview does.
pub fn wide_panel(name: impl AsRef<str>, look: &Look) -> Stateful<Div> {
    panel(name, look).w(px(880.0)).h(relative(0.85))
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
        // A title too long for a phone's sheet beside its actions wraps;
        // the actions keep their width.
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_role(TypeRole::Title, look)
                .text_color(look.palette.strong.hsla())
                .child(title.into()),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .gap(look.space(Space::M))
                .children(actions),
        )
}

/// A sheet's scrolling body.
pub fn body(id: impl Into<ElementId>, look: &Look) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_col()
        .gap(look.space(Space::S))
        .px(look.space(Space::L))
        // A step off the head's rule, as most sheets set it for themselves.
        .pt(look.space(Space::M))
        .pb(look.space(Space::L))
        // Shrinks below its content inside the panel's height, and scrolls;
        // in a `fill` panel, takes what the head and foot leave.
        .flex_grow()
        .min_h_0()
        .overflow_y_scroll()
}

/// What a sheet sits on: the whole window dimmed, taking every click, the
/// panel hung near the top. The owner listens to it for a click outside the
/// panel, which the panel stops. The dim fades in as it comes up; the owner
/// draws its panel in a `Rise`.
///
/// On a phone the panel stands on the foot of the window instead, the
/// width less a margin, inside the safe area on every edge and clear of the
/// keyboard while it is up, as an iOS sheet does; its `Rise` brings it up
/// from there.
pub fn scrim(id: impl Into<ElementId>, look: &Look, window: &Window) -> Stateful<Div> {
    if !look.touch {
        return scrim_top(id, look);
    }
    let insets = super::system_insets(super::Clear::Keyboard, window);
    div()
        .id(id.into())
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .items_end()
        .pl(insets.left + look.space(Space::S))
        .pr(insets.right + look.space(Space::S))
        .pt(insets.top + look.space(Space::L))
        .pb(insets.bottom + look.space(Space::L))
        .occlude()
        .child(Veil::new(look.veil(0.6)))
}

/// A scrim with the panel hung near the top on every platform: the search
/// box's, whose results run down toward the keyboard.
pub fn scrim_top(id: impl Into<ElementId>, look: &Look) -> Stateful<Div> {
    div()
        .id(id.into())
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .items_start()
        .pt(look.space(Space::Xxl))
        // On a phone the panel takes the width less a margin, under the
        // status bar.
        .when(look.touch, |scrim| {
            scrim
                .px(look.space(Space::S))
                .pt(look.space(Space::Xxl) * 2.)
        })
        .occlude()
        .child(Veil::new(look.veil(0.6)))
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
    /// The sheet and the question put away last, fading out where they
    /// stood.
    leaving_sheet: Option<(AnyView, Fading)>,
    leaving_ask: Option<(Ask, Fading)>,
}

/// Lets go, once a fade started now has run, of what began fading by then.
fn let_go(cx: &mut App) {
    let host = asking(cx);
    let started = Instant::now();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Fading::LENGTH).await;
        cx.update(|cx| {
            host.update(cx, |a, cx| {
                let sheet = a.leaving_sheet.take_if(|(_, f)| f.began_by(started));
                let ask = a.leaving_ask.take_if(|(_, f)| f.began_by(started));
                if sheet.is_some() || ask.is_some() {
                    cx.notify();
                }
            })
        });
    })
    .detach();
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
        a.leaving_sheet = None;
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
    let fading = asking(cx).update(cx, |a, cx| {
        a.held = false;
        let view = a.sheet.take()?;
        a.restore = a.sheet_focus.take();
        a.leaving_sheet = Fading::start(cx).map(|f| (view, f));
        cx.notify();
        a.leaving_sheet.is_some().then_some(())
    });
    if fading.is_some() {
        let_go(cx);
    }
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

/// The key context of everything drawn over the window: the app binds its
/// shortcuts outside it, so none acts behind a sheet or a question.
pub const MODAL: &str = "Modal";

/// `element` drawn on `layer`, covering its parent: the parent lays it out,
/// and it is painted once the window's content is.
///
/// It holds the keyboard as a modal: its keys are `MODAL`'s, and a ⌘ or Ctrl
/// key nothing inside bound is taken here, so the macOS menu bar's key
/// equivalent behind it does not act either. Keys that type are left alone.
pub fn layered(layer: Layer, element: impl IntoElement) -> impl IntoElement {
    deferred(
        div()
            .absolute()
            .inset_0()
            .key_context(MODAL)
            .on_key_down(|e: &KeyDownEvent, _, cx| {
                let m = &e.keystroke.modifiers;
                if m.platform || m.control || (m.alt && e.keystroke.key_char.is_none()) {
                    cx.stop_propagation();
                }
            })
            .child(element),
    )
    .with_priority(layer as usize)
}

/// The sheet presented, placed over the window, for the shell to draw under
/// `overlay`.
pub fn presented(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let asking = asking(cx);
    if let Some((sheet, before)) = asking.update(cx, |a, _| a.restore.take())
        // Unless something in the sheet's last moments took it elsewhere.
        && (sheet.contains_focused(window, cx) || window.focused(cx).is_none())
    {
        // Nothing had it before: the sheet fading out does not keep it.
        match before {
            Some(before) => window.focus(&before, cx),
            None => window.blur(cx),
        }
    }
    // Put away, it is drawn where it stood as it fades, holding nothing.
    let (view, focus, fading) = {
        let a = asking.read(cx);
        match (&a.sheet, &a.leaving_sheet) {
            (Some(view), _) => (
                view.clone(),
                a.sheet_focus.as_ref().map(|(f, _)| f.clone()),
                None,
            ),
            (None, Some((view, fading))) => (view.clone(), None, Some(fading.clone())),
            (None, None) => return None,
        }
    };
    let look = Look::of(window, cx);
    let scrim = scrim("sheet", &look, window)
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
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(Rise::new(view));
    match fading {
        Some(fading) => Some(fading.draw(scrim)),
        None => Some(trapped(scrim, focus.as_ref(), cx)),
    }
}

/// A scrim holding the keyboard keeps Tab inside it.
fn trapped(scrim: Stateful<Div>, focus: Option<&FocusHandle>, cx: &mut App) -> AnyElement {
    use super::focus_trap::FocusTrap as _;
    match focus {
        Some(focus) => scrim.focus_trap(focus, cx).into_any_element(),
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
        a.leaving_ask = None;
        cx.notify();
    });
}

/// Puts the question away. Whether there was one to put away: a question
/// already fading, its buttons still drawn, has nothing left to answer.
fn dismiss(window: &mut Window, cx: &mut App) -> bool {
    let Some((before, fading)) = asking(cx).update(cx, |a, cx| {
        cx.notify();
        let mut ask = a.ask.take()?;
        ask._enter = None;
        let before = ask.before.take();
        a.leaving_ask = Fading::start(cx).map(|f| (ask, f));
        Some((before, a.leaving_ask.is_some()))
    }) else {
        return false;
    };
    match before {
        Some(before) => window.focus(&before, cx),
        // Nothing had the keyboard before: the field fading out does not
        // keep it.
        None => window.blur(cx),
    }
    if fading {
        let_go(cx);
    }
    true
}

/// The question being asked, over the window, for the shell to draw.
pub fn overlay(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let host = asking(cx);
    let a = host.read(cx);
    // Put away, it is drawn where it stood as it fades, taking no keys.
    let (ask, fading) = match (&a.ask, &a.leaving_ask) {
        (Some(ask), _) => (ask, None),
        (None, Some((ask, fading))) => (ask, Some(fading.clone())),
        (None, None) => return None,
    };
    let live = fading.is_none();
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
            if dismiss(window, cx) {
                run(typed, cx);
            }
        }
    };
    let go = Rc::new(go);
    let named = ask.name.is_some();
    let cancel = Button::new("ask-cancel", ButtonKind::Standard)
        .label("cancel")
        .on_click(|_, window, cx| {
            dismiss(window, cx);
        });
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
    let focus = ask.focus.clone().filter(|_| live);
    let scrim = scrim("ask", &look, window)
        .key_context("Ask")
        .when_some(focus.as_ref(), |d, focus| d.track_focus(focus))
        .when(live, |d| {
            d.on_key_down(
                move |e: &KeyDownEvent, window, cx| match e.keystroke.key.as_str() {
                    "escape" => {
                        dismiss(window, cx);
                    }
                    "enter" if !named => go(window, cx),
                    _ => {}
                },
            )
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(Rise::new(
            panel(ask.title.as_ref(), &look)
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
        ));
    match fading {
        Some(fading) => Some(fading.draw(scrim)),
        None => Some(trapped(scrim, focus.as_ref(), cx)),
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

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
    #[gpui::test]
    fn the_keyboard_goes_back_to_what_asked(cx: &mut TestAppContext) {
        cx.update(|cx| {
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

    /// A question fading out still draws its verb, which a screen reader or
    /// the keyboard can reach past the pointer's cover; answering it again
    /// runs nothing, so a destructive action happens once.
    #[gpui::test]
    fn a_fading_question_answers_once(cx: &mut TestAppContext) {
        use std::cell::Cell;

        use gpui::accesskit::Action;

        use crate::components::testing::{act, redraw, stage, tree};

        let runs = Rc::new(Cell::new(0));
        let (_, cx) = stage(cx, |window, cx| {
            overlay(window, cx)
                .map(|element| vec![("ask", element)])
                .unwrap_or_default()
        });
        cx.update(|window, cx| {
            let runs = runs.clone();
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
                move |_| runs.set(runs.get() + 1),
            );
        });
        redraw(cx);
        act(cx, "replace", Action::Click);
        assert_eq!(runs.get(), 1);
        redraw(cx);
        let fading = tree(cx)["nodes"]
            .as_object()
            .is_some_and(|nodes| nodes.values().any(|n| n["aria"]["label"] == "replace"));
        assert!(fading, "the question fades out where it stood");
        act(cx, "replace", Action::Click);
        assert_eq!(runs.get(), 1, "a fading question ran its action again");
    }

    /// A question fading out is drawn inert: a click on the window beneath
    /// lands at once, and Tab never reaches its buttons.
    #[gpui::test]
    fn a_fading_question_leaves_the_window_its_input(cx: &mut TestAppContext) {
        use std::cell::Cell;

        use gpui::accesskit::Action;

        use crate::components::testing::{act, bounds, click_at, redraw, stage, tree};

        let clicks = Rc::new(Cell::new(0));
        let page_focus = cx.update(|cx| cx.focus_handle().tab_stop(true));
        let (_, cx) = stage(cx, {
            let (clicks, page_focus) = (clicks.clone(), page_focus.clone());
            move |window, cx| {
                let clicks = clicks.clone();
                let page = div()
                    .id("page")
                    .size(px(200.))
                    .track_focus(&page_focus)
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                    .into_any_element();
                // The question over the page, as a window draws it.
                let window_like = div()
                    .relative()
                    .size(px(400.))
                    .child(page)
                    .children(overlay(window, cx))
                    .into_any_element();
                vec![("window", window_like)]
            }
        });
        cx.update(|window, cx| {
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
        redraw(cx);
        act(cx, "cancel", Action::Click);
        redraw(cx);
        let fading = tree(cx)["nodes"]
            .as_object()
            .is_some_and(|nodes| nodes.values().any(|n| n["aria"]["label"] == "replace"));
        assert!(fading, "the question fades out where it stood");
        let at = bounds(cx, "window").origin + point(px(100.), px(100.));
        click_at(cx, at);
        assert_eq!(clicks.get(), 1, "the fading question took the click");
        // The page is the window's one stop while the question fades.
        for _ in 0..4 {
            cx.update(|window, cx| window.focus_next(cx));
            cx.run_until_parked();
            assert!(
                cx.update(|window, _| page_focus.is_focused(window)),
                "tab entered the fading question"
            );
        }
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
    #[gpui::test]
    fn a_presented_sheet_goes_with_escape(cx: &mut TestAppContext) {
        cx.update(|cx| {
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
    #[gpui::test]
    fn a_held_sheet_stays_through_escape(cx: &mut TestAppContext) {
        cx.update(|cx| {
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
