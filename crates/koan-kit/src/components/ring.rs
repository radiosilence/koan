//! How far something has come, as a ring filling from twelve o'clock: a
//! download's bytes. With no size to measure against, seven-tenths of the
//! ring turn instead.
//!
//! Given a `Glide` (`ProgressRing::glide`) and a renderer with shaders, the
//! ring is a shader quad: the share at a moment and its rate, reckoned per
//! present, so it fills smoothly between the transfer's reports with no
//! frame drawn, and turns while the size is unknown. Elsewhere, as the
//! spinner is drawn: the ring's dots laid out once per size, close enough to
//! read as one line, and a frame choosing how many to draw from where. An
//! indeterminate ring asks for frames only while it is painted.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::*;

use super::{Glide, SHADED, ShaderProgram};
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
    glide: Option<Glide>,
}

/// The ring from twelve o'clock, from its `Glide`: track 0 the share, `u[0]`
/// 1 while the size is unknown (track 2 turning it), `u[1]` the share a
/// second past the last report, `u[8..12]` the colour. The stroke is the
/// ring's own share of its side, so a glide drawn at two sizes needs nothing
/// per size. Ahead of the last report it reckons at most a second on, so a
/// transfer that stalls is not drawn finishing.
static RING: ShaderProgram = ShaderProgram {
    name: "koan-progress-ring",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float side = min(size.x, size.y);
    float stroke = side * 0.125;
    float2 d = position - size * 0.5;
    float radius = (side - stroke) * 0.5;
    float edge = clamp(stroke * 0.5 - abs(length(d) - radius) + 0.5, 0.0, 1.0);
    if (edge <= 0.0) {
        return float4(0.0);
    }
    float a = atan2(d.x, -d.y) / 6.28318530718;
    if (a < 0.0) {
        a += 1.0;
    }
    float shown;
    if (u[0] > 0.5) {
        float start = a - koan_track(u, 2, time);
        if (start < 0.0) {
            start += 1.0;
        }
        shown = start < 0.7 ? 1.0 : 0.0;
    } else {
        float share = clamp(min(koan_track(u, 0, time), u[1]), 0.02, 1.0);
        shown = clamp((share - a) * side * 3.14159265, 0.0, 1.0);
    }
    return float4(u[8], u[9], u[10], u[11] * edge * shown);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let side = min(size.x, size.y);
    let stroke = side * 0.125;
    let d = position - size * 0.5;
    let radius = (side - stroke) * 0.5;
    let edge = clamp(stroke * 0.5 - abs(length(d) - radius) + 0.5, 0.0, 1.0);
    if (edge <= 0.0) {
        return vec4<f32>(0.0);
    }
    var a = atan2(d.x, -d.y) / 6.28318530718;
    if (a < 0.0) {
        a += 1.0;
    }
    var shown = 0.0;
    if (u[0].x > 0.5) {
        var start = a - koan_track(u, 2u, time);
        if (start < 0.0) {
            start += 1.0;
        }
        shown = select(0.0, 1.0, start < 0.7);
    } else {
        let share = clamp(min(koan_track(u, 0u, time), u[0].y), 0.02, 1.0);
        shown = clamp((share - a) * side * 3.14159265, 0.0, 1.0);
    }
    return vec4<f32>(u[2].xyz, u[2].w * edge * shown);
}
"#,
    prepare: None,
};

impl ProgressRing {
    /// `progress` from 0 to 1, or `None` while the size is unknown. Sized as
    /// a glyph beside `fine` type, as the availability marks are.
    pub fn new(progress: Option<f32>, color: Hsla) -> Self {
        Self {
            progress,
            color,
            role: TypeRole::Fine,
            glide: None,
        }
    }

    /// Filled from `glide`, the transfer's share at a moment and its rate,
    /// reckoned per present where the renderer draws shaders (`Glide::set`,
    /// or `set_unknown` while the size is unknown). Elsewhere the ring draws
    /// the glide's value as it was last set. The ring writes its colour into
    /// the glide, so a glide is drawn in one colour: one per colour, and
    /// never one a slider also draws.
    pub fn glide(mut self, glide: &Glide) -> Self {
        self.progress = glide.known().then(|| glide.value());
        self.glide = Some(glide.clone());
        self
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
        // Under Reduce Motion an unknown amount is the still arc drawn
        // below, the glide's turn left as it is for when motion returns; a
        // known share is information and still glides.
        let moving = crate::theme::KoanTheme::motion_allowed(cx);
        let still_unknown = self.glide.as_ref().is_some_and(|g| g.unknown()) && !moving;
        if let Some(glide) = self.glide.filter(|_| SHADED && !still_unknown) {
            let color = self.color;
            return canvas(
                move |_, window, cx| {
                    glide.0.set_color(8, color);
                    glide.0.attach(false, window, cx);
                    glide
                },
                |bounds, glide, window, _| glide.0.paint(&RING, None, bounds, window),
            )
            .size(side)
            .flex_none()
            .into_any_element();
        }
        let ring = dots(side);
        let progress = self.progress.filter(|_| !still_unknown);
        let color = self.color;
        // With Reduce Motion an unknown amount is a still arc.
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let elapsed = if moving {
                    START.get_or_init(std::time::Instant::now).elapsed()
                } else {
                    Duration::ZERO
                };
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
                if progress.is_none() && moving {
                    window.request_animation_frame();
                }
            },
        )
        .size(side)
        .flex_none()
        .into_any_element()
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

    /// Drawn without shaders, a transfer of unknown size turns; one known
    /// fills to its share.
    #[test]
    fn an_unknown_size_turns_rather_than_standing_empty() {
        let glide = Glide::new();
        glide.set_unknown();
        let ring = ProgressRing::new(Some(0.3), Hsla::default()).glide(&glide);
        assert_eq!(ring.progress, None);
        glide.set(0.4, 0.0, std::time::Instant::now());
        let ring = ProgressRing::new(None, Hsla::default()).glide(&glide);
        assert!(ring.progress.is_some_and(|p| (p - 0.4).abs() < 1e-3));
    }
}
