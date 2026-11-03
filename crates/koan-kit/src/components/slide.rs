//! A panel at the trailing edge of a page that slides in and out rather
//! than snapping: the lyrics, an inspector. Where the renderer draws shaders
//! its slot takes the panel's width at once and the panel slides in over
//! `Motion::SETTLE` as an `AnimatedLayer`, with no frame drawn (#1859);
//! sliding out, the slot is let go as the slide ends. Elsewhere its width
//! eases on frames. At once under Reduce Motion. The owner keeps the panel
//! while it slides out and lets it go once its `SlideState` says it has
//! gone.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::layer::{AnimatedLayer, sliding};
use super::{SHADED, child};
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
        if SHADED {
            let travel = f32::from(self.width) * window.scale_factor();
            let slide = sliding(child(&self.id, "open"), self.shown, travel, window, cx);
            self.state.0.set(if slide.laid_out { 1.0 } else { 0.0 });
            // Gone: one more frame, for the owner to let the panel go, as
            // it reads the state before this sets it.
            if !slide.laid_out && !self.shown && self.content.is_some() {
                window.request_animation_frame();
            }
            let leaving = slide.laid_out && !self.shown;
            return div()
                .relative()
                .h_full()
                .flex_none()
                .w(if slide.laid_out { self.width } else { px(0.) })
                .overflow_hidden()
                .when(slide.laid_out, |d| {
                    d.child(AnimatedLayer::new(
                        &slide.animated,
                        div().w(self.width).h_full().children(self.content),
                    ))
                })
                // Sliding out, it takes no clicks: what it covers is going.
                .when(leaving, |d| d.child(div().absolute().inset_0().occlude()))
                .into_any_element();
        }
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
            .into_any_element()
    }
}
