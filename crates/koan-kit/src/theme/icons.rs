//! The kōan icon set, drawn through GPUI's SVG sprites.
//!
//! `glyphs` is compiled by `just icons` from the SVGs the apps draw
//! (`apps/macos/Resources/Icons`). The stroke is not scaled with the drawing:
//! its width is set by the size of the type beside the glyph, so each glyph is
//! rasterised per stroke width. Every (glyph, width, layer) is a path served
//! by `Assets`, which writes the SVG on first use; GPUI keeps the rendered
//! sprite in its atlas by path and size, so a frame draws a cached sprite and
//! allocates nothing.

mod glyphs;

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::LazyLock;

use gpui_kit::{
    App, AssetSource, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px, svg,
};
use parking_lot::Mutex;

pub use glyphs::Icon;

use super::{Look, Tone, TypeRole};

/// One glyph: its stroked outline, and the outline of its fill layer (the
/// heart of a favourite, the box of a selection) where it has one, as
/// absolute M/L/C/Z commands on a 24-unit grid.
pub struct Glyph {
    pub name: &'static str,
    pub strokes: &'static str,
    pub fills: &'static str,
}

const GRID: f32 = 24.0;
const PREFIX: &str = "koan-icon/";

/// The parts of a glyph to draw: both, or one layer alone, for a glyph whose
/// fill takes a second colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Layer {
    All,
    Fills,
    Strokes,
}

impl Layer {
    fn name(self) -> &'static str {
        match self {
            Layer::All => "all",
            Layer::Fills => "fills",
            Layer::Strokes => "strokes",
        }
    }

    fn parse(name: &str) -> Option<Layer> {
        match name {
            "all" => Some(Layer::All),
            "fills" => Some(Layer::Fills),
            "strokes" => Some(Layer::Strokes),
            _ => None,
        }
    }
}

/// The square a glyph is drawn in beside type of `points`: the optical size
/// of an SF Symbol at that size.
pub fn side(points: f32) -> f32 {
    (points * 1.2).round()
}

/// The stroke, in points, beside type of `points`: 1.2 at 15, 1.4 at 20,
/// never under a point.
pub fn stroke_width(points: f32) -> f32 {
    (0.6 + 0.04 * points).max(1.0)
}

/// The stroke on the glyph's own grid, in hundredths of a unit: what the
/// sprite is keyed by.
fn stroke_units(points: f32) -> u32 {
    (stroke_width(points) * GRID / side(points) * 100.0).round() as u32
}

/// The asset path of one rasterisation, made once and shared after.
fn path(icon: Icon, points: f32, layer: Layer) -> SharedString {
    type Paths = HashMap<(Icon, u32, Layer), SharedString>;
    static PATHS: LazyLock<Mutex<Paths>> = LazyLock::new(Default::default);
    let units = stroke_units(points);
    PATHS
        .lock()
        .entry((icon, units, layer))
        .or_insert_with(|| format!("{PREFIX}{}/{units}/{}", icon.glyph().name, layer.name()).into())
        .clone()
}

/// The SVG for a glyph at a stroke width, in one colour: GPUI draws an SVG as
/// a mask and tints it. With no second colour, the strokes of a glyph with a
/// fill are cut out of the fill, as the Apple apps draw it.
fn render(glyph: &Glyph, units: u32, layer: Layer) -> String {
    let width = units as f32 / 100.0;
    let mut out = String::with_capacity(glyph.strokes.len() + glyph.fills.len() + 512);
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" \
         stroke-width=\"{width}\" stroke-linecap=\"square\" stroke-linejoin=\"miter\">"
    );
    let fill = |out: &mut String, colour: &str| {
        let _ = write!(
            out,
            "<path d=\"{}\" fill=\"{colour}\" stroke=\"{colour}\"/>",
            glyph.fills
        );
    };
    let strokes = |out: &mut String, colour: &str| {
        if !glyph.strokes.is_empty() {
            let _ = write!(out, "<path d=\"{}\" stroke=\"{colour}\"/>", glyph.strokes);
        }
    };
    match layer {
        Layer::All if !glyph.fills.is_empty() => {
            out.push_str(
                "<defs><mask id=\"cut\" maskUnits=\"userSpaceOnUse\" x=\"0\" y=\"0\" \
                 width=\"24\" height=\"24\">",
            );
            fill(&mut out, "#fff");
            strokes(&mut out, "#000");
            out.push_str(
                "</mask></defs><rect width=\"24\" height=\"24\" fill=\"#000\" mask=\"url(#cut)\"/>",
            );
        }
        Layer::Fills => {
            if !glyph.fills.is_empty() {
                fill(&mut out, "#000");
            }
        }
        Layer::All | Layer::Strokes => strokes(&mut out, "#000"),
    }
    out.push_str("</svg>");
    out
}

fn glyph_named(name: &str) -> Option<&'static Glyph> {
    Icon::ALL
        .iter()
        .map(|icon| icon.glyph())
        .find(|glyph| glyph.name == name)
}

