//! Prism: the hidden rainbow appearance drawn on the GPU. A layer over the
//! wash that flows with the music, a pulse at the window's edges on the
//! beat, a mirror ball, confetti, a rainbow sweep between records, a sheen
//! over what the pointer is on, and rainbow programs for the playing bars
//! and the seek bar's fill (`PlayingBars::prism`, `Slider::prism`).
//!
//! Everything here is a fragment program on `Animated`: the analyser's bands
//! go into uniforms (`Prism::hear`), and the window presents each frame with
//! no GPUI frame drawn. Every quad is decorative, so its clock stops under
//! Reduce Motion, and an app mounts none of it while the appearance is off,
//! which leaves it costing nothing. Where the renderer draws no shaders
//! (`SHADED` false) there is nothing to mount.
//!
//! Two palettes: hyperpop (hot pink, cyber blue, holographic lilac, brat
//! lime) and brat's lime alone. The beat is an onset of the low band over
//! its running average, written once as a track falling from its strength to
//! nothing, which the programs read (sharper decays as powers of it), so a
//! beat fades out with nothing more written. Bursts keep their time on a
//! track too, held at their end once played.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::*;

use std::sync::Arc;

use parking_lot::Mutex;

use super::{Animated, AnimatedQuad, Ease, Repeat, ShaderProgram};

/// The palettes and a hash, in MSL, ahead of each program's `shade`.
macro_rules! prism_msl {
    () => {
        r#"
float3 koan_prism(float t, float brat) {
    t = fract(t);
    if (brat > 0.5) {
        float3 c[3] = { float3(0.541, 0.808, 0.0), float3(0.690, 0.941, 0.165), float3(0.435, 0.659, 0.0) };
        float x = t * 3.0;
        int i = int(floor(x)) % 3;
        return mix(c[i], c[(i + 1) % 3], smoothstep(0.0, 1.0, fract(x)));
    }
    float3 c[4] = { float3(1.0, 0.180, 0.604), float3(0.0, 0.784, 1.0), float3(0.702, 0.533, 1.0), float3(0.541, 0.808, 0.0) };
    float x = t * 4.0;
    int i = int(floor(x)) % 4;
    return mix(c[i], c[(i + 1) % 4], smoothstep(0.0, 1.0, fract(x)));
}

float koan_prism_hash(float2 p) {
    return fract(sin(dot(p, float2(127.1, 311.7))) * 43758.5453);
}
"#
    };
}

/// The same in WGSL.
macro_rules! prism_wgsl {
    () => {
        r#"
fn koan_prism(t0: f32, brat: f32) -> vec3<f32> {
    let t = fract(t0);
    if (brat > 0.5) {
        var c = array<vec3<f32>, 3>(vec3<f32>(0.541, 0.808, 0.0), vec3<f32>(0.690, 0.941, 0.165), vec3<f32>(0.435, 0.659, 0.0));
        let x = t * 3.0;
        let i = i32(floor(x)) % 3;
        return mix(c[i], c[(i + 1) % 3], smoothstep(0.0, 1.0, fract(x)));
    }
    var c = array<vec3<f32>, 4>(vec3<f32>(1.0, 0.180, 0.604), vec3<f32>(0.0, 0.784, 1.0), vec3<f32>(0.702, 0.533, 1.0), vec3<f32>(0.541, 0.808, 0.0));
    let x = t * 4.0;
    let i = i32(floor(x)) % 4;
    return mix(c[i], c[(i + 1) % 4], smoothstep(0.0, 1.0, fract(x)));
}

fn koan_prism_hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}
"#
    };
}

// The uniforms every `Prism` program reads: tracks 0..3 the bands (low,
// middle, high, 0 to 1, gliding between analyser frames); plain 3 the last
// beat's strength, 4 brat's palette, 5 a dark
// appearance, 6 the strength, 7 the low band's running average (the CPU's),
// 8 whether a window is in front (the mirror ball turns), 9 when the last
// beat landed, in seconds of shader time, 10 device pixels to a point.

/// Over the wash: a slow kaleidoscope of plasma through the palette, rings
/// rippling out on the low band and a beat, brighter with the high band.
static PRISM_WASH: ShaderProgram = ShaderProgram {
    name: "koan-prism-wash",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float2 p = (position - size * 0.5) / size.y;
    float pulse = pow(koan_track(u, 3, time), 1.0);
    float r = length(p);
    float seg = 6.2831853 / 6.0;
    float a = atan2(p.y, p.x) + time * 0.05 + 100.0 * seg;
    a = abs(fmod(a, seg) - seg * 0.5);
    float2 k = float2(cos(a), sin(a)) * r;
    float v = sin(k.x * 5.0 + time * 0.7) + sin(k.y * 6.0 - time * 0.55)
        + sin((p.x + p.y) * 3.0 + time * 0.25 + koan_track(u, 1, time) * 2.0)
        + sin(r * 14.0 - time * 2.6 - pulse * 3.0) * (0.4 + koan_track(u, 0, time));
    float3 colour = koan_prism(v * 0.12 + time * 0.04 + r * 0.3, u[4]);
    float glow = 0.5 + 0.5 * sin(v * 3.14159);
    float alpha = (u[5] > 0.5 ? 0.42 : 0.30) * (0.55 + 0.45 * glow) * (0.7 + 0.3 * koan_track(u, 2, time)) + pulse * 0.06;
    if (u[5] < 0.5) {
        colour = mix(colour, float3(1.0), 0.35);
    }
    return float4(colour, clamp(alpha * u[6], 0.0, 0.65));
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let p = (position - size * 0.5) / size.y;
    let pulse = pow(koan_track(u, 3u, time), 1.0);
    let r = length(p);
    let seg = 6.2831853 / 6.0;
    var a = atan2(p.y, p.x) + time * 0.05 + 100.0 * seg;
    a = abs(a % seg - seg * 0.5);
    let k = vec2<f32>(cos(a), sin(a)) * r;
    let v = sin(k.x * 5.0 + time * 0.7) + sin(k.y * 6.0 - time * 0.55)
        + sin((p.x + p.y) * 3.0 + time * 0.25 + koan_track(u, 1u, time) * 2.0)
        + sin(r * 14.0 - time * 2.6 - pulse * 3.0) * (0.4 + koan_track(u, 0u, time));
    var colour = koan_prism(v * 0.12 + time * 0.04 + r * 0.3, u[1].x);
    let glow = 0.5 + 0.5 * sin(v * 3.14159);
    let base = select(0.30, 0.42, u[1].y > 0.5);
    let alpha = base * (0.55 + 0.45 * glow) * (0.7 + 0.3 * koan_track(u, 2u, time)) + pulse * 0.06;
    if (u[1].y < 0.5) {
        colour = mix(colour, vec3<f32>(1.0), 0.35);
    }
    return vec4<f32>(colour, clamp(alpha * u[1].z, 0.0, 0.65));
}
"#
    ),
    prepare: None,
};

/// A glow in from the window's edges, widening and brightening on each beat,
/// the palette running round it.
static PRISM_PULSE: ShaderProgram = ShaderProgram {
    name: "koan-prism-pulse",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float d = min(min(position.x, size.x - position.x), min(position.y, size.y - position.y));
    float pulse = pow(koan_track(u, 3, time), 0.8);
    float width = u[10] * (10.0 + 30.0 * pulse + 8.0 * koan_track(u, 0, time));
    float glow = exp(-d / max(width, 1.0));
    if (glow < 0.004) {
        return float4(0.0);
    }
    float along = (position.x + position.y) / (size.x + size.y);
    float3 colour = koan_prism(along * 2.0 - time * 0.08, u[4]);
    return float4(colour, glow * (0.10 + 0.55 * pulse) * u[6]);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let d = min(min(position.x, size.x - position.x), min(position.y, size.y - position.y));
    let pulse = pow(koan_track(u, 3u, time), 0.8);
    let width = u[2].z * (10.0 + 30.0 * pulse + 8.0 * koan_track(u, 0u, time));
    let glow = exp(-d / max(width, 1.0));
    if (glow < 0.004) {
        return vec4<f32>(0.0);
    }
    let along = (position.x + position.y) / (size.x + size.y);
    let colour = koan_prism(along * 2.0 - time * 0.08, u[1].x);
    return vec4<f32>(colour, glow * (0.10 + 0.55 * pulse) * u[1].z);
}
"#
    ),
    prepare: None,
};

