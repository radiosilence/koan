//! What a drag shows: the chip under the pointer, saying what is carried,
//! and the target lit while a drag it accepts is over it. A target lights
//! only for the payloads it takes, so a drag over something that refuses it
//! shows nothing. Between rows, where a drop lands is the list's insertion
//! line, `INSERTION`, in the accent.

use gpui::prelude::*;
use gpui::*;

use super::{Cover, CoverArt, RowMetrics};
use crate::theme::{KoanStyled, Look, Space, TypeRole};

/// The share of the accent that lights a drop target.
const LIT: f32 = 0.18;

/// The insertion line's thickness: a hairline, doubled to read as a place.
pub const INSERTION: Pixels = px(2.);

/// The drag's preview: the first record's sleeve, when there is one, and
/// what is carried ("3 tracks", or the one thing's name).
#[derive(IntoElement)]
pub struct DragChip {
    sleeve: Option<CoverArt>,
    label: SharedString,
}

impl DragChip {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            sleeve: None,
            label: label.into(),
        }
    }

    pub fn sleeve(mut self, sleeve: CoverArt) -> Self {
        self.sleeve = Some(sleeve);
        self
    }
}

impl RenderOnce for DragChip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        div()
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .p(look.space(Space::Xs))
            .pr(look.space(Space::M))
            .bg(p.surface.hsla())
            .border(look.hairline)
            .border_color(p.rule.hsla())
            .text_role(TypeRole::Meta, &look)
            .text_color(p.ink.hsla())
            .whitespace_nowrap()
            .children(
                self.sleeve
                    .map(|sleeve| Cover::new(sleeve, RowMetrics::nav_art(&look))),
            )
            .child(self.label)
    }
}

/// A drop target lit while a drag of `T` is over it.
pub trait DropTarget: InteractiveElement + Sized {
    fn drop_highlight<T: 'static>(self, look: &Look) -> Self {
        let lit = look.accent.hsla().opacity(LIT);
        self.drag_over::<T>(move |style, _, _, _| style.bg(lit))
    }
}

impl<E: InteractiveElement> DropTarget for E {}
