//! A split view: a leading column the person can resize beside the rest, as
//! the Mac's sidebar. The divider is a hairline with a wider grip that shows
//! the resize cursor; dragging it moves the column between its least and
//! greatest widths, and a double click puts it back. The width is the
//! owner's, which keeps it (with the window's bounds, for the sidebar).
//!
//! Hidden and shown, the column's width eases to nothing and back over
//! `Motion::SETTLE`. Its content is laid out at full width and clipped, so
//! nothing in it reflows; the page beside it is laid out again at each
//! frame's width. At once under Reduce Motion. A hidden column draws
//! nothing from the frame its width reaches nothing.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;

use super::child;
use super::grip::{Edge, GRIP, Grip, OnWidth};
use crate::theme::{Look, Motion};

#[derive(IntoElement)]
pub struct SplitView {
    id: ElementId,
    width: Pixels,
    min: Pixels,
    max: Pixels,
    reset: Pixels,
    leading: Option<AnyElement>,
    shown: bool,
    trailing: Option<AnyElement>,
    on_resize: Option<OnWidth>,
    on_commit: Option<OnWidth>,
}

impl SplitView {
    /// `width` is the leading column's, held by the owner; `min` and `max`
    /// bound a drag, and `reset` is what a double click restores.
    pub fn new(
        id: impl Into<ElementId>,
        width: Pixels,
        min: Pixels,
        max: Pixels,
        reset: Pixels,
    ) -> Self {
        Self {
            id: id.into(),
            width: width.clamp(min, max),
            min,
            max,
            reset,
            leading: None,
            shown: true,
            trailing: None,
            on_resize: None,
            on_commit: None,
        }
    }

    /// The leading column; without one the trailing fills the view.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// Whether the leading column shows; its width eases as this changes,
    /// so the owner hands it over whether shown or not.
    pub fn shown(mut self, shown: bool) -> Self {
        self.shown = shown;
        self
    }

    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
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
}

impl RenderOnce for SplitView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let rule = look.palette.rule.hsla();
        let open = super::ease(
            child(&self.id, "shown"),
            self.shown,
            Motion::SETTLE,
            window,
            cx,
        );
        let Some(leading) = self.leading.filter(|_| open > 0.0) else {
            return div().size_full().flex().children(self.trailing);
        };
        let settled = self.shown && open >= 1.0;
        let width = self.width;
        let (grip, follow) = Grip {
            id: self.id.clone(),
            edge: Edge::Trailing,
            width,
            min: self.min,
            max: self.max,
            reset: self.reset,
            on_resize: self.on_resize,
            on_commit: self.on_commit,
        }
        .build(&look, window, cx);
        let grip = grip.left(width - px(GRIP / 2.));
        div()
            .relative()
            .size_full()
            .flex()
            .child(
                div()
                    .w(width * open)
                    .h_full()
                    .flex_none()
                    .overflow_hidden()
                    .relative()
                    .child(div().w(width).h_full().child(leading)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .border_l(look.hairline)
                    .border_color(rule)
                    .children(self.trailing),
            )
            .when(settled, |d| d.child(follow).child(grip))
    }
}