/// A mirror ball: a sphere of facets, silver tinted through the palette,
/// turning while a window is in front, its facets catching light on the
/// beat.
static MIRROR_BALL: ShaderProgram = ShaderProgram {
    name: "koan-prism-ball",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float radius = min(size.x, size.y) * 0.5 - u[10];
    float2 p = (position - size * 0.5) / radius;
    float r2 = dot(p, p);
    float edge = clamp((1.0 - sqrt(r2)) * radius, 0.0, 1.0);
    if (edge <= 0.0) {
        return float4(0.0);
    }
    float z = sqrt(max(1.0 - r2, 0.0));
    float spin = time * 0.6 * u[8];
    float2 facet = float2(atan2(p.x, z) + spin, asin(clamp(p.y, -1.0, 1.0))) * (10.0 / 3.14159);
    float2 cell = floor(facet);
    float2 inside = fract(facet);
    float h = koan_prism_hash(cell);
    float pulse = pow(koan_track(u, 3, time), 1.0);
    float sparkle = pow(max(0.0, sin(time * 3.0 * u[8] + h * 40.0)), 24.0) * (0.6 + pulse);
    float3 silver = float3(0.78, 0.80, 0.86) * (0.35 + 0.45 * z + 0.2 * h);
    float3 tint = koan_prism(h + time * 0.05 * u[8], u[4]);
    float3 colour = min(mix(silver, tint, 0.25 + 0.5 * sparkle) + sparkle * 0.6, float3(1.0));
    bool grout = inside.x < 0.08 || inside.y < 0.08;
    return float4(grout ? float3(0.22, 0.22, 0.26) * z : colour, edge * u[6]);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let radius = min(size.x, size.y) * 0.5 - u[2].z;
    let p = (position - size * 0.5) / radius;
    let r2 = dot(p, p);
    let edge = clamp((1.0 - sqrt(r2)) * radius, 0.0, 1.0);
    if (edge <= 0.0) {
        return vec4<f32>(0.0);
    }
    let z = sqrt(max(1.0 - r2, 0.0));
    let spin = time * 0.6 * u[2].x;
    let facet = vec2<f32>(atan2(p.x, z) + spin, asin(clamp(p.y, -1.0, 1.0))) * (10.0 / 3.14159);
    let cell = floor(facet);
    let inside = fract(facet);
    let h = koan_prism_hash(cell);
    let pulse = pow(koan_track(u, 3u, time), 1.0);
    let sparkle = pow(max(0.0, sin(time * 3.0 * u[2].x + h * 40.0)), 24.0) * (0.6 + pulse);
    let silver = vec3<f32>(0.78, 0.80, 0.86) * (0.35 + 0.45 * z + 0.2 * h);
    let tint = koan_prism(h + time * 0.05 * u[2].x, u[1].x);
    let colour = min(mix(silver, tint, 0.25 + 0.5 * sparkle) + sparkle * 0.6, vec3<f32>(1.0));
    let grout = inside.x < 0.08 || inside.y < 0.08;
    return vec4<f32>(select(colour, vec3<f32>(0.22, 0.22, 0.26) * z, grout), edge * u[1].z);
}
"#
    ),
    prepare: None,
};

/// The playing bars as ribbons of the palette: `BARS`' uniforms, each bar a
/// gradient up its height, 12 brat's palette.
pub(crate) static PRISM_BARS: ShaderProgram = ShaderProgram {
    name: "koan-prism-bars",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float pitch = u[8] + u[9];
    int i = int(floor(position.x / pitch));
    if (i < 0 || i > 2 || position.x - float(i) * pitch > u[8]) {
        return float4(0.0);
    }
    float top = size.y * (1.0 - max(koan_track(u, uint(i), time), u[10]));
    float cover = clamp(position.y - top + 0.5, 0.0, 1.0);
    float up = 1.0 - position.y / size.y;
    float3 colour = koan_prism(float(i) / 3.0 + up * 0.4 + koan_track(u, uint(i), time) * 0.2, u[12]);
    return float4(colour, u[7] * cover);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let pitch = u[2].x + u[2].y;
    let i = i32(floor(position.x / pitch));
    if (i < 0 || i > 2 || position.x - f32(i) * pitch > u[2].x) {
        return vec4<f32>(0.0);
    }
    let top = size.y * (1.0 - max(koan_track(u, u32(i), time), u[2].z));
    let cover = clamp(position.y - top + 0.5, 0.0, 1.0);
    let up = 1.0 - position.y / size.y;
    let colour = koan_prism(f32(i) / 3.0 + up * 0.4 + koan_track(u, u32(i), time) * 0.2, u[3].x);
    return vec4<f32>(colour, u[1].w * cover);
}
"#
    ),
    prepare: None,
};

/// The slider's fill as a rainbow trail running back from the playhead,
/// with a glint at its head: `GLIDE`'s uniforms, the fill's colour giving
/// only its opacity, and 5 brat's palette.
pub(crate) static PRISM_GLIDE: ShaderProgram = ShaderProgram {
    name: "koan-prism-glide",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float width = size.x - 2.0 * u[4];
    float x = position.x - u[4];
    float end = clamp(koan_track(u, 0, time) + koan_track(u, 1, time), 0.0, 1.0) * width;
    float dy = abs(position.y - size.y * 0.5);
    if (u[3] > 0.0 && abs(x - end) <= u[3] * 0.5 && dy <= u[3] * 0.5) {
        return float4(u[12], u[13], u[14], u[15]);
    }
    if (x < 0.0 || dy > u[2] * 0.5) {
        return float4(0.0);
    }
    float cover = clamp(end - x + 0.5, 0.0, 1.0);
    float3 colour = koan_prism(x / max(width, 1.0) * 0.9 - time * 0.12, u[5]);
    float glint = exp(-max(end - x, 0.0) / max(u[2] * 4.0, 1.0));
    return float4(mix(colour, float3(1.0), glint * 0.6), u[11] * cover);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let width = size.x - 2.0 * u[1].x;
    let x = position.x - u[1].x;
    let end = clamp(koan_track(u, 0u, time) + koan_track(u, 1u, time), 0.0, 1.0) * width;
    let dy = abs(position.y - size.y * 0.5);
    if (u[0].w > 0.0 && abs(x - end) <= u[0].w * 0.5 && dy <= u[0].w * 0.5) {
        return u[3];
    }
    if (x < 0.0 || dy > u[0].z * 0.5) {
        return vec4<f32>(0.0);
    }
    let cover = clamp(end - x + 0.5, 0.0, 1.0);
    let colour = koan_prism(x / max(width, 1.0) * 0.9 - time * 0.12, u[1].y);
    let glint = exp(-max(end - x, 0.0) / max(u[0].z * 4.0, 1.0));
    return vec4<f32>(mix(colour, vec3<f32>(1.0), glint * 0.6), u[2].w * cover);
}
"#
    ),
    prepare: None,
};

// A burst's uniforms: 0, 1 where it starts, in device pixels within the
// quad, 2 a seed, 3 brat's palette, 4 device pixels to a point, 5 how long
// it lasts in seconds. Its clock starts when it is fired.

