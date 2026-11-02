//! The playing bars: three bars standing as high as the music's low, middle
//! and high bands, beside whatever is playing.
//!
//! Where the renderer draws shader quads (macOS), the bars are one quad whose
//! fragment program draws them from the bands in its uniforms. The bands are
//! written from any thread and read as each frame is presented, so a moving
//! bar costs the GPU a quad and the CPU no frame; the quad is painted once
//! and replayed with its row's cached view. Elsewhere the bars are three
//! quads per slot, painted over the window's content by whoever holds the
//! slots' places (`BarsPlace::paint`, inside `bars_overlay`).
//!
//! What moves the bands is the app's: the element only asks, as it is laid
//! out, for the levels to draw (`PlayingBars::new`).

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;

use super::BarMetrics;

/// How far a bar stands with nothing playing through it, so a still bar reads
/// as a mark rather than as nothing.
pub const FLOOR: f32 = 0.15;

/// Whether this platform's renderer draws the bars as a shader.
pub const SHADED: bool = cfg!(target_os = "macos");

/// The bars as a fragment program. Uniforms: 0..3 the bands, 4..8 the
/// colour, 8 a bar's width and 9 the gap, in device pixels, 10 the floor.
pub static BARS: ShaderProgram = ShaderProgram {
    name: "koan-playing-bars",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float pitch = u[8] + u[9];
    int i = int(floor(position.x / pitch));
    if (i < 0 || i > 2 || position.x - float(i) * pitch > u[8]) {
        return float4(0.0);
    }
    float top = size.y * (1.0 - max(u[i], u[10]));
    float cover = clamp(position.y - top + 0.5, 0.0, 1.0);
    return float4(u[4], u[5], u[6], u[7] * cover);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>) -> vec4<f32> {
    let pitch = u[2].x + u[2].y;
    let i = i32(floor(position.x / pitch));
    if (i < 0 || i > 2 || position.x - f32(i) * pitch > u[2].x) {
        return vec4<f32>(0.0);
    }
    var bands = u[0];
    let top = size.y * (1.0 - max(bands[i], u[2].z));
    let cover = clamp(position.y - top + 0.5, 0.0, 1.0);
    return vec4<f32>(u[1].xyz, u[1].w * cover);
}
"#,
    prepare: None,
};

/// One slot's bands as the renderer reads them: shared, written from any
/// thread. Live while the bands move, which keeps the window presenting; see
/// GPUI's `ShaderUniforms`.
#[derive(Clone, Default)]
pub struct BarLevels(Arc<ShaderUniforms>);

impl BarLevels {
    pub fn new() -> Self {
        Self::default()
    }

    /// How high each bar stands, 0 to 1, low band to high.
    pub fn set(&self, bands: [f32; 3]) {
        for (i, band) in bands.into_iter().enumerate() {
            self.0.set(i, band);
        }
    }

    pub fn band(&self, i: usize) -> f32 {
        self.0.get(i)
    }

    pub fn set_live(&self, live: bool) {
        self.0.set_live(live);
    }

    pub fn is_live(&self) -> bool {
        self.0.is_live()
    }

    /// The look, in device pixels: written as the slot is laid out, a
    /// change only when the colour or the scale changed.
    fn style(&self, color: Hsla, scale: f32) {
        let rgba = color.to_rgb();
        for (i, v) in [rgba.r, rgba.g, rgba.b, rgba.a].into_iter().enumerate() {
            self.0.set(4 + i, v);
        }
        self.0.set(8, BarMetrics::BAR * scale);
        self.0.set(9, BarMetrics::GAP * scale);
        self.0.set(10, FLOOR);
    }
}

/// Where a slot was laid out and the clip it was laid out under, for painting
/// its bars as quads over the window's content.
#[derive(Clone, Copy)]
pub struct BarsPlace {
    pub window: AnyWindowHandle,
    bounds: Bounds<Pixels>,
    mask: ContentMask<Pixels>,
}

impl BarsPlace {
    /// The bars at `bands` in `color`, as quads. Called from `bars_overlay`.
    pub fn paint(&self, window: &mut Window, bands: [f32; 3], color: Hsla) {
        let b = self.bounds;
        window.with_content_mask(Some(self.mask), |window| {
            for (i, band) in bands.iter().enumerate() {
                let height = b.size.height * band.max(FLOOR);
                let x = b.origin.x + px(i as f32 * (BarMetrics::BAR + BarMetrics::GAP));
                let y = b.origin.y + b.size.height - height;
                window.paint_quad(fill(
                    Bounds::new(point(x, y), size(px(BarMetrics::BAR), height)),
                    color,
                ));
            }
        });
    }
}

type Attach = dyn Fn(BarsPlace, &mut Window, &mut App) -> Option<BarLevels>;

/// A slot of playing bars in `color`, `BarMetrics` in size. As it is laid
/// out, `attach` is told where, and returns the levels to draw as a shader,
/// or `None` where the bars are painted as quads (`BarsPlace::paint`). Keep
/// a slot's state under its element id (`window.use_keyed_state`).
#[derive(IntoElement)]
pub struct PlayingBars {
    color: Hsla,
    attach: Rc<Attach>,
}

impl PlayingBars {
    pub fn new(
        color: Hsla,
        attach: impl Fn(BarsPlace, &mut Window, &mut App) -> Option<BarLevels> + 'static,
    ) -> Self {
        Self {
            color,
            attach: Rc::new(attach),
        }
    }
}

impl RenderOnce for PlayingBars {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Self { color, attach } = self;
        canvas(
            move |bounds, window, cx| {
                let place = BarsPlace {
                    window: window.window_handle(),
                    bounds,
                    mask: window.content_mask(),
                };
                let levels = attach(place, window, cx).filter(|_| SHADED)?;
                levels.style(color, window.scale_factor());
                Some(levels)
            },
            |bounds, levels, window, _| {
                if let Some(levels) = levels {
                    window.paint_shader(bounds, &BARS, levels.0);
                }
            },
        )
        .w(BarMetrics::WIDTH)
        .h(BarMetrics::HEIGHT)
        .flex_shrink_0()
    }
}

/// Covers its parent and runs `paint` after everything drawn before it: where
/// a window's root paints the bars that are quads.
pub fn bars_overlay(paint: impl Fn(&mut Window) + 'static) -> impl IntoElement {
    canvas(|_, _, _| {}, move |_, _, window, _| paint(window))
        .absolute()
        .size_full()
}
