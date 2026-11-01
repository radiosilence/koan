//! The foot of a navigation list: what the app is connected to, as a line
//! with its glyph, and figures about what it holds, under a faint rule. The
//! block decides its own inset, so it lines up with the rows above it.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::{Icon, KoanIcon, KoanStyled, Look, ROW_RULE_OPACITY, Space, Tone, TypeRole};

#[derive(IntoElement, Default)]
pub struct StatusBlock {
    lines: Vec<(Icon, SharedString, Tone)>,
    figures: Vec<SharedString>,
}

impl StatusBlock {
    pub fn new() -> Self {
        Self::default()
    }

    /// A state, with its glyph: "connected to …", "sharing on port …".
    pub fn line(mut self, icon: Icon, text: impl Into<SharedString>, tone: Tone) -> Self {
        self.lines.push((icon, text.into(), tone));
        self
    }

    /// A figure, in `meta`, `muted`: "5,123 tracks".
    pub fn figure(mut self, text: impl Into<SharedString>) -> Self {
        self.figures.push(text.into());
        self
    }
}

impl RenderOnce for StatusBlock {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let truncated = |d: Div| {
            d.min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(look.spaced(6.0))
            .border_t(crate::theme::HAIRLINE)
            .border_color(look.palette.ink.hsla().opacity(ROW_RULE_OPACITY))
            .p(look.space(Space::L))
            .children(self.lines.into_iter().map(|(icon, text, tone)| {
                let color = look.text_color(tone);
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::Xs))
                    .text_role(TypeRole::Meta, &look)
                    .text_color(color)
                    .child(KoanIcon::new(icon).role(TypeRole::Meta).color(color))
                    .child(truncated(div()).child(text))
            }))
            .when(!self.figures.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(look.spaced(2.0))
                        .text_role(TypeRole::Meta, &look)
                        .text_color(look.palette.muted.hsla())
                        .children(self.figures.into_iter().map(|f| truncated(div()).child(f))),
                )
            })
    }
}