/// Confetti and glitter thrown up from a point and falling away.
static CONFETTI: ShaderProgram = ShaderProgram {
    name: "koan-prism-confetti",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = koan_track(u, 0, time);
    if (t < 0.0 || t >= u[5]) {
        return float4(0.0);
    }
    float s = u[4];
    float2 c = float2(u[0], u[1]);
    if (distance(position, c) > s * (520.0 + 260.0 * t * t)) {
        return float4(0.0);
    }
    float fade = 1.0 - smoothstep(u[5] * 0.6, u[5], t);
    float4 colour = float4(0.0);
    for (int i = 0; i < 96; i++) {
        float fi = float(i);
        float h1 = koan_prism_hash(float2(fi, u[2]));
        float h2 = koan_prism_hash(float2(u[2], fi + 7.0));
        float h3 = koan_prism_hash(float2(fi * 1.7, fi + u[2] * 3.1));
        float angle = -1.5708 + (h1 - 0.5) * 3.6;
        float2 v = float2(cos(angle), sin(angle)) * s * (160.0 + 300.0 * h2);
        float2 at = c + v * (1.0 - exp(-2.2 * t)) / 2.2 + float2(0.0, s * 260.0 * t * t);
        float2 d = position - at;
        float spin = t * (6.0 + 10.0 * h3) + h1 * 6.28;
        float2 r = float2(d.x * cos(spin) + d.y * sin(spin), -d.x * sin(spin) + d.y * cos(spin));
        bool glitter = h3 > 0.7;
        float2 extent = glitter ? float2(s * 1.6) : float2(s * 4.0, s * (0.4 + 1.8 * abs(cos(spin * 1.3))));
        float2 q = abs(r) - extent;
        float inside = clamp(0.5 - max(q.x, q.y), 0.0, 1.0);
        if (inside > 0.0) {
            float twinkle = glitter ? 0.5 + 0.5 * sin(t * 30.0 + fi) : 1.0;
            float3 tint = glitter ? mix(float3(1.0), koan_prism(h2, u[3]), 0.3) : koan_prism(h1 + h2, u[3]);
            colour = float4(tint, max(colour.a, inside * twinkle * fade));
        }
    }
    return colour;
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = koan_track(u, 0u, time);
    if (t < 0.0 || t >= u[1].y) {
        return vec4<f32>(0.0);
    }
    let s = u[1].x;
    let c = u[0].xy;
    if (distance(position, c) > s * (520.0 + 260.0 * t * t)) {
        return vec4<f32>(0.0);
    }
    let fade = 1.0 - smoothstep(u[1].y * 0.6, u[1].y, t);
    var colour = vec4<f32>(0.0);
    for (var i = 0; i < 96; i++) {
        let fi = f32(i);
        let h1 = koan_prism_hash(vec2<f32>(fi, u[0].z));
        let h2 = koan_prism_hash(vec2<f32>(u[0].z, fi + 7.0));
        let h3 = koan_prism_hash(vec2<f32>(fi * 1.7, fi + u[0].z * 3.1));
        let angle = -1.5708 + (h1 - 0.5) * 3.6;
        let v = vec2<f32>(cos(angle), sin(angle)) * s * (160.0 + 300.0 * h2);
        let at = c + v * (1.0 - exp(-2.2 * t)) / 2.2 + vec2<f32>(0.0, s * 260.0 * t * t);
        let d = position - at;
        let spin = t * (6.0 + 10.0 * h3) + h1 * 6.28;
        let r = vec2<f32>(d.x * cos(spin) + d.y * sin(spin), -d.x * sin(spin) + d.y * cos(spin));
        let glitter = h3 > 0.7;
        let extent = select(vec2<f32>(s * 4.0, s * (0.4 + 1.8 * abs(cos(spin * 1.3)))), vec2<f32>(s * 1.6), glitter);
        let q = abs(r) - extent;
        let inside = clamp(0.5 - max(q.x, q.y), 0.0, 1.0);
        if (inside > 0.0) {
            let twinkle = select(1.0, 0.5 + 0.5 * sin(t * 30.0 + fi), glitter);
            let tint = select(koan_prism(h1 + h2, u[0].w), mix(vec3<f32>(1.0), koan_prism(h2, u[0].w), 0.3), glitter);
            colour = vec4<f32>(tint, max(colour.a, inside * twinkle * fade));
        }
    }
    return colour;
}
"#
    ),
    prepare: None,
};

/// A ring of the palette sweeping out from a point across the window and
/// fading: the change from one record to the next.
static SWEEP: ShaderProgram = ShaderProgram {
    name: "koan-prism-sweep",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = koan_track(u, 0, time) / u[5];
    if (t < 0.0 || t >= 1.0) {
        return float4(0.0);
    }
    float2 c = float2(u[0], u[1]);
    float r = distance(position, c);
    float far = length(size);
    float width = far * 0.18;
    float band = 1.0 - smoothstep(0.0, width, abs(r - far * t * (2.0 - t)));
    if (band <= 0.0) {
        return float4(0.0);
    }
    float angle = atan2(position.y - c.y, position.x - c.x) / 6.2831853;
    float3 colour = koan_prism(angle + r / width * 0.3 - t, u[3]);
    return float4(colour, band * 0.45 * (1.0 - t));
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = koan_track(u, 0u, time) / u[1].y;
    if (t < 0.0 || t >= 1.0) {
        return vec4<f32>(0.0);
    }
    let c = u[0].xy;
    let r = distance(position, c);
    let far = length(size);
    let width = far * 0.18;
    let band = 1.0 - smoothstep(0.0, width, abs(r - far * t * (2.0 - t)));
    if (band <= 0.0) {
        return vec4<f32>(0.0);
    }
    let angle = atan2(position.y - c.y, position.x - c.x) / 6.2831853;
    let colour = koan_prism(angle + r / width * 0.3 - t, u[0].w);
    return vec4<f32>(colour, band * 0.45 * (1.0 - t));
}
"#
    ),
    prepare: None,
};

/// A band of the palette passing diagonally over an element, again and
/// again: refraction on what the pointer is on, a shimmer through a title.
/// Uniforms: 0 brat's palette, 1 the strength, 2 a glitch every five seconds.
static SHEEN: ShaderProgram = ShaderProgram {
    name: "koan-prism-sheen",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float s = (position.x + position.y * 0.6) / (size.x + size.y);
    float sweep = fract(s * 0.8 - time * 0.35);
    float band = smoothstep(0.0, 0.25, sweep) * (1.0 - smoothstep(0.25, 0.55, sweep));
    float gloss = pow(band, 10.0);
    float3 colour = mix(mix(koan_prism(s * 1.5 + time * 0.2, u[0]), float3(1.0, 0.75, 0.88), 0.25), float3(1.0), gloss);
    float alpha = clamp(band * u[1] + gloss * 0.6, 0.0, 1.0);
    if (u[2] > 0.5 && u[15] > 0.5 && fract(time / 5.0) < 0.035) {
        float slice = floor(position.y / max(size.y / 6.0, 1.0));
        float h = koan_prism_hash(float2(slice, floor(time / 5.0)));
        if (h > 0.45) {
            colour = h > 0.72 ? float3(1.0, 0.18, 0.6) : float3(0.0, 0.78, 1.0);
            alpha = 0.6;
        }
    }
    return float4(colour, alpha);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let s = (position.x + position.y * 0.6) / (size.x + size.y);
    let sweep = fract(s * 0.8 - time * 0.35);
    let band = smoothstep(0.0, 0.25, sweep) * (1.0 - smoothstep(0.25, 0.55, sweep));
    let gloss = pow(band, 10.0);
    var colour = mix(mix(koan_prism(s * 1.5 + time * 0.2, u[0].x), vec3<f32>(1.0, 0.75, 0.88), 0.25), vec3<f32>(1.0), gloss);
    var alpha = clamp(band * u[0].y + gloss * 0.6, 0.0, 1.0);
    if (u[0].z > 0.5 && u[3].w > 0.5 && fract(time / 5.0) < 0.035) {
        let slice = floor(position.y / max(size.y / 6.0, 1.0));
        let h = koan_prism_hash(vec2<f32>(slice, floor(time / 5.0)));
        if (h > 0.45) {
            colour = select(vec3<f32>(0.0, 0.78, 1.0), vec3<f32>(1.0, 0.18, 0.6), h > 0.72);
            alpha = 0.6;
        }
    }
    return vec4<f32>(colour, alpha);
}
"#
    ),
    prepare: None,
};

