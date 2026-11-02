// KOAN: this module is koan's addition (#1641): a quad drawn by a fragment
// program the app supplies, with a small block of uniforms the app may write
// from any thread. The uniforms are read when a frame is presented, not when
// it is drawn, and while any shader on screen is live the window presents its
// last frame again each refresh without drawing a new one. Something that
// changes every frame then costs the GPU a quad and the CPU an encode, with no
// layout, prepaint or paint.

#![allow(missing_docs)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use crate::{AtlasTile, Bounds, ContentMask, DrawOrder, Primitive, ScaledPixels};

/// How many floats a shader's uniform block holds.
pub const SHADER_UNIFORMS: usize = 16;

/// A fragment program, written once for each renderer.
///
/// Each source defines one function, `shade`, which returns the colour of a
/// pixel of the quad, not premultiplied:
///
/// - Metal: `float4 shade(float2 position, float2 size, float time, constant float *uniforms, texture2d<float> image, float4 tile)`,
///   where `koan_sample(image, tile, uv)` reads the shader's image
///   (`Window::paint_shader_image`) at `uv`, 0 to 1 across it, filtered and
///   kept to its edge
/// - WGSL: `fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, uniforms: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32>`,
///   where `koan_sample(tile, uv)` reads the image the same way
///
/// `position` is the pixel's centre within the quad and `size` the quad's
/// size, both in device pixels; `time` is seconds since the uniforms' origin
/// ([`ShaderUniforms::set_origin`], or [`shader_epoch`] if none was set),
/// reckoned in double precision as the frame is presented, so it stays exact
/// however long the app runs.
/// Programs are compiled the first time they are drawn and kept, by `name`.
#[derive(Debug)]
pub struct ShaderProgram {
    /// Unique among the app's programs: what the compiled program is kept by.
    pub name: &'static str,
    pub msl: &'static str,
    pub wgsl: &'static str,
    /// Run on the CPU once per quad as each frame is presented, given its
    /// uniforms and `time`, to write values every pixel would otherwise
    /// work out alike (a pose, an eased opacity) into the uniforms the
    /// program reads. The app's copy is left as it was.
    pub prepare: Option<fn(&mut [f32; SHADER_UNIFORMS], f32)>,
}

/// The instant shader time counts from when the uniforms set no origin.
pub fn shader_epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}


/// A shader's uniforms, shared between the app and the renderer: written by
/// the app from any thread, read as each frame is presented.
///
/// A refresh presents the last frame again, with no new frame drawn, where
/// a shader on screen changed since the last present, or on every refresh
/// for a `live` and `continuous` shader, which moves with `time` alone.
/// While any shader on screen is `live` the window keeps its refreshes
/// coming; otherwise it parks, and a change is seen at the next refresh,
/// which `Window::request_present` asks for.
#[derive(Debug, Default)]
pub struct ShaderUniforms {
    values: [AtomicU32; SHADER_UNIFORMS],
    live: AtomicBool,
    continuous: AtomicBool,
    changed: AtomicBool,
    /// Nanoseconds from [`shader_epoch`] to the origin `time` counts from.
    origin: AtomicU64,
}

impl ShaderUniforms {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the value changed.
    pub fn set(&self, index: usize, value: f32) -> bool {
        let bits = value.to_bits();
        let changed = self.values[index].swap(bits, Ordering::Relaxed) != bits;
        if changed {
            self.changed.store(true, Ordering::Relaxed);
        }
        changed
    }

    pub fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    /// The moment `time` counts from in the program: where an anchor was
    /// taken, so the program reckons from it.
    pub fn set_origin(&self, origin: Instant) -> bool {
        let nanos = origin.saturating_duration_since(shader_epoch()).as_nanos() as u64;
        let changed = self.origin.swap(nanos, Ordering::Relaxed) != nanos;
        if changed {
            self.changed.store(true, Ordering::Relaxed);
        }
        changed
    }

    /// Seconds from the origin to `now`, as the program reads `time`.
    pub fn time_at(&self, now: Instant) -> f32 {
        let now = now.saturating_duration_since(shader_epoch()).as_nanos() as f64;
        ((now - self.origin.load(Ordering::Relaxed) as f64) / 1e9) as f32
    }

    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Relaxed);
    }

    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// Presented on every refresh while live, for a shader moved by `time`.
    pub fn set_continuous(&self, continuous: bool) {
        self.continuous.store(continuous, Ordering::Relaxed);
    }

    /// Whether this refresh should present: changed since the last time
    /// this was asked, or live and continuous.
    pub fn take_present(&self) -> bool {
        let changed = self.changed.swap(false, Ordering::Relaxed);
        changed || (self.is_live() && self.continuous.load(Ordering::Relaxed))
    }

    /// Every value, as a renderer copies them into a frame.
    pub fn snapshot(&self) -> [f32; SHADER_UNIFORMS] {
        std::array::from_fn(|i| self.get(i))
    }
}

/// A shader as the scene holds it.
#[derive(Clone, Debug)]
pub struct PaintShader {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub program: &'static ShaderProgram,
    pub uniforms: std::sync::Arc<ShaderUniforms>,
    /// The image the program samples, in the sprite atlas.
    pub image: Option<AtlasTile>,
}

impl From<PaintShader> for Primitive {
    fn from(shader: PaintShader) -> Self {
        Primitive::Shader(shader)
    }
}

/// One shader as a renderer hands it to the GPU: its quad, its clip and its
/// uniforms and time as they were when the frame was presented. Laid out for
/// both Metal and WGSL: blocks of 16-byte aligned floats.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct ShaderInstance {
    /// x, y, width, height.
    pub bounds: [f32; 4],
    /// The content mask: x, y, width, height.
    pub clip: [f32; 4],
    pub uniforms: [f32; SHADER_UNIFORMS],
    /// `time` first; the rest pads the block.
    pub time: [f32; 4],
    /// The image's tile in its atlas texture, in texels: x, y, width, height.
    pub tile: [f32; 4],
}

impl ShaderInstance {
    pub fn of(shader: &PaintShader, now: Instant) -> Self {
        let time = shader.uniforms.time_at(now);
        let mut uniforms = shader.uniforms.snapshot();
        if let Some(prepare) = shader.program.prepare {
            prepare(&mut uniforms, time);
        }
        let rect = |b: &Bounds<ScaledPixels>| {
            [
                b.origin.x.0,
                b.origin.y.0,
                b.size.width.0,
                b.size.height.0,
            ]
        };
        Self {
            bounds: rect(&shader.bounds),
            clip: rect(&shader.content_mask.bounds),
            uniforms,
            time: [time, 0.0, 0.0, 0.0],
            tile: shader.image.as_ref().map_or([0.0; 4], |tile| {
                let b = tile.bounds;
                [
                    b.origin.x.0 as f32,
                    b.origin.y.0 as f32,
                    b.size.width.0 as f32,
                    b.size.height.0 as f32,
                ]
            }),
        }
    }
}
