//! The kōan theme as the library pages draw it: one value per render with
//! the window's colours (its appearance, and the record's accent) and the
//! type and spacing scale (its text size), so helpers that build elements
//! take a `&Paint` rather than each reading the theme.

use gpui_kit::*;

use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole, tokens as theme};

pub use crate::theme::TypeRole as Type;

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
    /// The accent as a fill, indicator or ring.
    pub accent: Hsla,
    /// The accent as text: `ink` where the accent cannot reach 4.5:1.
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
        self.hover.opacity(theme::ROW_HOVER_OPACITY)
    }

    /// A faint hairline of `ink` between regions.
    pub fn row_rule(&self) -> Hsla {
        self.ink.opacity(theme::ROW_RULE_OPACITY)
    }

    /// A flat dim over artwork, under the hover play mark. Not a fade.
    pub fn dim(&self) -> Hsla {
        hsla(0.0, 0.0, 0.0, 0.35)
    }
}

/// Text in a role and colour, on the window's scale.
pub trait Typeset: Styled + Sized {
    fn typeset(self, p: &Paint, role: TypeRole, color: Hsla) -> Self {
        self.text_role(role, &p.look).text_color(color)
    }
}

impl<T: Styled> Typeset for T {}

pub fn hairline() -> Pixels {
    theme::HAIRLINE
}
