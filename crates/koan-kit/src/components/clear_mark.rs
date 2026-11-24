//! The clear mark a text field shows at its trailing edge while it holds
//! text (the Mac's): `muted`, `ink` under the pointer, emptying the field on
//! a press and keeping the keyboard there. What was cleared fades out
//! leftward for a moment, a layer the GPU moves, so the clear is seen. The
//! filter field and the command bar's query both draw it.

use crate::components::field::FieldState as InputState;
use gpui::prelude::*;
use gpui::*;

use super::{Animated, AnimatedLayer, Ease, Repeat};
use crate::theme::{Icon, KoanIcon, KoanStyled, KoanTheme, Look, Motion, Tone, TypeRole};

/// The text a clear took, fading out, and the timer that lets it go.
#[derive(Default)]
struct Cleared {
    animated: Animated,
    ghost: Option<SharedString>,
    _end: Option<Task<()>>,
}

/// A field's clear: its mark, the cleared text's fade, and the clear itself
/// for a key (Escape) to run too. Made at render, from the view drawing the
/// field.
#[derive(Clone)]
pub(super) struct ClearMark {
    input: Entity<InputState>,
    cleared: Entity<Cleared>,
    view: EntityId,
    role: TypeRole,
}

impl ClearMark {
    /// The clear of `input`, whose text is set in `role`.
    pub(super) fn of(
        input: &Entity<InputState>,
        role: TypeRole,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let cleared = window.use_keyed_state(
            ElementId::from(("field-cleared", input.entity_id().as_u64())),
            cx,
            |_, _| Cleared::default(),
        );
        Self {
            input: input.clone(),
            cleared,
            view: window.current_view(),
            role,
        }
    }

    /// Empties the field, leaving what it held to fade out leftward over
    /// `Motion::SETTLE`: nothing under Reduce Motion.
    pub(super) fn clear(&self, window: &mut Window, cx: &mut App) {
        let text: SharedString = self.input.read(cx).value().to_string().into();
        self.input.update(cx, |input, cx| input.clean(window, cx));
        let view = self.view;
        if text.is_empty() || !KoanTheme::motion_allowed(cx) {
            return;
        }
        let travel = -12.0 * window.scale_factor();
        self.cleared.update(cx, |c, cx| {
            let at = Animated::now();
            super::layer::standing(&c.animated);
            c.animated.segment(
                0,
                1.0,
                0.0,
                at,
                Motion::SETTLE,
                Ease::QuickOut,
                Repeat::Once,
            );
            c.animated.segment(
                1,
                0.0,
                travel,
                at,
                Motion::SETTLE,
                Ease::QuickOut,
                Repeat::Once,
            );
            c.ghost = Some(text);
            c._end = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Motion::SETTLE).await;
                this.update(cx, |c, _| c.ghost = None).ok();
                cx.update(|cx| cx.notify(view));
            }));
        });
        cx.notify(view);
    }

    /// `field` taking the rest of the row, with the cleared text fading over
    /// it. An input is as wide as its parent, which a flex row otherwise
    /// sizes to the input's content, nothing.
    pub(super) fn text(&self, field: impl IntoElement, look: &Look, cx: &App) -> Div {
        let cleared = self.cleared.read(cx);
        let fading = cleared.animated.clone();
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .child(field)
            .children(cleared.ghost.clone().map(|ghost| {
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child(AnimatedLayer::new(
                        &fading,
                        div()
                            .whitespace_nowrap()
                            .text_role(self.role, look)
                            .text_color(look.palette.ink.hsla())
                            .child(ghost),
                    ))
            }))
    }

    /// The mark, while the field holds text.
    pub(super) fn mark(&self, look: &Look, cx: &App) -> Option<Stateful<Div>> {
        if self.input.read(cx).value().is_empty() {
            return None;
        }
        let clear = self.clone();
        Some(
            div()
                .id("field-clear")
                .map(|d| {
                    #[cfg(test)]
                    let d = d.debug_selector(|| "filter-clear-mark".into());
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
                    clear.clear(window, cx);
                    clear.input.update(cx, |input, cx| input.focus(window, cx));
                }),
        )
    }
}
