//! The chart's fragment programs. Each draws one mark over the plot from
//! its image (`data.rs`) and its tracks, so a series easing to new values,
//! an axis rescaling and a point settling are all worked out at present
//! time, with no frame drawn.
//!
//! Every program reads the axes as tracks in its own box's frame: a value
//! stored as share `s` of its box stands at `A * s + B` of the plot's width
//! (x) or height (y, from the foot). Tracks 1 to 4 are `Ax`, `Bx`, `Ay` and
//! `By`; track 0, where a program has data, is the phase between the
//! keyframe's `from` and `to` rows.

use crate::components::ShaderProgram;

/// The helpers every chart program shares, in MSL.
macro_rules! chart_msl {
    () => {
        r#"
// A value stored by `data::encode`: 24 bits across blue, green and red.
float koan_chart_value(float4 c) {
    return (round(c.b * 255.0) * 65536.0 + round(c.g * 255.0) * 256.0 + round(c.r * 255.0)) / 16777215.0;
}

// Texel `i` of row `row`, of an image `n` texels wide and `rows` high.
float koan_chart_read(texture2d<float> image, float4 tile, float n, float rows, float i, float row) {
    return koan_chart_value(koan_sample(image, tile, float2((i + 0.5) / n, (row + 0.5) / rows)));
}

// Sample `i` of a series, in device pixels from the plot's top left.
float2 koan_chart_at(texture2d<float> image, float4 tile, float n, float i, float phase, float4 m, float2 size) {
    float x = koan_chart_read(image, tile, n, 5.0, i, 0.0);
    float y0 = koan_chart_read(image, tile, n, 5.0, i, 1.0);
    float y1 = koan_chart_read(image, tile, n, 5.0, i, 2.0);
    float y = y0 + (y1 - y0) * phase;
    return float2((m.x * x + m.y) * size.x, (1.0 - (m.z * y + m.w)) * size.y);
}

float koan_chart_x(texture2d<float> image, float4 tile, float n, float i, float4 m, float2 size) {
    return (m.x * koan_chart_read(image, tile, n, 5.0, i, 0.0) + m.y) * size.x;
}

// The auxiliary row at sample `i`, as drawn.
float koan_chart_aux(texture2d<float> image, float4 tile, float n, float i, float phase) {
    float a0 = koan_chart_read(image, tile, n, 5.0, i, 3.0);
    float a1 = koan_chart_read(image, tile, n, 5.0, i, 4.0);
    return a0 + (a1 - a0) * phase;
}

// The last sample at or left of `x`, by bisection: samples rise in x.
float koan_chart_find(texture2d<float> image, float4 tile, float n, float4 m, float2 size, float x) {
    float lo = 0.0;
    float hi = n - 1.0;
    for (int k = 0; k < 16; k++) {
        if (lo >= hi) {
            break;
        }
        float mid = floor((lo + hi + 1.0) * 0.5);
        if (koan_chart_x(image, tile, n, mid, m, size) <= x) {
            lo = mid;
        } else {
            hi = mid - 1.0;
        }
    }
    return lo;
}
"#
    };
}

/// The same helpers in WGSL.
macro_rules! chart_wgsl {
    () => {
        r#"
fn koan_chart_value(c: vec4<f32>) -> f32 {
    return (round(c.b * 255.0) * 65536.0 + round(c.g * 255.0) * 256.0 + round(c.r * 255.0)) / 16777215.0;
}

fn koan_chart_read(tile: vec4<f32>, n: f32, rows: f32, i: f32, row: f32) -> f32 {
    return koan_chart_value(koan_sample(tile, vec2<f32>((i + 0.5) / n, (row + 0.5) / rows)));
}

fn koan_chart_at(tile: vec4<f32>, n: f32, i: f32, phase: f32, m: vec4<f32>, size: vec2<f32>) -> vec2<f32> {
    let x = koan_chart_read(tile, n, 5.0, i, 0.0);
    let y0 = koan_chart_read(tile, n, 5.0, i, 1.0);
    let y1 = koan_chart_read(tile, n, 5.0, i, 2.0);
    let y = y0 + (y1 - y0) * phase;
    return vec2<f32>((m.x * x + m.y) * size.x, (1.0 - (m.z * y + m.w)) * size.y);
}

fn koan_chart_x(tile: vec4<f32>, n: f32, i: f32, m: vec4<f32>, size: vec2<f32>) -> f32 {
    return (m.x * koan_chart_read(tile, n, 5.0, i, 0.0) + m.y) * size.x;
}

fn koan_chart_aux(tile: vec4<f32>, n: f32, i: f32, phase: f32) -> f32 {
    let a0 = koan_chart_read(tile, n, 5.0, i, 3.0);
    let a1 = koan_chart_read(tile, n, 5.0, i, 4.0);
    return a0 + (a1 - a0) * phase;
}

fn koan_chart_find(tile: vec4<f32>, n: f32, m: vec4<f32>, size: vec2<f32>, x: f32) -> f32 {
    var lo = 0.0;
    var hi = n - 1.0;
    for (var k = 0; k < 16; k++) {
        if (lo >= hi) {
            break;
        }
        let mid = floor((lo + hi + 1.0) * 0.5);
        if (koan_chart_x(tile, n, mid, m, size) <= x) {
            lo = mid;
        } else {
            hi = mid - 1.0;
        }
    }
    return lo;
}

fn koan_chart_axes(u: array<vec4<f32>, 20>, time: f32) -> vec4<f32> {
    return vec4<f32>(koan_track(u, 1u, time), koan_track(u, 2u, time), koan_track(u, 3u, time), koan_track(u, 4u, time));
}
"#
    };
}