/// Over the page: the mirror ball's spots wheeling across the window,
/// stage beams sweeping up from its foot, stars twinkling, glitter falling,
/// and a flash on the beat. Uniforms
/// as every `Prism` program's, and 11, 12 where the ball hangs: points in
/// from the right and down from the top.
static DISCO: ShaderProgram = ShaderProgram {
    name: "koan-prism-disco",
    msl: concat!(
        prism_msl!(),
        r#"
float koan_disco_blend(thread float3 &colour, thread float &alpha, float3 c, float a) {
    if (a > 0.0) {
        colour = mix(colour, c, a / max(alpha + a, 1e-4));
        alpha = max(alpha, a);
    }
    return a;
}

float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float s = u[10];
    float spin = time * 0.35 * u[8];
    float beat = pow(koan_track(u, 3, time), 1.6);
    float far = length(size);
    float3 colour = float3(0.0);
    float alpha = 0.0;
    // The mirror ball's spots, wheeling across the window from it.
    float2 ball = float2(size.x - u[11] * s, u[12] * s);
    // Each spot's place and size from its index by the golden ratio's
    // fractions, with no trigonometry to find them; a pixel outside a spot
    // goes on to the next before any colour is worked out.
    float lift = 0.22 + 0.25 * beat + 0.15 * koan_track(u, 2, time);
    float swell = 1.0 + 0.6 * beat;
    for (int i = 0; i < 48; i++) {
        float fi = float(i);
        float h1 = fract(fi * 0.6180340 + 0.31);
        float h2 = fract(fi * 0.7548777 + 0.57);
        float radius = s * (10.0 + 14.0 * h1) * swell;
        float angle = h1 * 6.2831853 + spin * (0.6 + 0.4 * h2);
        float2 at = ball + float2(cos(angle), sin(angle) * 0.6 + 0.35) * far * (0.15 + 0.85 * h2);
        float d = distance(position, at);
        if (d >= radius) {
            continue;
        }
        float spot = 1.0 - smoothstep(radius * 0.4, radius, d);
        float3 c = mix(float3(1.0), koan_prism(h1 + time * 0.1, u[4]), 0.55);
        koan_disco_blend(colour, alpha, c, spot * lift);
    }
    // Stage beams sweeping up from the foot of the window.
    for (int i = 0; i < 4; i++) {
        float fi = float(i);
        float2 foot = float2(size.x * (0.15 + 0.233 * fi), size.y + 20.0 * s);
        float angle = 1.5708 + sin(time * (0.5 + 0.13 * fi) + fi * 1.7) * 0.55;
        float2 dir = float2(cos(angle), -sin(angle));
        float2 rel = position - foot;
        float along = dot(rel, dir);
        if (along > 0.0) {
            float across = abs(rel.x * dir.y - rel.y * dir.x);
            float width = s * 14.0 + along * 0.08;
            float beam = (1.0 - smoothstep(width * 0.3, width, across)) * max(1.0 - along / (far * 1.1), 0.0);
            koan_disco_blend(colour, alpha, koan_prism(fi * 0.25 + time * 0.07, u[4]), beam * (0.10 + 0.18 * beat + 0.08 * koan_track(u, 1, time)));
        }
    }
    // Glimmers: four-pointed stars twinkling in and out on a grid.
    float cell_side = 46.0 * s;
    float2 cell = floor(position / cell_side);
    float phase = floor(time * 2.5);
    if (koan_prism_hash(cell + phase * 0.37) > 0.93) {
        float2 jitter = float2(koan_prism_hash(cell + 1.3), koan_prism_hash(cell + 2.9)) - 0.5;
        float2 q = abs(position - (cell + 0.5 + jitter * 0.6) * cell_side);
        float twinkle = sin(fract(time * 2.5) * 3.14159);
        float reach = max(s * 9.0 * twinkle, 1e-3);
        float star = (1.0 - smoothstep(0.0, s * 1.2, q.y)) * (1.0 - smoothstep(0.0, reach, q.x))
            + (1.0 - smoothstep(0.0, s * 1.2, q.x)) * (1.0 - smoothstep(0.0, reach, q.y));
        koan_disco_blend(colour, alpha, float3(1.0), clamp(star, 0.0, 1.0) * 0.9 * twinkle);
    }
    // Glitter falling down some of the columns.
    float pitch = s * 9.0;
    float column = floor(position.x / pitch);
    float hc = koan_prism_hash(float2(column, 11.0));
    if (hc > 0.8) {
        float y = fract((time * s * (60.0 + 90.0 * hc) + hc * 4000.0) / size.y) * size.y;
        float d = max(abs(position.x - (column + 0.5) * pitch), abs(position.y - y));
        float glint = (1.0 - smoothstep(0.0, s * 1.6, d)) * (0.5 + 0.5 * sin(time * 20.0 + hc * 50.0));
        koan_disco_blend(colour, alpha, mix(float3(1.0), koan_prism(hc + time * 0.2, u[4]), 0.4), glint * 0.8);
    }
    // The beat's flash over the whole window. Beats come at most three a
    // second (`Prism::hear`), WCAG's limit for flashing.
    koan_disco_blend(colour, alpha, koan_prism(time * 0.3, u[4]) * 0.5 + 0.5, beat * 0.10);
    return float4(colour, clamp(alpha * u[6], 0.0, 0.85));
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let s = u[2].z;
    let spin = time * 0.35 * u[2].x;
    let beat = pow(koan_track(u, 3u, time), 1.6);
    let far = length(size);
    var colour = vec3<f32>(0.0);
    var alpha = 0.0;
    let ball = vec2<f32>(size.x - u[2].w * s, u[3].x * s);
    let lift = 0.22 + 0.25 * beat + 0.15 * koan_track(u, 2u, time);
    let swell = 1.0 + 0.6 * beat;
    for (var i = 0; i < 48; i++) {
        let fi = f32(i);
        let h1 = fract(fi * 0.6180340 + 0.31);
        let h2 = fract(fi * 0.7548777 + 0.57);
        let radius = s * (10.0 + 14.0 * h1) * swell;
        let angle = h1 * 6.2831853 + spin * (0.6 + 0.4 * h2);
        let at = ball + vec2<f32>(cos(angle), sin(angle) * 0.6 + 0.35) * far * (0.15 + 0.85 * h2);
        let d = distance(position, at);
        if (d >= radius) {
            continue;
        }
        let spot = 1.0 - smoothstep(radius * 0.4, radius, d);
        let c = mix(vec3<f32>(1.0), koan_prism(h1 + time * 0.1, u[1].x), 0.55);
        let a = spot * lift;
        if (a > 0.0) {
            colour = mix(colour, c, a / max(alpha + a, 1e-4));
            alpha = max(alpha, a);
        }
    }
    for (var i = 0; i < 4; i++) {
        let fi = f32(i);
        let foot = vec2<f32>(size.x * (0.15 + 0.233 * fi), size.y + 20.0 * s);
        let angle = 1.5708 + sin(time * (0.5 + 0.13 * fi) + fi * 1.7) * 0.55;
        let dir = vec2<f32>(cos(angle), -sin(angle));
        let rel = position - foot;
        let along = dot(rel, dir);
        if (along > 0.0) {
            let across = abs(rel.x * dir.y - rel.y * dir.x);
            let width = s * 14.0 + along * 0.08;
            let beam = (1.0 - smoothstep(width * 0.3, width, across)) * max(1.0 - along / (far * 1.1), 0.0);
            let a = beam * (0.10 + 0.18 * beat + 0.08 * koan_track(u, 1u, time));
            if (a > 0.0) {
                colour = mix(colour, koan_prism(fi * 0.25 + time * 0.07, u[1].x), a / max(alpha + a, 1e-4));
                alpha = max(alpha, a);
            }
        }
    }
    let cell_side = 46.0 * s;
    let cell = floor(position / cell_side);
    let phase = floor(time * 2.5);
    if (koan_prism_hash(cell + phase * 0.37) > 0.93) {
        let jitter = vec2<f32>(koan_prism_hash(cell + 1.3), koan_prism_hash(cell + 2.9)) - 0.5;
        let q = abs(position - (cell + 0.5 + jitter * 0.6) * cell_side);
        let twinkle = sin(fract(time * 2.5) * 3.14159);
        let reach = max(s * 9.0 * twinkle, 1e-3);
        let star = (1.0 - smoothstep(0.0, s * 1.2, q.y)) * (1.0 - smoothstep(0.0, reach, q.x))
            + (1.0 - smoothstep(0.0, s * 1.2, q.x)) * (1.0 - smoothstep(0.0, reach, q.y));
        let a = clamp(star, 0.0, 1.0) * 0.9 * twinkle;
        if (a > 0.0) {
            colour = mix(colour, vec3<f32>(1.0), a / max(alpha + a, 1e-4));
            alpha = max(alpha, a);
        }
    }
    let pitch = s * 9.0;
    let column = floor(position.x / pitch);
    let hc = koan_prism_hash(vec2<f32>(column, 11.0));
    if (hc > 0.8) {
        let y = fract((time * s * (60.0 + 90.0 * hc) + hc * 4000.0) / size.y) * size.y;
        let d = max(abs(position.x - (column + 0.5) * pitch), abs(position.y - y));
        let glint = (1.0 - smoothstep(0.0, s * 1.6, d)) * (0.5 + 0.5 * sin(time * 20.0 + hc * 50.0));
        let a = glint * 0.8;
        if (a > 0.0) {
            colour = mix(colour, mix(vec3<f32>(1.0), koan_prism(hc + time * 0.2, u[1].x), 0.4), a / max(alpha + a, 1e-4));
            alpha = max(alpha, a);
        }
    }
    let flash = beat * 0.10;
    if (flash > 0.0) {
        colour = mix(colour, koan_prism(time * 0.3, u[1].x) * 0.5 + 0.5, flash / max(alpha + flash, 1e-4));
        alpha = max(alpha, flash);
    }
    return vec4<f32>(colour, clamp(alpha * u[1].z, 0.0, 0.85));
}
"#
    ),
    prepare: None,
};

