//! A control answering what was done to it: a small mark drawn by the GPU
//! around or under the control for a moment, then gone. The kit's controls
//! call it from their own handlers (a button pressed, a toggle turned, a
//! heart filled, something copied), so every page that uses them answers
//! alike with no code of its own.
//!
//! Subtle by rule: one shader quad over the control and a little past its
//! edge, in the accent at low opacity, never over the control's text, for
//! no longer than `Motion::SETTLE`. The clock runs only while an answer is
//! drawn and the quad leaves the scene when it ends, so a control at rest,
//! or off screen, costs nothing. Under Reduce Motion nothing is drawn.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Animated, AnimatedQuad, SHADED, child};
use crate::theme::{KoanTheme, Motion};

/// What an answer draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// One ring leaving the control's outline: a button pressed, a toggle
    /// turned on.
    Ring,
    /// Six dots leaving the control and fading: a heart filled.
    Burst,
    /// One ring closing in on the control: a heart emptied.
    Close,
    /// A line drawn under the control from its leading edge: something
    /// copied, something arrived at a destination.
    Sweep,
    /// The control's ground lifting to the accent and settling back: a
    /// bordered button pressed, rows landing.
    Lift,
}

impl Answer {
    fn code(self) -> f32 {
        match self {
            Answer::Ring => 0.0,
            Answer::Burst => 1.0,
            Answer::Close => 2.0,
            Answer::Sweep => 3.0,
            Answer::Lift => 4.0,
        }
    }

    fn duration(self) -> Duration {
        match self {
            Answer::Ring | Answer::Close | Answer::Sweep => Motion::SETTLE,
            Answer::Burst | Answer::Lift => Motion::SETTLE,
        }
    }

    /// How far past the control's edge the answer reaches, in points.
    fn spill(self) -> f32 {
        match self {
            Answer::Ring | Answer::Close => 6.0,
            Answer::Burst => 9.0,
            Answer::Sweep | Answer::Lift => 0.0,
        }
    }
}

/// The answers as one program. Uniforms: 0 the answer, 1 its length in
/// seconds, 2..6 the colour, 6 the spill and 7 a point, in device pixels.
pub static ANSWER: ShaderProgram = ShaderProgram {
    name: "koan-answer",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = clamp(time / max(u[1], 0.001), 0.0, 1.0);
    if (t >= 1.0) { return float4(0.0); }
    float e = 1.0 - (1.0 - t) * (1.0 - t);
    float spill = u[6];
    float pt = u[7];
    float2 inner = size - 2.0 * spill;
    float2 c = size * 0.5;
    float half_side = 0.5 * min(inner.x, inner.y);
    float d = length(position - c);
    float a = 0.0;
    int kind = int(u[0] + 0.5);
    if (kind == 0) {
        float r = half_side + spill * 0.9 * e;
        a = 0.35 * (1.0 - t) * clamp(0.75 * pt - abs(d - r) + 0.5, 0.0, 1.0);
    } else if (kind == 1) {
        float r = mix(half_side * 0.9, half_side + spill * 0.9, e);
        float dot = 1.2 * pt * (1.0 - 0.5 * t);
        for (int i = 0; i < 6; i++) {
            float ang = 0.5236 + float(i) * 1.0472;
            float2 p = c + r * float2(cos(ang), sin(ang));
            a = max(a, clamp(dot - length(position - p) + 0.5, 0.0, 1.0));
        }
        a *= 0.6 * (1.0 - t);
    } else if (kind == 2) {
        float r = mix(half_side + spill * 0.9, half_side * 0.8, e);
        a = 0.3 * (1.0 - t) * clamp(0.75 * pt - abs(d - r) + 0.5, 0.0, 1.0);
    } else if (kind == 3) {
        float line = size.y - pt;
        bool under = position.y >= line && position.x <= size.x * e;
        a = under ? 0.6 * (1.0 - t * t) : 0.0;
    } else {
        a = 0.12 * (1.0 - e);
    }
    return float4(u[2], u[3], u[4], u[5] * a);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>) -> vec4<f32> {
    let t = clamp(time / max(u[0].y, 0.001), 0.0, 1.0);
    if (t >= 1.0) { return vec4<f32>(0.0); }
    let e = 1.0 - (1.0 - t) * (1.0 - t);
    let spill = u[1].z;
    let pt = u[1].w;
    let inner = size - 2.0 * spill;
    let c = size * 0.5;
    let half_side = 0.5 * min(inner.x, inner.y);
    let d = length(position - c);
    var a = 0.0;
    let kind = i32(u[0].x + 0.5);
    if (kind == 0) {
        let r = half_side + spill * 0.9 * e;
        a = 0.35 * (1.0 - t) * clamp(0.75 * pt - abs(d - r) + 0.5, 0.0, 1.0);
    } else if (kind == 1) {
        let r = mix(half_side * 0.9, half_side + spill * 0.9, e);
        let dot_r = 1.2 * pt * (1.0 - 0.5 * t);
        for (var i = 0; i < 6; i++) {
            let ang = 0.5236 + f32(i) * 1.0472;
            let p = c + r * vec2<f32>(cos(ang), sin(ang));
            a = max(a, clamp(dot_r - length(position - p) + 0.5, 0.0, 1.0));
        }
        a = a * 0.6 * (1.0 - t);
    } else if (kind == 2) {
        let r = mix(half_side + spill * 0.9, half_side * 0.8, e);
        a = 0.3 * (1.0 - t) * clamp(0.75 * pt - abs(d - r) + 0.5, 0.0, 1.0);
    } else if (kind == 3) {
        let line = size.y - pt;
        if (position.y >= line && position.x <= size.x * e) {
            a = 0.6 * (1.0 - t * t);
        }
    } else {
        a = 0.12 * (1.0 - e);
    }
    let rgba = vec4<f32>(u[0].z, u[0].w, u[1].x, u[1].y);
    return vec4<f32>(rgba.xyz, rgba.w * a);
}
"#,
    prepare: None,
};

