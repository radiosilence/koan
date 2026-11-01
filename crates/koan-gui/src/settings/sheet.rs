//! The Settings window's sheet: one at a time, over a scrim that takes every
//! click, with the keyboard held inside it. Each write that changes data
//! asks here first, saying what will change; a prompt carries the fields it
//! needs; a key just made, or an invite, is shown here once.

use std::rc::Rc;

use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;
use crate::components::{Button, ButtonKind, TextField};
use crate::theme::{HAIRLINE, KoanStyled, Look, Space, TypeRole, label};

type Run = Rc<dyn Fn(&[String], &mut Window, &mut App)>;
type Content = Rc<dyn Fn(&Look, &mut Window, &mut App) -> AnyElement>;

/// A sheet's action. It closes the sheet, then runs with what the fields
/// hold.
pub struct Action {
    label: SharedString,
    destructive: bool,
    /// Whether it can run with what the fields hold.
    ready: Option<Rc<dyn Fn(&[String]) -> bool>>,
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
    /// The way out that does nothing: "cancel", or "done" for a sheet that
    /// only shows something.
    dismiss: SharedString,
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
        }
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
pub fn field(placeholder: &str, secret: bool, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let placeholder = SharedString::from(placeholder.to_owned());
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .masked(secret)
    })
}

#[derive(Default)]
pub struct Sheets {
    open: Option<Sheet>,
    focus: Option<FocusHandle>,
    _fields: Vec<Subscription>,
}

impl Sheets {
    /// Puts a sheet up, replacing any other, with the keyboard in its first
    /// field or on the sheet.
    pub fn show(&mut self, sheet: Sheet, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        match sheet.fields.first() {
            Some((_, state)) => state.update(cx, |state, cx| state.focus(window, cx)),
            None => window.focus(&focus, cx),
        }
        // A field's text decides whether an action can run.
        self._fields = sheet
            .fields
            .iter()
            .map(|(_, state)| cx.observe(state, |_, _, cx| cx.notify()))
            .collect();
        self.open = Some(sheet);
        self.focus = Some(focus);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.focus = None;
        self._fields.clear();
        cx.notify();
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub(super) fn render(
        &self,
        this: &Entity<Self>,
        look: &Look,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let sheet = self.open.as_ref()?;
        let focus = self.focus.clone()?;
        let values: Vec<String> = sheet
            .fields
            .iter()
            .map(|(_, state)| state.read(cx).value().to_string())
            .collect();
        let close = {
            let this = this.clone();
            move |cx: &mut App| this.update(cx, |sheets, cx| sheets.close(cx))
        };
        let escape = close.clone();

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
                        let close = close.clone();
                        move |_, _, cx| close(cx)
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
            let run = action.run.clone();
            let close = close.clone();
            let values = values.clone();
            buttons = buttons.child(
                Button::new(("sheet-action", index), kind)
                    .label(action.label.as_ref())
                    .destructive(action.destructive)
                    .disabled(!ready)
                    .on_click(move |_, window, cx| {
                        close(cx);
                        run(&values, window, cx);
                    }),
            );
        }

        let panel = div()
            .id("sheet")
            .track_focus(&focus)
            .key_context("SettingsSheet")
            .on_key_down(move |event, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    escape(cx);
                }
            })
            .w(px(460.))
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(look.space(Space::S))
            .p(look.space(Space::L))
            .bg(look.palette.bg.hsla())
            .border(HAIRLINE)
            .border_color(look.palette.rule.hsla())
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
            .children(sheet.content.as_ref().map(|content| content(look, window, cx)))
            .child(buttons)
            .focus_trap("sheet-trap", &focus);

        Some(
            div()
                .id("sheet-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(look.palette.bg.hsla().opacity(0.6))
                // Every click on the window behind goes nowhere.
                .occlude()
                .child(panel)
                .into_any_element(),
        )
    }
}