/// The palette as a gradient running along a fill, a glint passing through
/// it: a progress bar's fill, the chosen tab's rule. Uniforms: 0 brat's
/// palette.
static GRADIENT: ShaderProgram = ShaderProgram {
    name: "koan-prism-gradient",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float x = position.x / max(size.x, 1.0);
    float3 colour = koan_prism(x * 0.8 - time * 0.2, u[0]);
    float glint = pow(max(0.0, 1.0 - abs(fract(x * 0.5 - time * 0.35) - 0.5) * 6.0), 4.0);
    return float4(mix(colour, float3(1.0), glint * 0.5), 1.0);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let x = position.x / max(size.x, 1.0);
    let colour = koan_prism(x * 0.8 - time * 0.2, u[0].x);
    let glint = pow(max(0.0, 1.0 - abs(fract(x * 0.5 - time * 0.35) - 0.5) * 6.0), 4.0);
    return vec4<f32>(mix(colour, vec3<f32>(1.0), glint * 0.5), 1.0);
}
"#
    ),
    prepare: None,
};

/// The sticker slapped on mid-window as the appearance comes out: brat's
/// wordmark on lime for a track that brought it, a chrome y2k star
/// otherwise. It pops in past its size, settles with a wobble at a tilt,
/// holds and fades. A burst's uniforms, 3 choosing brat's.
static STICKER: ShaderProgram = ShaderProgram {
    name: "koan-prism-sticker",
    msl: concat!(
        prism_msl!(),
        r#"
float koan_segment(float2 p, float2 a, float2 b) {
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// "brat", lowercase, drawn in strokes on a 300 by 124 card centred on 0.
float koan_brat_word(float2 p) {
    float d = 1e5;
    d = min(d, koan_segment(p, float2(-130.0, -55.0), float2(-130.0, 40.0)));
    d = min(d, abs(length(p - float2(-105.0, 13.0)) - 25.0));
    d = min(d, koan_segment(p, float2(-55.0, -12.0), float2(-55.0, 40.0)));
    if (p.y < 3.0 && p.x > -55.0) {
        d = min(d, abs(length(p - float2(-30.0, 3.0)) - 25.0));
    }
    d = min(d, abs(length(p - float2(30.0, 15.0)) - 25.0));
    d = min(d, koan_segment(p, float2(55.0, -10.0), float2(55.0, 40.0)));
    d = min(d, koan_segment(p, float2(100.0, -45.0), float2(100.0, 40.0)));
    d = min(d, koan_segment(p, float2(84.0, -15.0), float2(118.0, -15.0)));
    return 1.0 - smoothstep(6.0, 7.5, d);
}

float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = koan_track(u, 0, time);
    if (t < 0.0 || t >= u[5]) {
        return float4(0.0);
    }
    float s = u[4];
    float pop = t < 0.3 ? (1.0 - pow(1.0 - t / 0.3, 3.0)) * 1.12 : (t < 0.45 ? mix(1.12, 1.0, (t - 0.3) / 0.15) : 1.0);
    float fade = 1.0 - smoothstep(u[5] - 0.5, u[5], t);
    float tilt = -0.14 + 0.05 * sin(t * 9.0) * exp(-t * 3.0);
    float2 p = (position - float2(u[0], u[1])) / (s * max(pop, 0.001));
    if (length(p) > 220.0) {
        return float4(0.0);
    }
    float cs = cos(tilt);
    float sn = sin(tilt);
    p = float2(p.x * cs - p.y * sn, p.x * sn + p.y * cs);
    if (u[3] > 0.5) {
        float2 q = abs(p) - float2(150.0, 62.0);
        float edge = clamp(0.5 - max(q.x, q.y), 0.0, 1.0);
        if (edge <= 0.0) {
            return float4(0.0);
        }
        float3 lime = float3(0.541, 0.808, 0.0);
        return float4(mix(lime, float3(0.0), koan_brat_word(p)), edge * fade);
    }
    float r = length(p);
    float a = atan2(p.y, p.x);
    float star = 110.0 * (0.45 + 0.55 * pow(abs(cos(a * 2.0)), 10.0));
    float inside = clamp((star - r) * 0.8, 0.0, 1.0);
    if (inside <= 0.0) {
        return float4(0.0);
    }
    float3 chrome = mix(float3(0.62, 0.66, 0.74), float3(1.0), 0.5 + 0.5 * sin(p.y * 0.05 + t * 3.0));
    float3 holo = koan_prism(a / 6.2831853 + r * 0.004 + t * 0.2, 0.0);
    float sweep = pow(max(0.0, 1.0 - abs(p.x + p.y * 0.6 - (t * 320.0 - 160.0)) / 40.0), 3.0);
    return float4(min(mix(chrome, holo, 0.35) + sweep * 0.6, float3(1.0)), inside * fade);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn koan_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

fn koan_brat_word(p: vec2<f32>) -> f32 {
    var d = 1e5;
    d = min(d, koan_segment(p, vec2<f32>(-130.0, -55.0), vec2<f32>(-130.0, 40.0)));
    d = min(d, abs(length(p - vec2<f32>(-105.0, 13.0)) - 25.0));
    d = min(d, koan_segment(p, vec2<f32>(-55.0, -12.0), vec2<f32>(-55.0, 40.0)));
    if (p.y < 3.0 && p.x > -55.0) {
        d = min(d, abs(length(p - vec2<f32>(-30.0, 3.0)) - 25.0));
    }
    d = min(d, abs(length(p - vec2<f32>(30.0, 15.0)) - 25.0));
    d = min(d, koan_segment(p, vec2<f32>(55.0, -10.0), vec2<f32>(55.0, 40.0)));
    d = min(d, koan_segment(p, vec2<f32>(100.0, -45.0), vec2<f32>(100.0, 40.0)));
    d = min(d, koan_segment(p, vec2<f32>(84.0, -15.0), vec2<f32>(118.0, -15.0)));
    return 1.0 - smoothstep(6.0, 7.5, d);
}

fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = koan_track(u, 0u, time);
    if (t < 0.0 || t >= u[1].y) {
        return vec4<f32>(0.0);
    }
    let s = u[1].x;
    var pop = 1.0;
    if (t < 0.3) {
        pop = (1.0 - pow(1.0 - t / 0.3, 3.0)) * 1.12;
    } else if (t < 0.45) {
        pop = mix(1.12, 1.0, (t - 0.3) / 0.15);
    }
    let fade = 1.0 - smoothstep(u[1].y - 0.5, u[1].y, t);
    let tilt = -0.14 + 0.05 * sin(t * 9.0) * exp(-t * 3.0);
    var p = (position - u[0].xy) / (s * max(pop, 0.001));
    if (length(p) > 220.0) {
        return vec4<f32>(0.0);
    }
    let cs = cos(tilt);
    let sn = sin(tilt);
    p = vec2<f32>(p.x * cs - p.y * sn, p.x * sn + p.y * cs);
    if (u[0].w > 0.5) {
        let q = abs(p) - vec2<f32>(150.0, 62.0);
        let edge = clamp(0.5 - max(q.x, q.y), 0.0, 1.0);
        if (edge <= 0.0) {
            return vec4<f32>(0.0);
        }
        let lime = vec3<f32>(0.541, 0.808, 0.0);
        return vec4<f32>(mix(lime, vec3<f32>(0.0), koan_brat_word(p)), edge * fade);
    }
    let r = length(p);
    let a = atan2(p.y, p.x);
    let star = 110.0 * (0.45 + 0.55 * pow(abs(cos(a * 2.0)), 10.0));
    let inside = clamp((star - r) * 0.8, 0.0, 1.0);
    if (inside <= 0.0) {
        return vec4<f32>(0.0);
    }
    let chrome = mix(vec3<f32>(0.62, 0.66, 0.74), vec3<f32>(1.0), 0.5 + 0.5 * sin(p.y * 0.05 + t * 3.0));
    let holo = koan_prism(a / 6.2831853 + r * 0.004 + t * 0.2, 0.0);
    let sweep = pow(max(0.0, 1.0 - abs(p.x + p.y * 0.6 - (t * 320.0 - 160.0)) / 40.0), 3.0);
    return vec4<f32>(min(mix(chrome, holo, 0.35) + sweep * 0.6, vec3<f32>(1.0)), inside * fade);
}
"#
    ),
    prepare: None,
};

