//! A control that opens a panel under it (`opens_panel`): a toolbar's menu,
//! a filter's choices, the selection's playlists. The panel is made each
//! time it opens and appears at once, as a menu does; the keyboard moves
//! into it and goes back to where it was when it closes. A press outside,
//! Escape, or the trigger again closes it.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;

use super::Dismiss;

/// A trigger drawn pressed while what it opened shows.
pub trait Selectable {
    fn selected(self, selected: bool) -> Self;
}

struct PanelState {
    open: bool,
    focus: FocusHandle,
    /// Where the keyboard was before the panel took it.
    before: Option<WeakFocusHandle>,
}

impl PanelState {
    fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        if open {
            self.before = window.focused(cx).map(|f| f.downgrade());
            // Once the panel is drawn and its focus can be taken.
            let focus = self.focus.clone();
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        } else if self.focus.contains_focused(window, cx) || window.focused(cx).is_none() {
            // Back to where it was, unless a choice took the keyboard on
            // somewhere else (a sheet, a field).
            if let Some(before) = self.before.take().and_then(|b| b.upgrade()) {
                window.focus(&before, cx);
            }
        }
        cx.notify();
    }
}

/// A control that opens a panel under it, made each time it opens: a
/// toolbar's menu, a filter's choices, the selection's playlists. The
/// trigger is drawn pressed while the panel shows; the panel's rows call
/// the `Dismiss` they are given before they act. A press outside closes it.
pub fn opens_panel<T: Selectable + IntoElement + 'static>(
    id: impl Into<ElementId>,
    trigger: T,
    content: impl Fn(Dismiss, &mut Window, &mut App) -> AnyElement + 'static,
) -> impl IntoElement {
    OpensPanel {
        id: id.into(),
        trigger: Box::new(move |open| trigger.selected(open).into_any_element()),
        content: Rc::new(content),
    }
}

/// The trigger in either state, chosen as the panel's state is read.
type Trigger = Box<dyn FnOnce(bool) -> AnyElement>;

#[derive(IntoElement)]
struct OpensPanel {
    id: ElementId,
    trigger: Trigger,
    content: Rc<dyn Fn(Dismiss, &mut Window, &mut App) -> AnyElement>,
}

impl RenderOnce for OpensPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| PanelState {
            open: false,
            focus: cx.focus_handle(),
            before: None,
        });
        let open = state.read(cx).open;
        let focus = state.read(cx).focus.clone();
        let toggle = state.clone();
        let trigger = div()
            .id(super::child(&self.id, "trigger"))
            // A press on the trigger while the panel shows is also a press
            // outside it, which has closed it by now: only a press that
            // finds the panel as this frame drew it toggles.
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                toggle.update(cx, |s, cx| {
                    if s.open == open {
                        s.set_open(!open, window, cx)
                    }
                });
            })
            .child((self.trigger)(open));
        let panel = open.then(|| {
            let dismiss: Dismiss = {
                let state = state.downgrade();
                Rc::new(move |window, cx| {
                    state.update(cx, |s, cx| s.set_open(false, window, cx)).ok();
                })
            };
            let content = (self.content)(dismiss.clone(), window, cx);
            let (out, escape) = (dismiss.clone(), dismiss);
            // Pinned under the trigger; the anchor keeps it in the window.
            div().absolute().top_full().left_0().child(
                deferred(
                    anchored().anchor(Anchor::TopLeft).snap_to_window().child(
                        div()
                            .id(super::child(&self.id, "panel"))
                            .track_focus(&focus)
                            .occlude()
                            .on_mouse_down_out(move |_, window, cx| out(window, cx))
                            .on_key_down(move |event, window, cx| {
                                if event.keystroke.key == "escape" {
                                    cx.stop_propagation();
                                    escape(window, cx);
                                }
                            })
                            .child(content),
                    ),
                )
                .with_priority(1),
            )
        });
        div().id(self.id).relative().child(trigger).children(panel)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use crate::components::TextTrigger;
    // `gpui::*` brings GPUI's `test` attribute, which would expand its own
    // `#[test]` into itself.
    use core::prelude::v1::test;

    struct Page {
        focus: FocusHandle,
    }

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(
                    div()
                        .id("before")
                        .track_focus(&self.focus)
                        .debug_selector(|| "outside".into())
                        .size(px(40.0)),
                )
                .child(opens_panel(
                    "genre",
                    TextTrigger::new("genre-trigger", "any genre"),
                    |_, _, _| {
                        div()
                            .debug_selector(|| "panel".into())
                            .size(px(80.0))
                            .into_any_element()
                    },
                ))
        }
    }

    fn page(cx: &mut TestAppContext) -> (FocusHandle, &mut VisualTestContext) {
        cx.update(crate::theme::init);
        let (page, cx) = cx.add_window_view(|_, cx| Page {
            focus: cx.focus_handle(),
        });
        let focus = page.read_with(cx, |p, _| p.focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.run_until_parked();
        (focus, cx)
    }

    fn press(cx: &mut VisualTestContext, selector: &'static str) {
        let at = cx.debug_bounds(selector).expect("drawn").center();
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
    }

    fn trigger(cx: &mut VisualTestContext) {
        let at = cx.debug_bounds("outside").expect("drawn").bottom_left() + point(px(4.0), px(8.0));
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
    }

    /// The trigger opens the panel and the keyboard goes into it; Escape
    /// closes it and gives the keyboard back.
    #[gpui::test]
    fn escape_closes_the_panel_and_gives_the_keyboard_back(cx: &mut TestAppContext) {
        let (focus, cx) = page(cx);
        trigger(cx);
        assert!(cx.debug_bounds("panel").is_some(), "the panel opens");
        assert!(!cx.update(|window, _| focus.is_focused(window)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds("panel").is_none(), "Escape closes it");
        assert!(cx.update(|window, _| focus.is_focused(window)));
    }

    /// A press outside closes the panel; the trigger pressed again while it
    /// shows closes it rather than opening it afresh.
    #[gpui::test]
    fn a_press_outside_or_the_trigger_closes_the_panel(cx: &mut TestAppContext) {
        let (_, cx) = page(cx);
        trigger(cx);
        press(cx, "outside");
        assert!(
            cx.debug_bounds("panel").is_none(),
            "a press outside closes it"
        );
        trigger(cx);
        assert!(cx.debug_bounds("panel").is_some());
        trigger(cx);
        assert!(cx.debug_bounds("panel").is_none(), "the trigger closes it");
    }
}
