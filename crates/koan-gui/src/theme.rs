//! The kōan theme's tokens, from `docs/design/koan-theme.md`.
//!
//! Colours and the spacing scale only. Type, glyphs, components, the record
//! accent and the wash belong to the full theme module that replaces this one.

use gpui_kit::{App, Global, Hsla, Pixels, WindowAppearance, px};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Hsla,
    pub surface: Hsla,
    pub rule: Hsla,
    pub hover: Hsla,
    pub ink: Hsla,
    pub strong: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub bad: Hsla,
}

pub const DARK: Palette = Palette {
    bg: hex(0x1e1e1e),
    surface: hex(0x2a2a2a),
    rule: hex(0x383838),
    hover: hex(0x4d4d4d),
    ink: hex(0xcccccc),
    strong: hex(0xffffff),
    muted: hex(0x919191),
    accent: hex(0x7dd3a7),
    bad: hex(0xef6b73),
};

pub const LIGHT: Palette = Palette {
    bg: hex(0xffffff),
    surface: hex(0xf2f2f2),
    rule: hex(0xe0e0e0),
    hover: hex(0xc4c4c4),
    ink: hex(0x333333),
    strong: hex(0x111111),
    muted: hex(0x666666),
    accent: hex(0x1f7a50),
    bad: hex(0xc43f3f),
};

const fn hex(value: u32) -> Hsla {
    // GPUI's `rgb` is not const, so the conversion is spelled out.
    let r = ((value >> 16) & 0xff) as f32 / 255.0;
    let g = ((value >> 8) & 0xff) as f32 / 255.0;
    let b = (value & 0xff) as f32 / 255.0;
    let max = if r > g {
        if r > b { r } else { b }
    } else if g > b {
        g
    } else {
        b
    };
    let min = if r < g {
        if r < b { r } else { b }
    } else if g < b {
        g
    } else {
        b
    };
    let l = (max + min) / 2.0;
    if max == min {
        return Hsla {
            h: 0.0,
            s: 0.0,
            l,
            a: 1.0,
        };
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    Hsla {
        h: h / 6.0,
        s,
        l,
        a: 1.0,
    }
}

/// The palette in force, chosen by the window's appearance.
pub struct Theme(pub Palette);

impl Global for Theme {}

impl Theme {
    pub fn follow(appearance: WindowAppearance, cx: &mut App) {
        let palette = match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => LIGHT,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => DARK,
        };
        cx.set_global(Theme(palette));
        components(&palette, cx);
    }
}

pub fn palette(cx: &App) -> Palette {
    cx.global::<Theme>().0
}

/// The component library's own theme, held to the tokens, for the few of its
/// pieces drawn here (context menus): square, flat, on `bg` in a `rule`
/// hairline.
fn components(p: &Palette, cx: &mut App) {
    let theme = gpui_kit::component::Theme::global_mut(cx);
    let c = &mut theme.colors;
    c.background = p.bg;
    c.foreground = p.ink;
    c.popover = p.bg;
    c.popover_foreground = p.ink;
    c.border = p.rule;
    c.accent = p.surface;
    c.accent_foreground = p.strong;
    c.muted_foreground = p.muted;
    c.list_hover = p.surface;
    c.list_active = p.surface;
    c.list_active_border = p.accent;
    c.danger = p.bad;
    theme.radius = px(0.);
    theme.radius_lg = px(0.);
    theme.shadow = false;
}

/// The spacing scale: every gap is one of these.
#[allow(dead_code)]
pub mod space {
    use super::*;
    pub const XS: Pixels = px(4.);
    pub const S: Pixels = px(8.);
    pub const M: Pixels = px(12.);
    pub const L: Pixels = px(16.);
    pub const XL: Pixels = px(22.);
    pub const XXL: Pixels = px(32.);
}

pub const HAIRLINE: Pixels = px(1.);

/// Type sizes, drawn at the Mac's 0.86 of the spec's points.
#[allow(dead_code)]
pub mod text {
    use super::*;
    pub const TITLE: Pixels = px(26. * 0.86);
    pub const BODY: Pixels = px(15. * 0.86);
    pub const CONTROL: Pixels = px(14. * 0.86);
    pub const META: Pixels = px(13. * 0.86);
    pub const FINE: Pixels = px(12. * 0.86);
}
