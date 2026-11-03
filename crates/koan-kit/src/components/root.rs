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
