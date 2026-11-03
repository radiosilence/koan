//! The Settings window's sheet: one at a time, over a scrim that takes every
//! click, with the keyboard held inside it. Each write that changes data
//! asks here first, saying what will change; a prompt carries the fields it
//! needs; a key just made, or an invite, is shown here once.

use std::rc::Rc;

use crate::components::{Button, ButtonKind, TextField};
use crate::theme::{KoanStyled, Look, Space, TypeRole, label};
use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::*;

type Run = Rc<dyn Fn(&[String], &mut Window, &mut App)>;
type Content = Rc<dyn Fn(&Look, &mut Window, &mut App) -> AnyElement>;
type Ready = Rc<dyn Fn(&[String]) -> bool>;
pub type Dismissed = Rc<dyn Fn(&mut Window, &mut App)>;

/// A sheet's action. It closes the sheet, then runs with what the fields
/// hold.
pub struct Action {
    label: SharedString,
    destructive: bool,
    /// Whether it can run with what the fields hold.
    ready: Option<Ready>,
    run: Run,
}

impl Action {
    pub fn new(text: &str, run: impl Fn(&[String], &mut Window, &mut App) + 'static) -> Self {
        Self {
            label: label(text),
            destructive: false,
            ready: None,
            run: Rc::new(run),
        }
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn ready(mut self, ready: impl Fn(&[String]) -> bool + 'static) -> Self {
        self.ready = Some(Rc::new(ready));
        self
    }
}

pub struct Sheet {
    title: SharedString,
    message: Option<SharedString>,
    fields: Vec<(SharedString, Entity<InputState>)>,
    content: Option<Content>,
    actions: Vec<Action>,
    /// The way out: "cancel", or "done" for a sheet that only shows
    /// something.
    dismiss: SharedString,
    /// What the way out does, for a sheet whose question someone else is
    /// waiting on the answer to. Nothing, otherwise.
    dismissed: Option<Dismissed>,
    /// Shown only this once (a key, a password in an invite): another sheet
    /// waits behind it rather than replacing it.
    kept: bool,
}

impl Sheet {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            message: None,
            fields: Vec::new(),
            content: None,
            actions: Vec::new(),
            dismiss: label("Cancel"),
            dismissed: None,
            kept: false,
        }
    }

    /// What it shows is shown only this once: a sheet put up meanwhile waits
    /// until this one is closed.
    pub fn kept(mut self) -> Self {
        self.kept = true;
        self
    }

    /// Whether another sheet must wait behind it: it is shown once, or asks
    /// a question someone is waiting on.
    fn holds(&self) -> bool {
        self.kept || self.dismissed.is_some()
    }

    /// The way out answers the question, as Decline does a device waiting
    /// to sign in: closing the sheet or pressing Escape runs it too.
    pub fn dismiss_with(
        mut self,
        text: &str,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.dismiss = label(text);
        self.dismissed = Some(Rc::new(run));
        self
    }

    pub fn message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// A field, its text handed to the actions in the order added.
    pub fn field(mut self, text: &str, state: Entity<InputState>) -> Self {
        self.fields.push((label(text), state));
        self
    }

    pub fn content(
        mut self,
        content: impl Fn(&Look, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.content = Some(Rc::new(content));
        self
    }

    pub fn action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    /// No action of its own: one "done" closes it.
    pub fn done(mut self) -> Self {
        self.dismiss = label("Done");
        self
    }
}

/// A field for a sheet's prompt.
pub fn field(
    placeholder: &str,
    secret: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    let placeholder = crate::theme::label(placeholder);
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .masked(secret)
    })
}

#[derive(Default)]
pub struct Sheets {
    open: Option<Sheet>,
    /// Sheets put up while one that holds was open, in turn.
    waiting: std::collections::VecDeque<Sheet>,
    focus: Option<FocusHandle>,
    /// Where the keyboard was before the sheet, to go back to.
    before: Option<FocusHandle>,
    _fields: Vec<Subscription>,
}

impl Sheets {
    /// Puts a sheet up, with the keyboard in its first field or on the
    /// sheet. It replaces one that is open, unless that one is shown once or
    /// waits on an answer, when it goes up after it. Return in a field runs
    /// the sheet's last action, as the default button does.
    pub fn show(&mut self, sheet: Sheet, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(Sheet::holds) {
            self.waiting.push_back(sheet);
            return;
        }
        if self.open.is_none() {
            self.before = window.focused(cx);
        }
        let focus = cx.focus_handle();
        match sheet.fields.first() {
            Some((_, state)) => state.update(cx, |state, cx| state.focus(window, cx)),
            None => window.focus(&focus, cx),
        }
        let this = cx.entity();
        self._fields = sheet
            .fields
            .iter()
            .flat_map(|(_, state)| {
                let this = this.clone();
                [
                    // A field's text decides whether an action can run.
                    cx.observe(state, |_, _, cx| cx.notify()),
                    cx.subscribe_in(
                        state,
                        window,
                        move |_, _, event: &InputEvent, window, cx| {
                            if matches!(event, InputEvent::PressEnter { .. }) {
                                let last =
                                    this.read(cx).open.as_ref().map_or(0, |s| s.actions.len());
                                if let Some(index) = last.checked_sub(1) {
                                    Sheets::act(&this, index, window, cx);
                                }
                            }
                        },
                    ),
                ]
            })
            .collect();
        self.open = Some(sheet);
        self.focus = Some(focus);
        cx.notify();
    }

