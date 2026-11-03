//! A panel at the trailing edge of a page that slides in and out rather
//! than snapping: the lyrics, an inspector. Its width eases between nothing
//! and the panel's over `Motion::SETTLE`, the panel laid out at full width
//! and clipped, so nothing in it reflows as it goes; at once under Reduce
//! Motion. The owner keeps the panel while it slides out and lets it go once
//! its `SlideState` says it has gone.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{child, fade};
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
        let open = fade(
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
