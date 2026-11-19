//! A horizontal bar filled from the left to a fraction: a speed, a latency,
//! a share. Where the renderer draws shaders the GPU eases the fill to each
//! new fraction from where it stood, so a meter fed at a poll moves between
//! the values without a frame drawn for it. The fill is kept by the meter's
//! id from one render to the next, so callers hold no state of their own.

use std::time::Duration;

use gpui::prelude::*;
use gpui::*;

use super::{Animated, AnimatedQuad, Ease, SHADED, ShaderProgram};
use crate::theme::{KoanTheme, Look, Motion, Tone};

/// The bar's thickness, in points, unless the caller gives one.
const HEIGHT: f32 = 3.0;

/// The fill. Uniforms: track 0 the fraction, 4..8 the colour.
static METER: ShaderProgram = ShaderProgram {
    name: "koan-meter",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float w = clamp(koan_track(u, 0, time), 0.0, 1.0) * size.x;
    float cover = clamp(w - position.x + 0.5, 0.0, 1.0);
    return float4(u[4], u[5], u[6], u[7] * cover);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let w = clamp(koan_track(u, 0u, time), 0.0, 1.0) * size.x;
    let cover = clamp(w - position.x + 0.5, 0.0, 1.0);
    return vec4<f32>(u[1].xyz, u[1].w * cover);
}
"#,
    prepare: None,
};

/// The fill kept between renders: its track and the fraction it was last
/// sent to.
struct Fill {
    animated: Animated,
    at: f32,
}

/// A bar filled to `fraction`, from 0 to 1, in the accent over the rule's
/// colour, unless given others.
#[derive(IntoElement)]
pub struct Meter {
    id: ElementId,
    fraction: f32,
    color: Option<Hsla>,
    track: Option<Hsla>,
    height: Option<Pixels>,
    ease: Duration,
}

impl Meter {
    /// A fraction that is not finite is drawn as 0; one outside 0..1 is
    /// clamped.
    pub fn new(id: impl Into<ElementId>, fraction: f32) -> Self {
        Self {
            id: id.into(),
            fraction: if fraction.is_finite() {
                fraction.clamp(0.0, 1.0)
            } else {
                0.0
            },
            color: None,
            track: None,
            height: None,
            ease: Motion::SETTLE,
        }
    }

    /// The fill's colour.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// The colour of the bar behind the fill.
    pub fn track(mut self, track: Hsla) -> Self {
        self.track = Some(track);
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// How long the fill takes to reach a new fraction. A meter fed at a
    /// poll takes the poll's interval, so it moves without stopping.
    pub fn ease(mut self, ease: Duration) -> Self {
        self.ease = ease;
        self
    }
}

impl RenderOnce for Meter {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let color = self.color.unwrap_or_else(|| look.color(Tone::Accent));
        let bar = div()
            .w_full()
            .h(self.height.unwrap_or(px(HEIGHT)))
            .bg(self.track.unwrap_or_else(|| look.palette.rule.hsla()))
            .relative()
            .overflow_hidden();
        if !SHADED || !KoanTheme::motion_allowed(cx) {
            return bar.child(div().h_full().w(relative(self.fraction)).bg(color));
        }
        let fraction = self.fraction;
        let state = window.use_keyed_state(self.id, cx, |_, _| {
            let animated = Animated::new();
            animated.hold(0, fraction);
            Fill {
                animated,
                at: fraction,
            }
        });
        let animated = state.update(cx, |fill, _| {
            if fill.at != fraction {
                fill.animated.to(0, fraction, self.ease, Ease::QuickOut);
                fill.at = fraction;
            }
            fill.animated.clone()
        });
        animated.set_color(4, color);
        bar.child(
            AnimatedQuad::new(&METER, &animated)
                .decorative()
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
        )
    }
}