/// A series' line. Uniforms: 0..4 the colour, 4 half the stroke's width
/// and 6 the line's whole length, both in device pixels, 5 the samples;
/// 8..12 the dash, on and off lengths in device pixels (all zero for a
/// solid line). Rows 3 and 4 hold how far along the line each sample is,
/// as a share of its whole length.
pub static LINE: ShaderProgram = ShaderProgram {
    name: "koan-chart-line",
    msl: concat!(
        chart_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float n = u[5];
    if (n < 2.0) {
        return float4(0.0);
    }
    float phase = koan_track(u, 0, time);
    float4 m = float4(koan_track(u, 1, time), koan_track(u, 2, time), koan_track(u, 3, time), koan_track(u, 4, time));
    float half_width = u[4];
    float reach = half_width + 1.0;
    if (koan_chart_x(image, tile, n, 0.0, m, size) > position.x + reach
        || koan_chart_x(image, tile, n, n - 1.0, m, size) < position.x - reach) {
        return float4(0.0);
    }
    float period = u[8] + u[9] + u[10] + u[11];
    float i = koan_chart_find(image, tile, n, m, size, position.x - reach);
    float2 a = koan_chart_at(image, tile, n, i, phase, m, size);
    float arc_a = period > 0.0 ? koan_chart_aux(image, tile, n, i, phase) * u[6] : 0.0;
    float best = 1e9;
    float along = 0.0;
    for (int k = 0; k < 64; k++) {
        float j = i + float(k) + 1.0;
        if (j > n - 1.0 || a.x > position.x + reach) {
            break;
        }
        float2 b = koan_chart_at(image, tile, n, j, phase, m, size);
        float arc_b = period > 0.0 ? koan_chart_aux(image, tile, n, j, phase) * u[6] : 0.0;
        float2 ab = b - a;
        float len2 = dot(ab, ab);
        float t = len2 > 0.0 ? clamp(dot(position - a, ab) / len2, 0.0, 1.0) : 0.0;
        float d = length(position - (a + ab * t));
        if (d < best) {
            best = d;
            along = arc_a + (arc_b - arc_a) * t;
        }
        a = b;
        arc_a = arc_b;
    }
    float cover = clamp(half_width + 0.5 - best, 0.0, 1.0);
    if (cover <= 0.0) {
        return float4(0.0);
    }
    if (period > 0.0) {
        float s = fmod(max(along, 0.0), period);
        float first = min(s, u[8] - s);
        float second = u[10] > 0.0 ? min(s - u[8] - u[9], u[8] + u[9] + u[10] - s) : -1e9;
        float inside = max(max(first, second), s - period);
        cover *= clamp(inside + 0.5, 0.0, 1.0);
    }
    return float4(u[0], u[1], u[2], u[3] * cover);
}
"#
    ),
    wgsl: concat!(
        chart_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let n = u[1].y;
    if (n < 2.0) {
        return vec4<f32>(0.0);
    }
    let phase = koan_track(u, 0u, time);
    let m = koan_chart_axes(u, time);
    let half_width = u[1].x;
    let reach = half_width + 1.0;
    if (koan_chart_x(tile, n, 0.0, m, size) > position.x + reach
        || koan_chart_x(tile, n, n - 1.0, m, size) < position.x - reach) {
        return vec4<f32>(0.0);
    }
    let dash = u[2];
    let period = dash.x + dash.y + dash.z + dash.w;
    let i = koan_chart_find(tile, n, m, size, position.x - reach);
    var a = koan_chart_at(tile, n, i, phase, m, size);
    var arc_a = 0.0;
    if (period > 0.0) {
        arc_a = koan_chart_aux(tile, n, i, phase) * u[1].z;
    }
    var best = 1e9;
    var along = 0.0;
    for (var k = 0; k < 64; k++) {
        let j = i + f32(k) + 1.0;
        if (j > n - 1.0 || a.x > position.x + reach) {
            break;
        }
        let b = koan_chart_at(tile, n, j, phase, m, size);
        var arc_b = 0.0;
        if (period > 0.0) {
            arc_b = koan_chart_aux(tile, n, j, phase) * u[1].z;
        }
        let ab = b - a;
        let len2 = dot(ab, ab);
        var t = 0.0;
        if (len2 > 0.0) {
            t = clamp(dot(position - a, ab) / len2, 0.0, 1.0);
        }
        let d = length(position - (a + ab * t));
        if (d < best) {
            best = d;
            along = arc_a + (arc_b - arc_a) * t;
        }
        a = b;
        arc_a = arc_b;
    }
    var cover = clamp(half_width + 0.5 - best, 0.0, 1.0);
    if (cover <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (period > 0.0) {
        let p = max(along, 0.0);
        let s = p - period * floor(p / period);
        let first = min(s, dash.x - s);
        var second = -1e9;
        if (dash.z > 0.0) {
            second = min(s - dash.x - dash.y, dash.x + dash.y + dash.z - s);
        }
        let inside = max(max(first, second), s - period);
        cover *= clamp(inside + 0.5, 0.0, 1.0);
    }
    return vec4<f32>(u[0].xyz, u[0].w * cover);
}
"#
    ),
    prepare: None,
};

