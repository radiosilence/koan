//! The kōan icon set, drawn as inline SVG.
//!
//! `glyphs` is compiled by koan's `just icons` from the SVGs every kōan app draws (`apps/ios/Resources/Icons`), and
//! from the few only the web draws (`koan-kit-web/icons`). Each glyph is plain strokes on a 24-unit grid on the theme's
//! stroke rule (koan's docs/design/koan-theme.md#icons): square caps, mitred joins, the colour of the text beside it,
//! and a width set by the size it is drawn at, so a small glyph is not spindly and a large one is not heavy.

mod glyphs;

pub use glyphs::*;

use crate::arg::{Amount, Text};
use crate::attrs::{Attrs, component};

/// One glyph: its stroked outline, and the outline of its fill layer (the heart of a favourite, the tick of a
/// selection) where it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    /// Its name, as its SVG is named: `heart-filled`.
    pub name: &'static str,
    /// Its stroked outline, as an SVG path.
    pub strokes: &'static str,
    /// Its filled outline, as an SVG path, or empty.
    pub fills: &'static str,
}

impl Glyph {
    /// The glyph drawn `px` CSS pixels square, as an [`Icon`]: decorative, hidden from screen readers, until
    /// [`label`](Icon::label) names it.
    pub fn svg(&self, px: impl Into<Amount>) -> Icon<'static> {
        Icon {
            glyph: *self,
            px: px.into(),
            label: Text::default(),
            attrs: Attrs::default(),
        }
    }

    /// The glyph named `name`, as its SVG is named.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        ALL.iter().find(|glyph| glyph.name == name).copied()
    }
}

/// A glyph drawn at a size: `kit::icon::HEART.svg(15)`.
#[must_use = "an icon draws nothing until it is rendered"]
#[derive(Clone, Debug, askama::Template)]
#[template(path = "icon.html")]
pub struct Icon<'a> {
    glyph: Glyph,
    px: Amount,
    label: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Icon<'a> {
    /// What the glyph means, for a glyph that stands alone: it becomes an image with this name. Without one it is
    /// decoration beside words that already say it, and hidden from screen readers.
    pub fn label(mut self, label: impl Into<Text<'a>>) -> Self {
        self.label = label.into();
        self
    }

    /// The stroke, in grid units: 0.6 + 0.04 × the point size of the text the glyph stands beside, where the glyph's
    /// square is 1.2 times that size.
    fn stroke_width(&self) -> f64 {
        let px = self.px.get().max(1.0);
        let points = px / 1.2;
        (0.6 + 0.04 * points) * 24.0 / px
    }
}

impl Icon<'_> {
    /// The stroke width as the template writes it.
    fn stroke(&self) -> String {
        format!("{:.2}", self.stroke_width())
    }
}

component!(Icon);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_is_whole_and_every_glyph_has_a_shape() {
        assert!(ALL.len() > 100, "the set has {} glyphs", ALL.len());
        for glyph in ALL {
            assert!(
                !glyph.strokes.is_empty() || !glyph.fills.is_empty(),
                "{} has no shape",
                glyph.name
            );
            assert!(
                !glyph.strokes.contains('"') && !glyph.fills.contains('"'),
                "{} would end its path's attribute",
                glyph.name
            );
        }
        assert_eq!(Glyph::named("heart"), Some(HEART));
    }

    #[test]
    fn an_icon_is_decoration_until_it_is_named() {
        let decoration = HEART.svg(15).to_string();
        assert!(decoration.contains("aria-hidden=\"true\""), "{decoration}");
        let named = HEART.svg(15).label("favourite").to_string();
        assert!(
            named.contains("role=\"img\" aria-label=\"favourite\""),
            "{named}"
        );
        assert!(!named.contains("aria-hidden"), "{named}");
    }

    #[test]
    fn a_glyph_is_drawn_on_the_stroke_rule() {
        let svg = HEART_FILLED.svg(15).to_string();
        assert!(svg.contains("width=\"15\" height=\"15\""), "{svg}");
        assert!(
            svg.contains("stroke-linecap=\"square\" stroke-linejoin=\"miter\""),
            "{svg}"
        );
        assert!(
            svg.contains("fill=\"currentColor\" stroke=\"none\""),
            "the fill layer is drawn: {svg}"
        );
        let small: f64 = HEART.svg(12).stroke_width();
        let large: f64 = HEART.svg(32).stroke_width();
        assert!(
            small > large,
            "a small glyph's stroke is wider on the grid, so it is not spindly on screen"
        );
    }
}
