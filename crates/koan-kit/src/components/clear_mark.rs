//! The clear mark a text field shows at its trailing edge while it holds
//! text the person can change (the Mac's): `muted`, `ink` under the
//! pointer, emptying the field on a press with `FieldState::clear` and
//! keeping the keyboard there. The filter field and the command bar's query
//! both draw it, and their owners run the same clear from a key.

use std::cell::Cell;
use std::rc::Rc;

use crate::components::field::FieldState as InputState;
use gpui::prelude::*;
use gpui::*;

use crate::theme::{Icon, KoanIcon, Look, Tone, TypeRole};

/// A field's clear: the row holding the field and its mark, which owns
/// where the keyboard is kept under touch. Made at render.
pub(super) struct ClearMark {
    input: Entity<InputState>,
    role: TypeRole,
    disabled: bool,
}

/// What an owner hears of the row's bounds as painted: where a filter field
/// stood, for it to open out from.
pub(super) type Measured = Box<dyn FnOnce(Bounds<Pixels>, &mut App)>;

/// A full-size canvas over its parent, handing its bounds to `measured` as
/// it is painted.
fn measure(measured: impl FnOnce(Bounds<Pixels>, &mut App) + 'static) -> impl IntoElement {
    canvas(move |bounds, _, cx| measured(bounds, cx), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// Sets where `input` keeps the keyboard, only when it moved.
fn keep(input: &Entity<InputState>, keeps: Vec<Bounds<Pixels>>, cx: &mut App) {
    if input.read(cx).keeps != keeps {
        input.update(cx, |state, _| state.keeps = keeps);
    }
}

impl ClearMark {
    /// The clear of `input`, whose text is set in `role`; a `disabled` field
    /// draws no mark.
    pub(super) fn of(input: &Entity<InputState>, role: TypeRole, disabled: bool) -> Self {
        Self {
            input: input.clone(),
            role,
            disabled,
        }
    }

    /// A row of `leading`, `field` taking the rest of it, and the mark while
    /// there is text to clear. Under touch the row and the mark's hit area,
    /// two rectangles, keep the keyboard: a tap on the mark or the glyph is
    /// the field's, not somewhere else to tap, and a tap beside a mark that
    /// stands past the row is elsewhere. `mark` dresses the mark (its hit
    /// size) for the owner's row; `measured` hears the row's bounds each
    /// paint.
    pub(super) fn row(
        &self,
        leading: Option<AnyElement>,
        field: impl IntoElement,
        mark: impl FnOnce(Stateful<Div>) -> Stateful<Div>,
        measured: Option<Measured>,
        look: &Look,
        cx: &App,
    ) -> Div {
        // The row is painted before its mark: with a mark, the mark's
        // measure writes both rectangles at once, once a frame.
        let row = Rc::new(Cell::new(None));
        let shown = self.mark(row.clone(), look, cx);
        let input = self.input.clone();
        let has_mark = shown.is_some();
        let measured_row = measure(move |bounds, cx| {
            if has_mark {
                row.set(Some(bounds));
            } else {
                keep(&input, vec![bounds], cx);
            }
            if let Some(measured) = measured {
                measured(bounds, cx);
            }
        });
        div()
            .relative()
            .flex()
            .items_center()
            .child(measured_row)
            .children(leading)
            // An input is as wide as its parent, which a flex row otherwise
            // sizes to the input's content, nothing.
            .child(div().flex_1().min_w_0().child(field))
            .children(shown.map(mark))
    }

    /// The mark, while the field holds text the person can change and is
    /// not disabled. Its whole hit area, which the owner may make taller
    /// than the row, keeps the keyboard beside the row's.
    fn mark(
        &self,
        row: Rc<Cell<Option<Bounds<Pixels>>>>,
        look: &Look,
        cx: &App,
    ) -> Option<Stateful<Div>> {
        let state = self.input.read(cx);
        if self.disabled || !state.editable() || state.value().is_empty() {
            return None;
        }
        let (input, measured) = (self.input.clone(), self.input.clone());
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
                .child(measure(move |bounds, cx| {
                    let keeps = row.get().into_iter().chain([bounds]).collect();
                    keep(&measured, keeps, cx);
                }))
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
