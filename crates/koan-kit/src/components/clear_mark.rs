//! The clear mark a text field shows at its trailing edge while it holds
//! text (the Mac's): `muted`, `ink` under the pointer, emptying the field on
//! a press and keeping the keyboard there. What was cleared fades out
//! leftward for a moment from where it stood, a layer the GPU moves, and the
//! placeholder waits for it to go, so the clear is seen and the two never
//! overlap. The filter field and the command bar's query both draw it, and
//! their owners run the same clear from a key with `FieldState::clear`.

use crate::components::field::FieldState as InputState;
use gpui::prelude::*;
use gpui::*;

use super::{Animated, AnimatedLayer, Ease, Repeat};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Motion, Tone, TypeRole};

/// The text a clear took, fading out, kept on the field's state so a clear
/// run from outside the field's render (a key) fades as the mark's does.
#[derive(Default)]
pub(crate) struct Cleared {
    animated: Animated,
    /// What was cleared, and how far the field had scrolled it.
    ghost: Option<(SharedString, Pixels)>,
    _end: Option<Task<()>>,
}

impl Cleared {
    /// Whether cleared text is still fading: the placeholder waits for it.
    pub(crate) fn fading(&self) -> bool {
        self.ghost.is_some()
    }

    /// `text`, scrolled `scrolled` into its field, fading out leftward over
    /// `Motion::SETTLE` from now; `end` lets it go once it has.
    pub(crate) fn fade(
        &mut self,
        text: SharedString,
        scrolled: Pixels,
        window: &Window,
        end: Task<()>,
    ) {
        let travel = -12.0 * window.scale_factor();
        let at = Animated::now();
        super::layer::standing(&self.animated);
        self.animated.segment(
            0,
            1.0,
            0.0,
            at,
            Motion::SETTLE,
            Ease::QuickOut,
            Repeat::Once,
        );
        self.animated.segment(
            1,
            0.0,
            travel,
            at,
            Motion::SETTLE,
            Ease::QuickOut,
            Repeat::Once,
        );
        self.ghost = Some((text, scrolled));
        self._end = Some(end);
    }

    /// The fade over: the placeholder shows again.
    pub(crate) fn gone(&mut self) {
        self.ghost = None;
        self._end = None;
    }
}

/// A field's clear: the row holding the field and its mark, and the cleared
/// text's fade. Made at render.
pub(super) struct ClearMark {
    input: Entity<InputState>,
    role: TypeRole,
}

impl ClearMark {
    /// The clear of `input`, whose text is set in `role`.
    pub(super) fn of(input: &Entity<InputState>, role: TypeRole) -> Self {
        Self {
            input: input.clone(),
            role,
        }
    }

    /// A row of `leading`, `field` taking the rest of it with the cleared
    /// text fading over it, and the mark while there is text. Under touch
    /// the whole row keeps the keyboard: a tap on its mark or its glyph is
    /// the field's, not somewhere else to tap. `mark` dresses the mark (its
    /// hit size) for the owner's row.
    pub(super) fn row(
        &self,
        leading: Option<AnyElement>,
        field: impl IntoElement,
        mark: impl FnOnce(Stateful<Div>) -> Stateful<Div>,
        look: &Look,
        cx: &App,
    ) -> Div {
        let keeps = self.input.clone();
        let measure = canvas(
            move |bounds, _, cx| {
                if keeps.read(cx).keeps != Some(bounds) {
                    keeps.update(cx, |state, _| state.keeps = Some(bounds));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .relative()
            .flex()
            .items_center()
            .child(measure)
            .children(leading)
            .child(self.text(field, look, cx))
            .children(self.mark(look, cx).map(mark))
    }

    /// `field` taking the rest of the row, with the cleared text fading over
    /// it from where the field had scrolled it. An input is as wide as its
    /// parent, which a flex row otherwise sizes to the input's content,
    /// nothing.
    fn text(&self, field: impl IntoElement, look: &Look, cx: &App) -> Div {
        let cleared = &self.input.read(cx).cleared;
        let fading = cleared.animated.clone();
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .child(field)
            .children(cleared.ghost.clone().map(|(ghost, scrolled)| {
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child(AnimatedLayer::new(
                        &fading,
                        div()
                            .ml(-scrolled)
                            .whitespace_nowrap()
                            .text_role(self.role, look)
                            .text_color(look.palette.ink.hsla())
                            .child(ghost),
                    ))
            }))
    }

    /// The mark, while the field holds text.
    fn mark(&self, look: &Look, cx: &App) -> Option<Stateful<Div>> {
        if self.input.read(cx).value().is_empty() {
            return None;
        }
        let input = self.input.clone();
        Some(
            div()
                .id("field-clear")
                .map(|d| {
                    #[cfg(test)]
                    let d = d.debug_selector(|| "field-clear".into());
                    d
                })
                .flex_none()
                .cursor_pointer()
                .group("field-clear")
                .child(
                    KoanIcon::new(Icon::ClearField)
                        .role(self.role)
                        .tone(Tone::Muted)
                        .hover_color("field-clear", look.palette.ink.hsla()),
                )
                // Down rather than click, so the field never loses the
                // keyboard to the mark.
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    window.prevent_default();
                    input.update(cx, |input, cx| {
                        input.clear(window, cx);
                        input.focus(window, cx);
                    });
                }),
        )
    }
}
