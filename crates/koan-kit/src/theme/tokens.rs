//! The kōan theme's tokens: colour, type, spacing and motion, as
//! docs/design/koan-theme.md sets them out.

use std::time::Duration;

use gpui_kit::{FontWeight, Hsla, Pixels, Rgba, px, rgb};

/// A colour token as 0xRRGGBB, for arithmetic that needs the raw value (the
/// accent's contrast, the wash's ground).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hex(pub u32);

impl Hex {
    pub fn hsla(self) -> Hsla {
        rgb(self.0).into()
    }

    pub fn rgba(self) -> Rgba {
        rgb(self.0)
    }

    pub fn channels(self) -> [u8; 3] {
        [(self.0 >> 16) as u8, (self.0 >> 8) as u8, self.0 as u8]
    }
}

/// The colour tokens of one appearance. The accent is not here: it follows
/// the record playing (`theme::accent`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: Hex,
    pub surface: Hex,
    pub rule: Hex,
    pub hover: Hex,
    pub ink: Hex,
    pub strong: Hex,
    pub muted: Hex,
    pub bad: Hex,
    /// A picked list row's ground, opaque over the wash: the Mac's tables'
    /// selection, as the person's graphite accent draws it.
    pub selection: Hex,
    /// Text and marks on `selection`.
    pub on_selection: Hex,
}

impl Palette {
    pub const DARK: Palette = Palette {
        bg: Hex(0x1E1E1E),
        surface: Hex(0x2A2A2A),
        rule: Hex(0x383838),
        hover: Hex(0x4D4D4D),
        ink: Hex(0xCCCCCC),
        strong: Hex(0xFFFFFF),
        muted: Hex(0x919191),
        bad: Hex(0xEF6B73),
        selection: Hex(0x1F1D1E),
        on_selection: Hex(0xFFFFFF),
    };

    pub const LIGHT: Palette = Palette {
        bg: Hex(0xFFFFFF),
        surface: Hex(0xF2F2F2),
        rule: Hex(0xE0E0E0),
        hover: Hex(0xC4C4C4),
        ink: Hex(0x333333),
        strong: Hex(0x111111),
        muted: Hex(0x666666),
        bad: Hex(0xC43F3F),
        selection: Hex(0x5C5C5C),
        on_selection: Hex(0xFFFFFF),
    };

    pub fn of(dark: bool) -> Palette {
        if dark { Self::DARK } else { Self::LIGHT }
    }
}

/// The opacity of `ink` in the faint hairline that may mark where one region
/// of a list runs into the next (`koanRowRule`).
pub const ROW_RULE_OPACITY: f32 = 0.12;

/// The opacity of `hover` behind a list row under the pointer.
pub const ROW_HOVER_OPACITY: f32 = 0.3;

/// The opacity of a disabled control's label and outline.
pub const DISABLED_OPACITY: f32 = 0.4;

/// One rule's width: a point, not a device pixel, which at `rule`'s contrast
/// is too faint on a 2× display.
pub const HAIRLINE: Pixels = px(1.0);

/// The width of the accent rule on a selected navigation row's leading edge,
/// and of the focus ring.
pub const MARK: Pixels = px(2.0);

/// The smallest a control's hit area is in either direction.
pub const HIT: Pixels = px(44.0);

/// A toolbar button's cell, the size of the platform's own.
pub const TOOLBAR_BUTTON: Pixels = px(36.0);

/// What the type is drawn at against the scale's points, as on the Mac:
/// Geist Mono at 15 points stands as tall as a system 13-point body at 12.9.
pub const MAC_FIT: f32 = 0.86;

/// The text size steps, in percent.
pub const TEXT_SIZES: std::ops::RangeInclusive<u32> = 80..=130;
pub const TEXT_SIZE_STEP: u32 = 5;

/// The scales the text size gives: type at `MAC_FIT` times the size chosen,
/// spacing at half the type's change, rows at the size alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub text: f32,
    pub space: f32,
    pub size: f32,
}

impl Scale {
    pub fn of(text_size: u32) -> Scale {
        let size = text_size.clamp(*TEXT_SIZES.start(), *TEXT_SIZES.end()) as f32 / 100.0;
        let text = MAC_FIT * size;
        Scale {
            text,
            space: 1.0 - (1.0 - text) / 2.0,
            size,
        }
    }
}

/// The spacing scale, in points at 100 %. Every gap is one of these, through
/// `KoanTheme::space`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Xs,
    S,
    M,
    L,
    Xl,
    Xxl,
}

