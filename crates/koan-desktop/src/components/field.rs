use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    App, Entity, FluentBuilder, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px,
};

use super::ring;
use crate::theme::{KoanStyled, Look, Space, TypeRole};

/// A text field: `control` type in `ink` on a square `surface` field with no
/// bezel, or on nothing at all without `ground`, as the sidebar's search is.
///
/// The text, the caret, the selection and input methods are gpui-component's
/// `Input`, whose own frame is turned off and whose padding and type are
/// replaced with the theme's. It reads its colours from gpui-component's
/// theme, which `KoanTheme::drive` keeps in the window's look. The owner
/// makes the `InputState` (`cx.new(|cx| InputState::new(window, cx))`), which
/// carries the placeholder, drawn in `muted`.
#[derive(IntoElement)]
pub struct TextField {
    state: Entity<InputState>,
    ground: bool,
}

impl TextField {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            ground: true,
        }
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
        let handle = self.state.read(cx).focus_handle(cx);
        let line = look.line_height(TypeRole::Control);
        div()
            .relative()
            .flex()
            .items_center()
            .px(look.space(Space::M))
            .py(look.space(Space::S))
            .when(self.ground, |field| field.bg(look.palette.surface.hsla()))
            .child(
                Input::new(&self.state)
                    .appearance(false)
                    .px(px(0.0))
                    .py(px(0.0))
                    .h(line)
                    .text_role(TypeRole::Control, &look)
                    .text_color(look.palette.ink.hsla()),
            )
            .children(ring(&handle, &look, window))
    }
}
