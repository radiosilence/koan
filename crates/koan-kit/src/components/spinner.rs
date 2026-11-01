//! Something on its way: the ensō drawing itself, the brush's head running
//! round the circle and then its tail following it off, looping. The same
//! mark as the `Placeholder`, so loading and empty read as one family.
//!
//! All on the GPU's side: the stroke is laid out once per size, as a run of
//! round quads close enough to read as one brush line, and a frame only
//! chooses which of them are drawn, from the head to the tail. No path is
//! built or tessellated and nothing is allocated per frame. It moves only
//! while it is painted (each frame asks for the next), so one scrolled away
//! or behind a page costs nothing. With Reduce Motion the whole mark stands
//! still.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::*;

use super::placeholder::TRACE;
use crate::theme::{KoanTheme, Look};

/// One loop: drawn, then undrawn.
const TURN: Duration = Duration::from_millis(1600);
/// The brush's width in the trace's 512-point space, the placeholder's.
const BRUSH: f32 = 48.0;
/// Quads per brush width along the stroke: enough that their overlap reads
/// as a line at any size.
const DENSITY: f32 = 4.0;

#[derive(IntoElement)]
pub struct Spinner {
    side: Pixels,
    /// The room what is loading will take, held so nothing moves when it
    /// arrives: the spinner is centred in it.
    reserve: Option<Size<Pixels>>,
}

impl Spinner {
    /// `side` is the square it is drawn in.
    pub fn new(side: Pixels) -> Self {
        Self {
            side,
            reserve: None,
        }
    }

    /// Holds exactly `room`, the size of what is loading, with the spinner
    /// in its middle.
    pub fn reserving(mut self, room: Size<Pixels>) -> Self {
        self.reserve = Some(room);
        self
    }
}

/// The stroke at one size: each quad's offset from the square's origin, in
/// order along the brush, and the quads' side.
struct Stroke {
    quads: Vec<Point<Pixels>>,
    brush: Pixels,
}

thread_local! {
    /// By side in points, a hundredth apart: a spinner comes in a few sizes,
    /// each laid out once.
    static STROKES: RefCell<HashMap<u32, Rc<Stroke>>> = RefCell::new(HashMap::new());
}

/// The trace's points, read once.
fn trace() -> &'static [(f32, f32)] {
    static POINTS: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    POINTS.get_or_init(|| {
        TRACE
            .split(['M', 'L'])
            .filter_map(|pair| {
                let mut xy = pair.split_whitespace().map(str::parse::<f32>);
                Some((xy.next()?.ok()?, xy.next()?.ok()?))
            })
            .collect()
    })
}

/// The quads along the trace, fitted to a square of `side`, the brush
/// inside it.
fn lay_out(side: f32) -> Stroke {
    let points = trace();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let extent = (max_x - min_x).max(max_y - min_y) + BRUSH;
    let scale = side / extent;
    let brush = BRUSH * scale;
    // Centred, the brush's half-width in from each edge.
    let off_x = (side - (max_x - min_x) * scale) / 2.0;
    let off_y = (side - (max_y - min_y) * scale) / 2.0;
    let at = |(x, y): (f32, f32)| ((x - min_x) * scale + off_x, (y - min_y) * scale + off_y);
    let step = brush / DENSITY;
    let mut quads = Vec::new();
    let mut carry = 0.0;
    for pair in points.windows(2) {
        let (a, b) = (at(pair[0]), at(pair[1]));
        let length = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let mut along = carry;
        while along < length {
            let t = along / length;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            quads.push(point(px(x - brush / 2.0), px(y - brush / 2.0)));
            along += step;
        }
        carry = along - length;
    }
    Stroke {
        quads,
        brush: px(brush),
    }
}

fn stroke(side: Pixels) -> Rc<Stroke> {
    let key = (f32::from(side) * 100.0).round() as u32;
    STROKES.with(|strokes| {
        strokes
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| Rc::new(lay_out(f32::from(side))))
            .clone()
    })
}

/// Smooth at both ends, so the head eases in and out of each half.
fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Which of `count` quads are drawn at `phase` of a loop: the head running
/// round over the first half, the tail following it off over the second.
fn shown(phase: f32, count: usize) -> std::ops::Range<usize> {
    let (tail, head) = if phase < 0.5 {
        (0.0, ease(phase * 2.0))
    } else {
        (ease((phase - 0.5) * 2.0), 1.0)
    };
    let n = count as f32;
    (tail * n) as usize..(head * n).ceil() as usize
}

/// When the first spinner was drawn: every spinner turns in step.
static START: OnceLock<Instant> = OnceLock::new();

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let color = look.palette.muted.hsla();
        let moving = KoanTheme::motion_allowed(cx);
        let stroke = stroke(self.side);
        let mark = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let range = if moving {
                    let start = *START.get_or_init(Instant::now);
                    let phase = (start.elapsed().as_secs_f32() / TURN.as_secs_f32()).fract();
                    shown(phase, stroke.quads.len())
                } else {
                    0..stroke.quads.len()
                };
                let size = size(stroke.brush, stroke.brush);
                let round = stroke.brush / 2.0;
                let end = range.end.min(stroke.quads.len());
                for offset in &stroke.quads[range.start.min(end)..end] {
                    window.paint_quad(
                        fill(Bounds::new(bounds.origin + *offset, size), color).corner_radii(round),
                    );
                }
                if moving {
                    window.request_animation_frame();
                }
            },
        )
        .size(self.side)
        .flex_none();
        match self.reserve {
            Some(room) => div()
                .w(room.width)
                .h(room.height)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(mark)
                .into_any_element(),
            None => mark.into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn the_stroke_is_laid_out_inside_its_square() {
        let stroke = lay_out(24.0);
        assert!(stroke.quads.len() > 50);
        let brush = f32::from(stroke.brush);
        for q in &stroke.quads {
            let (x, y) = (f32::from(q.x), f32::from(q.y));
            assert!(x >= -0.01 && y >= -0.01, "{x},{y}");
            assert!(x + brush <= 24.01 && y + brush <= 24.01, "{x},{y}");
        }
    }

    #[test]
    fn a_loop_draws_the_mark_then_undraws_it() {
        assert_eq!(shown(0.0, 100), 0..0);
        assert_eq!(shown(0.5, 100), 0..100);
        assert!(shown(0.999, 100).len() <= 1);
        assert!(shown(0.25, 100).end > 20 && shown(0.25, 100).end < 80);
    }
}
