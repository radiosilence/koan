// KOAN: this module is koan's addition (#1641): a quad drawn by a fragment
// program the app supplies, with a small block of uniforms the app may write
// from any thread. The uniforms are read when a frame is presented, not when
// it is drawn, and while any shader on screen is live the window presents its
// last frame again each refresh without drawing a new one. Something that
// changes every frame then costs the GPU a quad and the CPU an encode, with no
// layout, prepaint or paint.

#![allow(missing_docs)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use crate::{Bounds, ContentMask, DrawOrder, Primitive, ScaledPixels};

/// How many floats a shader's uniform block holds.
pub const SHADER_UNIFORMS: usize = 16;

/// A fragment program, written once for each renderer.
///
/// Each source defines one function, `shade`, which returns the colour of a
/// pixel of the quad, not premultiplied:
///
/// - Metal: `float4 shade(float2 position, float2 size, float time, constant float *uniforms)`
/// - WGSL: `fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, uniforms: array<vec4<f32>, 4>) -> vec4<f32>`
///
/// `position` is the pixel's centre within the quad and `size` the quad's
/// size, both in device pixels; `time` is seconds since [`shader_epoch`].
/// Programs are compiled the first time they are drawn and kept, by `name`.
#[derive(Debug)]
pub struct ShaderProgram {
    /// Unique among the app's programs: what the compiled program is kept by.
    pub name: &'static str,
    pub msl: &'static str,
    pub wgsl: &'static str,
}

/// The instant shader time counts from: `time` in a program is seconds since
/// it, so an app can hand a program a moment in the same clock.
pub fn shader_epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Seconds since [`shader_epoch`] at `at`, as a program reads `time`.
pub fn shader_time(at: Instant) -> f32 {
    at.saturating_duration_since(shader_epoch()).as_secs_f32()
}

/// A shader's uniforms, shared between the app and the renderer: written by
/// the app from any thread, read as each frame is presented.
///
/// While `live` is set the window presents again on every refresh, so the
/// shader can move with no new frame drawn. Setting it from false needs a
/// frame to notice: draw one (notify the view that paints the shader) when a
/// shader starts moving. Clearing it stops the presents within a refresh.
#[derive(Debug, Default)]
pub struct ShaderUniforms {
    values: [AtomicU32; SHADER_UNIFORMS],
    live: AtomicBool,
}

impl ShaderUniforms {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, index: usize, value: f32) {
        self.values[index].store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Relaxed);
    }

    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
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
}

impl From<PaintShader> for Primitive {
    fn from(shader: PaintShader) -> Self {
        Primitive::Shader(shader)
    }
}

/// One shader as a renderer hands it to the GPU: its quad, its clip and its
/// uniforms as they were when the frame was presented. Laid out for both
/// Metal and WGSL: three blocks of 16-byte aligned floats.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct ShaderInstance {
    /// x, y, width, height.
    pub bounds: [f32; 4],
    /// The content mask: x, y, width, height.
    pub clip: [f32; 4],
    pub uniforms: [f32; SHADER_UNIFORMS],
}

impl ShaderInstance {
    pub fn of(shader: &PaintShader) -> Self {
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
            uniforms: shader.uniforms.snapshot(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_round_trip_and_liveness_is_separate() {
        let u = ShaderUniforms::new();
        u.set(3, 0.25);
        assert_eq!(u.get(3), 0.25);
        assert_eq!(u.snapshot()[3], 0.25);
        assert!(!u.is_live());
        u.set_live(true);
        assert!(u.is_live());
    }

    #[test]
    fn the_instance_is_three_aligned_blocks() {
        assert_eq!(std::mem::size_of::<ShaderInstance>(), 4 * (4 + 4 + 16));
    }
}
