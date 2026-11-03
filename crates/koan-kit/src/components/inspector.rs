//! A panel beside the page, the Mac's inspector: its title in the app's
//! words with what trails it (a source, a spinner), a hairline under the
//! head, a hairline down its leading edge, and its body. On `bg` unless the
//! wash runs under the whole window, where it is clear over the wash.
//!
//! `SyncedLines` (`lyrics.rs`) is the body a panel of timed text takes.

use gpui::prelude::*;
use gpui::*;

use super::RowMetrics;
use crate::theme::{KoanStyled, Look, Space, TypeRole, label};

#[derive(IntoElement)]
pub struct Inspector {
    title: SharedString,
    trailing: Option<AnyElement>,
    body: Option<AnyElement>,
    clear: bool,
}

impl Inspector {
    pub fn new(title: impl AsRef<str>) -> Self {
        Self {
            title: label(title),
            trailing: None,
            body: None,
            clear: false,
        }
    }

    /// At the head's far edge: where the content came from, or that it is
    /// still coming.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// No ground of its own: the wash runs under the whole window.
    pub fn clear(mut self, clear: bool) -> Self {
        self.clear = clear;
        self
    }
}

impl RenderOnce for Inspector {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        div()
            .w(RowMetrics::inspector(&look))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .when(!self.clear, |d| d.bg(p.bg.hsla()))
            .border_l(look.hairline)
            .border_color(p.rule.hsla())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(look.space(Space::L))
                    .py(look.spaced(11.0))
                    .border_b(look.hairline)
                    .border_color(p.rule.hsla())
                    .child(
                        div()
                            .text_role(TypeRole::Body, &look)
                            .text_color(p.strong.hsla())
                            .child(self.title),
                    )
                    .children(self.trailing.map(|t| {
                        div()
                            .text_role(TypeRole::Fine, &look)
                            .text_color(p.muted.hsla())
                            .child(t)
                    })),
            )
            .child(div().flex_1().min_h_0().children(self.body))
    }
}

/// Text that does not follow the song, in `control`, `ink`, wrapping.
pub fn plain_text(text: impl Into<SharedString>, look: &Look) -> Div {
    div()
        .p(look.spaced(18.0))
        .text_role(TypeRole::Control, look)
        .text_color(look.palette.ink.hsla())
        .child(text.into())
}
