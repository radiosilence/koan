use gpui_kit::component::input::{Input, InputState};

/// What a `TextField` edits and what it reports: the page owns the state,
/// made with `cx.new(|cx| InputState::new(window, cx))`, and subscribes to
/// its `InputEvent`s.
pub use gpui_kit::component::input::{InputEvent, InputState as FieldState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{App, Entity, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px};

use crate::theme::{KoanStyled, Look, Space, TypeRole};

/// A text field: `control` type in `ink` on a square `surface` field with no
/// bezel, or on nothing at all without `ground`, as the sidebar's search is.
///
/// The text, the caret, the selection and input methods are gpui-component's
/// `Input`, whose own frame is turned off and whose padding and type are
/// replaced with the theme's. It reads its colours from gpui-component's
/// theme, which `KoanTheme::drive` keeps in the window's look; the spec
/// gives no selection colour, so a selection is the accent at 30 %. As on
/// the Mac, a field draws no focus ring: the caret shows where typing goes.
/// The owner
/// makes the `InputState` (`cx.new(|cx| InputState::new(window, cx))`), which
/// carries the placeholder, drawn in `muted`.
#[derive(IntoElement)]
pub struct TextField {
    state: Entity<InputState>,
    ground: bool,
    disabled: bool,
}

impl TextField {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            ground: true,
            disabled: false,
        }
    }

    /// Not editable, and faded: while what it feeds is under way.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// No field of its own, on the page's ground.
    pub fn bare(mut self) -> Self {
        self.ground = false;
        self
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let line = look.line_height(TypeRole::Control);
        div()
            .relative()
            .flex()
            .items_center()
            .when(self.ground, |field| {
                field
                    .px(look.space(Space::M))
                    .py(look.space(Space::S))
                    .bg(look.palette.surface.hsla())
            })
            .when(self.disabled, |field| {
                field.opacity(crate::theme::DISABLED_OPACITY)
            })
            .child(
                Input::new(&self.state)
                    .disabled(self.disabled)
                    .appearance(false)
                    .px(px(0.0))
                    .py(px(0.0))
                    .h(line)
                    .text_role(TypeRole::Control, &look)
                    .text_color(look.palette.ink.hsla()),
            )
    }
}