/// The party over the page: six lasers criss-crossing from the window's top
/// corners, brighter with the high band; pyro shooting up from its foot on
/// each beat, taller with the low band (brat's in lime); glossy hearts
/// floating up through it, swaying. A `Prism` program; nothing flashes.
static PARTY: ShaderProgram = ShaderProgram {
    name: "koan-prism-party",
    msl: concat!(
        prism_msl!(),
        r#"
float koan_dot2(float2 v) {
    return dot(v, v);
}

// Inigo Quilez's heart, tip at the origin, a unit tall, y up.
float koan_heart(float2 p) {
    p.x = abs(p.x);
    if (p.y + p.x > 1.0) {
        return sqrt(koan_dot2(p - float2(0.25, 0.75))) - 0.35355339;
    }
    return sqrt(min(koan_dot2(p - float2(0.0, 1.0)), koan_dot2(p - 0.5 * max(p.x + p.y, 0.0)))) * sign(p.x - p.y);
}

float koan_party_blend(thread float3 &colour, thread float &alpha, float3 c, float a) {
    if (a > 0.0) {
        colour = mix(colour, c, a / max(alpha + a, 1e-4));
        alpha = max(alpha, a);
    }
    return a;
}

float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float s = u[10];
    float low = koan_track(u, 0, time);
    float high = koan_track(u, 2, time);
    float beat = pow(koan_track(u, 3, time), 0.6);
    float3 colour = float3(0.0);
    float alpha = 0.0;
    // Lasers from the top corners, criss-crossing.
    for (int i = 0; i < 6; i++) {
        float fi = float(i);
        float side = i < 3 ? 1.0 : -1.0;
        float2 origin = float2(i < 3 ? 0.0 : size.x, -10.0 * s);
        float theta = 0.25 + 0.55 * (0.5 + 0.5 * sin(time * (1.1 + 0.31 * fi) + fi * 2.1));
        float2 dir = float2(sin(theta) * side, cos(theta));
        float2 rel = position - origin;
        float along = dot(rel, dir);
        if (along > 0.0) {
            float across = abs(rel.x * dir.y - rel.y * dir.x);
            float beam = exp(-across / (1.4 * s)) + 0.35 * exp(-across / (7.0 * s));
            koan_party_blend(colour, alpha, mix(koan_prism(fi / 6.0 + time * 0.12, u[4]), float3(1.0), 0.25), beam * (0.25 + 0.45 * high));
        }
    }
    // Pyro: flames shooting up from the foot on each beat.
    float from_foot = size.y - position.y;
    for (int k = 0; k < 5; k++) {
        float fk = float(k);
        float cx = size.x * (0.1 + 0.2 * fk);
        float h = size.y * (0.08 + 0.5 * beat) * (0.6 + 0.4 * low) * (0.8 + 0.2 * sin(fk * 3.7 + time * 9.0));
        if (from_foot < h) {
            float rise = from_foot / max(h, 1.0);
            float width = s * (34.0 * (1.0 - rise) + 4.0);
            float flicker = koan_prism_hash(floor(float2((position.x - cx) / (5.0 * s), (from_foot - time * 520.0 * s) / (9.0 * s))));
            float flame = (1.0 - smoothstep(width * 0.5, width, abs(position.x - cx))) * (0.55 + 0.45 * flicker);
            float3 hot = u[4] > 0.5
                ? mix(float3(0.85, 1.0, 0.5), float3(0.54, 0.81, 0.0), rise)
                : mix(mix(float3(1.0, 1.0, 0.85), float3(1.0, 0.75, 0.1), smoothstep(0.0, 0.4, rise)), float3(1.0, 0.18, 0.6), smoothstep(0.4, 1.0, rise));
            koan_party_blend(colour, alpha, hot, flame * (1.0 - rise * 0.6) * 0.9);
        }
    }
    // Hearts floating up through the window, swaying.
    for (int j = 0; j < 14; j++) {
        float fj = float(j);
        float h1 = koan_prism_hash(float2(fj, 1.3));
        float h2 = koan_prism_hash(float2(2.9, fj));
        float h3 = koan_prism_hash(float2(fj * 0.7, 5.1));
        float radius = s * (16.0 + 22.0 * h2) * (1.0 + 0.15 * beat);
        float travel = size.y + 4.0 * radius;
        float y = size.y + 2.0 * radius - fract(time * (0.04 + 0.05 * h1) + h3) * travel;
        float x = h1 * size.x + sin(time * (0.6 + h2) + fj) * 40.0 * s;
        float2 p = float2(position.x - x, y - position.y) / radius + float2(0.0, 0.5);
        if (abs(p.x) < 1.2 && p.y > -0.3 && p.y < 1.3) {
            float d = koan_heart(p);
            float body = 1.0 - smoothstep(-0.02, 0.02, d);
            float shine = (1.0 - smoothstep(0.0, 0.12, length(p - float2(-0.28, 0.72)))) * body;
            float3 c = mix(koan_prism(h3 + time * 0.05, u[4]), float3(1.0, 0.6, 0.8), 0.3);
            koan_party_blend(colour, alpha, mix(c, float3(1.0), shine * 0.8), body * 0.8);
        }
    }
    return float4(colour, clamp(alpha * u[6], 0.0, 0.9));
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn koan_dot2(v: vec2<f32>) -> f32 {
    return dot(v, v);
}

fn koan_heart(q: vec2<f32>) -> f32 {
    let p = vec2<f32>(abs(q.x), q.y);
    if (p.y + p.x > 1.0) {
        return sqrt(koan_dot2(p - vec2<f32>(0.25, 0.75))) - 0.35355339;
    }
    return sqrt(min(koan_dot2(p - vec2<f32>(0.0, 1.0)), koan_dot2(p - 0.5 * max(p.x + p.y, 0.0)))) * sign(p.x - p.y);
}

fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let s = u[2].z;
    let low = koan_track(u, 0u, time);
    let high = koan_track(u, 2u, time);
    let beat = pow(koan_track(u, 3u, time), 0.6);
    var colour = vec3<f32>(0.0);
    var alpha = 0.0;
    for (var i = 0; i < 6; i++) {
        let fi = f32(i);
        let side = select(-1.0, 1.0, i < 3);
        let origin = vec2<f32>(select(size.x, 0.0, i < 3), -10.0 * s);
        let theta = 0.25 + 0.55 * (0.5 + 0.5 * sin(time * (1.1 + 0.31 * fi) + fi * 2.1));
        let dir = vec2<f32>(sin(theta) * side, cos(theta));
        let rel = position - origin;
        let along = dot(rel, dir);
        if (along > 0.0) {
            let across = abs(rel.x * dir.y - rel.y * dir.x);
            let beam = exp(-across / (1.4 * s)) + 0.35 * exp(-across / (7.0 * s));
            let a = beam * (0.25 + 0.45 * high);
            if (a > 0.0) {
                colour = mix(colour, mix(koan_prism(fi / 6.0 + time * 0.12, u[1].x), vec3<f32>(1.0), 0.25), a / max(alpha + a, 1e-4));
                alpha = max(alpha, a);
            }
        }
    }
    let from_foot = size.y - position.y;
    for (var k = 0; k < 5; k++) {
        let fk = f32(k);
        let cx = size.x * (0.1 + 0.2 * fk);
        let h = size.y * (0.08 + 0.5 * beat) * (0.6 + 0.4 * low) * (0.8 + 0.2 * sin(fk * 3.7 + time * 9.0));
        if (from_foot < h) {
            let rise = from_foot / max(h, 1.0);
            let width = s * (34.0 * (1.0 - rise) + 4.0);
            let flicker = koan_prism_hash(floor(vec2<f32>((position.x - cx) / (5.0 * s), (from_foot - time * 520.0 * s) / (9.0 * s))));
            let flame = (1.0 - smoothstep(width * 0.5, width, abs(position.x - cx))) * (0.55 + 0.45 * flicker);
            let fire = mix(mix(vec3<f32>(1.0, 1.0, 0.85), vec3<f32>(1.0, 0.75, 0.1), smoothstep(0.0, 0.4, rise)), vec3<f32>(1.0, 0.18, 0.6), smoothstep(0.4, 1.0, rise));
            let lime = mix(vec3<f32>(0.85, 1.0, 0.5), vec3<f32>(0.54, 0.81, 0.0), rise);
            let hot = select(fire, lime, u[1].x > 0.5);
            let a = flame * (1.0 - rise * 0.6) * 0.9;
            if (a > 0.0) {
                colour = mix(colour, hot, a / max(alpha + a, 1e-4));
                alpha = max(alpha, a);
            }
        }
    }
    for (var j = 0; j < 14; j++) {
        let fj = f32(j);
        let h1 = koan_prism_hash(vec2<f32>(fj, 1.3));
        let h2 = koan_prism_hash(vec2<f32>(2.9, fj));
        let h3 = koan_prism_hash(vec2<f32>(fj * 0.7, 5.1));
        let radius = s * (16.0 + 22.0 * h2) * (1.0 + 0.15 * beat);
        let travel = size.y + 4.0 * radius;
        let y = size.y + 2.0 * radius - fract(time * (0.04 + 0.05 * h1) + h3) * travel;
        let x = h1 * size.x + sin(time * (0.6 + h2) + fj) * 40.0 * s;
        let p = vec2<f32>(position.x - x, y - position.y) / radius + vec2<f32>(0.0, 0.5);
        if (abs(p.x) < 1.2 && p.y > -0.3 && p.y < 1.3) {
            let d = koan_heart(p);
            let body = 1.0 - smoothstep(-0.02, 0.02, d);
            let shine = (1.0 - smoothstep(0.0, 0.12, length(p - vec2<f32>(-0.28, 0.72)))) * body;
            let c = mix(koan_prism(h3 + time * 0.05, u[1].x), vec3<f32>(1.0, 0.6, 0.8), 0.3);
            let a = body * 0.8;
            if (a > 0.0) {
                colour = mix(colour, mix(c, vec3<f32>(1.0), shine * 0.8), a / max(alpha + a, 1e-4));
                alpha = max(alpha, a);
            }
        }
    }
    return vec4<f32>(colour, clamp(alpha * u[1].z, 0.0, 0.9));
}
"#
    ),
    prepare: None,
};

