//! A window's look as these views draw with it: the theme's `Look`, with its
//! colours flattened to the values the views paint with.

use crate::theme::{Look, Space, Tone, tokens};
use koan_kit::app::{App, Pixels, Window};
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{Hsla, hsla, px};

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
    pub on_selection: Hsla,
    /// Assistive technology is attached: what only a screen reader reads
    /// (rows' spoken lines, their places) is worked out only then.
    pub a11y: bool,
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
            on_selection: p.on_selection.hsla(),
            a11y: window.is_a11y_active(),
        }
    }

    /// The paint a list row draws with: as it is, or picked, with every
    /// colour text and marks take (`ink`, `strong`, `muted`, the accent,
    /// `bad`) as `on_selection`, as the Mac's selected rows are drawn.
    pub fn picked(&self, selected: bool) -> Paint {
        if !selected {
            return *self;
        }
        let on = self.on_selection;
        Paint {
            ink: on,
            strong: on,
            muted: on,
            accent: on,
            accent_text: on,
            bad: on,
            ..*self
        }
    }

    pub fn space(&self, space: Space) -> Pixels {
        self.look.space(space)
    }

    /// A width that holds text, in points at 100 %, grown with the text size.
    pub fn sized(&self, points: f32) -> Pixels {
        px(points * self.look.scale.size)
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
