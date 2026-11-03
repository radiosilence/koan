use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{Handler, accessible, child, focus, ring};
use crate::theme::{
    DISABLED_OPACITY, HIT, Icon, KoanIcon, KoanStyled, Look, Space, TypeRole, label,
};

/// A value moved a step at a time: the title leading in `body`, `ink`, and
/// two square buttons trailing, each disabled at its end of the range.
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    title: SharedString,
    value: i64,
    min: i64,
    max: i64,
    step: i64,
    shown: Option<SharedString>,
    disabled: bool,
    on_change: Option<Handler<i64>>,
}

impl Stepper {
    pub fn new(
        id: impl Into<ElementId>,
        title: impl AsRef<str>,
        value: i64,
        range: std::ops::RangeInclusive<i64>,
    ) -> Self {
        Self {
            id: id.into(),
            title: label(title),
            value,
            min: *range.start(),
            max: *range.end(),
            step: 1,
            shown: None,
            disabled: false,
            on_change: None,
        }
    }

    pub fn step(mut self, step: i64) -> Self {
        self.step = step;
        self
    }

    /// The value as the row shows it, in `control`, `muted`, before the
    /// buttons.
    pub fn shown(mut self, text: impl Into<SharedString>) -> Self {
        self.shown = Some(text.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&i64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let disabled = self.disabled;
        let button = |name: &'static str, icon: Icon, text: &str, to: i64, allowed: bool| {
            StepButton {
                id: child(&self.id, name),
                icon,
                label: label(text),
                to,
                allowed,
                // The row dims itself when disabled; the buttons only stop
                // taking the pointer and the keyboard.
                inert: disabled || !allowed,
                disabled,
                on_change: self.on_change.clone(),
            }
        };
        let down = (self.value - self.step).max(self.min);
        let up = (self.value + self.step).min(self.max);
        // A spin button to a screen reader: its value, said as shown, and a
        // step either way.
        let mut row = accessible(
            div().id(child(&self.id, "spin")),
            gpui_kit::Role::SpinButton,
            Some(self.title.clone()),
        )
        .aria_disabled(disabled)
        .aria_numeric_value(self.value as f64)
        .aria_min_numeric_value(self.min as f64)
        .aria_max_numeric_value(self.max as f64)
        .aria_numeric_value_step(self.step as f64)
        .when_some(self.shown.clone(), |d, shown| d.aria_value(shown));
        if let Some(on_change) = self.on_change.clone().filter(|_| !disabled) {
            for (action, to) in [
                (gpui_kit::AccessibleAction::Increment, up),
                (gpui_kit::AccessibleAction::Decrement, down),
            ] {
                let set = on_change.clone();
                row = row.on_a11y_action(action, move |_, window, cx| set(&to, window, cx));
            }
        }
        row.flex()
            .items_center()
            .gap(look.space(Space::Xs))
            .min_h(HIT)
            .when(self.disabled, |row| row.opacity(DISABLED_OPACITY))
            .child(
                div()
                    .flex_1()
                    .text_role(TypeRole::Body, &look)
                    .text_color(look.palette.ink.hsla())
                    .child(self.title),
            )
            .children(self.shown.map(|shown| {
                div()
                    .text_role(TypeRole::Control, &look)
                    .text_color(look.palette.muted.hsla())
                    .child(shown)
            }))
            .child(button(
                "down",
                Icon::Subtract,
                "less",
                down,
                self.value > self.min,
            ))
            .child(button("up", Icon::Add, "more", up, self.value < self.max))
    }
}

/// One of a stepper's buttons, as the Mac draws them: the glyph at 11
/// points in `ink`, in a 28-point square with a hairline `muted` outline,
/// which is also the whole of its target.
#[derive(IntoElement)]
struct StepButton {
    id: ElementId,
    icon: Icon,
    label: SharedString,
    to: i64,
    allowed: bool,
    inert: bool,
    disabled: bool,
    on_change: Option<Handler<i64>>,
}

impl RenderOnce for StepButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let label = self.label.clone();
        let to = self.to;
        div()
            .id(self.id)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(28.0))
            .border(look.hairline)
            .border_color(look.palette.muted.hsla())
            .when(!self.allowed, |button| button.opacity(DISABLED_OPACITY))
            .child(
                KoanIcon::new(self.icon)
                    .points(11.0)
                    .color(look.palette.ink.hsla()),
            )
            .tooltip(move |window, cx| super::Tooltip::view(label.clone(), window, cx))
            // A button keeps the keyboard at its end of the range, so the
            // person who stepped it there is not left without focus.
            .when(!self.disabled, |button| button.track_focus(&handle))
            .when_some(
                self.on_change.filter(|_| !self.inert),
                |button, on_change| {
                    button
                        .cursor_pointer()
                        .on_click(move |_, window, cx| on_change(&to, window, cx))
                },
            )
            .children(ring(&handle, &look, window))
    }
}
