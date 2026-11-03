//! A picture arriving: a sleeve that fades in over what stood before it (the
//! record's colour, or the ensō drawing itself), drawn as a shader quad that
//! samples the picture, its opacity a track the GPU moves (#1859). The view
//! that shows it draws once as it arrives and once as it lands, to let go of
//! what stood beneath; no frame between.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;

use super::{Animated, Ease, Repeat, SHADED, ShaderProgram};

/// The picture, cropped to fill its bounds as `ObjectFit::Cover` does, at the
/// opacity of track 0. Uniforms: 0..2 the crop's offset, 2..4 its scale, in
/// the picture's 0-to-1 coordinates.
static ARRIVE: ShaderProgram = ShaderProgram {
    name: "koan-arrive",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float2 uv = float2(u[0], u[1]) + position / size * float2(u[2], u[3]);
    float4 colour = koan_sample(image, tile, uv);
    return float4(colour.rgb, colour.a * clamp(koan_track(u, 0, time), 0.0, 1.0));
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let uv = u[0].xy + position / size * u[0].zw;
    let colour = koan_sample(tile, uv);
    return vec4<f32>(colour.rgb, colour.a * clamp(koan_track(u, 0u, time), 0.0, 1.0));
}
"#,
    prepare: None,
};

/// One picture's arrival: its opacity moving from nothing to whole over
/// `length` from when it was made. Kept by whoever shows the picture, for as
/// long as it fades.
#[derive(Clone)]
pub struct Arrival {
    animated: Animated,
    lands: Instant,
}

impl Arrival {
    pub fn new(length: Duration) -> Self {
        Self::since(Animated::now(), length)
    }

    /// An arrival that began at `start`: a picture decoded a moment before
    /// the view that shows it heard of it.
    pub fn since(start: Instant, length: Duration) -> Self {
        let animated = Animated::new();
        animated.segment(0, 0.0, 1.0, start, length, Ease::QuickOut, Repeat::Once);
        Self {
            animated,
            lands: start + length,
        }
    }

    /// When the picture stands whole: the moment to draw once more, without
    /// what stood beneath it.
    pub fn lands(&self) -> Instant {
        self.lands
    }

    pub fn landed(&self) -> bool {
        Animated::now() >= self.lands
    }
}

/// `image` filling its bounds, fading in by `arrival`. Under Reduce Motion,
/// and where the renderer draws no shaders, it stands whole at once.
pub fn arriving(image: Arc<RenderImage>, arrival: &Arrival) -> AnyElement {
    if !SHADED {
        return img(image)
            .object_fit(ObjectFit::Cover)
            .size_full()
            .into_any_element();
    }
    let animated = arrival.animated.clone();
    let source = image.size(0);
    canvas(
        move |bounds, window, cx| {
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let (iw, ih) = (source.width.0 as f32, source.height.0 as f32);
            let (sx, sy) = if w <= 0.0 || h <= 0.0 || iw <= 0.0 || ih <= 0.0 {
                (1.0, 1.0)
            } else if iw / ih > w / h {
                ((w / h) / (iw / ih), 1.0)
            } else {
                (1.0, (iw / ih) / (w / h))
            };
            animated.set_all(0, &[(1.0 - sx) / 2.0, (1.0 - sy) / 2.0, sx, sy]);
            animated.attach(true, window, cx);
            animated
        },
        move |bounds, animated, window, _| animated.paint(&ARRIVE, Some(image), bounds, window),
    )
    .size_full()
    .into_any_element()
}
