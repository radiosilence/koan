//! What a touch screen does in place of a pointer's secondary button: a
//! long press. Only under touch (`Look::touch`); a desktop never draws it.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;

use crate::theme::Look;

/// A long press on any element, under touch: `pressed` runs with where the
/// touch went down, once the press has been held long enough to be one. Of
/// several elements listening under the touch, the one drawn last (on top)
/// takes it. The press is claimed, so it becomes neither a tap nor a scroll.
pub trait KoanLongPress: ParentElement + Sized {
    fn koan_long_press(
        self,
        look: &Look,
        pressed: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        if !look.touch {
            return self;
        }
        self.child(long_press(Rc::new(pressed), None))
    }
}

impl<E: ParentElement> KoanLongPress for E {}

pub(crate) type Pressed = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;
pub(crate) type Lifted = Rc<dyn Fn(&mut Window, &mut App)>;

/// Whether a koan-kit listener has taken the press now under way. GPUI's own
/// claim cannot say: a draggable element claims every press over it so its
/// drag can lift, and a menu over it must still open.
#[derive(Default)]
struct Taken(bool);

impl Global for Taken {}

/// A layer over its parent that takes nothing from the pointer (its hitbox
/// leaves the elements under it hovered) and takes a long press that starts
/// over it and over nothing that blocks the pointer above it, unless a layer
/// drawn after it took the press first. `lifted` runs while a held touch
/// carries a drag, as moving a held row lifts one: what the press opened
/// makes way for it.
pub(crate) fn long_press(pressed: Pressed, lifted: Option<Lifted>) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            let pressed = pressed.clone();
            let lifted = lifted.clone();
            window.on_mouse_event(move |event: &LongPressEvent, phase, window, cx| {
                match (phase, event.phase) {
                    (DispatchPhase::Capture, TouchPhase::Started) => {
                        cx.default_global::<Taken>().0 = false;
                    }
                    (DispatchPhase::Bubble, TouchPhase::Started) => {
                        if cx.default_global::<Taken>().0 || !hitbox.is_hovered(window) {
                            return;
                        }
                        cx.default_global::<Taken>().0 = true;
                        // Claimed, and left to bubble: a draggable element under
                        // this one lifts its drag if the press then moves.
                        window.prevent_default();
                        pressed(event.start_position, window, cx);
                    }
                    (DispatchPhase::Bubble, TouchPhase::Moved) if cx.has_active_drag() => {
                        if let Some(lifted) = &lifted {
                            lifted(window, cx);
                        }
                    }
                    _ => {}
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
    use crate::theme::KoanTheme;

    struct Nested {
        heard: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for Nested {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let look = Look::of(window, cx);
            let (outer, inner) = (self.heard.clone(), self.heard.clone());
            div()
                .size_full()
                .debug_selector(|| "outer".into())
                .koan_long_press(&look, move |_, _, _| outer.borrow_mut().push("outer"))
                .child(
                    div()
                        .size(px(40.))
                        .debug_selector(|| "inner".into())
                        .koan_long_press(&look, move |_, _, _| inner.borrow_mut().push("inner")),
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

    /// A press held over the element drawn on top is that element's alone;
    /// one beside it reaches the one under it.
    #[gpui::test]
    fn the_listener_on_top_takes_the_press(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
            KoanTheme::set_touch(true, cx);
        });
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
