//! What arrives after its page (a photograph, a biography) fading in rather
//! than appearing, over `Motion::NORMAL` on the quick-out curve; with Reduce
//! Motion or Low Power Mode it appears. `Reveal` also makes room for it: the
//! slot grows from nothing to its size, moving what is beside it along.
//!
//! Both change layout or a subtree's opacity, which a shader quad cannot, so
//! they run on GPUI frames for their length and then stop.
//!
//! The wrapper takes no size of its own: in a row it is as wide as what it
//! holds, in a column as wide as the column, so the element lays out exactly
//! as it would unwrapped.

use gpui_kit::*;

use crate::theme::KoanTheme;
use crate::theme::tokens::Motion;

#[derive(IntoElement)]
pub struct FadeIn {
    id: ElementId,
    child: AnyElement,
}

impl FadeIn {
    /// `id` names the arrival: the fade runs once per id.
    pub fn new(id: impl Into<ElementId>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            child: child.into_any_element(),
        }
    }
}

impl RenderOnce for FadeIn {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        if !KoanTheme::motion_allowed(cx) {
            return self.child;
        }
        div()
            .flex_none()
            .child(self.child)
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
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Size { width, height } = self.size;
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
