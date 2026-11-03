//! A control answering what was done to it: a small mark drawn by the GPU
//! around or under the control for a moment, then gone. The kit's controls
//! call it from their own handlers (a button pressed, a toggle turned, a
//! heart filled, something copied), so every page that uses them answers
//! alike with no code of its own.
//!
//! Subtle by rule: one shader quad over the control and a little past its
//! edge, in the accent at low opacity, never over the control's text, for
//! no longer than `Motion::SETTLE`. Its progress is one track the GPU moves,
//! and the quad leaves the scene when it ends, so a control at rest, or off
//! screen, costs nothing. Under Reduce Motion nothing is drawn.
//!
//! The showpieces answer through the glyph itself (`GLYPH`): a heart's fill
//! popping in with particles or deflating, play folding into pause, shuffle's
//! arrows crossing, repeat spinning round, a skip nudged its way. The glyph
//! is drawn by the program from the icon's own strokes on the 24-point grid
//! while the answer runs, in place of the icon, which comes back as it ends.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::*;

use super::{Animated, AnimatedQuad, Ease, Repeat, SHADED, child};
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
    /// A short streak leaving the glyph the way it points: a skip, forward
    /// (`true`) or back.
    Streak(bool),
    /// A heart filling: the fill pops in past its size and settles, small
    /// particles thrown out round it.
    Fill,
    /// A heart emptying: the fill deflates, squashing, into the outline.
    Empty,
    /// The glyph turning into another (`Morph`).
    Morph(Morph),
    /// A skip's glyph nudged the way it points and back: forward (`true`)
    /// or back.
    Nudge(bool),
}

/// How a glyph turns into another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Morph {
    /// Play's triangle folding into pause's bars: its left side becomes
    /// the left bar, its slanted sides the right.
    ToPause,
    ToPlay,
    /// Shuffle turned on: the arrows swing apart and cross again.
    ShuffleOn,
    /// Shuffle turned off: the arrows sway, less.
    ShuffleOff,
    /// Repeat spinning round a turn.
    Spin,
    /// Repeat spinning round, its "one" popping in.
    SpinToOne,
    /// Repeat spinning round, its "one" shrinking away.
    SpinFromOne,
}

impl Answer {
    fn code(self) -> f32 {
        match self {
            Answer::Ring => 0.0,
            Answer::Burst => 1.0,
            Answer::Close => 2.0,
            Answer::Sweep => 3.0,
            Answer::Lift => 4.0,
            Answer::Streak(_) => 5.0,
            Answer::Fill => 10.0,
            Answer::Empty => 11.0,
            Answer::Morph(Morph::ToPause) => 12.0,
            Answer::Morph(Morph::ToPlay) => 13.0,
            Answer::Morph(Morph::ShuffleOn) => 14.0,
            Answer::Morph(Morph::ShuffleOff) => 15.0,
            Answer::Morph(Morph::Spin) => 16.0,
            Answer::Morph(Morph::SpinToOne) => 17.0,
            Answer::Morph(Morph::SpinFromOne) => 18.0,
            Answer::Nudge(_) => 19.0,
        }
    }

    /// Drawn through the glyph rather than around it: the control draws it
    /// with `Answerer::glyph_layer`, its icon hidden meanwhile.
    pub fn glyph(self) -> bool {
        matches!(
            self,
            Answer::Fill | Answer::Empty | Answer::Morph(_) | Answer::Nudge(_)
        )
    }

    /// Which way it travels, for a streak: 1 forward, -1 back.
    fn heading(self) -> f32 {
        match self {
            Answer::Streak(false) | Answer::Nudge(false) => -1.0,
            _ => 1.0,
        }
    }

    /// How long it runs: a ring, out or in, is a press's echo and as quick
    /// as a sleeve arriving; what travels (dots, a line drawn the width of
    /// a row) or lifts a ground takes the longest token.
    fn duration(self) -> Duration {
        match self {
            Answer::Ring | Answer::Close | Answer::Streak(_) => Motion::ARRIVE,
            Answer::Burst | Answer::Sweep | Answer::Lift => Motion::SETTLE,
            Answer::Fill | Answer::Empty | Answer::Morph(_) | Answer::Nudge(_) => Motion::SETTLE,
        }
    }

