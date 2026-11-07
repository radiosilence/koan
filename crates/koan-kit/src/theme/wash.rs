//! The record wash: the playing record's sleeve, blurred to colour fields and
//! drifting, behind the bare ground of a page. The one thing under the ground
//! that is not flat, and it carries data. The Apple apps' `DriftingWash` and
//! Android's `RecordWash`.
//!
//! Elsewhere each record is baked once per appearance into an opaque
//! texture (`bake`): saturated, blurred, held to the tone's luminance limit,
//! laid over `bg` at the wash's strength and dithered up to 1024 a side.
//!
//! Where the renderer draws shaders, the wash is the GPU's to make: each
//! record is reduced once, on a worker, to a small field (`field`: the
//! sleeve saturated and blurred, 32 texels a side, its cubic B-spline drawn
//! at 128), and the program (`WASH`) reads it in one filtered sample at
//! every present and does the rest per pixel:
//! the tone's luminance hold, the ground at the wash's strength and the
//! dither. An appearance or tint change is a uniform, not a bake. Each
//! record's field is one shader quad, handed the drift's and the dissolve's
//! ramps once per render, its pose and opacity worked out as each frame is
//! presented (`prepare`), so a drifting wash costs no GPUI frame and the
//! window re-presents its layers around it. The wash renders only when a
//! record, the appearance or the drift's wanting changes, and once more when
//! a ramp comes to rest. Elsewhere a frame of the drift is one textured quad,
//! two while one record dissolves into the next, drawn by a GPUI frame.
//! Either way nothing is drawn once playback stops and the drift has come to
//! rest.
//!
//! The wash is its own view. A window's root holds it beside the content view,
//! both `cached`, so a render of the wash reuses the rest of the window as it
//! was.
//!
//! The drift breathes, travels and turns through ±3° as the Apple apps' does;
//! the turn is what makes a field this soft visibly move. The shader turns
//! its sampling; the CPU's fallback, drawn as an image sprite, which takes no
//! transform, breathes and travels alone.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Bounds, Context, Corners, Entity, Global, IntoElement, ParentElement, Pixels, Render,
    RenderImage, Styled, Task, Window, canvas, div, point, px, size,
};
use smallvec::SmallVec;

use super::{KoanTheme, Look, Motion, Palette};
use crate::components::{Animated, Ease, Repeat, SHADED, ShaderProgram};

/// The texture's side. Blurred to mush, so drawn small and magnified: the
/// Apple apps blur 28 points of a 360-point texture, which is 10 pixels of
/// this one.
pub const SIDE: u32 = 128;
const RADIUS: usize = 10;
const SATURATION: f64 = 1.6;
/// The share of the toned sleeve over `bg`.
pub const STRENGTH: f32 = 0.6;

/// The overscan the texture is drawn at, and how far the drift travels in it:
/// about four of the wash's softest features, with the texture's edge out of
/// frame throughout.
const NEAR: f32 = 1.38;
const FAR: f32 = 1.58;
const REACH: f32 = 0.12;
const RISE: f32 = 0.10;
const SCALE_PERIOD: f32 = 13.0;
const TRAVEL_PERIOD: f32 = 23.0;
/// The turn's period at full drift: the Mac's 19 s. A field this soft
/// moves most visibly by turning.
const TURN_PERIOD: f32 = 19.0;
/// A dissolve between records, and the drift starting or coming to rest.
const EASE: Duration = Motion::DISSOLVE;
/// The drift's whole cycle: both swings back where they started. The shader's
/// clock is counted from a multiple of it, so it stays small and exact.
const CYCLE: f32 = 598.0;

/// The field's side: the blurred sleeve carries nothing finer.
pub const FIELD: u32 = 32;

/// The side the field is handed to the GPU at: its cubic B-spline,
/// evaluated here once a record rather than from four reads by every pixel
/// of every present, and filtered linearly between. A texel spans some
/// fifty pixels of a 6K window; the curve bends too little across one for
/// the straight line between to show.
pub const FIELD_DRAWN: u32 = 128;

/// The span of OKLab's a and b the field encodes, centred on grey: wider
/// than any sRGB colour reaches.
const LAB_SPAN: f64 = 0.7;

/// The wash as a fragment program, made from the record's field (`field`) at
/// each present: the field's B-spline as `field` drew it, the tone's luminance
/// hold, the ground at the wash's strength, and a dither of a level either
/// way, so a gradient this soft shows no bands at any size. Uniforms: 0
/// whether to lay it over `bg` (the layer drawn first), 1 whether the
/// appearance is dark, 2..5 `bg`, 5 the hold (0 to 1), 6 the tone's luminance
/// limit (`Tone::limit`, the one place it is set), 7 to 11 the pose
/// `prepare` works out from the tracks. Tracks: 0 the drift's
/// amplitude, 1 the opacity, 2 the breath's scale, 3 the travel (-1 to 1),
/// 4 the turn's swing (-1 to 1) and 5 how far it turns, in radians, each at
/// full amplitude; a new turn eases there rather than jumping (#1883). The
/// field is OKLab: L in red, a and b in green and blue over `LAB_SPAN`. The
/// overscan (1.38), reach (0.12), rise (0.10), the span (0.7) and
/// strength (0.6) are written into the source; a test holds them to the
/// constants.
pub static WASH: ShaderProgram = ShaderProgram {
    name: "koan-wash",
    msl: r#"
float3 koan_wash_from_lab(float3 f) {
    float L = f.r;
    float A = (f.g - 0.5) * 0.7;
    float B = (f.b - 0.5) * 0.7;
    float l = L + 0.3963377774 * A + 0.2158037573 * B;
    float m = L - 0.1055613458 * A - 0.0638541728 * B;
    float s = L - 0.0894841775 * A - 1.2914855480 * B;
    l = l * l * l;
    m = m * m * m;
    s = s * s * s;
    return max(float3(
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s), 0.0);
}

float3 koan_wash_gamma(float3 c) {
    c = clamp(c, 0.0, 1.0);
    return select(1.055 * pow(c, 1.0 / 2.4) - 0.055, 12.92 * c, c <= 0.0031308);
}

float koan_wash_ign(float2 p) {
    return fract(52.9829189 * fract(0.06711056 * p.x + 0.00583715 * p.y));
}

float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    // The pose, worked out once a present (`prepare`).
    float opacity = u[7];
    float scale = u[8];
    float travel = u[9];
    float c = u[10];
    float s = u[11];
    float side = max(size.x, size.y) * 1.38 * scale;
    float2 centre = size * 0.5 + float2(size.x * 0.12, size.y * 0.10) * travel;
    // Turned about the centre: the field is sampled the other way.
    float2 d = position - centre;
    float2 uv = float2(c * d.x + s * d.y, c * d.y - s * d.x) / side + 0.5;

    // The field as `field` evaluated it, its cubic B-spline drawn finer
    // than the eye can tell from the curve: one filtered read.
    float3 field = koan_sample(image, tile, uv).rgb;

    // The tone's hold: under `muted`'s limit in the dark, over `surface`'s
    // in the light, by the person's tint.
    float3 light = koan_wash_from_lab(field);
    float y = dot(light, float3(0.2126, 0.7152, 0.0722));
    float limit = u[6];
    if (u[1] > 0.5) {
        if (y > limit) {
            light *= 1.0 + (limit / y - 1.0) * u[5];
        }
    } else {
        if (y < limit) {
            light += (1.0 - light) * ((limit - y) / (1.0 - y) * u[5]);
        }
    }
    float3 ground = float3(u[2], u[3], u[4]);
    float3 colour = ground + (koan_wash_gamma(light) - ground) * 0.6;
    colour += (koan_wash_ign(position) + koan_wash_ign(position + float2(37.0, 91.0)) - 1.0) / 255.0;
    if (u[0] > 0.5) {
        return float4(mix(ground, colour, opacity), 1.0);
    }
    return float4(colour, opacity);
}
"#,
    wgsl: r#"
