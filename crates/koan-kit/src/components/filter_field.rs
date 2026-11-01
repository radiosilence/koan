//! The toolbar's filter field, the sidebar's search field's look: no box,
//! the glyph and a lowercase prompt in `muted`, on the page's ground. The
//! caller owns the `InputState` and listens to it.

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::TextField;
use crate::theme::{Icon, KoanIcon, Look, Space, Tone, TypeRole};

/// The field's width at 100 %, grown with the text size.
const WIDTH: f32 = 200.0;

#[derive(IntoElement)]
pub struct FilterField {
    input: Entity<InputState>,
}

impl FilterField {
    pub fn new(input: &Entity<InputState>) -> Self {
        Self {
            input: input.clone(),
        }
    }
}

impl RenderOnce for FilterField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex()
            .items_center()
            .gap(look.space(Space::Xs))
            .w(px(WIDTH * look.scale.size))
            .child(
                KoanIcon::new(Icon::Search)
                    .role(TypeRole::Control)
                    .tone(Tone::Muted),
            )
            .child(TextField::new(&self.input).bare())
    }
}
