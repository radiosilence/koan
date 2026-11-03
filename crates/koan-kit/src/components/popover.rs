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