impl Space {
    pub fn points(self) -> f32 {
        match self {
            Space::Xs => 4.0,
            Space::S => 8.0,
            Space::M => 12.0,
            Space::L => 16.0,
            Space::Xl => 22.0,
            Space::Xxl => 32.0,
        }
    }
}

/// The roles of the type scale. One face, Geist Mono.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeRole {
    Display,
    Title,
    TitleSmall,
    Body,
    Control,
    Meta,
    Fine,
}

impl TypeRole {
    pub const ALL: [TypeRole; 7] = [
        TypeRole::Display,
        TypeRole::Title,
        TypeRole::TitleSmall,
        TypeRole::Body,
        TypeRole::Control,
        TypeRole::Meta,
        TypeRole::Fine,
    ];

    /// Points at the scale's own size, before the Mac fit and the text size.
    pub fn points(self) -> f32 {
        match self {
            TypeRole::Display => 34.0,
            TypeRole::Title => 26.0,
            TypeRole::TitleSmall => 22.0,
            TypeRole::Body => 15.0,
            TypeRole::Control => 14.0,
            TypeRole::Meta => 13.0,
            TypeRole::Fine => 12.0,
        }
    }

    pub fn weight(self) -> FontWeight {
        match self {
            TypeRole::Display => FontWeight::EXTRA_LIGHT,
            TypeRole::Title | TypeRole::TitleSmall => FontWeight::LIGHT,
            _ => FontWeight::NORMAL,
        }
    }

    /// Line height as a multiple of the size.
    pub fn line_height(self) -> f32 {
        match self {
            TypeRole::Display => 1.1,
            TypeRole::Title | TypeRole::TitleSmall => 1.2,
            TypeRole::Body => 1.45,
            TypeRole::Control => 1.3,
            TypeRole::Meta | TypeRole::Fine => 1.35,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TypeRole::Display => "display",
            TypeRole::Title => "title",
            TypeRole::TitleSmall => "title-sm",
            TypeRole::Body => "body",
            TypeRole::Control => "control",
            TypeRole::Meta => "meta",
            TypeRole::Fine => "fine",
        }
    }
}

/// The face every role is set in.
pub const FONT_FAMILY: &str = "Geist Mono";

/// Motion: fast and direct. A quick-out curve, a snappy start and a decisive
/// stop, with no tail, delay or overshoot.
pub struct Motion;

impl Motion {
    /// States: pressed, hover, selection, toggles.
    pub const FAST: Duration = Duration::from_millis(80);
    /// A marker moving between places, such as a segment's underline.
    pub const NORMAL: Duration = Duration::from_millis(120);
    /// The accent arriving with a record.
    pub const SETTLE: Duration = Duration::from_millis(250);

    /// The quick-out curve, `cubic-bezier(0.2, 0.9, 0.3, 1)`, at `t` in 0..=1.
    pub fn quick_out(t: f32) -> f32 {
        bezier(0.2, 0.9, 0.3, 1.0, t)
    }

    /// The ease-in-out the wash's drift and dissolve take, Core Animation's
    /// `easeInEaseOut`: `cubic-bezier(0.42, 0, 0.58, 1)`.
    pub fn ease_in_out(t: f32) -> f32 {
        bezier(0.42, 0.0, 0.58, 1.0, t)
    }
}

/// A CSS cubic Bézier timing function: the curve's y where its x is `t`.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let curve = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    // x(s) rises monotonically for control points inside 0..=1, so bisection
    // finds s exactly enough in a fixed number of steps.
    let (mut low, mut high) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (low + high) / 2.0;
        if curve(x1, x2, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    curve(y1, y2, (low + high) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_follows_the_mac() {
        let at_100 = Scale::of(100);
        assert!((at_100.text - 0.86).abs() < 1e-6);
        assert!((at_100.space - 0.93).abs() < 1e-6);
        assert_eq!(Scale::of(500).size, 1.3);
        assert_eq!(Scale::of(10).size, 0.8);
    }

    #[test]
    fn quick_out_starts_fast_and_lands() {
        assert_eq!(Motion::quick_out(0.0), 0.0);
        assert_eq!(Motion::quick_out(1.0), 1.0);
        assert!(Motion::quick_out(0.25) > 0.6);
        assert!((Motion::ease_in_out(0.5) - 0.5).abs() < 1e-3);
    }
}
