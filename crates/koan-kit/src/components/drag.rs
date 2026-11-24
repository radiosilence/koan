//! What a drag shows: the chip under the pointer, saying what is carried,
//! and the target lit while a drag it accepts is over it. A target lights
//! only for the payloads it takes, so a drag over something that refuses it
//! shows nothing. Between rows, where a drop lands is the list's insertion
//! line, `INSERTION`, in the accent. Under touch a drag entering a target
//! is felt as a tick, and a drop that lands as a tap (`landed`).

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
    /// A chip naming what is carried.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            sleeve: None,
            label: label.into(),
        }
    }

    /// The first record's sleeve beside the name.
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

/// A drop target lit while a drag of `T` is over it, and felt as the drag
/// enters it.
pub trait DropTarget: InteractiveElement + Sized {
    fn drop_highlight<T: 'static>(self, look: &Look) -> Self {
        let lit = look.accent.hsla().opacity(LIT);
        self.drag_over::<T>(move |style, _, _, _| style.bg(lit))
            .on_drag_move::<T>(|event, _, cx| {
                entered(
                    event.bounds,
                    event.bounds.contains(&event.event.position),
                    cx,
                )
            })
    }
}

/// The target the drag under way is over, by its bounds. Every target hears
/// every step of a drag, so the one it leaves clears it, and a new drag's
/// first step clears it too unless it starts over that same target.
#[derive(Default)]
struct Over(Option<Bounds<Pixels>>);

impl Global for Over {}

fn entered(bounds: Bounds<Pixels>, inside: bool, cx: &mut App) {
    let over = &mut cx.default_global::<Over>().0;
    if inside && *over != Some(bounds) {
        *over = Some(bounds);
        cx.perform_haptic(HapticFeedback::Selection);
    } else if !inside && *over == Some(bounds) {
        *over = None;
    }
}

/// A drop that landed, felt as a light tap. Every drop handler that takes
/// what it is given calls it; one that refuses stays still.
pub fn landed(cx: &App) {
    cx.perform_haptic(HapticFeedback::Light);
}

impl<E: InteractiveElement> DropTarget for E {}