/// A series' area, from its line to a baseline. Uniforms: 0..4 the
/// colour, 5 the samples, 6 whether the baseline is the image's rows 3 and
/// 4 (another line) rather than 7, a share of the y box (a level).
pub static AREA: ShaderProgram = ShaderProgram {
    name: "koan-chart-area",
    msl: concat!(
        chart_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float n = u[5];
    if (n < 2.0) {
        return float4(0.0);
    }
    float phase = koan_track(u, 0, time);
    float4 m = float4(koan_track(u, 1, time), koan_track(u, 2, time), koan_track(u, 3, time), koan_track(u, 4, time));
    if (position.x < koan_chart_x(image, tile, n, 0.0, m, size)
        || position.x > koan_chart_x(image, tile, n, n - 1.0, m, size)) {
        return float4(0.0);
    }
    float i = min(koan_chart_find(image, tile, n, m, size, position.x), n - 2.0);
    float2 a = koan_chart_at(image, tile, n, i, phase, m, size);
    float2 b = koan_chart_at(image, tile, n, i + 1.0, phase, m, size);
    float t = b.x > a.x ? clamp((position.x - a.x) / (b.x - a.x), 0.0, 1.0) : 0.0;
    float line = a.y + (b.y - a.y) * t;
    float base_share = u[7];
    if (u[6] > 0.5) {
        float b0 = koan_chart_aux(image, tile, n, i, phase);
        float b1 = koan_chart_aux(image, tile, n, i + 1.0, phase);
        base_share = b0 + (b1 - b0) * t;
    }
    float base = (1.0 - (m.z * base_share + m.w)) * size.y;
    float top = min(line, base);
    float bottom = max(line, base);
    float cover = clamp(min(position.y - top, bottom - position.y) + 0.5, 0.0, 1.0);
    return float4(u[0], u[1], u[2], u[3] * cover);
}
"#
    ),
    wgsl: concat!(
        chart_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let n = u[1].y;
    if (n < 2.0) {
        return vec4<f32>(0.0);
    }
    let phase = koan_track(u, 0u, time);
    let m = koan_chart_axes(u, time);
    if (position.x < koan_chart_x(tile, n, 0.0, m, size)
        || position.x > koan_chart_x(tile, n, n - 1.0, m, size)) {
        return vec4<f32>(0.0);
    }
    let i = min(koan_chart_find(tile, n, m, size, position.x), n - 2.0);
    let a = koan_chart_at(tile, n, i, phase, m, size);
    let b = koan_chart_at(tile, n, i + 1.0, phase, m, size);
    var t = 0.0;
    if (b.x > a.x) {
        t = clamp((position.x - a.x) / (b.x - a.x), 0.0, 1.0);
    }
    let line = a.y + (b.y - a.y) * t;
    var base_share = u[1].w;
    if (u[1].z > 0.5) {
        let b0 = koan_chart_aux(tile, n, i, phase);
        let b1 = koan_chart_aux(tile, n, i + 1.0, phase);
        base_share = b0 + (b1 - b0) * t;
    }
    let base = (1.0 - (m.z * base_share + m.w)) * size.y;
    let top = min(line, base);
    let bottom = max(line, base);
    let cover = clamp(min(position.y - top, bottom - position.y) + 0.5, 0.0, 1.0);
    return vec4<f32>(u[0].xyz, u[0].w * cover);
}
"#
    ),
    prepare: None,
};

