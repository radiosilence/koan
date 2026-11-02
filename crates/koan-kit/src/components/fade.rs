//! What arrives after its page (a photograph, a biography) fading in rather
//! than appearing, over `Motion::NORMAL` on the quick-out curve; with Reduce
//! Motion or Low Power Mode it appears.
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
