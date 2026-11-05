//! What a touch screen does in place of a pointer's secondary button: a
//! long press, claimed by the innermost element listening for one.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;

/// A long press on any element: `pressed` runs with where the touch went
/// down, once the press has been held long enough to be one. The press is
/// claimed, so it becomes neither a tap nor a scroll, and an element inside
/// that also listens hears it first and keeps it.
pub trait KoanLongPress: ParentElement + Sized {
    fn koan_long_press(
        self,
        pressed: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.child(long_press(Rc::new(pressed), None))
    }
}

impl<E: ParentElement> KoanLongPress for E {}

type Pressed = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;
type Lifted = Rc<dyn Fn(&mut Window, &mut App)>;

/// A layer over its parent that takes nothing from the pointer (its hitbox
/// leaves the elements under it hovered) and claims a long press that starts
/// over it and over nothing that blocks the pointer above it. `lifted` runs
/// while a held touch carries a drag, as moving a held row lifts one: what
/// the press opened makes way for it.
pub(crate) fn long_press(pressed: Pressed, lifted: Option<Lifted>) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            let pressed = pressed.clone();
            let lifted = lifted.clone();
            window.on_mouse_event(move |event: &LongPressEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                match event.phase {
                    TouchPhase::Started => {
                        if window.default_prevented() || !hitbox.is_hovered(window) {
                            return;
                        }
                        // Claimed, but left to bubble: a draggable element under
                        // this one lifts its drag if the press then moves.
                        window.prevent_default();
                        pressed(event.start_position, window, cx);
                    }
                    TouchPhase::Moved if cx.has_active_drag() => {
                        if let Some(lifted) = &lifted {
                            lifted(window, cx);
                        }
                    }
                    TouchPhase::Moved | TouchPhase::Ended | TouchPhase::Cancelled => {}
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;
    use std::cell::RefCell;
    use std::time::Duration;

    use gpui::{TestAppContext, TouchEvent, TouchId, VisualTestContext};

    use super::*;

    struct Nested {
        heard: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for Nested {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (outer, inner) = (self.heard.clone(), self.heard.clone());
            div()
                .size_full()
                .debug_selector(|| "outer".into())
                .koan_long_press(move |_, _, _| outer.borrow_mut().push("outer"))
                .child(
                    div()
                        .size(px(40.))
                        .debug_selector(|| "inner".into())
                        .koan_long_press(move |_, _, _| inner.borrow_mut().push("inner")),
                )
        }
    }

    fn press(at: Point<Pixels>, cx: &mut VisualTestContext) {
        let touch = |phase| TouchEvent {
            id: TouchId(1),
            position: at,
            phase,
            ..Default::default()
        };
        cx.simulate_event(touch(TouchPhase::Started));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.simulate_event(touch(TouchPhase::Ended));
        cx.run_until_parked();
    }

    /// A press held over the inner element is the inner element's alone;
    /// one beside it reaches the outer.
    #[gpui::test]
    fn the_innermost_listener_keeps_the_press(cx: &mut TestAppContext) {
        let heard = Rc::new(RefCell::new(Vec::new()));
        let held = heard.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Nested { heard: held });
        cx.run_until_parked();
        let inner = cx.debug_bounds("inner").expect("drawn");
        press(inner.center(), cx);
        assert_eq!(*heard.borrow(), ["inner"]);
        press(inner.bottom_right() + point(px(20.), px(20.)), cx);
        assert_eq!(*heard.borrow(), ["inner", "outer"]);
    }
}
