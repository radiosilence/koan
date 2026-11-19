//! A title over a line of detail, each cut to the width it is given.

use gpui::prelude::FluentBuilder;
use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div};

use super::Text;
use crate::theme::{Tone, TypeRole};

/// A title over an optional subtitle, each on one line and cut short where
/// it does not fit: what is playing over its artist, a device's name over
/// its state. In a row it takes the width the row's other parts leave, so a
/// row of a picture, `TwoLines` and a `ControlGroup` gives the words the
/// rest of the room.
#[derive(IntoElement)]
pub struct TwoLines {
    title: SharedString,
    subtitle: Option<SharedString>,
}

impl TwoLines {
    /// The title, strong.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
        }
    }

    /// The line under the title, smaller and muted.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }
}

impl RenderOnce for TwoLines {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                Text::new(TypeRole::Meta)
                    .tone(Tone::Strong)
                    .truncate()
                    .child(self.title),
            )
            .when_some(self.subtitle, |lines, subtitle| {
                lines.child(
                    Text::new(TypeRole::Fine)
                        .tone(Tone::Muted)
                        .truncate()
                        .child(subtitle),
                )
            })
    }
}
