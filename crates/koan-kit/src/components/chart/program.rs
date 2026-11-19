//! The chart's fragment programs. Each draws one mark over the plot from
//! its image (`data.rs`) and its tracks, so a series easing to new values,
//! an axis rescaling and a point settling are all worked out at present
//! time, with no frame drawn.
//!
//! Every program reads the axes as tracks in its own box's frame: a value
//! stored as share `s` of its box stands at `A * s + B` of the plot's width
//! (x) or height (y, from the foot). Tracks 1 to 4 are `Ax`, `Bx`, `Ay` and
//! `By`; track 0, where a program has data, is the phase between the
//! keyframe's `from` and `to` rows. A line or area that scrolls reads
//! track 5 as a shift added to `Bx`, the share of the plot's width its
//! samples stand right of their places, and draws a smooth curve through
//! them rather than straight pieces.
//!
//! A mark's quad covers only the part of its frame (the plot, or the plot
//! grown by the points' reach) the mark can touch until it is laid out
//! again, so a band's area costs its band and not the plot. Uniforms 12 and
//! 13 are where the quad stands in that frame and 14 and 15 the frame's
//! size, all in device pixels, and `shade` hands the mark positions in the
//! whole frame (`chart_place_msl!`, `chart_place_wgsl!`).

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

// A monotone cubic's tangent at `b` (Fritsch and Butland): flat at a peak
// or a trough, so the curve rounds them off without passing beyond them.
float koan_chart_tangent(float a, float b, float c) {
    float d0 = b - a;
    float d1 = c - b;
    return d0 * d1 <= 0.0 ? 0.0 : 2.0 * d0 * d1 / (d0 + d1);
}

// Rows `row` and `row + 1` at sample `i`, held at the ends, as drawn.
float koan_chart_mixed(texture2d<float> image, float4 tile, float n, float i, float phase, float row) {
    i = clamp(i, 0.0, n - 1.0);
    float a = koan_chart_read(image, tile, n, 5.0, i, row);
    float b = koan_chart_read(image, tile, n, 5.0, i, row + 1.0);
    return a + (b - a) * phase;
}

// The smooth curve through rows `row` and `row + 1`, `t` of the way from
// sample `i` to the next: a cubic Hermite piece on monotone tangents.
float koan_chart_smooth(texture2d<float> image, float4 tile, float n, float i, float t, float phase, float row) {
    float a = koan_chart_mixed(image, tile, n, i - 1.0, phase, row);
    float b = koan_chart_mixed(image, tile, n, i, phase, row);
    float c = koan_chart_mixed(image, tile, n, i + 1.0, phase, row);
    float d = koan_chart_mixed(image, tile, n, i + 2.0, phase, row);
    float m1 = koan_chart_tangent(a, b, c);
    float m2 = koan_chart_tangent(b, c, d);
    float t2 = t * t;
    float t3 = t2 * t;
    return (2.0 * t3 - 3.0 * t2 + 1.0) * b + (t3 - 2.0 * t2 + t) * m1
        + (3.0 * t2 - 2.0 * t3) * c + (t3 - t2) * m2;
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

fn koan_chart_tangent(a: f32, b: f32, c: f32) -> f32 {
    let d0 = b - a;
    let d1 = c - b;
    if (d0 * d1 <= 0.0) {
        return 0.0;
    }
    return 2.0 * d0 * d1 / (d0 + d1);
}

fn koan_chart_mixed(tile: vec4<f32>, n: f32, i: f32, phase: f32, row: f32) -> f32 {
    let j = clamp(i, 0.0, n - 1.0);
    let a = koan_chart_read(tile, n, 5.0, j, row);
    let b = koan_chart_read(tile, n, 5.0, j, row + 1.0);
    return a + (b - a) * phase;
}

fn koan_chart_smooth(tile: vec4<f32>, n: f32, i: f32, t: f32, phase: f32, row: f32) -> f32 {
    let a = koan_chart_mixed(tile, n, i - 1.0, phase, row);
    let b = koan_chart_mixed(tile, n, i, phase, row);
    let c = koan_chart_mixed(tile, n, i + 1.0, phase, row);
    let d = koan_chart_mixed(tile, n, i + 2.0, phase, row);
    let m1 = koan_chart_tangent(a, b, c);
    let m2 = koan_chart_tangent(b, c, d);
    let t2 = t * t;
    let t3 = t2 * t;
    return (2.0 * t3 - 3.0 * t2 + 1.0) * b + (t3 - 2.0 * t2 + t) * m1
        + (3.0 * t2 - 2.0 * t3) * c + (t3 - t2) * m2;
}
"#
    };
}

