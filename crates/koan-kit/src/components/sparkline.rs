//! A line through a series of values, small: an EQ curve's thumbnail on its
//! stage's block.

use gpui_kit::prelude::*;
use gpui_kit::*;

/// `values` from `-range` to `range`, left to right across the line's box,
/// zero at the middle. Drawn at two points a pixel at most, which is all a
/// line this small can show. Nothing is drawn for fewer than two values.
#[derive(IntoElement)]
pub struct Sparkline {
    values: Vec<f64>,
    range: f64,
    color: Hsla,
    dash: Vec<f32>,
    size: Size<Pixels>,
}

impl Sparkline {
    /// The stroke's width, in pixels.
    const STROKE: f32 = 1.5;

    pub fn new(values: Vec<f64>, range: f64, color: Hsla) -> Self {
        Self {
            values,
            range: range.max(f64::EPSILON),
            color,
            dash: Vec::new(),
            size: size(px(64.0), px(26.0)),
        }
    }

    /// Dashed: on and off lengths, in pixels.
    pub fn dash(mut self, dash: Vec<f32>) -> Self {
        self.dash = dash;
        self
    }

    /// The line's box, 64 × 26 by default.
    pub fn size(mut self, size: Size<Pixels>) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for Sparkline {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Self {
            values,
            range,
            color,
            dash,
            size,
        } = self;
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                if values.len() < 2 {
                    return;
                }
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let mut b = PathBuilder::stroke(px(Self::STROKE));
                if !dash.is_empty() {
                    let dash: Vec<_> = dash.iter().map(|d| px(*d)).collect();
                    b = b.dash_array(&dash);
                }
                let n = values.len();
                let every = n.div_ceil((2.0 * w) as usize).max(1);
                for (i, v) in values
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % every == 0 || *i == n - 1)
                {
                    let at = bounds.origin
                        + point(
                            px(w * i as f32 / (n - 1) as f32),
                            px(h / 2.0 - h / 2.0 * (*v / range) as f32),
                        );
                    if i == 0 {
                        b.move_to(at);
                    } else {
                        b.line_to(at);
                    }
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .w(size.width)
        .h(size.height)
        .flex_none()
    }
}