    /// Takes the sheet down and gives the keyboard back to where it was.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = None;
        self.focus = None;
        self._fields.clear();
        if let Some(before) = self.before.take() {
            window.focus(&before, cx);
        }
        if let Some(next) = self.waiting.pop_front() {
            self.show(next, window, cx);
        }
        cx.notify();
    }

    /// The way out, and what it answers where it does.
    fn dismiss(this: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let dismissed = this
            .read(cx)
            .open
            .as_ref()
            .and_then(|sheet| sheet.dismissed.clone());
        this.update(cx, |sheets, cx| sheets.close(window, cx));
        if let Some(dismissed) = dismissed {
            dismissed(window, cx);
        }
    }

    /// Runs an action with what the fields hold now, if it can run with
    /// that, closing the sheet first.
    fn act(this: &Entity<Self>, index: usize, window: &mut Window, cx: &mut App) {
        let Some(sheet) = this.read(cx).open.as_ref() else {
            return;
        };
        let values: Vec<String> = sheet
            .fields
            .iter()
            .map(|(_, state)| state.read(cx).value().to_string())
            .collect();
        let Some(action) = sheet.actions.get(index) else {
            return;
        };
        if action.ready.as_ref().is_some_and(|ready| !ready(&values)) {
            return;
        }
        let run = action.run.clone();
        this.update(cx, |sheets, cx| sheets.close(window, cx));
        run(&values, window, cx);
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// What the sheets up and waiting do when they go unanswered, for the
    /// window to run as it closes: a device waiting to pair is declined, not
    /// left waiting.
    pub fn unanswered(&self) -> Vec<Dismissed> {
        self.open
            .iter()
            .chain(&self.waiting)
            .filter_map(|s| s.dismissed.clone())
            .collect()
    }

    /// The sheets up and waiting that show something only once, taken for
    /// the next Settings window as this one closes.
    pub fn take_kept(&mut self) -> Vec<Sheet> {
        let mut kept: Vec<Sheet> = self.open.take().into_iter().filter(|s| s.kept).collect();
        kept.extend(self.waiting.drain(..).filter(|s| s.kept));
        kept
    }

    /// The sheet up, over its scrim, if one is.
    pub(super) fn render(
        this: &Entity<Self>,
        look: &Look,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let focus = this.read(cx).focus.clone()?;
        let content = this.read(cx).open.as_ref()?.content.clone();
        let extra = content.map(|content| content(look, window, cx));
        let sheets = this.read(cx);
        let sheet = sheets.open.as_ref()?;
        let values: Vec<String> = sheet
            .fields
            .iter()
            .map(|(_, state)| state.read(cx).value().to_string())
            .collect();

        let mut buttons = div()
            .flex()
            .justify_end()
            .items_center()
            .gap(look.space(Space::M))
            .pt(look.space(Space::M))
            .child(
                Button::new("sheet-dismiss", ButtonKind::Compact)
                    .label(sheet.dismiss.as_ref())
                    .on_click({
                        let this = this.clone();
                        move |_, window, cx| Sheets::dismiss(&this, window, cx)
                    }),
            );
        let last = sheet.actions.len().saturating_sub(1);
        for (index, action) in sheet.actions.iter().enumerate() {
            let ready = action.ready.as_ref().is_none_or(|ready| ready(&values));
            // The last action is what the sheet is for; a destructive one is
            // compact and in `bad`, as the spec has it.
            let kind = if action.destructive {
                ButtonKind::Bordered
            } else if index == last {
                ButtonKind::Prominent
            } else {
                ButtonKind::Standard
            };
            let this = this.clone();
            buttons = buttons.child(
                Button::new(("sheet-action", index), kind)
                    .label(action.label.as_ref())
                    .destructive(action.destructive)
                    .disabled(!ready)
                    .on_click(move |_, window, cx| Sheets::act(&this, index, window, cx)),
            );
        }

        let panel = crate::components::sheet::panel(look)
            .track_focus(&focus)
            .key_context("SettingsSheet")
            // Before a focused field sees it, which may take Escape for its
            // own.
            .capture_key_down({
                let this = this.clone();
                move |event, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        Sheets::dismiss(&this, window, cx);
                    }
                }
            })
            .w(look.spaced(460.0))
            .overflow_y_scroll()
            .gap(look.space(Space::S))
            .p(look.space(Space::L))
            .child(
                div()
                    .text_role(TypeRole::TitleSmall, look)
                    .text_color(look.palette.strong.hsla())
                    .child(sheet.title.clone()),
            )
            .children(sheet.message.clone().map(|message| {
                div()
                    .text_role(TypeRole::Meta, look)
                    .text_color(look.palette.muted.hsla())
                    .child(message)
            }))
            .children(sheet.fields.iter().map(|(name, state)| {
                div()
                    .flex()
                    .flex_col()
                    .gap(look.space(Space::Xs))
                    .child(
                        div()
                            .text_role(TypeRole::Fine, look)
                            .text_color(look.palette.ink.hsla())
                            .child(name.clone()),
                    )
                    .child(TextField::new(state))
            }))
            .children(extra)
            .child(buttons);
        let panel = crate::components::sheet::trap_focus(panel, &focus);

        Some(
            div()
                .id("sheet-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(look.veil(0.6))
                // Every click on the window behind goes nowhere.
                .occlude()
                .child(panel)
                .into_any_element(),
        )
    }
}
