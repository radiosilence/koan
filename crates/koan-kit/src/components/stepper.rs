use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{Animated, AnimatedLayer, Ease, Handler, Repeat, accessible, child, focus, ring};
use crate::theme::{
    DISABLED_OPACITY, HIT, Icon, KoanIcon, KoanStyled, Look, Motion, Space, TypeRole, label,
};

/// The shown value's roll: its layer's tracks, and the value it last showed.
struct Roll {
    animated: Animated,
    value: i64,
}

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
        // A step rolls the shown value a line the way it went: up for more,
        // down for less, the new value coming in as the old would leave.
        let value = self.value;
        let roll = window.use_keyed_state(child(&self.id, "roll"), cx, |_, _| {
            let animated = Animated::new();
            super::layer::standing(&animated);
            Roll { animated, value }
        });
        let line = look.line_height(TypeRole::Control) * window.scale_factor();
        let rolling = roll.update(cx, |r, _| {
            if r.value != value {
                let from = if value > r.value { 1.0 } else { -1.0 } * f32::from(line) * 0.6;
                let at = Animated::now();
                r.animated.segment(
                    2,
                    from,
                    0.0,
                    at,
                    Motion::SETTLE,
                    Ease::QuickOut,
                    Repeat::Once,
                );
                // A step taken while the last still rolls fades on from where
                // the value stands, never out and back in.
                let shown = if r.animated.moving() {
                    r.animated.at(0, at).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                r.animated.segment(
                    0,
                    shown,
                    1.0,
                    at,
                    Motion::SETTLE,
                    Ease::QuickOut,
                    Repeat::Once,
                );
                r.value = value;
            }
            r.animated.clone()
        });
        let down = (self.value - self.step).max(self.min);
        let up = (self.value + self.step).min(self.max);
        // A spin button to a screen reader: its value, said as shown, and a
        // step either way.
        let mut row = accessible(
            div().id(child(&self.id, "spin")),
            gpui::Role::SpinButton,
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
                (gpui::AccessibleAction::Increment, up),
                (gpui::AccessibleAction::Decrement, down),
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
                AnimatedLayer::new(
                    &rolling,
                    div()
                        .text_role(TypeRole::Control, &look)
                        .text_color(look.palette.muted.hsla())
                        .child(shown),
                )
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
        let answers = super::Answerer::of(&self.id, window, cx);
        let accent = look.accent.hsla();
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
            .children(answers.layer(window, cx))
            .when_some(
                self.on_change.filter(|_| !self.inert),
                move |button, on_change| {
                    button.cursor_pointer().on_click(move |_, window, cx| {
                        answers.start(super::Answer::Lift, accent, cx);
                        on_change(&to, window, cx)
                    })
                },
            )
            .children(ring(&handle, &look, window))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::accesskit::Action;
    use gpui::{IntoElement, TestAppContext, VisualTestContext, point, px};

    use super::*;
    use crate::components::testing::{act, bounds, click_at, node, redraw, stage};

    /// A stepper over 1..=9 in twos holding its value in `value`, and a
    /// disabled one that must never change.
    fn steppers(
        cx: &mut TestAppContext,
        value: Rc<Cell<i64>>,
        refused: Rc<Cell<bool>>,
    ) -> &mut VisualTestContext {
        let (_, cx) = stage(cx, move |_, _| {
            let (value, refused) = (value.clone(), refused.clone());
            vec![
                (
                    "rate",
                    div()
                        .w(px(300.))
                        .child(
                            Stepper::new("rate", "rate", value.get(), 1..=9)
                                .step(2)
                                .shown(format!("{}×", value.get()))
                                .on_change(move |to, _, _| value.set(*to)),
                        )
                        .into_any_element(),
                ),
                (
                    "workers",
                    div()
                        .w(px(300.))
                        .child(
                            Stepper::new("workers", "workers", 4, 1..=8)
                                .disabled(true)
                                .on_change(move |_, _, _| refused.set(true)),
                        )
                        .into_any_element(),
                ),
            ]
        });
        cx
    }

    /// A click on the trailing button, "more".
    fn more(cx: &mut VisualTestContext, name: &'static str) {
        let b = bounds(cx, name);
        click_at(cx, point(b.right() - px(14.), b.center().y));
    }

    #[gpui::test]
    fn a_spin_button_says_its_value_range_and_step(cx: &mut TestAppContext) {
        let cx = steppers(cx, Rc::new(Cell::new(3)), Rc::default());
        let (_, rate) = node(cx, "rate");
        assert_eq!(rate["role"], "SpinButton");
        assert_eq!(rate["numeric_value"], 3.0);
        assert_eq!(rate["min_numeric_value"], 1.0);
        assert_eq!(rate["max_numeric_value"], 9.0);
        assert_eq!(rate["numeric_value_step"], 2.0);
        assert_eq!(rate["value"], "3×");
    }

    #[gpui::test]
    fn more_steps_up_and_stops_at_the_top(cx: &mut TestAppContext) {
        let value = Rc::new(Cell::new(5));
        let cx = steppers(cx, value.clone(), Rc::default());
        more(cx, "rate");
        assert_eq!(value.get(), 7);
        redraw(cx);
        more(cx, "rate");
        assert_eq!(value.get(), 9);
        redraw(cx);
        value.set(9);
        more(cx, "rate");
        assert_eq!(value.get(), 9, "stepped past the top");
    }

    #[gpui::test]
    fn a_screen_reader_steps_it_within_the_range(cx: &mut TestAppContext) {
        let value = Rc::new(Cell::new(2));
        let cx = steppers(cx, value.clone(), Rc::default());
        act(cx, "rate", Action::Decrement);
        assert_eq!(value.get(), 1, "clamped to the bottom");
        redraw(cx);
        act(cx, "rate", Action::Increment);
        assert_eq!(value.get(), 3);
    }

    #[gpui::test]
    fn a_disabled_stepper_refuses_input_and_says_so(cx: &mut TestAppContext) {
        let refused = Rc::new(Cell::new(false));
        let cx = steppers(cx, Rc::new(Cell::new(3)), refused.clone());
        more(cx, "workers");
        act(cx, "workers", Action::Increment);
        assert!(!refused.get(), "a disabled stepper changed");
        assert_eq!(node(cx, "workers").1["disabled"], true);
    }
}