    /// How what it draws travels: out from the control decisively and
    /// settling (quick-out); a ring closing in eases both ways, as a
    /// gathering rather than a throw.
    fn ease(self) -> Ease {
        match self {
            Answer::Close | Answer::Morph(_) => Ease::InOut,
            _ => Ease::QuickOut,
        }
    }

    /// How far past the control's edge the answer reaches, in points.
    fn spill(self) -> f32 {
        match self {
            Answer::Ring | Answer::Close => 6.0,
            Answer::Burst | Answer::Streak(_) => 9.0,
            Answer::Sweep | Answer::Lift => 0.0,
            Answer::Fill => 7.0,
            Answer::Empty | Answer::Morph(_) => 2.0,
            Answer::Nudge(_) => 4.0,
        }
    }
}

/// The answers as one program. Uniforms: 0 the answer, 1 which way a streak
/// goes, 2..6 the colour, 6
/// the spill and 7 a point, in device pixels; track 0 the answer's time
/// from 0 to 1, which its fade follows, and track 1 its travel, on the
/// answer's own curve.
pub static ANSWER: ShaderProgram = ShaderProgram {
    name: "koan-answer",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = clamp(koan_track(u, 0, time), 0.0, 1.0);
    if (t >= 1.0) { return float4(0.0); }
    float e = clamp(koan_track(u, 1, time), 0.0, 1.0);
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
    } else if (kind == 5) {
        float x = (position.x - c.x) * u[1];
        float half_w = 0.5 * inner.x;
        float head = half_w + spill * e;
        float tail = mix(half_w, head, t);
        float across = clamp(0.75 * pt - abs(position.y - c.y) + 0.5, 0.0, 1.0);
        a = (x >= tail && x <= head) ? 0.45 * (1.0 - t) * across : 0.0;
    } else {
        a = 0.12 * (1.0 - e);
    }
    return float4(u[2], u[3], u[4], u[5] * a);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = clamp(koan_track(u, 0u, time), 0.0, 1.0);
    if (t >= 1.0) { return vec4<f32>(0.0); }
    let e = clamp(koan_track(u, 1u, time), 0.0, 1.0);
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
    } else if (kind == 5) {
        let x = (position.x - c.x) * u[0].y;
        let half_w = 0.5 * inner.x;
        let head = half_w + spill * e;
        let tail = mix(half_w, head, t);
        let across = clamp(0.75 * pt - abs(position.y - c.y) + 0.5, 0.0, 1.0);
        if (x >= tail && x <= head) {
            a = 0.45 * (1.0 - t) * across;
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

/// The glyph answers as one program, drawing the icons' strokes on their
/// 24-point grid (`apps/macos/Resources/Icons`). Uniforms: 0 the answer, 1
/// which way a nudge goes, 2..6 the glyph's colour, 6 the spill, 8..12 the
/// accent (a heart's fill, its particles), 12 the glyph's side, all in
/// device pixels; track 0 the answer's time from 0 to 1, track 1 the same on
/// its curve.
pub static GLYPH: ShaderProgram = ShaderProgram {
    name: "koan-glyph",
    msl: r#"
float kg_seg(float2 p, float2 a, float2 b) {
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}
float kg_line(float2 p, float2 a, float2 b, float k) {
    return clamp((0.75 - kg_seg(p, a, b)) * k + 0.5, 0.0, 1.0);
}
float kg_heart(float2 g) {
    float d1 = length(g - float2(9.0, 10.0)) - 4.243;
    float d2 = length(g - float2(15.0, 10.0)) - 4.243;
    float d3 = (abs(g.x - 12.0) + abs(g.y - 13.0) - 6.0) * 0.7071;
    return min(min(d1, d2), d3);
}
float kg_back(float t) {
    float s = t - 1.0;
    return 1.0 + 2.70158 * s * s * s + 1.70158 * s * s;
}
float2 kg_turn(float2 g, float a) {
    float2 v = g - float2(12.0, 12.0);
    float cs = cos(a);
    float sn = sin(a);
    return float2(12.0, 12.0) + float2(cs * v.x - sn * v.y, sn * v.x + cs * v.y);
}
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = clamp(koan_track(u, 0, time), 0.0, 1.0);
    if (t >= 1.0) { return float4(0.0); }
    float e = clamp(koan_track(u, 1, time), 0.0, 1.0);
    float k = max(u[12], 1.0) / 24.0;
    float2 g = (position - u[6]) / k;
    float pi = 3.14159265;
    float line = 0.0;
    float fill = 0.0;
    int kind = int(u[0] + 0.5);
    if (kind == 10 || kind == 11) {
        float2 c = float2(12.0, 13.0);
        line = clamp((0.75 - abs(kg_heart(g))) * k + 0.5, 0.0, 1.0);
        float2 s2;
        if (kind == 10) {
            float s = max(kg_back(t), 0.001);
            s2 = float2(s, s);
            for (int i = 0; i < 7; i++) {
                float ang = -1.5708 + float(i) * 0.8976;
                float2 p = float2(12.0, 12.5) + (7.5 + 5.5 * e) * float2(cos(ang), sin(ang));
                float dot_r = 1.3 * (1.0 - t);
                fill = max(fill, clamp((dot_r - length(g - p)) * k + 0.5, 0.0, 1.0) * (1.0 - t));
            }
        } else {
            float q = sin(pi * t);
            float s = max(1.0 - e, 0.001);
            s2 = float2(s * (1.0 + 0.25 * q), s * (1.0 - 0.2 * q));
        }
        float d = kg_heart(c + (g - c) / s2) * min(s2.x, s2.y);
        fill = max(fill, clamp(-d * k + 0.5, 0.0, 1.0));
    } else if (kind == 12 || kind == 13) {
        float m = kind == 12 ? e : 1.0 - e;
        line = max(line, kg_line(g, mix(float2(7.0, 5.0), float2(8.5, 5.0), m), mix(float2(7.0, 19.0), float2(8.5, 19.0), m), k));
        line = max(line, kg_line(g, mix(float2(7.0, 5.0), float2(15.5, 5.0), m), mix(float2(19.0, 12.0), float2(15.5, 12.0), m), k));
        line = max(line, kg_line(g, mix(float2(19.0, 12.0), float2(15.5, 12.0), m), mix(float2(7.0, 19.0), float2(15.5, 19.0), m), k));
    } else if (kind == 14 || kind == 15) {
        float m = 1.0 - (kind == 14 ? 1.0 : 0.4) * sin(pi * e);
        float2 a3 = mix(float2(16.0, 7.0), float2(16.0, 17.0), m);
        float2 a4 = mix(float2(20.5, 7.0), float2(20.5, 17.0), m);
        float2 b3 = mix(float2(16.0, 17.0), float2(16.0, 7.0), m);
        float2 b4 = mix(float2(20.5, 17.0), float2(20.5, 7.0), m);
        line = max(line, kg_line(g, float2(3.0, 7.0), float2(8.0, 7.0), k));
        line = max(line, kg_line(g, float2(8.0, 7.0), a3, k));
        line = max(line, kg_line(g, a3, a4, k));
        line = max(line, kg_line(g, float2(3.0, 17.0), float2(8.0, 17.0), k));
        line = max(line, kg_line(g, float2(8.0, 17.0), b3, k));
        line = max(line, kg_line(g, b3, b4, k));
        line = max(line, kg_line(g, float2(18.0, 4.5), float2(20.5, 7.0), k));
        line = max(line, kg_line(g, float2(20.5, 7.0), float2(18.0, 9.5), k));
        line = max(line, kg_line(g, float2(18.0, 14.5), float2(20.5, 17.0), k));
        line = max(line, kg_line(g, float2(20.5, 17.0), float2(18.0, 19.5), k));
    } else if (kind >= 16 && kind <= 18) {
        float2 r = kg_turn(g, -2.0 * pi * e);
        line = max(line, kg_line(r, float2(4.0, 11.0), float2(4.0, 7.0), k));
        line = max(line, kg_line(r, float2(4.0, 7.0), float2(19.0, 7.0), k));
        line = max(line, kg_line(r, float2(16.5, 4.5), float2(19.0, 7.0), k));
        line = max(line, kg_line(r, float2(19.0, 7.0), float2(16.5, 9.5), k));
        line = max(line, kg_line(r, float2(20.0, 13.0), float2(20.0, 17.0), k));
        line = max(line, kg_line(r, float2(20.0, 17.0), float2(5.0, 17.0), k));
        line = max(line, kg_line(r, float2(7.5, 14.5), float2(5.0, 17.0), k));
        line = max(line, kg_line(r, float2(5.0, 17.0), float2(7.5, 19.5), k));
        float sb = kind == 17 ? kg_back(t) : (kind == 18 ? 1.0 - e : 0.0);
        if (sb > 0.01) {
            float2 b = float2(12.0, 12.0) + (g - float2(12.0, 12.0)) / sb;
            float d = min(kg_seg(b, float2(11.0, 10.5), float2(12.5, 10.0)), kg_seg(b, float2(12.5, 10.0), float2(12.5, 14.0))) * sb;
            line = max(line, clamp((0.75 - d) * k + 0.5, 0.0, 1.0));
        }
    } else if (kind == 19) {
        float x = g.x - 2.2 * sin(pi * t) * u[1];
        float2 n = float2(u[1] < 0.0 ? 24.0 - x : x, g.y);
        line = max(line, kg_line(n, float2(5.0, 6.0), float2(15.0, 12.0), k));
        line = max(line, kg_line(n, float2(15.0, 12.0), float2(5.0, 18.0), k));
        line = max(line, kg_line(n, float2(5.0, 18.0), float2(5.0, 6.0), k));
        line = max(line, kg_line(n, float2(19.0, 6.0), float2(19.0, 18.0), k));
    }
    float la = line * u[5];
    float fa = fill * u[11];
    float a = max(la, fa);
    if (a <= 0.0) { return float4(0.0); }
    float3 rgb = mix(float3(u[8], u[9], u[10]), float3(u[2], u[3], u[4]), la / (la + fa));
    return float4(rgb, a);
}
"#,
    wgsl: r#"
fn kg_seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}
fn kg_line(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, k: f32) -> f32 {
    return clamp((0.75 - kg_seg(p, a, b)) * k + 0.5, 0.0, 1.0);
}
fn kg_heart(g: vec2<f32>) -> f32 {
    let d1 = length(g - vec2<f32>(9.0, 10.0)) - 4.243;
    let d2 = length(g - vec2<f32>(15.0, 10.0)) - 4.243;
    let d3 = (abs(g.x - 12.0) + abs(g.y - 13.0) - 6.0) * 0.7071;
    return min(min(d1, d2), d3);
}
fn kg_back(t: f32) -> f32 {
    let s = t - 1.0;
    return 1.0 + 2.70158 * s * s * s + 1.70158 * s * s;
}
fn kg_turn(g: vec2<f32>, a: f32) -> vec2<f32> {
    let v = g - vec2<f32>(12.0, 12.0);
    let cs = cos(a);
    let sn = sin(a);
    return vec2<f32>(12.0, 12.0) + vec2<f32>(cs * v.x - sn * v.y, sn * v.x + cs * v.y);
}
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = clamp(koan_track(u, 0u, time), 0.0, 1.0);
    if (t >= 1.0) { return vec4<f32>(0.0); }
    let e = clamp(koan_track(u, 1u, time), 0.0, 1.0);
    let k = max(u[3].x, 1.0) / 24.0;
    let g = (position - u[1].z) / k;
    let pi = 3.14159265;
    let heading = u[0].y;
    var line = 0.0;
    var fill = 0.0;
    let kind = i32(u[0].x + 0.5);
    if (kind == 10 || kind == 11) {
        let c = vec2<f32>(12.0, 13.0);
        line = clamp((0.75 - abs(kg_heart(g))) * k + 0.5, 0.0, 1.0);
        var s2 = vec2<f32>(1.0, 1.0);
        if (kind == 10) {
            let s = max(kg_back(t), 0.001);
            s2 = vec2<f32>(s, s);
            for (var i = 0; i < 7; i++) {
                let ang = -1.5708 + f32(i) * 0.8976;
                let p = vec2<f32>(12.0, 12.5) + (7.5 + 5.5 * e) * vec2<f32>(cos(ang), sin(ang));
                let dot_r = 1.3 * (1.0 - t);
                fill = max(fill, clamp((dot_r - length(g - p)) * k + 0.5, 0.0, 1.0) * (1.0 - t));
            }
        } else {
            let q = sin(pi * t);
            let s = max(1.0 - e, 0.001);
            s2 = vec2<f32>(s * (1.0 + 0.25 * q), s * (1.0 - 0.2 * q));
        }
        let d = kg_heart(c + (g - c) / s2) * min(s2.x, s2.y);
        fill = max(fill, clamp(-d * k + 0.5, 0.0, 1.0));
    } else if (kind == 12 || kind == 13) {
        var m = e;
        if (kind == 13) { m = 1.0 - e; }
        line = max(line, kg_line(g, mix(vec2<f32>(7.0, 5.0), vec2<f32>(8.5, 5.0), m), mix(vec2<f32>(7.0, 19.0), vec2<f32>(8.5, 19.0), m), k));
        line = max(line, kg_line(g, mix(vec2<f32>(7.0, 5.0), vec2<f32>(15.5, 5.0), m), mix(vec2<f32>(19.0, 12.0), vec2<f32>(15.5, 12.0), m), k));
        line = max(line, kg_line(g, mix(vec2<f32>(19.0, 12.0), vec2<f32>(15.5, 12.0), m), mix(vec2<f32>(7.0, 19.0), vec2<f32>(15.5, 19.0), m), k));
    } else if (kind == 14 || kind == 15) {
        var reach = 0.4;
        if (kind == 14) { reach = 1.0; }
        let m = 1.0 - reach * sin(pi * e);
        let a3 = mix(vec2<f32>(16.0, 7.0), vec2<f32>(16.0, 17.0), m);
        let a4 = mix(vec2<f32>(20.5, 7.0), vec2<f32>(20.5, 17.0), m);
        let b3 = mix(vec2<f32>(16.0, 17.0), vec2<f32>(16.0, 7.0), m);
        let b4 = mix(vec2<f32>(20.5, 17.0), vec2<f32>(20.5, 7.0), m);
        line = max(line, kg_line(g, vec2<f32>(3.0, 7.0), vec2<f32>(8.0, 7.0), k));
        line = max(line, kg_line(g, vec2<f32>(8.0, 7.0), a3, k));
        line = max(line, kg_line(g, a3, a4, k));
        line = max(line, kg_line(g, vec2<f32>(3.0, 17.0), vec2<f32>(8.0, 17.0), k));
        line = max(line, kg_line(g, vec2<f32>(8.0, 17.0), b3, k));
        line = max(line, kg_line(g, b3, b4, k));
        line = max(line, kg_line(g, vec2<f32>(18.0, 4.5), vec2<f32>(20.5, 7.0), k));
        line = max(line, kg_line(g, vec2<f32>(20.5, 7.0), vec2<f32>(18.0, 9.5), k));
        line = max(line, kg_line(g, vec2<f32>(18.0, 14.5), vec2<f32>(20.5, 17.0), k));
        line = max(line, kg_line(g, vec2<f32>(20.5, 17.0), vec2<f32>(18.0, 19.5), k));
    } else if (kind >= 16 && kind <= 18) {
        let r = kg_turn(g, -2.0 * pi * e);
        line = max(line, kg_line(r, vec2<f32>(4.0, 11.0), vec2<f32>(4.0, 7.0), k));
        line = max(line, kg_line(r, vec2<f32>(4.0, 7.0), vec2<f32>(19.0, 7.0), k));
        line = max(line, kg_line(r, vec2<f32>(16.5, 4.5), vec2<f32>(19.0, 7.0), k));
        line = max(line, kg_line(r, vec2<f32>(19.0, 7.0), vec2<f32>(16.5, 9.5), k));
        line = max(line, kg_line(r, vec2<f32>(20.0, 13.0), vec2<f32>(20.0, 17.0), k));
        line = max(line, kg_line(r, vec2<f32>(20.0, 17.0), vec2<f32>(5.0, 17.0), k));
        line = max(line, kg_line(r, vec2<f32>(7.5, 14.5), vec2<f32>(5.0, 17.0), k));
        line = max(line, kg_line(r, vec2<f32>(5.0, 17.0), vec2<f32>(7.5, 19.5), k));
        var sb = 0.0;
        if (kind == 17) { sb = kg_back(t); }
        if (kind == 18) { sb = 1.0 - e; }
        if (sb > 0.01) {
            let b = vec2<f32>(12.0, 12.0) + (g - vec2<f32>(12.0, 12.0)) / sb;
            let d = min(kg_seg(b, vec2<f32>(11.0, 10.5), vec2<f32>(12.5, 10.0)), kg_seg(b, vec2<f32>(12.5, 10.0), vec2<f32>(12.5, 14.0))) * sb;
            line = max(line, clamp((0.75 - d) * k + 0.5, 0.0, 1.0));
        }
    } else if (kind == 19) {
        let x = g.x - 2.2 * sin(pi * t) * heading;
        var nx = x;
        if (heading < 0.0) { nx = 24.0 - x; }
        let n = vec2<f32>(nx, g.y);
        line = max(line, kg_line(n, vec2<f32>(5.0, 6.0), vec2<f32>(15.0, 12.0), k));
        line = max(line, kg_line(n, vec2<f32>(15.0, 12.0), vec2<f32>(5.0, 18.0), k));
        line = max(line, kg_line(n, vec2<f32>(5.0, 18.0), vec2<f32>(5.0, 6.0), k));
        line = max(line, kg_line(n, vec2<f32>(19.0, 6.0), vec2<f32>(19.0, 18.0), k));
    }
    let la = line * u[1].y;
    let fa = fill * u[2].w;
    let a = max(la, fa);
    if (a <= 0.0) { return vec4<f32>(0.0); }
    let rgb = mix(u[2].xyz, vec3<f32>(u[0].z, u[0].w, u[1].x), la / (la + fa));
    return vec4<f32>(rgb, a);
}
"#,
    prepare: None,
};