/// A control's answer while one is drawn.
#[derive(Default)]
struct State {
    animated: Animated,
    live: Option<Answer>,
    _end: Option<Task<()>>,
}

/// A control's answers, taken as the control is drawn and moved into its
/// handlers: keyed state found again from a handler would not be the
/// control's own.
#[derive(Clone)]
pub struct Answerer(Entity<State>);

impl Answerer {
    /// The answers of the control `id`.
    pub fn of(id: &ElementId, window: &mut Window, cx: &mut App) -> Self {
        Self(window.use_keyed_state(child(id, "answer"), cx, |_, _| State::default()))
    }

    /// Starts `answer` in `color`, from the control's own handler. Nothing
    /// happens under Reduce Motion or where the renderer draws no shaders.
    pub fn start(&self, answer: Answer, color: Hsla, cx: &mut App) {
        if !SHADED || !KoanTheme::motion_allowed(cx) {
            return;
        }
        let rgba = color.to_rgb();
        self.0.update(cx, |s, cx| {
            s.animated
                .set_all(0, &[answer.code(), answer.duration().as_secs_f32()]);
            s.animated.set_all(2, &[rgba.r, rgba.g, rgba.b, rgba.a]);
            s.animated.set_origin(Instant::now());
            s.animated.run(true);
            s.live = Some(answer);
            s._end = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(answer.duration()).await;
                this.update(cx, |s, cx| {
                    s.animated.run(false);
                    s.live = None;
                    cx.notify();
                })
                .ok();
            }));
            cx.notify();
        });
    }

    /// The answer being drawn, as a quad over the control and a little past
    /// it, for the control to add as its last child; nothing at rest. The
    /// control is `relative`.
    pub fn layer(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let s = self.0.read(cx);
        let answer = s.live?;
        let look = crate::theme::Look::of(window, cx);
        let spill = look.spaced(answer.spill());
        let scale = window.scale_factor();
        s.animated
            .set_all(6, &[f32::from(spill) * scale, scale * look.scale.space]);
        Some(
            AnimatedQuad::new(&ANSWER, &s.animated)
                .absolute()
                .top(-spill)
                .left(-spill)
                .right(-spill)
                .bottom(-spill)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn every_answer_is_within_the_motion_tokens() {
        for answer in [
            Answer::Ring,
            Answer::Burst,
            Answer::Close,
            Answer::Sweep,
            Answer::Lift,
        ] {
            assert!(answer.duration() <= Motion::SETTLE);
        }
    }
}
