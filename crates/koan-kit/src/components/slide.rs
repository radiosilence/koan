//! A panel at the trailing edge of a page whose width eases in and out over
//! `Motion::SETTLE`: the lyrics, an inspector. The panel is laid out at its
//! full width and clipped, so it does not reflow; the page beside it does,
//! each frame. At once under Reduce Motion. The owner keeps the panel while
//! the width eases away and lets it go once its `SlideState` says it has
//! closed; a closed panel draws nothing.
//!
//! Made `resizable`, the panel's leading edge carries `SplitView`'s grip:
//! dragging it left widens the panel, and a double click restores its reset
//! width. The width is the owner's to keep.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;

use super::child;
use super::grip::{Edge, GRIP, Grip, OnWidth};
use crate::theme::{Look, Motion};

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
    /// The least and greatest widths a drag reaches, and the reset width.
    resize: Option<(Pixels, Pixels, Pixels)>,
    on_resize: Option<OnWidth>,
    on_commit: Option<OnWidth>,
}

impl Slide {
    pub fn new(id: impl Into<ElementId>, state: &SlideState, shown: bool, width: Pixels) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            shown,
            width,
            content: None,
            resize: None,
            on_resize: None,
            on_commit: None,
        }
    }

    /// A grip on the leading edge moves the width between `min` and `max`;
    /// a double click restores `reset`.
    pub fn resizable(mut self, min: Pixels, max: Pixels, reset: Pixels) -> Self {
        self.width = self.width.clamp(min, max);
        self.resize = Some((min, max, reset));
        self
    }

    /// Each width a drag reaches, for the owner to draw.
    pub fn on_resize(mut self, f: impl Fn(&Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_resize = Some(Rc::new(f));
        self
    }

    /// The width a drag ended on or a double click restored, for the owner
    /// to keep.
    pub fn on_commit(mut self, f: impl Fn(&Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_commit = Some(Rc::new(f));
        self
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
        let grip = self
            .resize
            .filter(|_| self.shown && open >= 1.0)
            .map(|(min, max, reset)| {
                Grip {
                    id: self.id.clone(),
                    edge: Edge::Leading,
                    width: self.width,
                    min,
                    max,
                    reset,
                    on_resize: self.on_resize,
                    on_commit: self.on_commit,
                }
                .build(&Look::of(window, cx), window, cx)
            });
        div()
            .relative()
            .h_full()
            .flex_none()
            .w(self.width * open)
            .child(div().size_full().overflow_hidden().when(open > 0.0, |d| {
                d.child(div().w(self.width).h_full().children(self.content))
            }))
            .when_some(grip, |d, (grip, follow)| {
                d.child(follow).child(grip.left(px(-GRIP / 2.)))
            })
    }
}