/// The grid: a rule at each tick of both axes. Uniforms: 0..4 the colour,
/// 4 a rule's width in device pixels, 5 and 6 the x and y ticks, 7 the
/// image's width. Row 0 of the image holds the x ticks as shares of the x
/// box, row 1 the y ticks of the y box.
pub static GRID: ShaderProgram = ShaderProgram {
    name: "koan-chart-grid",
    msl: concat!(
        chart_msl!(),
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float4 m = float4(koan_track(u, 1, time), koan_track(u, 2, time), koan_track(u, 3, time), koan_track(u, 4, time));
    float half_width = u[4] * 0.5;
    float cover = 0.0;
    for (int k = 0; k < 64; k++) {
        if (float(k) >= u[5]) {
            break;
        }
        float x = (m.x * koan_chart_read(image, tile, u[7], 2.0, float(k), 0.0) + m.y) * size.x;
        cover = max(cover, clamp(half_width + 0.5 - abs(position.x - x), 0.0, 1.0));
    }
    for (int k = 0; k < 64; k++) {
        if (float(k) >= u[6]) {
            break;
        }
        float y = (1.0 - (m.z * koan_chart_read(image, tile, u[7], 2.0, float(k), 1.0) + m.w)) * size.y;
        cover = max(cover, clamp(half_width + 0.5 - abs(position.y - y), 0.0, 1.0));
    }
    return float4(u[0], u[1], u[2], u[3] * cover);
}
"#
    ),
    wgsl: concat!(
        chart_wgsl!(),
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let m = koan_chart_axes(u, time);
    let half_width = u[1].x * 0.5;
    var cover = 0.0;
    for (var k = 0; k < 64; k++) {
        if (f32(k) >= u[1].y) {
            break;
        }
        let x = (m.x * koan_chart_read(tile, u[1].w, 2.0, f32(k), 0.0) + m.y) * size.x;
        cover = max(cover, clamp(half_width + 0.5 - abs(position.x - x), 0.0, 1.0));
    }
    for (var k = 0; k < 64; k++) {
        if (f32(k) >= u[1].z) {
            break;
        }
        let y = (1.0 - (m.z * koan_chart_read(tile, u[1].w, 2.0, f32(k), 1.0) + m.w)) * size.y;
        cover = max(cover, clamp(half_width + 0.5 - abs(position.y - y), 0.0, 1.0));
    }
    return vec4<f32>(u[0].xyz, u[0].w * cover);
}
"#
    ),
    prepare: None,
};

/// A point. Uniforms: 0..4 the colour, 4 how far the quad reaches past the
/// plot on each side, in device pixels. Tracks: 0 and 1 where it stands, as
/// shares of the plot's width and height (from the foot), 2 its radius in
/// device pixels.
pub static DOT: ShaderProgram = ShaderProgram {
    name: "koan-chart-dot",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float pad = u[4];
    float2 plot = size - 2.0 * pad;
    float2 centre = float2(koan_track(u, 0, time) * plot.x, (1.0 - koan_track(u, 1, time)) * plot.y) + pad;
    float cover = clamp(koan_track(u, 2, time) + 0.5 - length(position - centre), 0.0, 1.0);
    return float4(u[0], u[1], u[2], u[3] * cover);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let pad = u[1].x;
    let plot = size - 2.0 * pad;
    let centre = vec2<f32>(koan_track(u, 0u, time) * plot.x, (1.0 - koan_track(u, 1u, time)) * plot.y) + pad;
    let cover = clamp(koan_track(u, 2u, time) + 0.5 - length(position - centre), 0.0, 1.0);
    return vec4<f32>(u[0].xyz, u[0].w * cover);
}
"#,
    prepare: None,
};