/// A glyph's state as an answer's mark: the icon and, for a toggle, whether
/// it is on.
pub(crate) fn glyph_mark(icon: crate::theme::Icon, on: Option<bool>) -> u64 {
    (icon as u64) * 4
        + match on {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }
}

/// How a glyph answers moving from the state `from` to `to` (marks from
/// `glyph_mark`): play and pause fold into each other, shuffle's arrows
/// cross, repeat spins round with its "one". `None` for a move that is not
/// one of these.
pub(crate) fn glyph_motion(from: u64, to: u64) -> Option<Answer> {
    use crate::theme::Icon;
    let m = glyph_mark;
    let moves = [
        ((Icon::Play, None), (Icon::Pause, None), Morph::ToPause),
        ((Icon::Pause, None), (Icon::Play, None), Morph::ToPlay),
        (
            (Icon::Shuffle, Some(false)),
            (Icon::Shuffle, Some(true)),
            Morph::ShuffleOn,
        ),
        (
            (Icon::Shuffle, Some(true)),
            (Icon::Shuffle, Some(false)),
            Morph::ShuffleOff,
        ),
        (
            (Icon::RepeatQueue, Some(false)),
            (Icon::RepeatQueue, Some(true)),
            Morph::Spin,
        ),
        (
            (Icon::RepeatQueue, Some(true)),
            (Icon::RepeatOne, Some(true)),
            Morph::SpinToOne,
        ),
        (
            (Icon::RepeatOne, Some(true)),
            (Icon::RepeatQueue, Some(false)),
            Morph::SpinFromOne,
        ),
        (
            (Icon::RepeatOne, Some(true)),
            (Icon::RepeatQueue, Some(true)),
            Morph::SpinFromOne,
        ),
        (
            (Icon::RepeatQueue, Some(true)),
            (Icon::RepeatQueue, Some(false)),
            Morph::Spin,
        ),
    ];
    moves
        .into_iter()
        .find(|((fi, fo), (ti, to_on), _)| m(*fi, *fo) == from && m(*ti, *to_on) == to)
        .map(|(_, _, morph)| Answer::Morph(morph))
}