fn koan_wash_from_lab(f: vec3<f32>) -> vec3<f32> {
    let L = f.x;
    let A = (f.y - 0.5) * 0.7;
    let B = (f.z - 0.5) * 0.7;
    let l0 = L + 0.3963377774 * A + 0.2158037573 * B;
    let m0 = L - 0.1055613458 * A - 0.0638541728 * B;
    let s0 = L - 0.0894841775 * A - 1.2914855480 * B;
    let l = l0 * l0 * l0;
    let m = m0 * m0 * m0;
    let s = s0 * s0 * s0;
    return max(vec3<f32>(
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s), vec3<f32>(0.0));
}

fn koan_wash_gamma(c0: vec3<f32>) -> vec3<f32> {
    let c = clamp(c0, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, 12.92 * c, c <= vec3<f32>(0.0031308));
}

fn koan_wash_ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(0.06711056 * p.x + 0.00583715 * p.y));
}

fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let opacity = u[1].w;
    let scale = u[2].x;
    let travel = u[2].y;
    let c = u[2].z;
    let s = u[2].w;
    let side = max(size.x, size.y) * 1.38 * scale;
    let centre = size * 0.5 + vec2<f32>(size.x * 0.12, size.y * 0.10) * travel;
    let d = position - centre;
    let uv = vec2<f32>(c * d.x + s * d.y, c * d.y - s * d.x) / side + 0.5;
    let field = koan_sample(tile, uv).rgb;
    var light = koan_wash_from_lab(field);
    let y = dot(light, vec3<f32>(0.2126, 0.7152, 0.0722));
    let limit = u[1].z;
    if (u[0].y > 0.5) {
        if (y > limit) {
            light = light * (1.0 + (limit / y - 1.0) * u[1].y);
        }
    } else {
        if (y < limit) {
            light = light + (1.0 - light) * ((limit - y) / (1.0 - y) * u[1].y);
        }
    }
    let ground = vec3<f32>(u[0].z, u[0].w, u[1].x);
    var colour = ground + (koan_wash_gamma(light) - ground) * 0.6;
    colour = colour + (koan_wash_ign(position) + koan_wash_ign(position + vec2<f32>(37.0, 91.0)) - 1.0) / 255.0;
    if (u[0].x > 0.5) {
        return vec4<f32>(mix(ground, colour, opacity), 1.0);
    }
    return vec4<f32>(colour, opacity);
}
"#,
    prepare: Some(prepare),
};

/// The pose every pixel of a present shares, from the tracks, into plain
/// uniforms 7 to 11: the opacity, the breath's scale, the travel, and the
/// turn's cosine and sine. Worked out here once a present rather than by
/// each of a 4K window's eight million pixels.
fn prepare(u: &mut [f32; gpui::SHADER_UNIFORMS], time: f32) {
    use crate::components::track_at;
    let amplitude = track_at(u, 0, time);
    let opacity = track_at(u, 1, time);
    let scale = 1.0 + (track_at(u, 2, time) - 1.0) * amplitude;
    let travel = track_at(u, 3, time) * amplitude;
    let turn = track_at(u, 4, time) * track_at(u, 5, time) * amplitude;
    u[7] = opacity;
    u[8] = scale;
    u[9] = travel;
    u[10] = turn.cos();
    u[11] = turn.sin();
}

/// Where the texture is `t` seconds into the drift at `amplitude`: its scale
/// over the overscan, and its travel across the page from -1 to 1.
fn pose_at(t: f32, amplitude: f32) -> (f32, f32) {
    if amplitude <= 0.0 {
        return (1.0, 0.0);
    }
    let swing = |t: f32, period: f32| {
        let phase = (t / period).rem_euclid(2.0);
        Motion::ease_in_out(if phase < 1.0 { phase } else { 2.0 - phase })
    };
    let scale = 1.0 + (FAR / NEAR - 1.0) * swing(t, SCALE_PERIOD);
    // Half a swing in, so the travel starts from the middle of the page.
    let travel = 2.0 * swing(t + TRAVEL_PERIOD / 2.0, TRAVEL_PERIOD) - 1.0;
    (1.0 + (scale - 1.0) * amplitude, travel * amplitude)
}

/// The theme's hold on the wash: every sample kept on the side of a luminance
/// limit where `muted` text over it still reads.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tone {
    /// No brighter than `muted` (#919191) at 4.6:1 allows.
    Dark,
    /// No darker than keeps the wash, drawn over white, at least as light as
    /// `surface`.
    Light,
}

impl Tone {
    fn limit(self) -> f64 {
        match self {
            Tone::Dark => (0.2831 + 0.05) / 4.6 - 0.05,
            Tone::Light => 0.86,
        }
    }
}

