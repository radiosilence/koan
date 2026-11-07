//! A chart's key: each series' stroke, short, beside its name, wrapping onto
//! as many lines as the width needs.

use gpui::prelude::*;
use gpui::*;

use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole};

/// How a key draws its series: a short line in the series' colour, dashed
/// or solid, a hairline for a rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swatch {
    color: Hsla,
    dashed: bool,
    thin: bool,
}

impl Swatch {
    pub fn line(color: Hsla) -> Self {
        Self {
            color,
            dashed: false,
            thin: false,
        }
    }

    pub fn dashed(mut self, dashed: bool) -> Self {
        self.dashed = dashed;
        self
    }

    /// A hairline, as a rule is drawn.
    pub fn thin(mut self) -> Self {
        self.thin = true;
        self
    }

    fn render(self, look: &Look) -> AnyElement {
        let height = if self.thin { look.hairline } else { px(2.0) };
        if !self.dashed {
            return div()
                .flex_none()
                .w(px(14.0))
                .h(height)
                .bg(self.color)
                .into_any_element();
        }
        div()
            .flex()
            .flex_none()
            .gap(px(2.0))
            .children((0..3).map(move |_| div().w(px(3.0)).h(height).bg(self.color)))
            .into_any_element()
    }
}

/// The keys, then anything else said about the chart (a level, a unit), in
/// one wrapping line of fine, muted text.
#[derive(IntoElement, Default)]
pub struct Legend {
    keys: Vec<(SharedString, Swatch)>,
    extra: Vec<AnyElement>,
}

impl Legend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn key(mut self, name: impl Into<SharedString>, swatch: Swatch) -> Self {
        self.keys.push((name.into(), swatch));
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.extra.push(child.into_any_element());
        self
    }
}

impl RenderOnce for Legend {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex()
            .flex_wrap()
            .gap(look.space(Space::M))
            .text_role(TypeRole::Fine, &look)
            .text_color(look.text_color(Tone::Muted))
            .children(self.keys.into_iter().map(|(name, swatch)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(swatch.render(&look))
                    .child(name)
            }))
            .children(self.extra)
    }
}
