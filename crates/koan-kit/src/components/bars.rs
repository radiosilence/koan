//! The playing bars: three bars standing as high as the music's low, middle
//! and high bands, beside whatever is playing.
//!
//! Where the renderer draws shader quads (macOS), the bars are an
//! `AnimatedQuad` whose program draws them from the bands in its uniforms
//! (`BarLevels`, an `Animated` driven by values). A change of the bands is
//! presented at the next refresh with no frame drawn; the quad is painted
//! once and replayed with its row's cached view. Elsewhere the bars are three
//! quads per slot, painted over the window's content by whoever holds the
//! slots' places (`BarsPlace::paint`, inside `bars_overlay`).
//!
//! What moves the bands is the app's: the element only asks, as it is laid
//! out, for the levels to draw (`PlayingBars::new`).

use std::rc::Rc;

use gpui_kit::*;

use super::{Animated, BarMetrics, SHADED};

/// How far a bar stands with nothing playing through it, so a still bar reads
/// as a mark rather than as nothing.
pub const FLOOR: f32 = 0.15;

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
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
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

/// One slot's bands, written from any thread; a change is presented at the
/// next refresh.
#[derive(Clone, Default)]
pub struct BarLevels(Animated);

impl BarLevels {
    pub fn new() -> Self {
        Self::default()
    }

    /// How high each bar stands, 0 to 1, low band to high.
    pub fn set(&self, bands: [f32; 3]) {
        self.0.set_all(0, &bands);
    }

    pub fn band(&self, i: usize) -> f32 {
        self.0.get(i)
    }

    /// The look, in device pixels: written as the slot is laid out, a
    /// change only when the colour or the scale changed.
    fn style(&self, color: Hsla, scale: f32) {
        let rgba = color.to_rgb();
        self.0.set_all(
            4,
            &[
                rgba.r,
                rgba.g,
                rgba.b,
                rgba.a,
                BarMetrics::BAR * scale,
                BarMetrics::GAP * scale,
                FLOOR,
            ],
        );
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
                let bars = attach(place, window, cx).filter(|_| SHADED)?;
                bars.style(color, window.scale_factor());
                bars.0.attach(false, window, cx);
                Some(bars)
            },
            |bounds, bars, window, _| {
                if let Some(bars) = bars {
                    bars.0.paint(&BARS, None, bounds, window);
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

/// In the playing bars' place while the music plays on a renderer that is
/// handed the file itself: nothing passes through here to measure, so the
/// slot says where it plays rather than standing still, and its tooltip why.
pub fn renderer_mark(color: Hsla) -> impl IntoElement {
    div()
        .id("renderer-mark")
        .tooltip(|window, cx| {
            super::Tooltip::view(
                "playing on a network speaker, which plays the file itself".into(),
                window,
                cx,
            )
        })
        .flex_shrink_0()
        .h(BarMetrics::HEIGHT)
        .flex()
        .items_center()
        .child(
            crate::theme::KoanIcon::new(crate::theme::Icon::Renderer)
                .points(f32::from(BarMetrics::HEIGHT))
                .color(color),
        )
}
