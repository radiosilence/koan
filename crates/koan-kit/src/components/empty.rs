use gpui_kit::{
    App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px,
};

use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// An empty page: the place's glyph in `muted`, a title in `body`, `ink`,
/// and what to do about it in `meta`, `muted`, centred.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: Icon,
    title: SharedString,
    detail: Option<SharedString>,
}

impl EmptyState {
    pub fn new(icon: Icon, title: impl AsRef<str>) -> Self {
        Self {
            icon,
            title: label(title),
            detail: None,
        }
    }

    /// A sentence, which keeps its case.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(look.space(Space::M))
                    .p(look.space(Space::Xxl))
                    // Room for a title on one line, at the size type is drawn.
                    .max_w(px(420.0 * look.scale.text))
                    .text_center()
                    .child(
                        KoanIcon::new(self.icon)
                            .points(28.0 * look.scale.text)
                            .tone(Tone::Muted),
                    )
                    .child(
                        div()
                            .text_role(TypeRole::Body, &look)
                            .text_color(look.palette.ink.hsla())
                            .child(self.title),
                    )
                    .children(self.detail.map(|detail| {
                        div()
                            .text_role(TypeRole::Meta, &look)
                            .text_color(look.palette.muted.hsla())
                            .child(detail)
                    })),
            )
    }
}
