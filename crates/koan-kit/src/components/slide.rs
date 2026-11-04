//! A panel at the trailing edge of a page whose width eases in and out over
//! `Motion::SETTLE`: the lyrics, an inspector. The panel is laid out at its
//! full width and clipped, so it does not reflow; the page beside it does,
//! each frame. At once under Reduce Motion. The owner keeps the panel while
//! the width eases away and lets it go once its `SlideState` says it has
//! closed; a closed panel draws nothing.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;

use super::child;
use crate::theme::Motion;

/// How far a slide stands open, as last drawn: held by the owner, which
/// lets the panel go once `closed`.
#[derive(Clone, Default)]
pub struct SlideState(Rc<Cell<f32>>);

impl SlideState {
    pub fn closed(&self) -> bool {
        self.0.get() <= 0.0
    }
}

#[derive(IntoElement)]
pub struct Slide {
    id: ElementId,
    state: SlideState,
    shown: bool,
    width: Pixels,
    content: Option<AnyElement>,
}

impl Slide {
    pub fn new(id: impl Into<ElementId>, state: &SlideState, shown: bool, width: Pixels) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            shown,
            width,
            content: None,
        }
    }

    pub fn child(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }
}

impl RenderOnce for Slide {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let open = super::ease(
            child(&self.id, "open"),
            self.shown,
            Motion::SETTLE,
            window,
            cx,
        );
        self.state.0.set(open);
        // Gone: one more frame, for the owner to let the panel go.
        if open <= 0.0 && !self.shown && self.content.is_some() {
            window.request_animation_frame();
        }
        div()
            .h_full()
            .flex_none()
            .w(self.width * open)
            .overflow_hidden()
            .when(open > 0.0, |d| {
                d.child(div().w(self.width).h_full().children(self.content))
            })
    }
}
