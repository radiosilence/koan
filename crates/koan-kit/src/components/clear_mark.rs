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

/// A field's clear mark, kept on the field's state: the text a clear took,
/// fading out, so a clear run from outside the field's render (a key) fades
/// as the mark's does, and the bounds the keyboard is kept over.
#[derive(Default)]
pub(crate) struct Cleared {
    /// A clear mark's row draws the field: only then does a clear fade, and
    /// only then does the row own `FieldState::keeps`.
    pub(crate) drawn: bool,
    animated: Animated,
    /// What was cleared, as drawn, and how far the field had scrolled it.
    ghost: Option<(SharedString, Pixels)>,
    _end: Option<Task<()>>,
    /// The row and the mark's hit area as last painted, whose union keeps
    /// the keyboard under touch.
    row: Option<Bounds<Pixels>>,
    mark: Option<Bounds<Pixels>>,
}

impl Cleared {
    /// Whether cleared text is still fading: the placeholder waits for it.
    pub(crate) fn fading(&self) -> bool {
        self.ghost.is_some()
    }

    /// The cleared text as it fades.
    #[cfg(test)]
    pub(crate) fn ghost(&self) -> Option<&str> {
        self.ghost.as_ref().map(|(text, _)| text.as_ref())
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

    /// The fade over, or cut short by an edit: the placeholder shows again.
    pub(crate) fn gone(&mut self) {
        self.ghost = None;
        self._end = None;
    }
}

/// Where a press keeps the keyboard under touch: the row, and the mark's
/// hit area where it stands past the row.
fn keep(state: &mut InputState) {
    state.keeps = match (state.cleared.row, state.cleared.mark) {
        (Some(row), Some(mark)) => Some(row.union(&mark)),
        (row, mark) => row.or(mark),
    };
}

/// A field's clear: the row holding the field and its mark, and the cleared
/// text's fade. Made at render.
pub(super) struct ClearMark {
    input: Entity<InputState>,
    role: TypeRole,
    disabled: bool,
}

/// What an owner hears of the row's bounds as painted: where a filter field
/// stood, for it to open out from.
pub(super) type Measured = Box<dyn FnOnce(Bounds<Pixels>, &mut App)>;

impl ClearMark {
    /// The clear of `input`, whose text is set in `role`; a `disabled` field
    /// draws no mark.
    pub(super) fn of(
        input: &Entity<InputState>,
        role: TypeRole,
        disabled: bool,
        cx: &mut App,
    ) -> Self {
        if !input.read(cx).cleared.drawn {
            input.update(cx, |state, _| state.cleared.drawn = true);
        }
        Self {
            input: input.clone(),
            role,
            disabled,
        }
    }

    /// A row of `leading`, `field` taking the rest of it with the cleared
    /// text fading over it, and the mark while there is text. Under touch
    /// the row and the mark's hit area keep the keyboard: a tap on the mark
    /// or the glyph is the field's, not somewhere else to tap. `mark`
    /// dresses the mark (its hit size) for the owner's row; `measured` hears
    /// the row's bounds each paint.
    pub(super) fn row(
        &self,
        leading: Option<AnyElement>,
        field: impl IntoElement,
        mark: impl FnOnce(Stateful<Div>) -> Stateful<Div>,
        measured: Option<Measured>,
        look: &Look,
        cx: &App,
    ) -> Div {
        let input = self.input.clone();
        let measure = canvas(
            move |bounds, _, cx| {
                input.update(cx, |state, _| {
                    state.cleared.row = Some(bounds);
                    state.cleared.mark = None;
                    keep(state);
                });
                if let Some(measured) = measured {
                    measured(bounds, cx);
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
                    .map(|d| {
                        #[cfg(test)]
                        let d = d.debug_selector(|| "field-cleared".into());
                        d
                    })
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

    /// The mark, while the field holds text and is not disabled. Its whole
    /// hit area, which the owner may make taller than the row, keeps the
    /// keyboard.
    fn mark(&self, look: &Look, cx: &App) -> Option<Stateful<Div>> {
        if self.disabled || self.input.read(cx).value().is_empty() {
            return None;
        }
        let input = self.input.clone();
        let measured = self.input.clone();
        let measure = canvas(
            move |bounds, _, cx| {
                measured.update(cx, |state, _| {
                    state.cleared.mark = Some(bounds);
                    keep(state);
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        Some(
            div()
                .id("field-clear")
                .map(|d| {
                    #[cfg(test)]
                    let d = d.debug_selector(|| "field-clear".into());
                    d
                })
                .relative()
                .flex_none()
                .cursor_pointer()
                .group("field-clear")
                .child(measure)
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
