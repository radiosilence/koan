use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use super::Handler;
use crate::theme::{KoanStyled, Look, Tone, TypeRole};

/// A small fact set apart, such as a format: `fine` in `ink` in a square
/// `rule` outline, or in the accent where the fact is a match.
#[derive(IntoElement)]
pub struct Badge {
    text: SharedString,
    accent: bool,
}

impl Badge {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            accent: false,
        }
    }

    pub fn accent(mut self, accent: bool) -> Self {
        self.accent = accent;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let (text, outline) = if self.accent {
            (look.text_color(Tone::Accent), look.accent.hsla())
        } else {
            (look.palette.ink.hsla(), look.palette.rule.hsla())
        };
        // A label to a screen reader, named by its text.
        super::accessible(
            div().id(ElementId::Name(format!("badge-{}", self.text).into())),
            gpui_kit::Role::Label,
            Some(self.text.clone()),
        )
        .flex_none()
        .px(px(7.0))
        .py(px(2.0))
        .border(look.hairline)
        .border_color(outline)
        .text_role(TypeRole::Fine, &look)
        .text_color(text)
        .whitespace_nowrap()
        .child(self.text)
    }
}

/// A chip that is a choice, a device or a preset: a square `muted` outline.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    text: SharedString,
    on_click: Option<Handler<ClickEvent>>,
}

impl Chip {
    /// A chip names a thing (a device, a preset), so its text keeps its case.
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            on_click: None,
        }
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let on_click = self.on_click;
        div()
            .id(self.id)
            .flex_none()
            .px(look.spaced(12.0))
            .py(look.spaced(6.0))
            .border(look.hairline)
            .border_color(look.palette.muted.hsla())
            .text_role(TypeRole::Meta, &look)
            .text_color(look.palette.ink.hsla())
            .whitespace_nowrap()
            .child(self.text)
            .when_some(on_click, |chip, on_click| {
                chip.cursor_pointer()
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
    }
}