/// Whether `icon` answers a change of state through its own glyph.
pub(crate) fn morphs(icon: crate::theme::Icon) -> bool {
    use crate::theme::Icon;
    matches!(
        icon,
        Icon::Play | Icon::Pause | Icon::Shuffle | Icon::RepeatQueue | Icon::RepeatOne
    )
}

/// A control's answer while one is drawn.
#[derive(Default)]
struct State {
    animated: Animated,
    live: Option<Answer>,
    /// When the control's own handler last started an answer: the change it
    /// answered is not answered again as the control draws it, once.
    own: Option<Instant>,
    /// The mark `follow` was last given.
    seen: Option<u64>,
    _end: Option<Task<()>>,
}

/// A control's answers, taken as the control is drawn and moved into its
/// handlers: keyed state found again from a handler would not be the
/// control's own.
///
/// The state is made only when something is answered or followed: a button
/// at rest, or scrolled past in a long list, holds an empty cell in its
/// element state and nothing else, no entity and no observer. The view that
/// drew the control is told directly when an answer starts and ends.
#[derive(Clone)]
pub struct Answerer {
    cell: Rc<RefCell<Option<Entity<State>>>>,
    view: EntityId,
}

impl Answerer {
    /// The answers of the control `id`.
    pub fn of(id: &ElementId, window: &mut Window, _: &mut App) -> Self {
        let view = window.current_view();
        let cell = window.with_global_id(child(id, "answer"), |global_id, window| {
            window.with_element_state(
                global_id,
                |cell: Option<Rc<RefCell<Option<Entity<State>>>>>, _| {
                    let cell = cell.unwrap_or_default();
                    (cell.clone(), cell)
                },
            )
        });
        Self { cell, view }
    }

