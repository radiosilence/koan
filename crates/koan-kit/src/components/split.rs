//! A split view: a leading column the person can resize beside the rest, as
//! the Mac's sidebar. The divider is a hairline with a wider grip that shows
//! the resize cursor; dragging it moves the column between its least and
//! greatest widths, and a double click puts it back. The width is the
//! owner's, which keeps it (with the window's bounds, for the sidebar).

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::child;
use crate::theme::{HAIRLINE, HIT, Look};

type OnWidth = Rc<dyn Fn(&Pixels, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct SplitView {
    id: ElementId,
    width: Pixels,
    min: Pixels,
    max: Pixels,
    reset: Pixels,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
    on_resize: Option<OnWidth>,
    on_commit: Option<OnWidth>,
}

/// A drag under way: where the pointer and the column started.
#[derive(Clone, Copy)]
struct Drag {
    from_x: Pixels,
    from_width: Pixels,
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
        let Some(leading) = self.leading else {
            return div().size_full().flex().children(self.trailing);
        };
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
                .left(width - HIT / 2.)
                .w(HIT)
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
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || event.pressed_button != Some(MouseButton::Left)
                    {
                        return;
                    }
                    let Some(d) = *moving.read(cx) else {
                        return;
                    };
                    let to = (d.from_width + event.position.x - d.from_x).clamp(min, max);
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
                    if ending.update(cx, |d, _| d.take()).is_some()
                        && let Some(f) = &on_commit
                    {
                        f(&width, window, cx);
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
            .child(div().w(width).h_full().flex_none().child(leading))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .border_l(HAIRLINE)
                    .border_color(rule)
                    .children(self.trailing),
            )
            .child(follow)
            .child(grip)
    }
}
