//! The record wash: the playing record's sleeve, blurred to colour fields and
//! drifting, behind the bare ground of a page. The one thing under the ground
//! that is not flat, and it carries data. The Apple apps' `DriftingWash` and
//! Android's `RecordWash`.
//!
//! Each record is baked once per appearance into a small opaque texture
//! (`bake`): saturated, blurred, held to the tone's luminance limit and laid
//! over `bg` at the wash's strength.
//!
//! Where the renderer draws shaders (macOS), the drift is the GPU's: each
//! texture is one shader quad (`WASH`) handed the drift's and the dissolve's
//! ramps once per render, and its pose and opacity are worked out as each
//! frame is presented (`prepare`), so a drifting wash costs no GPUI frame and
//! the window re-presents its layers around it. The wash renders only when a
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
//! The Apple apps also turn the texture through ±3°. GPUI's image sprites take
//! no transform, so here the drift is the breath and the travel alone.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    Bounds, Context, Corners, IntoElement, ParentElement, Pixels, Render, RenderImage,
    ShaderProgram, ShaderUniforms, Styled, Task, Window, canvas, div, point, px, size,
};
use smallvec::SmallVec;

use super::{KoanTheme, Look, Motion, Palette};
use crate::components::SHADED;

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
/// A dissolve between records, and the drift starting or coming to rest:
/// long enough that the room is seen to have changed without being caught
/// changing.
const EASE: Duration = Duration::from_secs(2);
/// The drift's whole cycle: both swings back where they started. The shader's
/// clock is counted from a multiple of it, so it stays small and exact.
const CYCLE: f32 = 598.0;

/// A texture of the wash as a fragment program. Uniforms: 0..3 the drift's
/// amplitude ramp (from, to, start in seconds of shader time, NaN for one
/// standing still), 3..6 the opacity's, 6 whether to lay it over `bg` (the
/// layer drawn first), 7 the overscan, 8..11 `bg`, 11 the reach and 15 the
/// rise; `prepare` writes the scale (12), travel (13) and opacity (14).
pub static WASH: ShaderProgram = ShaderProgram {
    name: "koan-wash",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float side = max(size.x, size.y) * u[7] * u[12];
    float2 centre = size * 0.5 + float2(size.x * u[11], size.y * u[15]) * u[13];
    float2 uv = (position - (centre - side * 0.5)) / side;
    float3 colour = koan_sample(image, tile, uv).rgb;
    if (u[6] > 0.5) {
        return float4(mix(float3(u[8], u[9], u[10]), colour, u[14]), 1.0);
    }
    return float4(colour, u[14]);
}
"#,
    wgsl: "",
    prepare: Some(prepare),
};

/// The pose and opacity at `time`, for the quad's every pixel alike.
fn prepare(u: &mut [f32; 16], time: f32) {
    let ramp = |from: f32, to: f32, start: f32| {
        if start.is_nan() {
            to
        } else {
            from + (to - from) * Motion::ease_in_out((time - start) / EASE.as_secs_f32())
        }
    };
    let amplitude = ramp(u[0], u[1], u[2]);
    let (scale, travel) = pose_at(time, amplitude);
    u[12] = scale;
    u[13] = travel;
    u[14] = ramp(u[3], u[4], u[5]);
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
    let source = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(
        sleeve.width,
        sleeve.height,
        &sleeve.rgba[..],
    )
    .expect("a sleeve is made with its length checked");
    let small = image::imageops::resize(&source, SIDE, SIDE, image::imageops::FilterType::Triangle);
    let tone = if dark { Tone::Dark } else { Tone::Light };
    let ground = Palette::of(dark).bg.channels();
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

    /// As the shader takes it: from, to, and the start in seconds after
    /// `origin`, NaN for one standing still.
    fn uniforms(self, origin: Instant) -> [f32; 3] {
        let start = self.started.map_or(f32::NAN, |started| {
            if started >= origin {
                started.duration_since(origin).as_secs_f32()
            } else {
                -origin.duration_since(started).as_secs_f32()
            }
        });
        [self.from, self.to, start]
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
    shader: Arc<ShaderUniforms>,
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
                shader: Arc::default(),
            });
            return;
        }
        if let Some(outgoing) = outgoing {
            // Held whole under the new record, however far its own
            // dissolve had come, or faded out to the ground when there is
            // none: a part-faded record held under the next would let the
            // ground show through for the length of a dissolve.
            let opacity = if texture.is_some() {
                Ramp::still(1.0)
            } else {
                Ramp::still(1.0).toward(0.0, now)
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
        self.current = texture.map(|texture| Layer {
            texture,
            opacity: Ramp::still(0.0).toward(1.0, now),
            shader: Arc::default(),
        });
    }

    fn start_bake(&mut self, dark: bool, cx: &mut Context<Self>) {
        let Some(sleeve) = self.sleeve.clone() else {
            return;
        };
        let key = (self.generation, dark, self.hold);
        if self.baked == Some(key) {
            return;
        }
        self.baked = Some(key);
        let hold = self.hold;
        let task = cx
            .background_executor()
            .spawn(async move { bake(&sleeve, dark, hold as f64 / 100.0) });
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

    /// Where the texture is this frame: its scale over the overscan, and its
    /// travel across the page from -1 to 1.
    fn pose(&self, now: Instant) -> (f32, f32) {
        let t = now.duration_since(self.epoch).as_secs_f32();
        pose_at(t, self.amplitude.at(now))
    }

    /// The drift's clock for the shader: the last whole cycle since the
    /// epoch, so the phase the shader works out is the one `pose` would.
    fn origin(&self, now: Instant) -> Instant {
        let cycles = (now.duration_since(self.epoch).as_secs_f64() / CYCLE as f64).floor();
        self.epoch + Duration::from_secs_f64(cycles * CYCLE as f64)
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
                    || layer.texture.dark != look.dark
                    || layer.texture.hold != self.hold
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
        for image in self.released.drain(..) {
            // After the frame that last drew it.
            window.on_next_frame(move |window, _| {
                window.drop_image(image).ok();
            });
        }

        let shown = self.current.is_some() || self.previous.is_some();
        let target = if self.wanted && shown && KoanTheme::motion_allowed(cx) {
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
        if SHADED {
            return self
                .render_shaded(moving, now, &look, cx)
                .into_any_element();
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
        moving: bool,
        now: Instant,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let origin = self.origin(now);
        let amplitude = self.amplitude.uniforms(origin);
        let bg = look.palette.bg.hsla().to_rgb();
        let mut quads: Vec<(Arc<RenderImage>, Arc<ShaderUniforms>)> = Vec::new();
        let mut rests = self.amplitude.rests(now);
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
            let values = amplitude
                .into_iter()
                .chain(opacity.uniforms(origin))
                .chain([if first { 1.0 } else { 0.0 }, NEAR, bg.r, bg.g, bg.b, REACH])
                .enumerate();
            for (i, v) in values {
                shader.set(i, v);
            }
            shader.set(15, RISE);
            shader.set_origin(origin);
            shader.set_continuous(moving);
            shader.set_live(moving);
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
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        for (image, shader) in quads {
                            window.paint_shader_image(bounds, &WASH, shader, image).ok();
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
        let [b, _, r, a] = baked.get_pixel(64, 64).0;
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

    #[test]
    fn at_rest_the_pose_is_home() {
        let wash = Wash::new();
        assert_eq!(wash.pose(Instant::now()), (1.0, 0.0));
    }
}
