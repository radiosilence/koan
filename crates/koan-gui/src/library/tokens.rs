//! The kōan theme's tokens as the library pages use them, from
//! `docs/design/koan-theme.md`. Dark values only, and plain constants: the
//! theme module replaces this file when it lands, and every page names a role
//! here rather than a literal so the switch is mechanical.

use gpui_kit::*;

pub const fn hex(rgb: u32) -> Rgba {
    Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.0,
        g: ((rgb >> 8) & 0xff) as f32 / 255.0,
        b: (rgb & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

pub const fn alpha(c: Rgba, a: f32) -> Rgba {
    Rgba { a, ..c }
}

pub const BG: Rgba = hex(0x1e1e1e);
pub const SURFACE: Rgba = hex(0x2a2a2a);
pub const RULE: Rgba = hex(0x383838);
pub const HOVER: Rgba = hex(0x4d4d4d);
pub const INK: Rgba = hex(0xcccccc);
pub const STRONG: Rgba = hex(0xffffff);
pub const MUTED: Rgba = hex(0x919191);
/// Mint: the accent with nothing playing. The record's accent arrives with
/// the theme.
pub const ACCENT: Rgba = hex(0x7dd3a7);
pub const BAD: Rgba = hex(0xef6b73);
/// A flat dim over artwork, under the hover play mark. Not a fade.
pub const DIM: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.35,
};

pub const FONT: &str = "Geist Mono";

/// The Mac draws the type scale at 0.86 of its points, where Geist Mono's
/// x-height meets the system face's 13-point body. Linux and Windows take the
/// same until their own faces are measured against it.
const SCALE: f32 = 0.86;

#[derive(Clone, Copy)]
pub enum Type {
    Title,
    TitleSmall,
    Body,
    Control,
    Meta,
    Fine,
}

impl Type {
    pub fn size(self) -> Pixels {
        px(SCALE
            * match self {
                Type::Title => 26.0,
                Type::TitleSmall => 22.0,
                Type::Body => 15.0,
                Type::Control => 14.0,
                Type::Meta => 13.0,
                Type::Fine => 12.0,
            })
    }

    pub fn weight(self) -> FontWeight {
        match self {
            Type::Title | Type::TitleSmall => FontWeight::LIGHT,
            _ => FontWeight::NORMAL,
        }
    }

    pub fn line_height(self) -> Pixels {
        self.size()
            * match self {
                Type::Title | Type::TitleSmall => 1.2,
                Type::Body => 1.45,
                Type::Control => 1.3,
                Type::Meta | Type::Fine => 1.35,
            }
    }
}

/// Text in a role and tone.
pub trait Typeset: Styled + Sized {
    fn typeset(self, role: Type, tone: Rgba) -> Self {
        self.font_family(FONT)
            .text_size(role.size())
            .font_weight(role.weight())
            .line_height(role.line_height())
            .text_color(tone)
    }
}

impl<T: Styled> Typeset for T {}

/// The steps of the spacing scale the library uses, at half the type's scale
/// on the Mac (0.93 at 100 %).
pub mod space {
    use gpui_kit::{Pixels, px};
    const S: f32 = 0.93;
    pub fn s4() -> Pixels {
        px(4.0 * S)
    }
    pub fn s8() -> Pixels {
        px(8.0 * S)
    }
    pub fn s12() -> Pixels {
        px(12.0 * S)
    }
    pub fn s16() -> Pixels {
        px(16.0 * S)
    }
}

pub fn hairline() -> Pixels {
    px(1.0)
}
