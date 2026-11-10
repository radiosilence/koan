//! What arrives after its page (a photograph, a biography) fading in rather
//! than appearing, over `Motion::NORMAL` on the quick-out curve; with Reduce
//! Motion or Low Power Mode it appears. `Reveal` also makes room for it: the
//! slot grows from nothing to its size, moving what is beside it along.
//!
//! Where the renderer draws shaders, what they hold is an `AnimatedLayer`:
//! drawn once into a texture and faded or revealed by its tracks on the GPU,
//! with no frame drawn while it moves (#1859). A reveal's slot takes its full
//! size at once, so what is beside it moves along in one step while its
//! content is revealed. Elsewhere both run on GPUI frames for their length.
//!
//! The wrapper takes no size of its own: in a row it is as wide as what it
//! holds, in a column as wide as the column, so the element lays out exactly
//! as it would unwrapped.

use gpui::prelude::FluentBuilder;
use gpui::*;

use super::layer::{AnimatedLayer, standing};
use super::{Animated, Ease, Repeat, SHADED};
use crate::theme::KoanTheme;
use crate::theme::tokens::Motion;

/// A layer's tracks for an arrival under `id`, set going the first time it is
/// drawn: its opacity, and with `reveal` how much of it shows, from nothing.
fn arrival(id: ElementId, reveal: bool, window: &mut Window, cx: &mut App) -> Animated {
    window
        .use_keyed_state(id, cx, |_, _| {
            let animated = Animated::new();
            standing(&animated);
            let now = Animated::now();
            animated.segment(
                0,
                0.0,
                1.0,
                now,
                Motion::NORMAL,
                Ease::QuickOut,
                Repeat::Once,
            );
            if reveal {
                animated.segment(
                    3,
                    0.0,
                    1.0,
                    now,
                    Motion::NORMAL,
                    Ease::QuickOut,
                    Repeat::Once,
                );
            }
            animated
        })
        .read(cx)
        .clone()
}

#[derive(IntoElement)]
pub struct FadeIn {
    id: ElementId,
    child: AnyElement,
    fill: bool,
    leaving: bool,
}

impl FadeIn {
    /// How long something told it is `leaving` takes to go.
    pub const LEAVE: std::time::Duration = Motion::LEAVE;

    /// `id` names the arrival: the fade runs once per id.
    pub fn new(id: impl Into<ElementId>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            child: child.into_any_element(),
            fill: false,
            leaving: false,
        }
    }

    /// Fading back out, on the GPU, for its owner to stop drawing it `LEAVE`
    /// later; told otherwise before then, it comes back from where it got to.
    pub fn leaving(mut self, leaving: bool) -> Self {
        self.leaving = leaving;
        self
    }

    /// As large as what holds it, for content that fills its parent (a
    /// pane, a centred sleeve) rather than sizing to itself.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }
}

impl RenderOnce for FadeIn {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fill = self.fill;
        let child = if fill {
            div().size_full().child(self.child).into_any_element()
        } else {
            self.child
        };
        if !KoanTheme::motion_allowed(cx) {
            return child;
        }
        if SHADED {
            let animated = arrival(self.id, false, window, cx);
            let opacity = if self.leaving { 0.0 } else { 1.0 };
            if animated.track(0).to != opacity {
                animated.to(0, opacity, Self::LEAVE, Ease::InOut);
            }
            return div()
                .flex_none()
                .when(fill, |d| d.size_full())
                .child(AnimatedLayer::new(&animated, child))
                .into_any_element();
        }
        div()
            .flex_none()
            .when(fill, |d| d.size_full())
            .child(child)
            .with_animation(
                self.id,
                Animation::new(Motion::NORMAL).with_easing(Motion::quick_out),
                |el, delta| el.opacity(delta),
            )
            .into_any_element()
    }
}

/// A slot of `size` that, when `arriving`, grows from nothing to its size
/// over `Motion::NORMAL`, what is beside it moving along to make room, while
/// what it holds fades in. Not arriving (present when the page first drew),
/// it stands at its size. What it holds is laid out at its full size and
/// clipped while the slot grows, never squeezed.
#[derive(IntoElement)]
pub struct Reveal {
    id: ElementId,
    size: Size<Pixels>,
    child: AnyElement,
    arriving: bool,
}

impl Reveal {
    /// `id` names the arrival: it runs once per id.
    pub fn new(id: impl Into<ElementId>, size: Size<Pixels>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            size,
            child: child.into_any_element(),
            arriving: false,
        }
    }

    pub fn arriving(mut self, arriving: bool) -> Self {
        self.arriving = arriving;
        self
    }
}

impl RenderOnce for Reveal {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Size { width, height } = self.size;
        if SHADED && self.arriving && KoanTheme::motion_allowed(cx) {
            let animated = arrival(self.id, true, window, cx);
            return div()
                .flex_none()
                .w(width)
                .h(height)
                .child(AnimatedLayer::new(
                    &animated,
                    div().w(width).h(height).flex_none().child(self.child),
                ))
                .into_any_element();
        }
        let slot = div()
            .flex_none()
            .overflow_hidden()
            .child(div().w(width).h(height).flex_none().child(self.child));
        if !self.arriving || !KoanTheme::motion_allowed(cx) {
            return slot.w(width).h(height).into_any_element();
        }
        slot.with_animation(
            self.id,
            Animation::new(Motion::NORMAL).with_easing(Motion::quick_out),
            move |el, t| el.w(width * t).h(height * t).opacity(t),
        )
        .into_any_element()
    }
}
