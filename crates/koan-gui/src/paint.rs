//! A window's look as these views draw with it: the theme's `Look`, with its
//! colours flattened to the values the views paint with.

use gpui_kit::{App, Hsla, Pixels, Window};
use koan_gui::theme::{Look, Space, Tone};

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
}