/// How long a beat takes to fade.
const BEAT_DECAY: Duration = Duration::from_millis(700);
/// The least time between two beats, in seconds: the flashes they bring
/// stay under three a second, the photosensitive limit for flashing.
const MIN_BEAT_GAP: Duration = Duration::from_millis(334);

/// A window's prism: the uniforms the wash layer, the pulse and the mirror
/// ball read, fed by the analyser. Clones share it; keep one per window,
/// since it wakes the window it was last drawn in.
#[derive(Clone)]
pub struct Prism {
    animated: Animated,
    /// When the last beat landed, to keep beats a third of a second apart.
    last_beat: Arc<Mutex<Option<Instant>>>,
    /// Whether its clock runs, and the clocks that run with it: the sheens
    /// and gradients drawn in its window.
    running: Arc<std::sync::atomic::AtomicBool>,
    followers: Arc<Mutex<Vec<super::WeakAnimated>>>,
}

/// The uniform a follower of a prism is told in whether the clock runs: a
/// sheen's glitch shows only then, so one stopped mid-blink as the music
/// pauses does not stay over the title.
const FOLLOWING: usize = 15;

/// Each window's prism, for the sheens and gradients drawn in it to run
/// with: registered by the window's root while the rainbow is drawn.
#[derive(Default)]
pub struct Prisms(HashMap<WindowId, Prism>);

impl Global for Prisms {}

impl Prisms {
    /// `prism` is `window`'s, or with `None` the window has none.
    pub fn set(window: WindowId, prism: Option<Prism>, cx: &mut App) {
        let prisms = cx.default_global::<Prisms>();
        match prism {
            Some(prism) => prisms.0.insert(window, prism),
            None => prisms.0.remove(&window),
        };
    }

    /// Runs `animated` with the clock of the prism of the window being
    /// drawn, or stands it still where the window has none; whether it runs.
    fn follow(animated: &Animated, window: &Window, cx: &App) -> bool {
        let id = window.window_handle().window_id();
        match cx.try_global::<Prisms>().and_then(|p| p.0.get(&id)) {
            Some(prism) => prism.follow(animated),
            None => {
                animated.run(false);
                false
            }
        }
    }
}

impl Default for Prism {
    fn default() -> Self {
        Self::new()
    }
}

impl Prism {
    pub fn new() -> Self {
        let animated = Animated::new();
        animated.set_all(6, &[1.0, 0.0, 1.0]);
        animated.hold(3, 0.0);
        Self {
            animated,
            last_beat: Arc::default(),
            running: Arc::default(),
            followers: Arc::default(),
        }
    }

    /// `animated` runs and stops with this prism's clock while it lasts;
    /// whether the clock runs now.
    fn follow(&self, animated: &Animated) -> bool {
        let mut followers = self.followers.lock();
        if !followers.iter().any(|f| f.is(animated)) {
            followers.push(animated.downgrade());
        }
        let running = self.running.load(std::sync::atomic::Ordering::Relaxed);
        animated.run(running);
        animated.set(FOLLOWING, running as u8 as f32);
        running
    }

    /// A frame from the analyser: the three bands, each 0 to 1. A low band
    /// well over its running average is a beat.
    pub fn hear(&self, bands: [f32; 3], over: Duration) {
        let a = &self.animated;
        let average = a.get(7);
        // Tracks, each gliding to its band over the gap between frames, so
        // the layers move at the display's rate between the analyser's.
        for (i, band) in bands.into_iter().enumerate() {
            a.to(i, band, over, super::Ease::Linear);
        }
        a.set(7, average * 0.92 + bands[0] * 0.08);
        let onset = ((bands[0] - average * 1.25 - 0.04) * 4.0).min(1.0);
        let now = Animated::now();
        let held = a.at(3, now);
        let mut last = self.last_beat.lock();
        let rested = last.is_none_or(|at| now.saturating_duration_since(at) >= MIN_BEAT_GAP);
        if onset > 0.05 && onset > held && rested {
            *last = Some(now);
            // From its strength to nothing, quickly then slowly: the
            // programs take powers of it for sharper decays.
            a.segment(3, onset, 0.0, now, BEAT_DECAY, Ease::QuickOut, Repeat::Once);
        }
    }

    /// The window's one flash gate, which its bursts share (`Burst::sharing`):
    /// a beat and a burst, however they fall, start at most three times a
    /// second between them.
    pub fn gate(&self) -> FlashGate {
        FlashGate(self.last_beat.clone())
    }

    /// The bands standing where they are, for a prism newly fed.
    pub fn hold_bands(&self, bands: [f32; 3]) {
        for (i, band) in bands.into_iter().enumerate() {
            self.animated.hold(i, band);
        }
    }

    /// The appearance, the palette and the window's scale.
    pub fn style(&self, dark: bool, brat: bool, scale: f32) {
        self.animated.set(4, brat as u8 as f32);
        self.animated.set(5, dark as u8 as f32);
        self.animated.set(10, scale);
    }

    /// Whether its clock runs: while the appearance is on, a window of the
    /// app is in front and it shows. Still, it holds its last frame and the
    /// mirror ball stops turning.
    pub fn run(&self, running: bool) {
        self.animated.set(8, running as u8 as f32);
        self.animated.run(running);
        self.running
            .store(running, std::sync::atomic::Ordering::Relaxed);
        self.followers.lock().retain(|f| match f.upgrade() {
            Some(animated) => {
                animated.run(running);
                animated.set(FOLLOWING, running as u8 as f32);
                true
            }
            None => false,
        });
    }

    /// The layer over the wash, under the page.
    pub fn wash(&self) -> AnimatedQuad {
        AnimatedQuad::new(&PRISM_WASH, &self.animated).decorative()
    }

    /// Where the mirror ball hangs, in points in from the window's right
    /// and down from its top, which the disco layer's spots wheel out from.
    pub fn ball_at(&self, right: f32, top: f32) {
        self.animated.set_all(11, &[right, top]);
    }

    /// The disco layer, over the page: spots, beams, glimmers, glitter and
    /// the beat's flash.
    pub fn disco(&self) -> AnimatedQuad {
        AnimatedQuad::new(&DISCO, &self.animated).decorative()
    }

    /// The party over the page: lasers, pyro on the beat, floating hearts.
    pub fn party(&self) -> AnimatedQuad {
        AnimatedQuad::new(&PARTY, &self.animated).decorative()
    }

    /// The beat's glow at the window's edges, over the page.
    pub fn pulse(&self) -> AnimatedQuad {
        AnimatedQuad::new(&PRISM_PULSE, &self.animated).decorative()
    }

    /// The mirror ball, as large as it is laid out.
    pub fn ball(&self) -> AnimatedQuad {
        AnimatedQuad::new(&MIRROR_BALL, &self.animated).decorative()
    }
}

/// A burst's clock before it is fired: past any length, so it draws nothing.
fn spent() -> Animated {
    let animated = Animated::new();
    animated.hold(0, f32::MAX);
    animated
}

