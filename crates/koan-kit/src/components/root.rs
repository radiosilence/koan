//! What every window's content sits in: Tab and Shift-Tab through the
//! window's controls, kept inside a sheet or menu that holds the keyboard
//! (`focus_trap`), under the `Root` key context the app's bindings name.

use gpui::prelude::*;
use gpui::{AnyView, App, Context, KeyBinding, Window, actions, div};

actions!(koan_root, [Tab, TabPrev]);

/// The key context of every window's root.
pub const CONTEXT: &str = "Root";

pub(crate) fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", Tab, Some(CONTEXT)),
        KeyBinding::new("shift-tab", TabPrev, Some(CONTEXT)),
    ]);
}

pub struct Root {
    view: AnyView,
}

impl Root {
    pub fn new(view: impl Into<AnyView>) -> Self {
        Self { view: view.into() }
    }
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("root")
            .key_context(CONTEXT)
            .on_action(
                cx.listener(|_, _: &Tab, window, cx| super::focus_trap::step(true, window, cx)),
            )
            .on_action(
                cx.listener(|_, _: &TabPrev, window, cx| {
                    super::focus_trap::step(false, window, cx)
                }),
            )
            .relative()
            .size_full()
            .child(self.view.clone())
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Entity, FocusHandle, TestAppContext, VisualTestContext, px};

    use super::super::focus_trap::FocusTrap as _;
    use super::*;
    // `gpui::*` brings GPUI's `test` attribute, which would expand its own
    // `#[test]` into itself.
    use core::prelude::v1::test;

    struct Page {
        outside: Vec<FocusHandle>,
        trap: FocusHandle,
        inside: Vec<FocusHandle>,
    }

    impl Render for Page {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let trapped = div()
                .id("trap")
                .tab_stop(false)
                .children(self.inside.iter().enumerate().map(|(i, focus)| {
                    div()
                        .id(("inside", i))
                        .track_focus(focus)
                        .tab_index(0)
                        .size(px(1.0))
                }))
                .focus_trap(&self.trap, cx);
            div()
                .tab_group()
                .size_full()
                .children(self.outside.iter().enumerate().map(|(i, focus)| {
                    div()
                        .id(("outside", i))
                        .track_focus(focus)
                        .tab_index(0)
                        .size(px(1.0))
                }))
                .child(trapped)
        }
    }

    fn page(inside: usize, cx: &mut TestAppContext) -> (Entity<Page>, &mut VisualTestContext) {
        cx.update(crate::theme::init);
        let (root, cx) = cx.add_window_view(|_, cx| {
            let page = cx.new(|cx| Page {
                // More stops outside than a walk of the window would pass
                // in a hundred steps.
                outside: (0..256).map(|_| cx.focus_handle().tab_stop(true)).collect(),
                trap: cx.focus_handle(),
                inside: (0..inside)
                    .map(|_| cx.focus_handle().tab_stop(true))
                    .collect(),
            });
            Root::new(page)
        });
        let page = root.read_with(cx, |root, _| {
            root.view.clone().downcast::<Page>().expect("the page")
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        (page, cx)
    }

    /// Tab and Shift-Tab wrap among a trap's own controls, never reaching
    /// those outside it.
    #[gpui::test]
    fn tab_wraps_inside_a_trap(cx: &mut TestAppContext) {
        let (page, cx) = page(2, cx);
        let (inside, trap) = page.read_with(cx, |p, _| (p.inside.clone(), p.trap.clone()));
        cx.update(|window, cx| window.focus(&inside[1], cx));
        cx.run_until_parked();
        let focused = |cx: &mut VisualTestContext| cx.update(|window, cx| window.focused(cx));
        cx.dispatch_action(Tab);
        assert_eq!(
            focused(cx).as_ref(),
            Some(&inside[0]),
            "past the last, the first"
        );
        cx.dispatch_action(TabPrev);
        assert_eq!(
            focused(cx).as_ref(),
            Some(&inside[1]),
            "before the first, the last"
        );
        assert!(cx.update(|window, cx| trap.contains_focused(window, cx)));
    }

    /// A trap with no control of its own keeps the keyboard itself.
    #[gpui::test]
    fn a_trap_with_no_controls_keeps_the_keyboard(cx: &mut TestAppContext) {
        let (page, cx) = page(0, cx);
        let trap = page.read_with(cx, |p, _| p.trap.clone());
        cx.update(|window, cx| window.focus(&trap, cx));
        cx.run_until_parked();
        cx.dispatch_action(Tab);
        assert_eq!(cx.update(|window, cx| window.focused(cx)), Some(trap));
    }
}
