//! The kōan theme's components, drawn from its tokens: flat, square, and in
//! Geist Mono. docs/design/koan-theme.md#components describes each; the
//! comments here say only where GPUI asks for something the spec does not.
//!
//! Every component is a `RenderOnce` element that reads the window's `Look`
//! as it renders. Interactive ones take an id, under which they keep what
//! the pointer is doing and the state of their transitions.

mod badge;
mod button;
mod empty;
mod field;
mod form;
mod panel;
mod rows;
mod segmented;
mod slider;
mod stepper;
mod toggle;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, Div, ElementId, Entity, FocusHandle, Hsla, InteractiveElement, MouseButton, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

pub use badge::{Badge, Chip};
pub use button::{Button, ButtonKind};
pub use empty::EmptyState;
pub use field::TextField;
pub use form::{FormRow, SectionHeader};
pub use panel::{MenuPanel, MenuRow, Tooltip};
pub use rows::{ListRow, NavRow};
pub use segmented::Segmented;
pub use slider::Slider;
pub use stepper::Stepper;
pub use toggle::{IconToggle, Toggle};

use crate::theme::{Look, MARK, Motion};

/// A handler a component calls with the window and the app.
pub type Handler<T> = Arc<dyn Fn(&T, &mut Window, &mut App) + 'static>;

/// What the pointer is doing to a control.
#[derive(Default)]
pub(crate) struct Pointer {
    pub hovered: bool,
    pub pressed: bool,
}

pub(crate) fn pointer(id: &ElementId, window: &mut Window, cx: &mut App) -> Entity<Pointer> {
    window.use_keyed_state(id.clone(), cx, |_, _| Pointer::default())
}

/// Keeps `state` up to date with the pointer over `element`.
pub(crate) fn track<E: InteractiveElement + StatefulInteractiveElement>(
    element: E,
    state: &Entity<Pointer>,
) -> E {
    let set = |state: &Entity<Pointer>, change: fn(&mut Pointer)| {
        let state = state.clone();
        move |cx: &mut App| {
            state.update(cx, |pointer, cx| {
                change(pointer);
                cx.notify();
            })
        }
    };
    let hover = state.clone();
    let down = set(state, |p| p.pressed = true);
    let up = set(state, |p| p.pressed = false);
    let up_out = set(state, |p| p.pressed = false);
    element
        .on_hover(move |hovered, _, cx| {
            let hovered = *hovered;
            hover.update(cx, |pointer, cx| {
                pointer.hovered = hovered;
                cx.notify();
            })
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| down(cx))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| up(cx))
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| up_out(cx))
}

/// A focus handle kept under `id`, so a `RenderOnce` control can take the
/// keyboard and show where it is.
pub(crate) fn focus(id: &ElementId, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state(child(id, "focus"), cx, |_, cx| {
            cx.focus_handle().tab_stop(true)
        })
        .read(cx)
        .clone()
}

pub(crate) fn child(id: &ElementId, name: &'static str) -> ElementId {
    ElementId::NamedChild(Arc::new(id.clone()), SharedString::new_static(name))
}

/// A state moving between off (0) and on (1) over a motion token, from
/// wherever it stood. Instant with Reduce Motion.
struct Fade {
    on: bool,
    from: f32,
    started: Option<Instant>,
    duration: Duration,
}

impl Fade {
    fn at(&self, now: Instant) -> f32 {
        let target = if self.on { 1.0 } else { 0.0 };
        let Some(started) = self.started else {
            return target;
        };
        let t = now.duration_since(started).as_secs_f32() / self.duration.as_secs_f32();
        if t >= 1.0 {
            return target;
        }
        self.from + (target - self.from) * Motion::quick_out(t)
    }
}

/// How far a state has moved toward `on`, kept under `key`. The first frame
/// a control is drawn it stands where it is, with nothing to animate; frames
/// are asked for only while a change is under way.
pub(crate) fn fade(
    key: ElementId,
    on: bool,
    duration: Duration,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let now = Instant::now();
    let reduce = cx.reduce_motion();
    let state = window.use_keyed_state(key, cx, |_, _| Fade {
        on,
        from: 0.0,
        started: None,
        duration,
    });
    let (value, moving) = state.update(cx, |fade, _| {
        if fade.on != on {
            fade.from = fade.at(now);
            fade.on = on;
            fade.started = (!reduce).then_some(now);
        }
        let value = fade.at(now);
        let moving = fade
            .started
            .is_some_and(|started| now.duration_since(started) < fade.duration);
        (value, moving)
    });
    if moving {
        window.request_animation_frame();
    }
    value
}

/// The keyboard's focus ring, 2 points of accent standing 4 outside the
/// control, drawn over it as its last child. The control is `relative`.
pub(crate) fn ring(handle: &FocusHandle, look: &Look, window: &Window) -> Option<Div> {
    let focused = handle.is_focused(window) && window.last_input_was_keyboard();
    focused.then(|| {
        let standoff = px(-4.0);
        div()
            .absolute()
            .top(standoff)
            .left(standoff)
            .right(standoff)
            .bottom(standoff)
            .border(MARK)
            .border_color(look.accent.hsla())
    })
}

/// Two colours mixed, `t` of the way from `a` to `b`.
pub(crate) fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (gpui_kit::Rgba::from(a), gpui_kit::Rgba::from(b));
    let at = |x: f32, y: f32| x + (y - x) * t;
    gpui_kit::Rgba {
        r: at(a.r, b.r),
        g: at(a.g, b.g),
        b: at(a.b, b.b),
        a: at(a.a, b.a),
    }
    .into()
}

/// A fill under a control, faded in and out with its state.
pub(crate) fn fill(color: Hsla, amount: f32) -> Div {
    div()
        .absolute()
        .size_full()
        .bg(color.opacity(color.a * amount))
}
