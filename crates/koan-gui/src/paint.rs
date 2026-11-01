//! A window's look as these views draw with it: the theme's `Look`, with its
//! colours flattened to the values the views paint with.

use crate::theme::{Look, Space, Tone, tokens};
use gpui_kit::{App, Hsla, Pixels, Window, hsla, px};

#[derive(Clone, Copy)]
pub struct Paint {
    pub look: Look,
    pub bg: Hsla,
    pub surface: Hsla,
    pub rule: Hsla,
    pub hover: Hsla,
    pub ink: Hsla,
    pub strong: Hsla,
    pub muted: Hsla,
    /// The record's accent, for fills, marks and rules.
    pub accent: Hsla,
    /// The accent where it would be text: `ink` when the accent cannot be
    /// read as text on the ground.
    pub accent_text: Hsla,
    pub bad: Hsla,
}

impl Paint {
    pub fn of(window: &Window, cx: &App) -> Self {
        let look = Look::of(window, cx);
        let p = look.palette;
        Self {
            look,
            bg: p.bg.hsla(),
            surface: p.surface.hsla(),
            rule: p.rule.hsla(),
            hover: p.hover.hsla(),
            ink: p.ink.hsla(),
            strong: p.strong.hsla(),
            muted: p.muted.hsla(),
            accent: look.color(Tone::Accent),
            accent_text: look.text_color(Tone::Accent),
            bad: p.bad.hsla(),
        }
    }

    pub fn space(&self, space: Space) -> Pixels {
        self.look.space(space)
    }

    /// A width that holds text, in points at 100 %, grown with the text size.
    pub fn sized(&self, points: f32) -> Pixels {
        px(points * self.look.scale.size)
    }

    /// A row's hover fill: `hover` at the theme's row opacity.
    pub fn row_hover(&self) -> Hsla {
        self.hover.opacity(tokens::ROW_HOVER_OPACITY)
    }

    /// A faint hairline of `ink` between regions.
    pub fn row_rule(&self) -> Hsla {
        self.ink.opacity(tokens::ROW_RULE_OPACITY)
    }

    /// A flat dim over artwork, under the hover play mark. Not a fade.
    pub fn dim(&self) -> Hsla {
        hsla(0.0, 0.0, 0.0, 0.35)
    }
}
