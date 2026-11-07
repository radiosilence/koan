use gpui::{
    App, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
    div, px,
};

use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// An empty page: the place's glyph in `muted`, a title in `body`, `ink`,
/// and what to do about it in `meta`, `muted`, centred.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: Icon,
    title: SharedString,
    detail: Option<SharedString>,
    tone: Tone,
}

impl EmptyState {
    pub fn new(icon: Icon, title: impl AsRef<str>) -> Self {
        Self {
            icon,
            title: label(title),
            detail: None,
            tone: Tone::Muted,
        }
    }

    /// Something went wrong rather than nothing being there: the glyph in
    /// `bad`, as the Mac's startup error draws its warning.
    pub fn warning(mut self) -> Self {
        self.tone = Tone::Bad;
        self
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
        // A group to a screen reader, named by what it says.
        let spoken: gpui::SharedString = match &self.detail {
            Some(detail) => format!("{}. {detail}", self.title).into(),
            None => self.title.clone(),
        };
        super::accessible(
            div().id(gpui::ElementId::Name(
                format!("empty-{}", self.title).into(),
            )),
            gpui::Role::Group,
            Some(spoken),
        )
        .size_full()
        // Centred in what a phone's foot leaves of the page.
        .pb(super::foot_inset(window, cx))
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
                // Room for a title on one line, and a sentence of detail on
                // one or two with no word left alone on the last, at the size
                // type is drawn.
                .max_w(px(480.0 * look.scale.text))
                .text_center()
                .child(
                    KoanIcon::new(self.icon)
                        .points(28.0 * look.scale.text)
                        .tone(self.tone),
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
