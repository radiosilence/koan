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
use crate::theme::{Look, Motion};

type OnWidth = Rc<dyn Fn(&Pixels, &mut Window, &mut App)>;

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

/// A drag under way: where the pointer and the column started.
#[derive(Clone, Copy)]
struct Drag {
    from_x: Pixels,
    from_width: Pixels,
    /// The width the drag last reached, which the release commits.
    to: Pixels,
}

/// How wide the grip is, centred on the divider: wide enough to find, narrow
/// enough to leave the controls at either column's edge to their own clicks.
const GRIP: f32 = 8.0;

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
        // Set on a press on the grip and cleared on release, without a
        // notify: nothing is drawn from it.
        let drag = window.use_keyed_state(child(&self.id, "drag"), cx, |_, _| None::<Drag>);
        let (min, max, reset, width) = (self.min, self.max, self.reset, self.width);
        let (on_resize, on_commit) = (self.on_resize, self.on_commit);
        let grip = {
            let (starting, on_resize, on_commit) =
                (drag.clone(), on_resize.clone(), on_commit.clone());
            div()
                .id(child(&self.id, "grip"))
                .absolute()
                .top_0()
                .bottom_0()
                .left(width - px(GRIP / 2.))
                .w(px(GRIP))
                .cursor_col_resize()
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        if let Some(f) = &on_resize {
                            f(&reset, window, cx);
                        }
                        if let Some(f) = &on_commit {
                            f(&reset, window, cx);
                        }
                        return;
                    }
                    starting.update(cx, |d, _| {
                        *d = Some(Drag {
                            from_x: event.position.x,
                            from_width: width,
                            to: width,
                        })
                    });
                })
        };
        // The drag follows the pointer past the grip and the window until it
        // is let go, through window-wide listeners, as the slider's does:
        // GPUI's drag and drop would redraw the whole window at every move.
        let follow = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let (moving, ending) = (drag.clone(), drag);
                let on_moved = on_resize.clone();
                let on_lost = on_commit.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    // Let go outside the window, where its release is not
                    // heard: the drag ends at the next move without the
                    // button, where it had got to.
                    if event.pressed_button != Some(MouseButton::Left) {
                        if let Some(d) = moving.update(cx, |d, _| d.take())
                            && let Some(f) = &on_lost
                        {
                            f(&d.to, window, cx);
                        }
                        return;
                    }
                    let Some(d) = *moving.read(cx) else {
                        return;
                    };
                    let to = (d.from_width + event.position.x - d.from_x).clamp(min, max);
                    moving.update(cx, |d, _| {
                        if let Some(d) = d {
                            d.to = to;
                        }
                    });
                    if to != width
                        && let Some(f) = &on_moved
                    {
                        f(&to, window, cx);
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }
                    if let Some(d) = ending.update(cx, |d, _| d.take())
                        && let Some(f) = &on_commit
                    {
                        f(&d.to, window, cx);
                    }
                });
            },
        )
        .absolute()
        .size_full();
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
