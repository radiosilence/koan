//! The grip a resizable column is dragged by, shared by `SplitView` (the
//! leading column's trailing edge) and `Slide` (a trailing panel's leading
//! edge). It shows the resize cursor; dragging it moves the width between
//! its least and greatest, and a double click restores the reset width.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;

use super::KoanTouchDrag as _;

use super::child;
use crate::theme::Look;

pub(super) type OnWidth = Rc<dyn Fn(&Pixels, &mut Window, &mut App)>;

/// How wide the grip is, centred on the divider: wide enough to find, narrow
/// enough to leave the controls at either column's edge to their own clicks.
pub(super) const GRIP: f32 = 8.0;

/// Which edge of the column the grip is on, and so which way a drag widens
/// it.
#[derive(Clone, Copy)]
pub(super) enum Edge {
    /// The column's right edge: dragging right widens it.
    Trailing,
    /// The column's left edge: dragging left widens it.
    Leading,
}

/// A drag under way: where the pointer and the column started.
#[derive(Clone, Copy)]
struct Drag {
    from_x: Pixels,
    from_width: Pixels,
    /// The width the drag last reached, which the release commits.
    to: Pixels,
}

pub(super) struct Grip {
    pub id: ElementId,
    pub edge: Edge,
    pub width: Pixels,
    pub min: Pixels,
    pub max: Pixels,
    pub reset: Pixels,
    pub on_resize: Option<OnWidth>,
    pub on_commit: Option<OnWidth>,
}

impl Grip {
    /// The grip, for the owner to place over the divider, and the element
    /// that follows a drag past it: drawn anywhere, it listens window-wide.
    pub fn build(
        self,
        look: &Look,
        window: &mut Window,
        cx: &mut App,
    ) -> (Stateful<Div>, impl IntoElement + use<>) {
        // Set on a press on the grip and cleared on release, without a
        // notify: nothing is drawn from it.
        let drag = window.use_keyed_state(child(&self.id, "drag"), cx, |_, _| None::<Drag>);
        let Self {
            id,
            edge,
            width,
            min,
            max,
            reset,
            on_resize,
            on_commit,
        } = self;
        let grip = {
            let (starting, on_resize, on_commit) =
                (drag.clone(), on_resize.clone(), on_commit.clone());
            div()
                .id(child(&id, "grip"))
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(GRIP))
                .cursor_col_resize()
                .koan_touch_drag(look)
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
                    let moved = event.position.x - d.from_x;
                    let to = match edge {
                        Edge::Trailing => d.from_width + moved,
                        Edge::Leading => d.from_width - moved,
                    }
                    .clamp(min, max);
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
        (grip, follow)
    }
}
