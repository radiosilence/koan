//! The kōan icon set, as the web UI and the share page draw it.
//!
//! `glyphs` is compiled by `just icons` from the same SVGs the apps draw
//! (`apps/macos/Resources/Icons`). Each glyph is plain 24-unit strokes on the
//! theme's stroke rule (docs/design/koan-theme.md#icons): square caps, mitred
//! joins, the colour of the text beside it, and a width set by size.

mod glyphs;

pub use glyphs::*;

/// One glyph: its stroked outline, and the outline of its fill layer (the
/// heart of a favourite, the tick of a selection) where it has one.
pub struct Glyph {
    pub name: &'static str,
    pub strokes: &'static str,
    pub fills: &'static str,
}

impl Glyph {
    /// The glyph as an inline `<svg>` drawn `px` CSS pixels square, with the
    /// stylesheet's `class` on it. The stroke is the rule's, 0.6 + 0.04 × the
    /// point size beside which it stands, where the glyph's square is 1.2 times
    /// that size.
    pub fn svg(&self, px: f32, class: &str) -> String {
        let points = px / 1.2;
        let width = (0.6 + 0.04 * points) * 24.0 / px;
        let mut out = format!(
            "<svg class=\"{class}\" viewBox=\"0 0 24 24\" fill=none stroke=currentColor \
stroke-width=\"{width:.2}\" stroke-linecap=square stroke-linejoin=miter aria-hidden=true>"
        );
        if !self.strokes.is_empty() {
            out.push_str(&format!("<path d=\"{}\"/>", self.strokes));
        }
        if !self.fills.is_empty() {
            out.push_str(&format!(
                "<path d=\"{}\" fill=currentColor stroke=none/>",
                self.fills
            ));
        }
        out.push_str("</svg>");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_is_whole_and_drawn_square() {
        assert!(ALL.len() > 100);
        for g in ALL {
            assert!(
                !g.strokes.is_empty() || !g.fills.is_empty(),
                "{} has no shape",
                g.name
            );
        }
        let svg = PLAY.svg(18.0, "x");
        assert!(svg.contains("stroke-linecap=square") && svg.contains("class=\"x\""));
        assert!(HEART_FILLED.svg(15.0, "").contains("fill=currentColor"));
    }
}