/// The luminance limit the wash is held to in an appearance, as the program
/// takes it (uniform 6).
pub fn tone_limit(dark: bool) -> f32 {
    if dark { Tone::Dark } else { Tone::Light }.limit() as f32
}

/// A decoded sleeve, as straight-alpha RGBA.
#[derive(Clone)]
pub struct Sleeve {
    rgba: Arc<[u8]>,
    width: u32,
    height: u32,
}

impl Sleeve {
    /// `None` unless `rgba` holds exactly `width` × `height` pixels.
    pub fn new(rgba: Arc<[u8]>, width: u32, height: u32) -> Option<Sleeve> {
        let pixels = width as usize * height as usize;
        (pixels > 0 && rgba.len() == pixels * 4).then_some(Sleeve {
            rgba,
            width,
            height,
        })
    }

    fn same(&self, other: &Sleeve) -> bool {
        Arc::ptr_eq(&self.rgba, &other.rgba)
    }
}

/// The sleeve baked into the wash for one appearance: `hold` is the person's
/// "theme tint", how far each sample moves toward the tone's limit, from 1,
/// all the way, to 0, the sleeve's own colour. Returns BGRA pixels, as GPUI's
/// image sprites take them.
pub fn bake(sleeve: &Sleeve, dark: bool, hold: f64) -> image::RgbaImage {
    let [r, g, b] = blurred(sleeve);
    let n = (SIDE * SIDE) as usize;
    let tone = if dark { Tone::Dark } else { Tone::Light };
    let ground = Palette::of(dark).bg.channels();
    let limit = tone.limit();
    // The wash over the ground, per channel, in encoded values as a layer's
    // opacity composites, still in floats: quantised only once, dithered,
    // at the output's size.
    let ground = [ground[0], ground[1], ground[2]].map(|v| v as f64);
    let mut mixed = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
    for i in 0..n {
        let (mut lr, mut lg, mut lb) = (r[i], g[i], b[i]);
        let y = 0.2126 * lr + 0.7152 * lg + 0.0722 * lb;
        match tone {
            Tone::Dark if y > limit => {
                let k = 1.0 + (limit / y - 1.0) * hold;
                lr *= k;
                lg *= k;
                lb *= k;
            }
            Tone::Light if y < limit => {
                let t = (limit - y) / (1.0 - y) * hold;
                lr += (1.0 - lr) * t;
                lg += (1.0 - lg) * t;
                lb += (1.0 - lb) * t;
            }
            _ => {}
        }
        for (c, v) in [lr, lg, lb].into_iter().enumerate() {
            let wash = super::accent::oklch::gamma(v) * 255.0;
            mixed[c][i] = ground[c] + (wash - ground[c]) * STRENGTH as f64;
        }
    }
    dither_up(&mixed)
}

/// The wash's field for the GPU: the sleeve saturated and blurred as `bake`
/// does it, at `FIELD` texels a side, its cubic B-spline drawn at
/// `FIELD_DRAWN`, encoded. The program reads it once a pixel and does the rest (the tone's hold, the ground, the dither) per
/// pixel at each present, so a new appearance or tint is a uniform, not a
/// bake. BGRA, as GPUI's images are. The field is band-limited by the blur,
/// so this many texels lose nothing a cubic reconstruction would show; the
/// reconstruction is made here, once a record, not by every pixel.
pub fn field(sleeve: &Sleeve) -> image::RgbaImage {
    let channels = blurred(sleeve);
    let side = SIDE as usize;
    let n = FIELD as usize;
    let cell = side / n;
    // The field in OKLab, so the reads blend in an even space, not in
    // sRGB's encoding, which would darken and dull a colour edge.
    let lab: Vec<[f64; 3]> = (0..n * n)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            let linear = [0, 1, 2].map(|c| {
                let mut sum = 0.0;
                for dy in 0..cell {
                    for dx in 0..cell {
                        sum += channels[c][(y * cell + dy) * side + x * cell + dx];
                    }
                }
                sum / (cell * cell) as f64
            });
            super::accent::oklch::lab(linear)
        })
        .collect();
    let drawn = spline_up(&lab, n, FIELD_DRAWN as usize);
    let byte = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    let chroma = |v: f64| byte(v / LAB_SPAN + 0.5);
    let mut out = image::RgbaImage::new(FIELD_DRAWN, FIELD_DRAWN);
    for (pixel, [l, a, b]) in out.pixels_mut().zip(drawn) {
        pixel.0 = [chroma(b), chroma(a), byte(l), 255];
    }
    out
}

/// The uniform cubic B-spline through `field` (`n` a side, its texels the
/// control points, held at the edges), evaluated at the centres of `m` by
/// `m` texels over the same square.
fn spline_up(field: &[[f64; 3]], n: usize, m: usize) -> Vec<[f64; 3]> {
    let weights = |f: f64| {
        let (f2, f3) = (f * f, f * f * f);
        [
            (-f3 + 3.0 * f2 - 3.0 * f + 1.0) / 6.0,
            (3.0 * f3 - 6.0 * f2 + 4.0) / 6.0,
            (-3.0 * f3 + 3.0 * f2 + 3.0 * f + 1.0) / 6.0,
            f3 / 6.0,
        ]
    };
    // Along one axis: for each drawn texel, the four control points and
    // their weights.
    let taps: Vec<([usize; 4], [f64; 4])> = (0..m)
        .map(|j| {
            let t = (j as f64 + 0.5) / m as f64 * n as f64 - 0.5;
            let i = t.floor();
            let at = |k: f64| (i + k).clamp(0.0, (n - 1) as f64) as usize;
            ([at(-1.0), at(0.0), at(1.0), at(2.0)], weights(t - i))
        })
        .collect();
    let mut out = Vec::with_capacity(m * m);
    for (ys, wy) in &taps {
        for (xs, wx) in &taps {
            let mut v = [0.0; 3];
            for (y, wy) in ys.iter().zip(wy) {
                for (x, wx) in xs.iter().zip(wx) {
                    let w = wx * wy;
                    let texel = field[y * n + x];
                    for c in 0..3 {
                        v[c] += texel[c] * w;
                    }
                }
            }
            out.push(v);
        }
    }
    out
}