/// The app's assets: the icon set's sprites, and gpui-kit's own assets (the
/// glyphs gpui-component's controls draw) for every other path.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        let Some(rest) = path.strip_prefix(PREFIX) else {
            return gpui_kit::assets::Assets.load(path);
        };
        let mut parts = rest.split('/');
        let (Some(name), Some(units), Some(layer), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Ok(None);
        };
        let (Some(glyph), Ok(units), Some(layer)) =
            (glyph_named(name), units.parse(), Layer::parse(layer))
        else {
            return Ok(None);
        };
        Ok(Some(Cow::Owned(render(glyph, units, layer).into_bytes())))
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

/// A glyph of the set, in the colour of the text beside it.
#[derive(IntoElement)]
pub struct KoanIcon {
    icon: Icon,
    size: Size,
    tone: Colour,
    fill: Option<Colour>,
    hover: Option<(SharedString, Hsla)>,
}

enum Size {
    Role(TypeRole),
    Points(f32),
}

enum Colour {
    Tone(Tone),
    Exact(Hsla),
}

impl KoanIcon {
    /// Sized for `body` type, in `ink`.
    pub fn new(icon: Icon) -> Self {
        Self {
            icon,
            size: Size::Role(TypeRole::Body),
            tone: Colour::Tone(Tone::Ink),
            fill: None,
            hover: None,
        }
    }

    /// The colour it takes while the pointer is over the group named, as the
    /// label of a button does: a glyph is drawn in a colour of its own, and
    /// does not follow its parent's.
    pub fn hover_color(mut self, group: impl Into<SharedString>, color: Hsla) -> Self {
        self.hover = Some((group.into(), color));
        self
    }

    /// Sized beside type of this role.
    pub fn role(mut self, role: TypeRole) -> Self {
        self.size = Size::Role(role);
        self
    }

    /// Sized beside type of this many points, as drawn.
    pub fn points(mut self, points: f32) -> Self {
        self.size = Size::Points(points);
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Colour::Tone(tone);
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.tone = Colour::Exact(color);
        self
    }

    /// Draws a fill layer in its own colour, with the strokes over it rather
    /// than cut out of it, as the album grid's tick is.
    pub fn fill(mut self, color: Hsla) -> Self {
        self.fill = Some(Colour::Exact(color));
        self
    }
}

impl RenderOnce for KoanIcon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let points = match self.size {
            Size::Role(role) => look.points(role),
            Size::Points(points) => points,
        };
        let resolve = |colour: Colour| match colour {
            Colour::Tone(tone) => look.color(tone),
            Colour::Exact(color) => color,
        };
        let color = resolve(self.tone);
        let side = px(side(points));
        let sprite = |layer: Layer, color: Hsla| {
            svg()
                .path(path(self.icon, points, layer))
                .size(side)
                .text_color(color)
        };
        // A glyph's colour is its own, so following the pointer is two
        // sprites, the second shown while the group is under it.
        if let Some((group, hovered)) = self.hover {
            return div()
                .relative()
                .flex_none()
                .size(side)
                .child(
                    div()
                        .absolute()
                        .group_hover(group.clone(), |s| s.opacity(0.))
                        .child(sprite(Layer::All, color)),
                )
                .child(
                    div()
                        .absolute()
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .child(sprite(Layer::All, hovered)),
                );
        }
        match self.fill.map(resolve) {
            Some(fill) if !self.icon.glyph().fills.is_empty() => div()
                .relative()
                .flex_none()
                .size(side)
                .child(sprite(Layer::Fills, fill).absolute())
                .child(sprite(Layer::Strokes, color).absolute()),
            _ => div()
                .flex_none()
                .size(side)
                .child(sprite(Layer::All, color)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_follows_the_rule() {
        assert!((stroke_width(15.0) - 1.2).abs() < 1e-6);
        assert!((stroke_width(20.0) - 1.4).abs() < 1e-6);
        assert_eq!(stroke_width(5.0), 1.0);
        assert_eq!(side(12.9), 15.0);
    }

    #[test]
    fn every_icon_has_a_glyph_and_its_svg_parses() {
        assert!(Icon::ALL.len() > 100);
        for icon in Icon::ALL {
            let glyph = icon.glyph();
            assert!(!glyph.strokes.is_empty() || !glyph.fills.is_empty());
            for layer in [Layer::All, Layer::Fills, Layer::Strokes] {
                let path = path(*icon, 12.9, layer);
                let bytes = Assets.load(&path).unwrap().unwrap();
                let text = std::str::from_utf8(&bytes).unwrap();
                assert!(
                    text.starts_with("<svg") && text.ends_with("</svg>"),
                    "{path}"
                );
            }
        }
    }

    #[test]
    fn fills_cut_their_strokes_out() {
        let svg = render(Icon::Selecting.glyph(), 100, Layer::All);
        assert!(svg.contains("mask=\"url(#cut)\""));
        assert!(svg.contains("stroke=\"#000\"/>"));
        assert!(!render(Icon::Play.glyph(), 100, Layer::All).contains("mask"));
    }
}