/// A burst drawn over the whole window, fired at a point: confetti, or the
/// rainbow sweep between records.
#[derive(Clone)]
pub struct Burst {
    animated: Animated,
    program: &'static ShaderProgram,
    length: Duration,
    /// When a flash last started in its window: the gate it shares with the
    /// window's prism and other bursts (`sharing`).
    fired: Arc<Mutex<Option<Instant>>>,
}

/// The time the last flash started, shared by a window's prism and bursts.
#[derive(Clone, Default)]
pub struct FlashGate(Arc<Mutex<Option<Instant>>>);

impl Burst {
    /// Shares `gate` with the window's prism, so its throws and the beat's
    /// flashes keep one gap between them.
    pub fn sharing(mut self, gate: &FlashGate) -> Self {
        self.fired = gate.0.clone();
        self
    }

    pub fn confetti() -> Self {
        Self {
            animated: spent(),
            program: &CONFETTI,
            length: Duration::from_millis(1800),
            fired: Default::default(),
        }
    }

    /// The sticker: brat's when fired with `brat`, the chrome star
    /// otherwise.
    pub fn sticker() -> Self {
        Self {
            animated: spent(),
            program: &STICKER,
            length: Duration::from_millis(2600),
            fired: Default::default(),
        }
    }

    pub fn sweep() -> Self {
        Self {
            animated: spent(),
            program: &SWEEP,
            length: Duration::from_millis(1300),
            fired: Default::default(),
        }
    }

    /// Throws it from `at`, in the window's coordinates, scaled `scale`
    /// device pixels to a point. Nothing is thrown under Reduce Motion: the
    /// quad is decorative, so its clock does not start. A throw within
    /// `MIN_BEAT_GAP` of the last flash its gate saw, a beat or a burst, is
    /// dropped, so flashes start at most three times a second.
    pub fn fire(&self, at: Point<Pixels>, scale: f32, brat: bool) {
        if self.opens_gate() {
            self.throw(at, scale, brat);
        }
    }

    /// Throws it from `at` and `other` from `other_at` as one start: two
    /// bursts landing together are one flash, so the gate is asked once and
    /// either both fly or neither does. The confetti and the sticker as the
    /// appearance comes out.
    pub fn fire_with(
        &self,
        at: Point<Pixels>,
        other: &Burst,
        other_at: Point<Pixels>,
        scale: f32,
        brat: bool,
    ) {
        if self.opens_gate() {
            self.throw(at, scale, brat);
            other.throw(other_at, scale, brat);
        }
    }

    /// Whether a flash may start now, and if so, that one did.
    fn opens_gate(&self) -> bool {
        let now = Animated::now();
        let mut fired = self.fired.lock();
        if fired.is_some_and(|last| now.saturating_duration_since(last) < MIN_BEAT_GAP) {
            return false;
        }
        *fired = Some(now);
        true
    }

    fn throw(&self, at: Point<Pixels>, scale: f32, brat: bool) {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos() % 997);
        self.animated.set_all(
            0,
            &[
                f32::from(at.x) * scale,
                f32::from(at.y) * scale,
                seed as f32,
                brat as u8 as f32,
                scale,
                self.length.as_secs_f32(),
            ],
        );
        // Its time, as a track from nothing to its length: held at the end
        // once played, so no later present, however its clock moves, plays
        // it again.
        self.animated.segment(
            0,
            0.0,
            self.length.as_secs_f32(),
            Animated::now(),
            self.length,
            Ease::Linear,
            Repeat::Once,
        );
    }

    /// How long it flies once fired.
    pub fn length(&self) -> Duration {
        self.length
    }

    /// Fired and still flying: mounted only then.
    pub fn live(&self) -> bool {
        self.animated.moving()
    }

    /// The quad, laid out over the whole window.
    pub fn element(&self) -> AnimatedQuad {
        AnimatedQuad::new(self.program, &self.animated).decorative()
    }
}

/// A band of the palette passing over its parent while it is mounted: put it
/// in a `relative` element the pointer is on, or over a title.
#[derive(IntoElement)]
pub struct Sheen {
    id: ElementId,
    brat: bool,
    strength: f32,
    glitch: bool,
}

impl Sheen {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            brat: false,
            strength: 0.45,
            glitch: false,
        }
    }

    /// Every five seconds, for a blink, slices of what it covers turn pink
    /// and cyan: the playing title's glitch.
    pub fn glitch(mut self, glitch: bool) -> Self {
        self.glitch = glitch;
        self
    }

    pub fn brat(mut self, brat: bool) -> Self {
        self.brat = brat;
        self
    }

    /// How strongly the band shows, 0 to 1.
    pub fn strength(mut self, strength: f32) -> Self {
        self.strength = strength;
        self
    }
}

impl RenderOnce for Sheen {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Under Reduce Motion nothing is drawn: a band standing still, or a
        // glitch caught mid-blink over a title, is no calmer than one moving.
        if !crate::theme::KoanTheme::motion_allowed(cx) {
            return div().into_any_element();
        }
        let animated = window
            .use_keyed_state(self.id, cx, |_, _| {
                let animated = Animated::new();
                animated.set_origin(Instant::now());
                animated
            })
            .read(cx)
            .clone();
        // Moving only while its window's prism runs: playing, in front and
        // shown. A still sheen holds no glitch: one caught mid-blink as the
        // music pauses would stay over the title (`FOLLOWING`, which the
        // prism writes as it stops, and this as it draws).
        let running = Prisms::follow(&animated, window, cx);
        animated.set_all(
            0,
            &[
                self.brat as u8 as f32,
                self.strength,
                (self.glitch && running) as u8 as f32,
            ],
        );
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(
                AnimatedQuad::new(&SHEEN, &animated)
                    .decorative()
                    .size_full(),
            )
            .into_any_element()
    }
}

/// The palette as a gradient filling its parent, moving while it is drawn:
/// what carries the accent in a component carries this instead while the
/// rainbow is drawn (`KoanTheme::prism_gradient`).
#[derive(IntoElement)]
pub struct PrismFill {
    id: ElementId,
    brat: bool,
}

impl PrismFill {
    pub fn new(id: impl Into<ElementId>, brat: bool) -> Self {
        Self {
            id: id.into(),
            brat,
        }
    }
}

impl RenderOnce for PrismFill {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let animated = window
            .use_keyed_state(self.id, cx, |_, _| {
                let animated = Animated::new();
                animated.set_origin(Instant::now());
                animated
            })
            .read(cx)
            .clone();
        // Moving only while its window's prism runs; still, the gradient
        // stands where it was, as under Reduce Motion.
        Prisms::follow(&animated, window, cx);
        animated.set(0, self.brat as u8 as f32);
        AnimatedQuad::new(&GRADIENT, &animated)
            .decorative()
            .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    /// A burst thrown again inside `MIN_BEAT_GAP` is dropped: holding Next
    /// through a shuffled queue sweeps at most three times a second. Bursts
    /// sharing a prism's gate hold off each other and the beat.
    #[test]
    fn a_burst_starts_at_most_three_times_a_second() {
        let sweep = Burst::sweep();
        let at = Point::new(px(10.), px(10.));
        sweep.fire(at, 2.0, false);
        let first = sweep.animated.track(0);
        std::thread::sleep(Duration::from_millis(20));
        sweep.clone().fire(at, 2.0, false);
        assert_eq!(
            sweep.animated.track(0),
            first,
            "a throw inside the gap is dropped"
        );
        let prism = Prism::new();
        let (confetti, sticker) = (
            Burst::confetti().sharing(&prism.gate()),
            Burst::sticker().sharing(&prism.gate()),
        );
        confetti.fire(at, 2.0, false);
        let before = sticker.animated.track(0);
        sticker.fire(at, 2.0, false);
        assert_eq!(
            sticker.animated.track(0),
            before,
            "one gate for the window's flashes"
        );
    }

    /// The appearance coming out throws the confetti and the sticker as one
    /// start on the window's gate: both fly, where two throws in a row would
    /// drop the second.
    #[test]
    fn a_coming_out_throws_both() {
        let prism = Prism::new();
        let (confetti, sticker) = (
            Burst::confetti().sharing(&prism.gate()),
            Burst::sticker().sharing(&prism.gate()),
        );
        let at = Point::new(px(10.), px(10.));
        let (spent_confetti, spent_sticker) =
            (confetti.animated.track(0), sticker.animated.track(0));
        confetti.fire_with(at, &sticker, at, 2.0, false);
        assert_ne!(
            confetti.animated.track(0),
            spent_confetti,
            "the confetti flies"
        );
        assert_ne!(
            sticker.animated.track(0),
            spent_sticker,
            "the sticker flies"
        );
        assert!(confetti.live() && sticker.live());
    }
}