/// The sleeve at `SIDE`, over black where it is transparent, saturated and
/// blurred, as linear red, green and blue.
fn blurred(sleeve: &Sleeve) -> [Vec<f64>; 3] {
    let source = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(
        sleeve.width,
        sleeve.height,
        &sleeve.rgba[..],
    )
    .expect("a sleeve is made with its length checked");
    let small = image::imageops::resize(&source, SIDE, SIDE, image::imageops::FilterType::Triangle);
    let n = (SIDE * SIDE) as usize;
    let decode: Vec<f64> = (0..256)
        .map(|v| super::accent::oklch::linear(v as f64 / 255.0))
        .collect();

    let (mut r, mut g, mut b) = (vec![0.0; n], vec![0.0; n], vec![0.0; n]);
    for (i, pixel) in small.pixels().enumerate() {
        // Transparent parts of a sleeve are drawn over black, as Core Graphics
        // composites a premultiplied image.
        let alpha = pixel.0[3] as f64 / 255.0;
        let [lr, lg, lb] = [0, 1, 2].map(|c| decode[pixel.0[c] as usize] * alpha);
        // Core Image's colour controls: away from luma by the saturation.
        let y = 0.2125 * lr + 0.7154 * lg + 0.0721 * lb;
        r[i] = (y + (lr - y) * SATURATION).clamp(0.0, 1.0);
        g[i] = (y + (lg - y) * SATURATION).clamp(0.0, 1.0);
        b[i] = (y + (lb - y) * SATURATION).clamp(0.0, 1.0);
    }
    for channel in [&mut r, &mut g, &mut b] {
        blur(channel, SIDE as usize);
    }
    [r, g, b]
}

/// The baked texture's side: the wash at `SIDE`, magnified in floats and
/// dithered as it is quantised. A gradient this soft spans a few levels of
/// 8 bits across a whole window, and quantised at `SIDE` and magnified by
/// the GPU it shows as bands; dithered at this size, the GPU's last few
/// times of magnification leave the noise too fine to see and no bands.
pub const OUT: u32 = 1024;

/// `channels` (red, green, blue, 0–255 floats at `SIDE`) bilinearly
/// magnified to `OUT` and quantised with triangular noise of a level either
/// way, from interleaved gradient noise: no pattern the eye picks out, and
/// the same every bake, so a sleeve's wash does not shimmer when baked again.
/// BGRA out, as the renderer takes it.
fn dither_up(channels: &[Vec<f64>; 3]) -> image::RgbaImage {
    let side = SIDE as usize;
    let scale = SIDE as f64 / OUT as f64;
    let mut out = image::RgbaImage::new(OUT, OUT);
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        // Texel centres line up with the source's, the edge clamped.
        let sx = ((x as f64 + 0.5) * scale - 0.5).clamp(0.0, (side - 1) as f64);
        let sy = ((y as f64 + 0.5) * scale - 0.5).clamp(0.0, (side - 1) as f64);
        let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(side - 1), (y0 + 1).min(side - 1));
        let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
        let noise = ign(x, y) + ign(x + 37, y + 91) - 1.0;
        let [r, g, b] = [0, 1, 2].map(|c| {
            let at = |xx: usize, yy: usize| channels[c][yy * side + xx];
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
            let v = top + (bottom - top) * fy;
            (v + noise + 0.5).floor().clamp(0.0, 255.0) as u8
        });
        pixel.0 = [b, g, r, 255];
    }
    out
}

/// Interleaved gradient noise (Jimenez), uniform in 0..1.
fn ign(x: u32, y: u32) -> f64 {
    (52.982_918_9 * (0.067_110_56 * x as f64 + 0.005_837_15 * y as f64).fract()).fract()
}

/// Three box passes, about a Gaussian of the radius, with the edge clamped:
/// an unclamped blur reads past the edge as black and gives the sleeve a dark
/// border.
fn blur(channel: &mut [f64], side: usize) {
    let mut line = vec![0.0; side];
    for _ in 0..3 {
        for y in 0..side {
            line.copy_from_slice(&channel[y * side..(y + 1) * side]);
            boxed(&line, |x, v| channel[y * side + x] = v);
        }
        for x in 0..side {
            for y in 0..side {
                line[y] = channel[y * side + x];
            }
            boxed(&line, |y, v| channel[y * side + x] = v);
        }
    }
}

fn boxed(line: &[f64], mut set: impl FnMut(usize, f64)) {
    let last = line.len() as isize - 1;
    let at = |i: isize| line[i.clamp(0, last) as usize];
    let width = (2 * RADIUS + 1) as f64;
    let radius = RADIUS as isize;
    let mut sum: f64 = (-radius..=radius).map(at).sum();
    for i in 0..line.len() as isize {
        set(i as usize, sum / width);
        sum += at(i + radius + 1) - at(i - radius);
    }
}

/// A value easing between two others over `EASE`, from wherever it stood.
#[derive(Clone, Copy)]
struct Ramp {
    from: f32,
    to: f32,
    /// When it set off; `None` for one standing still.
    started: Option<Instant>,
}

impl Ramp {
    fn at(self, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return self.to;
        };
        let t = now.duration_since(started).as_secs_f32() / EASE.as_secs_f32();
        self.from + (self.to - self.from) * Motion::ease_in_out(t)
    }

    fn moving(self, now: Instant) -> bool {
        self.started
            .is_some_and(|started| now.duration_since(started) < EASE)
    }

    /// When it comes to rest, if it is still moving.
    fn rests(self, now: Instant) -> Option<Instant> {
        self.started
            .map(|started| started + EASE)
            .filter(|end| *end > now)
    }

    /// Into track `k` of `shader`: the program eases it as `at` does.
    fn write(self, shader: &Animated, k: usize) {
        match self.started {
            Some(started) => shader.segment(
                k,
                self.from,
                self.to,
                started,
                EASE,
                Ease::InOut,
                Repeat::Once,
            ),
            None => shader.hold(k, self.to),
        }
    }

    /// Standing at `value`, not moving.
    fn still(value: f32) -> Ramp {
        Ramp {
            from: value,
            to: value,
            started: None,
        }
    }

    fn toward(self, to: f32, now: Instant) -> Ramp {
        Ramp {
            from: self.at(now),
            to,
            started: Some(now),
        }
    }
}

/// A baked texture and what it was baked for.
struct Texture {
    image: Arc<RenderImage>,
    generation: u64,
    dark: bool,
    hold: u32,
}

/// A texture on screen and its opacity over time, and the uniforms its
/// shader quad reads.
struct Layer<T> {
    texture: T,
    opacity: Ramp,
    shader: Animated,
}

