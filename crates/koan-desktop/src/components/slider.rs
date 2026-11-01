use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Bounds, ElementId, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    Point, RenderOnce, SharedString, Styled, Window, canvas, div, px,
};

use super::{Handler, child, focus, pointer, ring, track};
use crate::theme::{DISABLED_OPACITY, HAIRLINE, HIT, KoanStyled, Look, Space, TypeRole};

/// A 1-point `rule` track with a 3-point accent fill up to the value, and a
/// square 8 × 8 `ink` thumb shown only on hover, focus or drag. The values
/// at either end sit in `fine`, `muted`.
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    value: f32,
    low: Option<SharedString>,
    high: Option<SharedString>,
    step: f32,
    disabled: bool,
    on_change: Option<Handler<f32>>,
}

impl Slider {
    /// `value` is a fraction of the track, 0 to 1.
    pub fn new(id: impl Into<ElementId>, value: f32) -> Self {
        Self {
            id: id.into(),
            value: value.clamp(0.0, 1.0),
            low: None,
            high: None,
            step: 0.05,
            disabled: false,
            on_change: None,
        }
    }

    /// What sits at either end, such as the time played and the time left.
    pub fn ends(mut self, low: impl Into<SharedString>, high: impl Into<SharedString>) -> Self {
        self.low = Some(low.into());
        self.high = Some(high.into());
        self
    }

    /// How far an arrow key moves it.
    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

const THUMB: f32 = 8.0;
const FILL: f32 = 3.0;

fn at(bounds: Bounds<Pixels>, position: Point<Pixels>) -> f32 {
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return 0.0;
    }
    (f32::from(position.x - bounds.left()) / width).clamp(0.0, 1.0)
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let state = pointer(&self.id, window, cx);
        let track_bounds = window.use_keyed_state(child(&self.id, "track"), cx, |_, _| {
            Bounds::<Pixels>::default()
        });
        let (hovered, pressed) = {
            let p = state.read(cx);
            (p.hovered, p.pressed)
        };
        let focused = handle.is_focused(window);
        let thumb = !self.disabled && (hovered || pressed || focused);
        let value = self.value;

        let measure = track_bounds.clone();
        let rail = div()
            .relative()
            .flex_1()
            .h(HIT)
            .child(
                canvas(
                    move |bounds, _, cx| measure.update(cx, |b, _| *b = bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px((f32::from(HIT) - f32::from(HAIRLINE)) / 2.0))
                    .h(HAIRLINE)
                    .bg(look.palette.rule.hsla()),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .w(gpui_kit::relative(value))
                    .top(px((f32::from(HIT) - FILL) / 2.0))
                    .h(px(FILL))
                    .bg(look.accent.hsla()),
            )
            .when(thumb, |rail| {
                rail.child(
                    div()
                        .absolute()
                        .left(gpui_kit::relative(value))
                        .ml(px(-THUMB / 2.0))
                        .top(px((f32::from(HIT) - THUMB) / 2.0))
                        .size(px(THUMB))
                        .bg(look.palette.ink.hsla()),
                )
            });

        let end = |text: Option<SharedString>| {
            text.map(|text| {
                div()
                    .flex_none()
                    .text_role(TypeRole::Fine, &look)
                    .text_color(look.palette.muted.hsla())
                    .child(text)
            })
        };
        let mut slider = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .track_focus(&handle)
            .children(end(self.low))
            .child(rail)
            .children(end(self.high));
        if self.disabled {
            slider = slider.opacity(DISABLED_OPACITY);
        } else if let Some(on_change) = self.on_change {
            let (down, drag, key) = (on_change.clone(), on_change.clone(), on_change);
            let (down_bounds, drag_bounds) = (track_bounds.clone(), track_bounds);
            let dragging = state.clone();
            let step = self.step;
            slider = track(slider, &state)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    let bounds = *down_bounds.read(cx);
                    down(&at(bounds, event.position), window, cx);
                })
                .on_mouse_move(move |event, window, cx| {
                    if dragging.read(cx).pressed && event.dragging() {
                        let bounds = *drag_bounds.read(cx);
                        drag(&at(bounds, event.position), window, cx);
                    }
                })
                .on_key_down(move |event, window, cx| {
                    let to = match event.keystroke.key.as_str() {
                        "left" | "down" => value - step,
                        "right" | "up" => value + step,
                        _ => return,
                    };
                    cx.stop_propagation();
                    key(&to.clamp(0.0, 1.0), window, cx);
                });
        }
        slider.children(ring(&handle, &look, window))
    }
}
