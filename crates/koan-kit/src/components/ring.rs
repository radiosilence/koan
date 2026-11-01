//! How far something has come, as a ring filling from twelve o'clock: a
//! download's bytes. With no size to measure against, seven-tenths of the
//! ring turn instead.
//!
//! As the spinner is drawn, on the GPU's side: the ring's dots are laid out
//! once per size, close enough to read as one line, and a frame chooses how
//! many to draw from where. Nothing is built or allocated per frame. An
//! indeterminate ring asks for frames only while it is painted.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::*;

use crate::theme::{Look, TypeRole};

/// One turn of an indeterminate ring.
const TURN: Duration = Duration::from_millis(1000);
/// The ring's stroke, as a share of its side.
const STROKE: f32 = 0.125;

#[derive(IntoElement)]
pub struct ProgressRing {
    progress: Option<f32>,
    color: Hsla,
    role: TypeRole,
}

impl ProgressRing {
    /// `progress` from 0 to 1, or `None` while the size is unknown. Sized as
    /// a glyph beside `fine` type, as the availability marks are.
    pub fn new(progress: Option<f32>, color: Hsla) -> Self {
        Self {
            progress,
            color,
            role: TypeRole::Fine,
        }
    }

    /// Sized as a glyph beside `role` type instead.
    pub fn role(mut self, role: TypeRole) -> Self {
        self.role = role;
        self
    }
}

struct Dots {
    at: Vec<Point<Pixels>>,
    dot: Pixels,
}

thread_local! {
    static RINGS: RefCell<HashMap<u32, Rc<Dots>>> = RefCell::new(HashMap::new());
}

fn dots(side: Pixels) -> Rc<Dots> {
    let key = (f32::from(side) * 100.0).round() as u32;
    RINGS.with(|rings| {
        rings
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let side = f32::from(side);
                let dot = side * STROKE;
                let radius = (side - dot) / 2.0;
                let count = ((TAU * radius) / (dot / 4.0)).ceil().max(8.0) as usize;
                let at = (0..count)
                    .map(|i| {
                        let angle = TAU * i as f32 / count as f32 - FRAC_PI_2;
                        point(
                            px(side / 2.0 + radius * angle.cos() - dot / 2.0),
                            px(side / 2.0 + radius * angle.sin() - dot / 2.0),
                        )
                    })
                    .collect();
                Rc::new(Dots { at, dot: px(dot) })
            })
            .clone()
    })
}

/// Which dots are drawn, as a start and a count, going round from the start.
fn drawn(progress: Option<f32>, elapsed: Duration, count: usize) -> (usize, usize) {
    match progress {
        Some(fraction) => (
            0,
            (fraction.clamp(0.02, 1.0) * count as f32).ceil() as usize,
        ),
        None => {
            let phase = (elapsed.as_secs_f32() / TURN.as_secs_f32()).fract();
            (
                (phase * count as f32) as usize,
                (0.7 * count as f32) as usize,
            )
        }
    }
}

static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

impl RenderOnce for ProgressRing {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let side = px(crate::theme::icons::side(look.points(self.role)));
        let ring = dots(side);
        let (progress, color) = (self.progress, self.color);
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let elapsed = START.get_or_init(std::time::Instant::now).elapsed();
                let count = ring.at.len();
                let (start, length) = drawn(progress, elapsed, count);
                let size = size(ring.dot, ring.dot);
                for i in 0..length.min(count) {
                    let at = ring.at[(start + i) % count];
                    window.paint_quad(
                        fill(Bounds::new(bounds.origin + at, size), color)
                            .corner_radii(ring.dot / 2.0),
                    );
                }
                if progress.is_none() {
                    window.request_animation_frame();
                }
            },
        )
        .size(side)
        .flex_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn a_ring_fills_with_its_progress() {
        assert_eq!(drawn(Some(0.5), Duration::ZERO, 100), (0, 50));
        assert_eq!(drawn(Some(0.0), Duration::ZERO, 100).1, 2);
        assert_eq!(drawn(None, Duration::from_millis(500), 100), (50, 70));
    }
}