/// The window's wash, for a screen over the page (Now Playing, a full
/// sheet on a phone) to draw behind itself, as the page's ground does;
/// `None` with less transparency asked for. The window's root sets it.
pub struct WindowWash(pub Option<Entity<Wash>>);

impl Global for WindowWash {}

impl WindowWash {
    pub fn shared(cx: &App) -> Option<Entity<Wash>> {
        cx.try_global::<Self>().and_then(|w| w.0.clone())
    }
}

pub struct Wash {
    sleeve: Option<Sleeve>,
    /// Which sleeve is set: a bake for an earlier one is dropped.
    generation: u64,
    /// The bake running, or the last one tried, as (generation, dark, hold).
    baked: Option<(u64, bool, u32)>,
    current: Option<Layer<Texture>>,
    /// The record before, held under the current one while it dissolves in,
    /// or fading out to the bare ground when there is none.
    previous: Option<Layer<Arc<RenderImage>>>,
    /// Whether the caller wants the drift: something playing, and the
    /// graphics setting allowing it.
    wanted: bool,
    /// How far the drift is drawn, from 0, at rest, to 1.
    amplitude: Ramp,
    /// The drift's clock: its phase is the time since, never a frame count,
    /// so frames the system skips (an occluded window) cost no motion.
    epoch: Instant,
    /// The pace the drift was last drawn at: a change moves the epoch so the
    /// drift carries on from where it stood.
    pace: f32,
    hold: u32,
    /// Textures the atlas no longer needs.
    released: Vec<Arc<RenderImage>>,
    /// A render when the last ramp comes to rest, where the shader drifts.
    rest: Option<Task<()>>,
}