    /// The state, made the first time it is needed.
    fn state(&self, cx: &mut App) -> Entity<State> {
        if let Some(state) = self.cell.borrow().as_ref() {
            return state.clone();
        }
        let state = cx.new(|_| State::default());
        *self.cell.borrow_mut() = Some(state.clone());
        state
    }

    /// Starts `answer` in `color`, from the control's own handler. Nothing
    /// happens under Reduce Motion or where the renderer draws no shaders.
    pub fn start(&self, answer: Answer, color: Hsla, cx: &mut App) {
        self.state(cx)
            .update(cx, |s, _| s.own = Some(Instant::now()));
        self.run(answer, color, cx);
    }

    /// Draws `answer` in `color`.
    fn run(&self, answer: Answer, color: Hsla, cx: &mut App) {
        if !SHADED || !KoanTheme::motion_allowed(cx) {
            return;
        }
        let (view, cell) = (self.view, self.cell.clone());
        self.state(cx).update(cx, |s, cx| {
            s.animated.set_all(0, &[answer.code(), answer.heading()]);
            s.animated.set_color(2, color);
            let at = Animated::now();
            for (k, ease) in [(0, Ease::Linear), (1, answer.ease())] {
                s.animated
                    .segment(k, 0.0, 1.0, at, answer.duration(), ease, Repeat::Once);
            }
            s.live = Some(answer);
            s._end = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(answer.duration()).await;
                // The quad leaves the scene once the answer has run, and a
                // control that follows nothing lets its state go with it.
                let follows = this
                    .update(cx, |s, _| {
                        s.live = None;
                        s.seen.is_some()
                    })
                    .unwrap_or(true);
                if !follows {
                    cell.borrow_mut().take();
                }
                cx.update(|cx| cx.notify(view));
            }));
        });
        cx.notify(view);
    }

    /// Answers a change the control is told of rather than one done to it
    /// (a favourite set from a shortcut, music arriving from a menu), as
    /// the control draws: `mark` names the state, and a draw that finds it
    /// moved starts `answer`. The first draw only notes it, so a control
    /// scrolled in or drawn fresh shows its state still, and a change the
    /// control answered from its own handler a moment ago is not answered
    /// twice.
    pub fn follow(
        &self,
        mark: u64,
        answer: Answer,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.moved(mark, false, answer, color, window, cx);
    }

    /// Notes `mark` without answering: a state the control answers only one
    /// way into (an option becoming the chosen one), passed while it stands
    /// in the other, so the next move in answers.
    pub fn note(&self, mark: u64, cx: &mut App) {
        self.state(cx).update(cx, |s, _| s.seen = Some(mark));
    }

    /// As `follow`, for something whose appearing is the news (a notice):
    /// its first draw answers too.
    pub fn announce(
        &self,
        mark: u64,
        answer: Answer,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.moved(mark, true, answer, color, window, cx);
    }

    fn moved(
        &self,
        mark: u64,
        first: bool,
        answer: Answer,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        let state = self.state(cx);
        let seen = state.read(cx).seen;
        if seen == Some(mark) {
            return;
        }
        // The change the control's own handler just answered, once; any
        // other change, however soon after, is answered.
        let answered = state.update(cx, |s, _| {
            s.seen = Some(mark);
            s.own
                .take()
                .is_some_and(|at| at.elapsed() < answer.duration())
        });
        if (first || seen.is_some()) && !answered {
            self.run(answer, color, cx);
            window.request_animation_frame();
        }
    }

    /// Whether an answer is being drawn.
    pub fn live(&self, cx: &App) -> bool {
        self.cell
            .borrow()
            .as_ref()
            .is_some_and(|s| s.read(cx).live.is_some())
    }

    /// Whether a glyph answer is being drawn: the control hides its icon
    /// meanwhile, and draws `glyph_layer` in its place.
    pub fn glyph_live(&self, cx: &App) -> bool {
        self.cell
            .borrow()
            .as_ref()
            .is_some_and(|s| s.read(cx).live.is_some_and(Answer::glyph))
    }

    /// The glyph answer being drawn, in place of the icon `side` points
    /// square that the control hides meanwhile, its strokes in `color`;
    /// nothing at rest. The quad stands over the icon and a little past it,
    /// for what is thrown out; the icon's parent is `relative`.
    pub fn glyph_layer(
        &self,
        side: Pixels,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let state = self.cell.borrow().clone()?;
        let s = state.read(cx);
        let answer = s.live.filter(|a| a.glyph())?;
        let look = crate::theme::Look::of(window, cx);
        let spill = look.spaced(answer.spill());
        let scale = window.scale_factor();
        s.animated.set_all(6, &[f32::from(spill) * scale]);
        s.animated.set_color(2, color);
        s.animated.set_color(8, look.accent.hsla());
        s.animated.set_all(12, &[f32::from(side) * scale]);
        Some(
            AnimatedQuad::new(&GLYPH, &s.animated)
                .absolute()
                .top(-spill)
                .left(-spill)
                .w(side + spill * 2.0)
                .h(side + spill * 2.0)
                .into_any_element(),
        )
    }

    /// The mark last followed or noted.
    pub(crate) fn seen(&self, cx: &App) -> Option<u64> {
        self.cell.borrow().as_ref().and_then(|s| s.read(cx).seen)
    }

    /// The answer being drawn, as a quad over the control and a little past
    /// it, for the control to add as its last child; nothing at rest. The
    /// control is `relative`.
    pub fn layer(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let state = self.cell.borrow().clone()?;
        let s = state.read(cx);
        let answer = s.live.filter(|a| !a.glyph())?;
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
    use gpui::{TestAppContext, VisualTestContext, div, red};

    /// Two controls side by side, each following its own mark.
    struct Two {
        marks: [u64; 2],
        answers: Vec<Answerer>,
    }

    impl Render for Two {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.answers = (0..2)
                .map(|i| {
                    let answers = Answerer::of(&ElementId::from(("control", i)), window, cx);
                    answers.follow(self.marks[i], Answer::Sweep, red(), window, cx);
                    answers
                })
                .collect();
            div()
        }
    }

    /// A control's answers are its own: one control's mark moving leaves
    /// its neighbour's state where it was.
    #[gpui::test]
    fn each_control_follows_its_own_mark(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| Two {
            marks: [0, 0],
            answers: Vec::new(),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        view.update(cx, |two, cx| {
            two.marks[0] = 1;
            cx.notify();
        });
        cx.run_until_parked();
        let seen = view.update(cx, |two, cx| {
            two.answers.iter().map(|a| a.seen(cx)).collect::<Vec<_>>()
        });
        assert_eq!(seen, [Some(1), Some(0)]);
    }

    /// A change the control's own handler answered is not answered again as
    /// it draws, once; the next change is answered.
    #[gpui::test]
    fn a_handler_s_answer_holds_off_one_change(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| Two {
            marks: [0, 0],
            answers: Vec::new(),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let own = |view: &Entity<Two>, cx: &mut VisualTestContext| {
            view.update(cx, |two, cx| {
                two.answers[0]
                    .cell
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.read(cx).own.is_some())
            })
        };
        view.update(cx, |two, cx| two.answers[0].start(Answer::Burst, red(), cx));
        assert!(own(&view, cx), "the handler's answer is noted");
        view.update(cx, |two, cx| {
            two.marks[0] = 1;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(!own(&view, cx), "the change it answered took the note");
        view.update(cx, |two, cx| {
            two.marks[0] = 2;
            cx.notify();
        });
        cx.run_until_parked();
        let seen = view.update(cx, |two, cx| two.answers[0].seen(cx));
        assert_eq!(seen, Some(2));
    }

    #[test]
    fn every_answer_is_within_the_motion_tokens() {
        for answer in [
            Answer::Ring,
            Answer::Burst,
            Answer::Close,
            Answer::Sweep,
            Answer::Lift,
            Answer::Streak(true),
        ] {
            assert!(answer.duration() <= Motion::SETTLE);
        }
    }
}