/// `shade` for a mark drawn by `koan_chart_mark`, given positions in its
/// whole frame rather than its quad.
macro_rules! chart_place_msl {
    () => {
        r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    if (u[14] > 0.0) {
        position += float2(u[12], u[13]);
        size = float2(u[14], u[15]);
    }
    return koan_chart_mark(position, size, time, u, image, tile);
}
"#
    };
}

macro_rules! chart_place_wgsl {
    () => {
        r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    var p = position;
    var s = size;
    if (u[3].z > 0.0) {
        p = position + u[3].xy;
        s = u[3].zw;
    }
    return koan_chart_mark(p, s, time, u, tile);
}
"#
    };
}

/// A series' line. Uniforms: 0..4 the colour, 4 half the stroke's width
/// and 6 the line's whole length, both in device pixels, 5 the samples;
/// 8..12 the dash, on and off lengths in device pixels (all zero for a
/// solid line), 7 whether it is drawn smooth. Rows 3 and 4 hold how far along the line each sample is,
/// as a share of its whole length.
pub static LINE: ShaderProgram = ShaderProgram {
    name: "koan-chart-line",
    msl: concat!(
        chart_msl!(),
        r#"
float4 koan_chart_mark(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float n = u[5];
    if (n < 2.0) {
        return float4(0.0);
    }
    float phase = koan_track(u, 0, time);
    float4 m = float4(koan_track(u, 1, time), koan_track(u, 2, time) + koan_track(u, 5, time), koan_track(u, 3, time), koan_track(u, 4, time));
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
        // A smooth line is followed in four straight pieces a sample.
        int pieces = u[7] > 0.5 ? 4 : 1;
        float2 p = a;
        for (int piece = 1; piece <= pieces; piece++) {
            float2 q = b;
            if (pieces > 1) {
                float tt = float(piece) / float(pieces);
                float y = koan_chart_smooth(image, tile, n, j - 1.0, tt, phase, 1.0);
                q = float2(a.x + (b.x - a.x) * tt, (1.0 - (m.z * y + m.w)) * size.y);
            }
            float2 pq = q - p;
            float len2 = dot(pq, pq);
            float t = len2 > 0.0 ? clamp(dot(position - p, pq) / len2, 0.0, 1.0) : 0.0;
            float d = length(position - (p + pq * t));
            if (d < best) {
                best = d;
                along = arc_a + (arc_b - arc_a) * (float(piece - 1) + t) / float(pieces);
            }
            p = q;
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
"#,
        chart_place_msl!()
    ),
    wgsl: concat!(
        chart_wgsl!(),
        r#"
fn koan_chart_mark(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let n = u[1].y;
    if (n < 2.0) {
        return vec4<f32>(0.0);
    }
    let phase = koan_track(u, 0u, time);
    var m = koan_chart_axes(u, time);
    m.y = m.y + koan_track(u, 5u, time);
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
        var pieces = 1;
        if (u[1].w > 0.5) {
            pieces = 4;
        }
        var p = a;
        for (var piece = 1; piece <= pieces; piece++) {
            var q = b;
            if (pieces > 1) {
                let tt = f32(piece) / f32(pieces);
                let y = koan_chart_smooth(tile, n, j - 1.0, tt, phase, 1.0);
                q = vec2<f32>(a.x + (b.x - a.x) * tt, (1.0 - (m.z * y + m.w)) * size.y);
            }
            let pq = q - p;
            let len2 = dot(pq, pq);
            var t = 0.0;
            if (len2 > 0.0) {
                t = clamp(dot(position - p, pq) / len2, 0.0, 1.0);
            }
            let d = length(position - (p + pq * t));
            if (d < best) {
                best = d;
                along = arc_a + (arc_b - arc_a) * (f32(piece - 1) + t) / f32(pieces);
            }
            p = q;
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
"#,
        chart_place_wgsl!()
    ),
    prepare: None,
};

/// A series' area, from its line to a baseline. Uniforms: 0..4 the
/// colour, 4 whether its edges are drawn smooth, 5 the samples, 6 whether the baseline is the image's rows 3 and
/// 4 (another line) rather than 7, a share of the y box (a level).
pub static AREA: ShaderProgram = ShaderProgram {
    name: "koan-chart-area",
    msl: concat!(
        chart_msl!(),
        r#"
float4 koan_chart_mark(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float n = u[5];
    if (n < 2.0) {
        return float4(0.0);
    }
    float phase = koan_track(u, 0, time);
    float4 m = float4(koan_track(u, 1, time), koan_track(u, 2, time) + koan_track(u, 5, time), koan_track(u, 3, time), koan_track(u, 4, time));
    if (position.x < koan_chart_x(image, tile, n, 0.0, m, size)
        || position.x > koan_chart_x(image, tile, n, n - 1.0, m, size)) {
        return float4(0.0);
    }
    float i = min(koan_chart_find(image, tile, n, m, size, position.x), n - 2.0);
    float2 a = koan_chart_at(image, tile, n, i, phase, m, size);
    float2 b = koan_chart_at(image, tile, n, i + 1.0, phase, m, size);
    float t = b.x > a.x ? clamp((position.x - a.x) / (b.x - a.x), 0.0, 1.0) : 0.0;
    float line = a.y + (b.y - a.y) * t;
    if (u[4] > 0.5) {
        float y = koan_chart_smooth(image, tile, n, i, t, phase, 1.0);
        line = (1.0 - (m.z * y + m.w)) * size.y;
    }
    float base_share = u[7];
    if (u[6] > 0.5) {
        float b0 = koan_chart_aux(image, tile, n, i, phase);
        float b1 = koan_chart_aux(image, tile, n, i + 1.0, phase);
        base_share = u[4] > 0.5
            ? koan_chart_smooth(image, tile, n, i, t, phase, 3.0)
            : b0 + (b1 - b0) * t;
    }
    float base = (1.0 - (m.z * base_share + m.w)) * size.y;
    float top = min(line, base);
    float bottom = max(line, base);
    // A sliver thinner than a pixel covers it by its height, so where the
    // line meets its base nothing is drawn.
    float cover = clamp(min(position.y - top, bottom - position.y) + 0.5, 0.0, 1.0)
        * clamp(bottom - top, 0.0, 1.0);
    return float4(u[0], u[1], u[2], u[3] * cover);
}
"#,
        chart_place_msl!()
    ),
    wgsl: concat!(
        chart_wgsl!(),
        r#"
fn koan_chart_mark(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let n = u[1].y;
    if (n < 2.0) {
        return vec4<f32>(0.0);
    }
    let phase = koan_track(u, 0u, time);
    var m = koan_chart_axes(u, time);
    m.y = m.y + koan_track(u, 5u, time);
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
    var line = a.y + (b.y - a.y) * t;
    if (u[1].x > 0.5) {
        let y = koan_chart_smooth(tile, n, i, t, phase, 1.0);
        line = (1.0 - (m.z * y + m.w)) * size.y;
    }
    var base_share = u[1].w;
    if (u[1].z > 0.5) {
        if (u[1].x > 0.5) {
            base_share = koan_chart_smooth(tile, n, i, t, phase, 3.0);
        } else {
            let b0 = koan_chart_aux(tile, n, i, phase);
            let b1 = koan_chart_aux(tile, n, i + 1.0, phase);
            base_share = b0 + (b1 - b0) * t;
        }
    }
    let base = (1.0 - (m.z * base_share + m.w)) * size.y;
    let top = min(line, base);
    let bottom = max(line, base);
    let cover = clamp(min(position.y - top, bottom - position.y) + 0.5, 0.0, 1.0)
        * clamp(bottom - top, 0.0, 1.0);
    return vec4<f32>(u[0].xyz, u[0].w * cover);
}
"#,
        chart_place_wgsl!()
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

/// A point. Uniforms: 0..4 the colour, 4 how far its frame reaches past the
/// plot on each side, in device pixels. Tracks: 0 and 1 where it stands, as
/// shares of the plot's width and height (from the foot), 2 its radius in
/// device pixels.
pub static DOT: ShaderProgram = ShaderProgram {
    name: "koan-chart-dot",
    msl: concat!(
        r#"
float4 koan_chart_mark(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float pad = u[4];
    float2 plot = size - 2.0 * pad;
    float2 centre = float2(koan_track(u, 0, time) * plot.x, (1.0 - koan_track(u, 1, time)) * plot.y) + pad;
    float cover = clamp(koan_track(u, 2, time) + 0.5 - length(position - centre), 0.0, 1.0);
    return float4(u[0], u[1], u[2], u[3] * cover);
}
"#,
        chart_place_msl!()
    ),
    wgsl: concat!(
        r#"
fn koan_chart_mark(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let pad = u[1].x;
    let plot = size - 2.0 * pad;
    let centre = vec2<f32>(koan_track(u, 0u, time) * plot.x, (1.0 - koan_track(u, 1u, time)) * plot.y) + pad;
    let cover = clamp(koan_track(u, 2u, time) + 0.5 - length(position - centre), 0.0, 1.0);
    return vec4<f32>(u[0].xyz, u[0].w * cover);
}
"#,
        chart_place_wgsl!()
    ),
    prepare: None,
};
