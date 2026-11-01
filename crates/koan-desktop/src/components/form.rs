use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
};

use crate::theme::{HIT, KoanStyled, Look, Space, TypeRole, label};

/// A form's row: the label leading in `body`, `ink`, and what it holds
/// trailing, either a value in `control`, `muted`, or a control.
#[derive(IntoElement)]
pub struct FormRow {
    label: SharedString,
    value: Option<SharedString>,
    control: Option<AnyElement>,
}

impl FormRow {
    /// A label in the app's words, drawn lowercase.
    pub fn new(text: impl AsRef<str>) -> Self {
        Self {
            label: label(text),
            value: None,
            control: None,
        }
    }

    /// A label that is data rather than the app's words (an AutoEQ maker, a
    /// server extension), which keeps its case.
    pub fn data(text: impl Into<SharedString>) -> Self {
        Self {
            label: text.into(),
            value: None,
            control: None,
        }
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }
}

impl RenderOnce for FormRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(look.space(Space::M))
            .min_h(HIT)
            .child(
                div()
                    .text_role(TypeRole::Body, &look)
                    .text_color(look.palette.ink.hsla())
                    .child(self.label),
            )
            .children(self.value.map(|value| {
                div()
                    .text_role(TypeRole::Control, &look)
                    .text_color(look.palette.muted.hsla())
                    .child(value)
            }))
            .children(self.control)
    }
}

/// A section's heading: `fine`, `ink`, lowercase, with 16 points above.
#[derive(IntoElement)]
pub struct SectionHeader {
    title: SharedString,
}

impl SectionHeader {
    pub fn new(title: impl AsRef<str>) -> Self {
        Self {
            title: label(title),
        }
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .pt(look.space(Space::L))
            .text_role(TypeRole::Fine, &look)
            .text_color(look.palette.ink.hsla())
            .child(self.title)
    }
}
