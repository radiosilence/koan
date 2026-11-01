//! The toolbar: its items placed leading, in the middle or trailing, as
//! SwiftUI's `ToolbarItem` placements are, at the toolbar's height and gaps;
//! and the toolbar button, a glyph in a 36-point cell.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Button, ButtonKind};
use crate::theme::{HAIRLINE, Icon, KoanTheme, Look, Space};

/// The toolbar's height on the Mac.
pub const TOOLBAR_HEIGHT: f32 = 38.0;

#[derive(IntoElement, Default)]
pub struct Toolbar {
    inset: Pixels,
    leading: Vec<AnyElement>,
    principal: Option<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Room left at the leading edge for the window's own buttons, where
    /// they sit over the toolbar.
    pub fn inset(mut self, inset: Pixels) -> Self {
        self.inset = inset;
        self
    }

    /// At the leading edge: back and forward, the sidebar's toggle.
    pub fn leading(mut self, item: impl IntoElement) -> Self {
        self.leading.push(item.into_any_element());
        self
    }

    /// Filling the middle: the page's title, or bare space that moves the
    /// window.
    pub fn principal(mut self, item: impl IntoElement) -> Self {
        self.principal = Some(item.into_any_element());
        self
    }

    /// At the trailing edge: the lyrics toggle, the window's own buttons.
    pub fn trailing(mut self, item: impl IntoElement) -> Self {
        self.trailing.push(item.into_any_element());
        self
    }
}

impl RenderOnce for Toolbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .h(px(TOOLBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .pl(self.inset + look.space(Space::S))
            .pr(look.space(Space::S))
            .gap(look.space(Space::Xs))
            // With the wash under the whole window the toolbar has no
            // ground and meets the page at a hairline; otherwise it is on
            // `bg`, flush with the page.
            .map(|d| {
                if cx.global::<KoanTheme>().wash_window {
                    d.border_b(HAIRLINE).border_color(look.palette.rule.hsla())
                } else {
                    d.bg(look.palette.bg.hsla())
                }
            })
            .children(self.leading)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .children(self.principal),
            )
            .children(self.trailing)
    }
}

/// A toolbar's button: the glyph in `ink` in a 36-point cell, `surface`
/// under the pointer and `hover` pressed; the word in its place with icons
/// off.
#[derive(IntoElement)]
pub struct ToolbarButton {
    button: Button,
}

impl ToolbarButton {
    /// `label` names it to VoiceOver, and is drawn with icons off.
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl AsRef<str>) -> Self {
        Self {
            button: Button::new(id, ButtonKind::Toolbar).icon(icon).label(label),
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.button = self.button.disabled(disabled);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.button = self.button.selected(selected);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.button = self.button.on_click(handler);
        self
    }
}

impl RenderOnce for ToolbarButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.button
    }
}
