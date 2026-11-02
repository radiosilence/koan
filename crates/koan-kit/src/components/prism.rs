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
//! its running average, held as a strength and the moment it landed; the
//! programs decay it themselves, so a beat fades out with nothing written.

use std::time::{Duration, Instant};

use gpui_kit::*;

use super::{Animated, AnimatedQuad, ShaderProgram};

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

// The uniforms every `Prism` program reads: 0..3 the bands (low, middle,
// high, 0 to 1), 3 the last beat's strength, 4 brat's palette, 5 a dark
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
    float pulse = u[3] * exp(-max(time - u[9], 0.0) * 5.0);
    float r = length(p);
    float seg = 6.2831853 / 6.0;
    float a = atan2(p.y, p.x) + time * 0.05 + 100.0 * seg;
    a = abs(fmod(a, seg) - seg * 0.5);
    float2 k = float2(cos(a), sin(a)) * r;
    float v = sin(k.x * 5.0 + time * 0.4) + sin(k.y * 6.0 - time * 0.3)
        + sin((p.x + p.y) * 3.0 + time * 0.25 + u[1] * 2.0)
        + sin(r * 14.0 - time * 1.6 - pulse * 3.0) * (0.4 + u[0]);
    float3 colour = koan_prism(v * 0.12 + time * 0.04 + r * 0.3, u[4]);
    float glow = 0.5 + 0.5 * sin(v * 3.14159);
    float alpha = (u[5] > 0.5 ? 0.26 : 0.18) * (0.55 + 0.45 * glow) * (0.7 + 0.3 * u[2]) + pulse * 0.10;
    if (u[5] < 0.5) {
        colour = mix(colour, float3(1.0), 0.35);
    }
    return float4(colour, clamp(alpha * u[6], 0.0, 0.5));
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
    let p = (position - size * 0.5) / size.y;
    let pulse = u[0].w * exp(-max(time - u[2].y, 0.0) * 5.0);
    let r = length(p);
    let seg = 6.2831853 / 6.0;
    var a = atan2(p.y, p.x) + time * 0.05 + 100.0 * seg;
    a = abs(a % seg - seg * 0.5);
    let k = vec2<f32>(cos(a), sin(a)) * r;
    let v = sin(k.x * 5.0 + time * 0.4) + sin(k.y * 6.0 - time * 0.3)
        + sin((p.x + p.y) * 3.0 + time * 0.25 + u[0].y * 2.0)
        + sin(r * 14.0 - time * 1.6 - pulse * 3.0) * (0.4 + u[0].x);
    var colour = koan_prism(v * 0.12 + time * 0.04 + r * 0.3, u[1].x);
    let glow = 0.5 + 0.5 * sin(v * 3.14159);
    let base = select(0.18, 0.26, u[1].y > 0.5);
    let alpha = base * (0.55 + 0.45 * glow) * (0.7 + 0.3 * u[0].z) + pulse * 0.10;
    if (u[1].y < 0.5) {
        colour = mix(colour, vec3<f32>(1.0), 0.35);
    }
    return vec4<f32>(colour, clamp(alpha * u[1].z, 0.0, 0.5));
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
    float pulse = u[3] * exp(-max(time - u[9], 0.0) * 4.0);
    float width = u[10] * (10.0 + 30.0 * pulse + 8.0 * u[0]);
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
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
    let d = min(min(position.x, size.x - position.x), min(position.y, size.y - position.y));
    let pulse = u[0].w * exp(-max(time - u[2].y, 0.0) * 4.0);
    let width = u[2].z * (10.0 + 30.0 * pulse + 8.0 * u[0].x);
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
    float pulse = u[3] * exp(-max(time - u[9], 0.0) * 5.0);
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
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
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
    let pulse = u[0].w * exp(-max(time - u[2].y, 0.0) * 5.0);
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
    float top = size.y * (1.0 - max(u[i], u[10]));
    float cover = clamp(position.y - top + 0.5, 0.0, 1.0);
    float up = 1.0 - position.y / size.y;
    float3 colour = koan_prism(float(i) / 3.0 + up * 0.4 + u[i] * 0.2, u[12]);
    return float4(colour, u[7] * cover);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
    let pitch = u[2].x + u[2].y;
    let i = i32(floor(position.x / pitch));
    if (i < 0 || i > 2 || position.x - f32(i) * pitch > u[2].x) {
        return vec4<f32>(0.0);
    }
    var bands = u[0];
    let top = size.y * (1.0 - max(bands[i], u[2].z));
    let cover = clamp(position.y - top + 0.5, 0.0, 1.0);
    let up = 1.0 - position.y / size.y;
    let colour = koan_prism(f32(i) / 3.0 + up * 0.4 + bands[i] * 0.2, u[3].x);
    return vec4<f32>(colour, u[1].w * cover);
}
"#
    ),
    prepare: None,
};