impl Wash {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            sleeve: None,
            generation: 0,
            baked: None,
            current: None,
            previous: None,
            wanted: false,
            amplitude: Ramp::still(0.0),
            epoch: now,
            pace: 1.0,
            hold: 100,
            released: Vec::new(),
            rest: None,
        }
    }

    /// The playing record's sleeve, or `None` for no record or none with
    /// art, which fades the wash out to the bare ground. A sleeve still being
    /// fetched is not `None`: leave the wash as it is until it arrives.
    pub fn set_sleeve(&mut self, sleeve: Option<Sleeve>, cx: &mut Context<Self>) {
        let unchanged = match (&self.sleeve, &sleeve) {
            (Some(a), Some(b)) => a.same(b),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        self.sleeve = sleeve;
        self.generation += 1;
        if self.sleeve.is_none() {
            self.install(None, true);
        }
        cx.notify();
    }

    /// Whether the wash should drift: something playing, and the graphics
    /// setting allowing it. Reduce Motion and Low Power Mode are read as it
    /// draws (`KoanTheme::motion_allowed`), kept current by the system's
    /// notifications. Starting and stopping ease over two seconds
    /// from wherever the drift stands.
    pub fn drift(&mut self, wanted: bool, cx: &mut Context<Self>) {
        if self.wanted != wanted {
            self.wanted = wanted;
            cx.notify();
        }
    }

    /// "Theme tint", in percent: how far the sleeve is held to the tone's
    /// luminance limit.
    pub fn set_hold(&mut self, percent: u32, cx: &mut Context<Self>) {
        self.hold = percent.min(100);
        cx.notify();
    }

    /// Puts a texture on, or takes the wash off with `None`. A new record
    /// dissolves in over the old; the same record rebaked for another
    /// appearance or tint replaces everything at once, since a texture of the
    /// old appearance must not linger under the new.
    fn install(&mut self, texture: Option<Texture>, dissolve: bool) {
        let now = Instant::now();
        let outgoing = self.current.take();
        if !dissolve {
            self.released
                .extend(outgoing.map(|layer| layer.texture.image));
            self.released
                .extend(self.previous.take().map(|layer| layer.texture));
            self.current = texture.map(|texture| Layer {
                texture,
                opacity: Ramp::still(1.0),
                shader: Animated::new(),
            });
            return;
        }
        let Some(texture) = texture else {
            // Taken off: what shows fades out to the ground from where it
            // stands, a record part-way through dissolving in and the one
            // under it alike, so the window never jumps to the newer one
            // first.
            if let Some(previous) = self.previous.as_mut() {
                previous.opacity = previous.opacity.toward(0.0, now);
            }
            self.current = outgoing.map(|mut layer| {
                layer.opacity = layer.opacity.toward(0.0, now);
                layer
            });
            return;
        };
        if let Some(outgoing) = outgoing {
            // Held whole under the new record, however far its own
            // dissolve had come: a part-faded record held under the next
            // would let the ground show through for the length of a
            // dissolve. One already fading out goes on fading as it was.
            let opacity = if outgoing.opacity.to <= 0.0 {
                outgoing.opacity
            } else {
                Ramp::still(1.0)
            };
            let held = Layer {
                texture: outgoing.texture.image,
                opacity,
                shader: outgoing.shader,
            };
            self.released
                .extend(self.previous.replace(held).map(|layer| layer.texture));
        }
        // With no record on before, `previous` keeps fading as it was.
        self.current = Some(Layer {
            texture,
            opacity: Ramp::still(0.0).toward(1.0, now),
            shader: Animated::new(),
        });
    }

    fn start_bake(&mut self, dark: bool, cx: &mut Context<Self>) {
        let Some(sleeve) = self.sleeve.clone() else {
            return;
        };
        // Where the GPU makes the wash, the field depends on the record
        // alone: the appearance and the tint are the program's uniforms.
        let (dark, hold) = if SHADED {
            (false, 0)
        } else {
            (dark, self.hold)
        };
        let key = (self.generation, dark, hold);
        if self.baked == Some(key) {
            return;
        }
        self.baked = Some(key);
        let task = cx.background_executor().spawn(async move {
            if SHADED {
                field(&sleeve)
            } else {
                bake(&sleeve, dark, hold as f64 / 100.0)
            }
        });
        cx.spawn(async move |wash, cx| {
            let baked = task.await;
            wash.update(cx, |wash, cx| {
                if wash.generation != key.0 || wash.baked != Some(key) {
                    return;
                }
                let same_record = wash
                    .current
                    .as_ref()
                    .is_some_and(|layer| layer.texture.generation == key.0);
                let image = Arc::new(RenderImage::new(SmallVec::from_elem(
                    image::Frame::new(baked),
                    1,
                )));
                let texture = Texture {
                    image,
                    generation: key.0,
                    dark,
                    hold,
                };
                wash.install(Some(texture), !same_record);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The person's pace (Settings → Appearance), drawn either way. A drift
    /// set to nothing keeps the last pace while its amplitude eases out, so
    /// it comes to rest as it does when the music stops.
    fn follow_pace(&mut self, now: Instant, cx: &App) {
        let drifting = self.amplitude.to > 0.0 || self.amplitude.moving(now);
        let percent = cx.global::<KoanTheme>().wash_drift;
        if !drifting || percent == 0 {
            return;
        }
        let pace = percent as f32 / 100.0;
        if pace != self.pace {
            // The phase (now - epoch) * pace stays where it was.
            let since = now.saturating_duration_since(self.epoch).as_secs_f64();
            let kept = since * self.pace as f64 / pace as f64;
            self.epoch = now - Duration::from_secs_f64(kept);
            self.pace = pace;
        }
    }

    /// Where the texture is this frame: its scale over the overscan, and its
    /// travel across the page from -1 to 1, at the person's pace. Image
    /// sprites take no transform, so drawn this way the wash does not turn.
    fn pose(&self, now: Instant) -> (f32, f32) {
        // Reduced to the cycle in f64 before narrowing, as the shader's clock
        // is: in f32, a window open for days counts time in steps a frame
        // long.
        let since = now.duration_since(self.epoch).as_secs_f64() * self.pace as f64;
        let t = (since % CYCLE as f64) as f32;
        pose_at(t, self.amplitude.at(now))
    }
}

impl Default for Wash {
    fn default() -> Self {
        Self::new()
    }
}

/// The square the texture fills: the page's longer side times the overscan,
/// centred, moved by the pose.
fn square(bounds: Bounds<Pixels>, scale: f32, travel: f32) -> Bounds<Pixels> {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let side = w.max(h) * NEAR * scale;
    let centre = bounds.center();
    let origin = point(
        centre.x + px(w * REACH * travel - side / 2.0),
        centre.y + px(h * RISE * travel - side / 2.0),
    );
    Bounds::new(origin, size(px(side), px(side)))
}

impl Render for Wash {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let look = Look::of(window, cx);
        let stale = match &self.current {
            Some(layer) => {
                layer.texture.generation != self.generation
                    || (!SHADED
                        && (layer.texture.dark != look.dark || layer.texture.hold != self.hold))
            }
            None => true,
        };
        if stale && self.sleeve.is_some() {
            self.start_bake(look.dark, cx);
        } else if !stale {
            // A bake still running for something no longer wanted (an
            // appearance flipped and flipped back) is dropped when it lands.
            self.baked = None;
        }
        // At rest only one texture is drawn: the one beneath goes once the
        // current one is opaque, or once it has faded out to the ground.
        let covered = self
            .current
            .as_ref()
            .is_some_and(|layer| !layer.opacity.moving(now));
        let faded = self
            .previous
            .as_ref()
            .is_some_and(|layer| !layer.opacity.moving(now) && layer.opacity.at(now) <= 0.0);
        if covered || faded {
            self.released
                .extend(self.previous.take().map(|layer| layer.texture));
        }
        // Taken off and faded out to the ground: nothing left to draw.
        let gone = self
            .current
            .as_ref()
            .is_some_and(|layer| layer.opacity.to <= 0.0 && !layer.opacity.moving(now));
        if gone {
            self.released
                .extend(self.current.take().map(|layer| layer.texture.image));
        }
        for image in self.released.drain(..) {
            // After the frame that last drew it.
            window.on_next_frame(move |window, _| {
                window.drop_image(image).ok();
            });
        }

        let shown = self.current.is_some() || self.previous.is_some();
        let paced = cx.global::<KoanTheme>().wash_drift > 0;
        let target = if self.wanted && shown && paced && KoanTheme::motion_allowed(cx) {
            1.0
        } else {
            0.0
        };
        if self.amplitude.to != target {
            self.amplitude = self.amplitude.toward(target, now);
        }
        let moving = self.amplitude.moving(now)
            || (shown && self.amplitude.to > 0.0)
            || self.current.as_ref().is_some_and(|l| l.opacity.moving(now))
            || self
                .previous
                .as_ref()
                .is_some_and(|l| l.opacity.moving(now));
        self.follow_pace(now, cx);
        if SHADED {
            return self.render_shaded(now, &look, cx).into_any_element();
        }
        if moving {
            window.request_animation_frame();
        }
        let (scale, travel) = self.pose(now);

        let layer = |image: Arc<RenderImage>, opacity: f32| {
            div().absolute().size_full().opacity(opacity).child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        // The whole square, unclipped: a sub-rectangle of a
                        // sprite is cut on whole texels, which at this
                        // magnification would move the drift in steps.
                        let square = square(bounds, scale, travel);
                        window
                            .paint_image(square, square, Corners::default(), image, 0, false)
                            .ok();
                    },
                )
                .size_full(),
            )
        };
        let previous = self
            .previous
            .as_ref()
            .map(|l| (l.texture.clone(), l.opacity.at(now)))
            .filter(|(_, opacity)| *opacity > 0.0);
        let current = self
            .current
            .as_ref()
            .map(|l| (l.texture.image.clone(), l.opacity.at(now)));
        div()
            .size_full()
            .overflow_hidden()
            .bg(look.palette.bg.hsla())
            .children(previous.map(|(image, opacity)| layer(image, opacity)))
            .children(current.map(|(image, opacity)| layer(image, opacity)))
            .into_any_element()
    }
}

impl Wash {
    /// The textures as shader quads, handed their ramps; a render again when
    /// the last ramp rests, to drop what is no longer seen and let the
    /// window sleep.
    fn render_shaded(
        &mut self,
        now: Instant,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let bg = look.palette.bg.hsla().to_rgb();
        let mut quads: Vec<(Arc<RenderImage>, Animated)> = Vec::new();
        let mut rests = self.amplitude.rests(now);
        // The breath, travel and turn repeat while the drift is drawn at all,
        // and hold once it has come to rest at nothing: a wash at rest
        // presents nothing.
        let drifting = self.amplitude.to > 0.0 || self.amplitude.moving(now);
        // The person's turn (Settings → Appearance); the pace is followed
        // in `render`, for both ways of drawing.
        let turn = cx.global::<KoanTheme>().wash_turn as f32 * std::f32::consts::PI / 180.0;
        let previous = self
            .previous
            .as_ref()
            .map(|l| (l.texture.clone(), l.opacity, l.shader.clone()));
        let current = self
            .current
            .as_ref()
            .map(|l| (l.texture.image.clone(), l.opacity, l.shader.clone()));
        for (image, opacity, shader) in previous.into_iter().chain(current) {
            let first = quads.is_empty();
            shader.set_all(
                0,
                &[
                    if first { 1.0 } else { 0.0 },
                    if look.dark { 1.0 } else { 0.0 },
                    bg.r,
                    bg.g,
                    bg.b,
                    self.hold as f32 / 100.0,
                    tone_limit(look.dark),
                ],
            );
            self.amplitude.write(&shader, 0);
            opacity.write(&shader, 1);
            if drifting {
                let pace = self.pace;
                let half = Duration::from_secs_f32(TRAVEL_PERIOD / pace / 2.0);
                for (k, from, to, start, period) in [
                    (2, 1.0, FAR / NEAR, self.epoch, SCALE_PERIOD / pace),
                    (3, -1.0, 1.0, self.epoch - half, TRAVEL_PERIOD / pace),
                    (4, -1.0, 1.0, self.epoch, TURN_PERIOD / pace),
                ] {
                    shader.segment(
                        k,
                        from,
                        to,
                        start,
                        Duration::from_secs_f32(period),
                        Ease::InOut,
                        Repeat::Alternate,
                    );
                }
            } else {
                shader.hold(2, 1.0);
                shader.hold(3, 0.0);
                shader.hold(4, 0.0);
            }
            // How far it turns: a change of Turn eases there over the
            // settle, so the angle moves rather than jumps (#1883).
            if shader.track(5).to != turn {
                shader.to(5, turn, Motion::SETTLE, Ease::InOut);
            }
            rests = rests.max(opacity.rests(now));
            quads.push((image, shader));
        }
        self.rest = rests.map(|at| {
            let timer = cx.background_executor().timer(at - now);
            cx.spawn(async move |wash, cx| {
                timer.await;
                wash.update(cx, |_, cx| cx.notify()).ok();
            })
        });
        let blank = quads.is_empty();
        div()
            .size_full()
            .overflow_hidden()
            .when(blank, |d| d.bg(look.palette.bg.hsla()))
            .child(
                canvas(
                    move |_, window, cx| {
                        // Not decorative: Reduce Motion stops the drift
                        // through its amplitude, and the dissolve between
                        // records still runs, as the Mac's does.
                        for (_, shader) in &quads {
                            shader.attach(false, window, cx);
                        }
                        quads
                    },
                    move |bounds, quads, window, _| {
                        for (image, shader) in quads {
                            shader.paint(&WASH, Some(image), bounds, window);
                        }
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The program writes the drift's figures, the field's side and the
    /// strength into its source, and must agree with the constants.
    #[test]
    fn the_program_draws_with_the_wash_s_figures() {
        for source in [WASH.msl, WASH.wgsl] {
            for figure in [
                format!("* {NEAR:?} *"),
                format!("size.x * {REACH:.2}"),
                format!("size.y * {RISE:.2}"),
                format!("* {STRENGTH:?};"),
                format!("- 0.5) * {LAB_SPAN:?};"),
            ] {
                assert!(source.contains(&figure), "{figure} missing");
            }
        }
        assert_eq!(SIDE % FIELD, 0);
    }

    /// What a record costs the CPU, the field against the bake it replaces:
    /// `cargo test -p koan-kit --release wash_costs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn wash_costs() {
        let pixels: Vec<u8> = (0..128 * 128)
            .flat_map(|i: u32| [(i % 251) as u8, (i % 113) as u8, (i % 37) as u8, 255])
            .collect();
        let sleeve = Sleeve::new(pixels.into(), 128, 128).unwrap();
        let time = |f: &dyn Fn()| {
            let start = Instant::now();
            for _ in 0..20 {
                f();
            }
            start.elapsed() / 20
        };
        let baked = time(&|| {
            bake(&sleeve, true, 1.0);
        });
        let field = time(&|| {
            field(&sleeve);
        });
        println!("bake {baked:?}, field {field:?} per record");
    }

    /// The spline drawn finer holds a flat field flat and a ramp straight,
    /// and is as smooth as the curve: no step between drawn texels larger
    /// than the field's own between its control points.
    #[test]
    fn the_field_is_drawn_as_its_spline() {
        let n = FIELD as usize;
        let flat = vec![[0.4, 0.1, -0.1]; n * n];
        for v in spline_up(&flat, n, 128) {
            for c in 0..3 {
                assert!((v[c] - flat[0][c]).abs() < 1e-12);
            }
        }
        let ramp: Vec<[f64; 3]> = (0..n * n)
            .map(|i| [(i % n) as f64 / n as f64, 0.0, 0.0])
            .collect();
        let drawn = spline_up(&ramp, n, 128);
        // Inside the held edges, a B-spline is exact on a straight line.
        for x in 16..112 {
            let t = (x as f64 + 0.5) / 128.0 * n as f64 - 0.5;
            assert!((drawn[64 * 128 + x][0] - t / n as f64).abs() < 1e-9, "{x}");
        }
    }

    /// Quantised to 8 bits after the spline, the drawn field read with a
    /// linear filter stays within half a level of the exact curve across a
    /// 6K window on a slow dark gradient: no more than the field read by a
    /// spline on the GPU erred, and under the program's dither of a level.
    #[test]
    fn the_drawn_field_bands_no_more_than_the_spline() {
        let n = FIELD as usize;
        let m = FIELD_DRAWN as usize;
        let row: Vec<f64> = (0..n)
            .map(|i| 0.10 + 0.04 * (0.5 + 0.5 * (i as f64 / n as f64 * 3.0).sin()))
            .collect();
        let field: Vec<[f64; 3]> = (0..n * n).map(|i| [row[i % n], 0.0, 0.0]).collect();
        let exact = spline_up(&field, n, 4096);
        let drawn: Vec<f64> = spline_up(&field, n, m)
            .iter()
            .map(|v| (v[0] * 255.0).round() / 255.0)
            .collect();
        let line = |j: usize| drawn[(m / 2) * m + j.min(m - 1)];
        let mut worst: f64 = 0.0;
        for k in 0..4096 {
            let t = (k as f64 + 0.5) / 4096.0 * m as f64 - 0.5;
            let i = t.floor().max(0.0);
            let f = (t - i).clamp(0.0, 1.0);
            let read = line(i as usize) + (line(i as usize + 1) - line(i as usize)) * f;
            worst = worst.max((read - exact[2048 * 4096 + k][0]).abs() * 255.0);
        }
        assert!(worst <= 0.55, "{worst} of a level");
    }

    #[test]
    fn the_field_is_the_blurred_sleeve_in_little() {
        let red = Sleeve::new([220, 20, 20, 255].repeat(64 * 64).into(), 64, 64).unwrap();
        let field = field(&red);
        assert_eq!(field.dimensions(), (FIELD_DRAWN, FIELD_DRAWN));
        let [b, a_ch, l, alpha] = field.get_pixel(FIELD_DRAWN / 2, FIELD_DRAWN / 2).0;
        let lab = [
            l as f64 / 255.0,
            (a_ch as f64 / 255.0 - 0.5) * LAB_SPAN,
            (b as f64 / 255.0 - 0.5) * LAB_SPAN,
        ];
        let [r, g, bl] = super::super::accent::oklch::linear_from_lab(lab)
            .map(super::super::accent::oklch::gamma);
        assert!(
            r > 0.75 && g < 0.25 && bl < 0.25,
            "red {r} green {g} blue {bl}"
        );
        assert_eq!(alpha, 255);
    }

    fn solid(r: u8, g: u8, b: u8) -> Sleeve {
        Sleeve::new([r, g, b, 255].repeat(64 * 64).into(), 64, 64).unwrap()
    }

    fn y(pixel: [u8; 4]) -> f64 {
        let [b, g, r, _] = pixel.map(|v| super::super::accent::oklch::linear(v as f64 / 255.0));
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    #[test]
    fn baked_red_stays_red_in_bgra() {
        let baked = bake(&solid(220, 20, 20), true, 1.0);
        // The middle of the texture, whatever size it is baked at.
        let [b, _, r, a] = baked.get_pixel(baked.width() / 2, baked.height() / 2).0;
        assert!(r > b, "red {r} blue {b}");
        assert_eq!(a, 255);
    }

    #[test]
    fn a_white_sleeve_is_held_under_the_dark_limit_and_light_over_its() {
        // Over bg at 0.6, every sample stays dim enough for muted text.
        let dark = bake(&solid(255, 255, 255), true, 1.0);
        let muted = 0.2831;
        let contrast = (muted + 0.05) / (y(dark.get_pixel(10, 10).0) + 0.05);
        assert!(contrast >= 4.5, "contrast {contrast}");
        // Over white at 0.6, as light as `surface` (Y 0.888) at least.
        let light = bake(&solid(0, 0, 0), false, 1.0);
        let surface = super::super::accent::oklch::luminance(
            Palette::LIGHT.surface.channels().map(|v| v as f64 / 255.0),
        );
        assert!(
            y(light.get_pixel(10, 10).0) >= surface - 0.005,
            "light wash too dark"
        );
    }

    /// A level between two of 8 bits comes out as a mix of both that
    /// averages to it, where rounding would give one level everywhere and,
    /// across a slow gradient, a band at each step.
    #[test]
    fn a_level_between_two_is_dithered_to_its_mean() {
        let n = (SIDE * SIDE) as usize;
        let baked = dither_up(&[vec![100.4; n], vec![30.0; n], vec![200.75; n]]);
        let mean =
            |c: usize| baked.pixels().map(|p| p.0[c] as f64).sum::<f64>() / (OUT * OUT) as f64;
        // BGRA.
        assert!((mean(2) - 100.4).abs() < 0.02, "red {}", mean(2));
        assert!((mean(0) - 200.75).abs() < 0.02, "blue {}", mean(0));
        let levels: std::collections::BTreeSet<u8> = baked.pixels().map(|p| p.0[2]).collect();
        assert!(levels.contains(&100) && levels.contains(&101), "{levels:?}");
        assert!(levels.iter().all(|l| (99..=102).contains(l)), "{levels:?}");
    }

    #[test]
    fn a_sleeve_must_hold_its_pixels() {
        assert!(Sleeve::new([0u8; 7].into(), 2, 1).is_none());
        assert!(Sleeve::new([0u8; 0].into(), 0, 0).is_none());
    }

    #[test]
    fn the_blur_keeps_the_edge() {
        let mut channel = vec![1.0; 32 * 32];
        blur(&mut channel, 32);
        assert!(channel.iter().all(|v| (v - 1.0).abs() < 1e-9));
    }

    fn texture(generation: u64) -> Texture {
        let frame = image::Frame::new(image::RgbaImage::new(1, 1));
        Texture {
            image: Arc::new(RenderImage::new(SmallVec::from_elem(frame, 1))),
            generation,
            dark: false,
            hold: 0,
        }
    }

    /// Taking the wash off while a record dissolves in fades what shows from
    /// where it stands: the record coming in at its part-way opacity, the
    /// one under it whole, both toward the ground.
    #[test]
    fn taken_off_mid_dissolve_it_fades_from_where_it_stands() {
        let mut wash = Wash::new();
        wash.install(Some(texture(1)), false);
        wash.install(Some(texture(2)), true);
        let now = Instant::now();
        // Half-way in.
        if let Some(current) = wash.current.as_mut() {
            current.opacity.started = Some(now - EASE / 2);
        }
        let coming = wash.current.as_ref().unwrap().opacity.at(now);
        assert!(coming > 0.2 && coming < 0.8);

        wash.install(None, true);
        let current = wash
            .current
            .as_ref()
            .expect("the record coming in still shows");
        let previous = wash.previous.as_ref().expect("and the one under it");
        let then = Instant::now();
        assert!(
            (current.opacity.at(then) - coming).abs() < 0.05,
            "no jump to the newer record"
        );
        assert!(previous.opacity.at(then) > 0.95);
        assert_eq!((current.opacity.to, previous.opacity.to), (0.0, 0.0));
    }

    #[test]
    fn at_rest_the_pose_is_home() {
        let wash = Wash::new();
        assert_eq!(wash.pose(Instant::now()), (1.0, 0.0));
    }
}