/// The slider's fill as a rainbow trail running back from the playhead,
/// with a glint at its head: `GLIDE`'s uniforms, the fill's colour giving
/// only its opacity.
pub(crate) static PRISM_GLIDE: ShaderProgram = ShaderProgram {
    name: "koan-prism-glide",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float width = size.x - 2.0 * u[4];
    float x = position.x - u[4];
    float easing = u[7] > 0.0 ? 1.0 - smoothstep(0.0, 1.0, (time - u[6]) / u[7]) : 0.0;
    float end = clamp(u[0] + u[1] * time + u[5] * easing, 0.0, 1.0) * width;
    float dy = abs(position.y - size.y * 0.5);
    if (u[3] > 0.0 && abs(x - end) <= u[3] * 0.5 && dy <= u[3] * 0.5) {
        return float4(u[12], u[13], u[14], u[15]);
    }
    if (x < 0.0 || dy > u[2] * 0.5) {
        return float4(0.0);
    }
    float cover = clamp(end - x + 0.5, 0.0, 1.0);
    float3 colour = koan_prism(x / max(width, 1.0) * 0.9 - time * 0.12, 0.0);
    float glint = exp(-max(end - x, 0.0) / max(u[2] * 4.0, 1.0));
    return float4(mix(colour, float3(1.0), glint * 0.6), u[11] * cover);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
    let width = size.x - 2.0 * u[1].x;
    let x = position.x - u[1].x;
    var easing = 0.0;
    if (u[1].w > 0.0) {
        easing = 1.0 - smoothstep(0.0, 1.0, (time - u[1].z) / u[1].w);
    }
    let end = clamp(u[0].x + u[0].y * time + u[1].y * easing, 0.0, 1.0) * width;
    let dy = abs(position.y - size.y * 0.5);
    if (u[0].w > 0.0 && abs(x - end) <= u[0].w * 0.5 && dy <= u[0].w * 0.5) {
        return u[3];
    }
    if (x < 0.0 || dy > u[0].z * 0.5) {
        return vec4<f32>(0.0);
    }
    let cover = clamp(end - x + 0.5, 0.0, 1.0);
    let colour = koan_prism(x / max(width, 1.0) * 0.9 - time * 0.12, 0.0);
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
    float t = time;
    if (t < 0.0 || t > u[5]) {
        return float4(0.0);
    }
    float s = u[4];
    float2 c = float2(u[0], u[1]);
    if (distance(position, c) > s * (520.0 + 260.0 * t * t)) {
        return float4(0.0);
    }
    float fade = 1.0 - smoothstep(u[5] * 0.6, u[5], t);
    float4 colour = float4(0.0);
    for (int i = 0; i < 56; i++) {
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
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
    let t = time;
    if (t < 0.0 || t > u[1].y) {
        return vec4<f32>(0.0);
    }
    let s = u[1].x;
    let c = u[0].xy;
    if (distance(position, c) > s * (520.0 + 260.0 * t * t)) {
        return vec4<f32>(0.0);
    }
    let fade = 1.0 - smoothstep(u[1].y * 0.6, u[1].y, t);
    var colour = vec4<f32>(0.0);
    for (var i = 0; i < 56; i++) {
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
    float t = time / u[5];
    if (t < 0.0 || t > 1.0) {
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
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
    let t = time / u[1].y;
    if (t < 0.0 || t > 1.0) {
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
/// Uniforms: 0 brat's palette, 1 the strength.
static SHEEN: ShaderProgram = ShaderProgram {
    name: "koan-prism-sheen",
    msl: concat!(
        prism_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float s = (position.x + position.y * 0.6) / (size.x + size.y);
    float sweep = fract(s * 0.8 - time * 0.35);
    float band = smoothstep(0.0, 0.25, sweep) * (1.0 - smoothstep(0.25, 0.55, sweep));
    float3 colour = mix(koan_prism(s * 1.5 + time * 0.2, u[0]), float3(1.0), 0.3);
    return float4(colour, band * u[1]);
}
"#
    ),
    wgsl: concat!(
        prism_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
    let s = (position.x + position.y * 0.6) / (size.x + size.y);
    let sweep = fract(s * 0.8 - time * 0.35);
    let band = smoothstep(0.0, 0.25, sweep) * (1.0 - smoothstep(0.25, 0.55, sweep));
    let colour = mix(koan_prism(s * 1.5 + time * 0.2, u[0].x), vec3<f32>(1.0), 0.3);
    return vec4<f32>(colour, band * u[0].y);
}
"#
    ),
    prepare: None,
};

/// How long a beat holds its strength, as the programs decay it: e^-5t.
const BEAT_DECAY: f32 = 5.0;

/// A window's prism: the uniforms the wash layer, the pulse and the mirror
/// ball read, fed by the analyser. Clones share it; keep one per window,
/// since it wakes the window it was last drawn in.
#[derive(Clone)]
pub struct Prism {
    animated: Animated,
    origin: Instant,
}

impl Default for Prism {
    fn default() -> Self {
        Self::new()
    }
}

impl Prism {
    pub fn new() -> Self {
        let animated = Animated::new();
        let origin = Instant::now();
        animated.set_origin(origin);
        animated.set_all(6, &[1.0, 0.0, 1.0]);
        Self { animated, origin }
    }

    /// A frame from the analyser: the three bands, each 0 to 1. A low band
    /// well over its running average is a beat.
    pub fn hear(&self, bands: [f32; 3]) {
        let a = &self.animated;
        let average = a.get(7);
        a.set_all(0, &bands);
        a.set(7, average * 0.92 + bands[0] * 0.08);
        let onset = ((bands[0] - average * 1.25 - 0.04) * 4.0).min(1.0);
        let now = self.origin.elapsed().as_secs_f32();
        let held = a.get(3) * (-(now - a.get(9)).max(0.0) * BEAT_DECAY).exp();
        if onset > 0.05 && onset > held {
            a.set(9, now);
            a.set(3, onset);
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
    }

    /// The layer over the wash, under the page.
    pub fn wash(&self) -> AnimatedQuad {
        AnimatedQuad::new(&PRISM_WASH, &self.animated).decorative()
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

/// A burst drawn over the whole window, fired at a point: confetti, or the
/// rainbow sweep between records.
#[derive(Clone)]
pub struct Burst {
    animated: Animated,
    program: &'static ShaderProgram,
    length: Duration,
}

impl Burst {
    pub fn confetti() -> Self {
        Self {
            animated: Animated::new(),
            program: &CONFETTI,
            length: Duration::from_millis(1800),
        }
    }

    pub fn sweep() -> Self {
        Self {
            animated: Animated::new(),
            program: &SWEEP,
            length: Duration::from_millis(1300),
        }
    }

    /// Throws it from `at`, in the window's coordinates, scaled `scale`
    /// device pixels to a point. Nothing is thrown under Reduce Motion: the
    /// quad is decorative, so its clock does not start.
    pub fn fire(&self, at: Point<Pixels>, scale: f32, brat: bool) {
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
        self.animated.set_origin(Instant::now());
        self.animated.run_for(self.length);
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
}

impl Sheen {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            brat: false,
            strength: 0.28,
        }
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
        let animated = window
            .use_keyed_state(self.id, cx, |_, _| {
                let animated = Animated::new();
                animated.set_origin(Instant::now());
                animated.run(true);
                animated
            })
            .read(cx)
            .clone();
        animated.set_all(0, &[self.brat as u8 as f32, self.strength]);
        div().absolute().top_0().left_0().size_full().child(
            AnimatedQuad::new(&SHEEN, &animated)
                .decorative()
                .size_full(),
        )
    }
}
