//! Something on its way: the ensō drawing itself, the brush's head running
//! round the circle and then its tail following it off, looping. The same
//! mark as the `Placeholder`, so loading and empty read as one family.
//!
//! Where the renderer draws shaders, one `AnimatedQuad` on the clock: the
//! stroke is baked once into a small field (`FIELD`), each texel holding how
//! far along the brush its nearest point is and how far from the brush's
//! line it lies, and the program draws the part between the tail and the
//! head at each present, edges and ends antialiased at any size. A spinner
//! costs no frame while it turns, every spinner turns in step, and one
//! scrolled away or behind a page is not in the scene. With Reduce Motion
//! the whole mark stands still.
//!
//! Elsewhere the stroke is laid out once per size as a run of round quads
//! close enough to read as one line, and each frame chooses which are drawn.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use futures::FutureExt as _;
use futures::future::Shared;
use std::time::{Duration, Instant};

use gpui_kit::*;
use smallvec::SmallVec;

use super::placeholder::TRACE;
use super::{Animated, AnimatedQuad, Ease, Repeat, SHADED, ShaderProgram};
use crate::theme::{KoanTheme, Look};

/// One loop: drawn, then undrawn.
const TURN: Duration = Duration::from_millis(1600);
/// The brush's width in the trace's 512-point space, the placeholder's.
const BRUSH: f32 = 48.0;
/// Quads per brush width along the stroke: enough that their overlap reads
/// as a line at any size.
const DENSITY: f32 = 4.0;

#[derive(IntoElement)]
pub struct Spinner {
    side: Pixels,
    /// The room what is loading will take, held so nothing moves when it
    /// arrives: the spinner is centred in it.
    reserve: Option<Size<Pixels>>,
    color: Option<Hsla>,
    /// What a screen reader announces while it shows, under its id.
    announce: Option<(ElementId, SharedString)>,
}

impl Spinner {
    /// `side` is the square it is drawn in.
    pub fn new(side: Pixels) -> Self {
        Self {
            side,
            reserve: None,
            color: None,
            announce: None,
        }
    }

    /// Standing for content on its way, as a loading pane's spinner does:
    /// a screen reader announces `label` ("loading keys") politely as it
    /// appears. A spinner beside words that already say so, or in a control
    /// that has its own name, stays decorative.
    pub fn announced(mut self, id: impl Into<ElementId>, label: impl AsRef<str>) -> Self {
        self.announce = Some((id.into(), crate::theme::label(label)));
        self
    }

    /// Drawn in `color` rather than `muted`: the placeholder's faded mark.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Holds exactly `room`, the size of what is loading, with the spinner
    /// in its middle.
    pub fn reserving(mut self, room: Size<Pixels>) -> Self {
        self.reserve = Some(room);
        self
    }
}

/// The stroke at one size: each quad's offset from the square's origin, in
/// order along the brush, and the quads' side.
struct Stroke {
    quads: Vec<Point<Pixels>>,
    brush: Pixels,
}

thread_local! {
    /// By side in points, a hundredth apart: a spinner comes in a few sizes,
    /// each laid out once.
    static STROKES: RefCell<HashMap<u32, Rc<Stroke>>> = RefCell::new(HashMap::new());
}

/// The trace's points, read once.
fn trace() -> &'static [(f32, f32)] {
    static POINTS: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    POINTS.get_or_init(|| {
        TRACE
            .split(['M', 'L'])
            .filter_map(|pair| {
                let mut xy = pair.split_whitespace().map(str::parse::<f32>);
                Some((xy.next()?.ok()?, xy.next()?.ok()?))
            })
            .collect()
    })
}

/// The quads along the trace, fitted to a square of `side`, the brush
/// inside it.
fn lay_out(side: f32) -> Stroke {
    let points = trace();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let extent = (max_x - min_x).max(max_y - min_y) + BRUSH;
    let scale = side / extent;
    let brush = BRUSH * scale;
    // Centred, the brush's half-width in from each edge.
    let off_x = (side - (max_x - min_x) * scale) / 2.0;
    let off_y = (side - (max_y - min_y) * scale) / 2.0;
    let at = |(x, y): (f32, f32)| ((x - min_x) * scale + off_x, (y - min_y) * scale + off_y);
    let step = brush / DENSITY;
    let mut quads = Vec::new();
    let mut carry = 0.0;
    for pair in points.windows(2) {
        let (a, b) = (at(pair[0]), at(pair[1]));
        let length = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let mut along = carry;
        while along < length {
            let t = along / length;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            quads.push(point(px(x - brush / 2.0), px(y - brush / 2.0)));
            along += step;
        }
        carry = along - length;
    }
    Stroke {
        quads,
        brush: px(brush),
    }
}

fn stroke(side: Pixels) -> Rc<Stroke> {
    let key = (f32::from(side) * 100.0).round() as u32;
    STROKES.with(|strokes| {
        strokes
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| Rc::new(lay_out(f32::from(side))))
            .clone()
    })
}

/// Where a spinner goes so its ensō lies exactly on the mark the
/// `Placeholder` draws at `mark` points a side (the trace's 512-point space
/// at that size): its square's side, and its centre's offset from the
/// mark's. The spinner fits the stroke's own extent, brush and all, to its
/// square; the mark draws the whole 512-point space, margins included. The
/// brush then comes out the mark's stroke width.
pub(crate) fn on_mark(mark: Pixels) -> (Pixels, Point<Pixels>) {
    let points = trace();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let unit = f32::from(mark) / 512.0;
    let extent = (max_x - min_x).max(max_y - min_y) + BRUSH;
    let centre = ((min_x + max_x) / 2.0 - 256.0, (min_y + max_y) / 2.0 - 256.0);
    (
        px(extent * unit),
        point(px(centre.0 * unit), px(centre.1 * unit)),
    )
}

/// Smooth at both ends, so the head eases in and out of each half.
fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Which of `count` quads are drawn at `phase` of a loop: the head running
/// round over the first half, the tail following it off over the second.
fn shown(phase: f32, count: usize) -> std::ops::Range<usize> {
    let (tail, head) = if phase < 0.5 {
        (0.0, ease(phase * 2.0))
    } else {
        (ease((phase - 0.5) * 2.0), 1.0)
    };
    let n = count as f32;
    (tail * n) as usize..(head * n).ceil() as usize
}

/// When the first spinner was drawn: every spinner turns in step.
static START: OnceLock<Instant> = OnceLock::new();

/// The field's side in texels: a distance field magnifies cleanly, so one
/// small bake serves every size.
const FIELD: u32 = 128;

/// The stroke as a fragment program sampling the field. Uniforms: 0 whether
/// it turns (else the whole mark), 2 the stroke's length and 3 the brush's
/// half-width, both in device pixels, 4 the field's distance range in device
/// pixels; 8..12 the colour. Track 0 the turn's phase, looping 0 to 1.
static ENSO: ShaderProgram = ShaderProgram {
    name: "koan-enso",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float4 field = koan_sample(image, tile, position / size);
    float along = field.r;
    float away = field.g * u[4];
    float cover = clamp(u[3] - away + 0.5, 0.0, 1.0);
    if (u[0] > 0.5) {
        float phase = koan_track(u, 0, time);
        float head = phase < 0.5 ? smoothstep(0.0, 1.0, phase * 2.0) : 1.0;
        float tail = phase < 0.5 ? 0.0 : smoothstep(0.0, 1.0, (phase - 0.5) * 2.0);
        cover *= clamp((head - along) * u[2] + 0.5, 0.0, 1.0);
        cover *= clamp((along - tail) * u[2] + 0.5, 0.0, 1.0);
    }
    return float4(u[8], u[9], u[10], u[11] * cover);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let field = koan_sample(tile, position / size);
    let along = field.r;
    let away = field.g * u[1].x;
    var cover = clamp(u[0].w - away + 0.5, 0.0, 1.0);
    if (u[0].x > 0.5) {
        let phase = koan_track(u, 0u, time);
        var head = 1.0;
        var tail = 0.0;
        if (phase < 0.5) {
            head = smoothstep(0.0, 1.0, phase * 2.0);
        } else {
            tail = smoothstep(0.0, 1.0, (phase - 0.5) * 2.0);
        }
        cover *= clamp((head - along) * u[0].z + 0.5, 0.0, 1.0);
        cover *= clamp((along - tail) * u[0].z + 0.5, 0.0, 1.0);
    }
    return vec4<f32>(u[2].xyz, u[2].w * cover);
}
"#,
    prepare: None,
};

/// The baked field and what it was baked from.
struct Field {
    image: Arc<RenderImage>,
    /// The stroke's length, in the field's units (the square is 1).
    length: f32,
    /// The distance that reads as 1 in the field, and the brush's
    /// half-width, in the same units.
    range: f32,
    half_brush: f32,
}

/// The field, baked once for every spinner.
static FIELD_IMAGE: OnceLock<Field> = OnceLock::new();

/// The bake in flight, for each spinner drawn before it lands to wait on.
struct Baking(Shared<Task<()>>);

impl Global for Baking {}

/// The field, once baked. The bake measures every texel against every
/// segment of the trace, too long for the main thread, so the first spinner
/// starts it on the background executor and draws its square empty; the
/// view that drew it is drawn again when the field lands.
fn baked(window: &mut Window, cx: &mut App) -> Option<&'static Field> {
    if let Some(field) = FIELD_IMAGE.get() {
        return Some(field);
    }
    let baking = match cx.try_global::<Baking>() {
        Some(baking) => baking.0.clone(),
        None => {
            let task = cx
                .background_executor()
                .spawn(async {
                    FIELD_IMAGE.get_or_init(bake_field);
                })
                .shared();
            cx.set_global(Baking(task.clone()));
            task
        }
    };
    let view = window.current_view();
    cx.spawn(async move |cx| {
        baking.await;
        cx.update(|cx| cx.notify(view));
    })
    .detach();
    None
}

/// For each texel of a unit square holding the ensō as `lay_out` fits it,
/// how far along the stroke (0 to 1) its nearest point on the brush's line
/// is, and how far from that line it lies, over `range`. BGRA, as GPUI's
/// images are.
fn bake_field() -> Field {
    let points = trace();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let extent = (max_x - min_x).max(max_y - min_y) + BRUSH;
    let off_x = (extent - (max_x - min_x)) / 2.0;
    let off_y = (extent - (max_y - min_y)) / 2.0;
    let line: Vec<(f32, f32)> = points
        .iter()
        .map(|&(x, y)| ((x - min_x + off_x) / extent, (y - min_y + off_y) / extent))
        .collect();
    let mut lengths = vec![0.0f32];
    for pair in line.windows(2) {
        let d = ((pair[1].0 - pair[0].0).powi(2) + (pair[1].1 - pair[0].1).powi(2)).sqrt();
        lengths.push(lengths.last().unwrap() + d);
    }
    let length = *lengths.last().unwrap();
    let half_brush = BRUSH / extent / 2.0;
    let range = half_brush * 2.0;
    let mut bgra = vec![0u8; (FIELD * FIELD * 4) as usize];
    for y in 0..FIELD {
        for x in 0..FIELD {
            let p = (
                (x as f32 + 0.5) / FIELD as f32,
                (y as f32 + 0.5) / FIELD as f32,
            );
            let (mut best, mut along) = (f32::MAX, 0.0);
            for (i, pair) in line.windows(2).enumerate() {
                let (a, b) = (pair[0], pair[1]);
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 {
                    (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (cx, cy) = (a.0 + dx * t, a.1 + dy * t);
                let d = ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt();
                if d < best {
                    best = d;
                    along = (lengths[i] + t * len2.sqrt()) / length;
                }
            }
            let i = ((y * FIELD + x) * 4) as usize;
            let level = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            bgra[i] = 0;
            bgra[i + 1] = level(best / range);
            bgra[i + 2] = level(along);
            bgra[i + 3] = 255;
        }
    }
    let frame = image::Frame::new(
        image::RgbaImage::from_raw(FIELD, FIELD, bgra).expect("the field holds its texels"),
    );
    Field {
        image: Arc::new(RenderImage::new(SmallVec::from_elem(frame, 1))),
        length,
        range,
        half_brush,
    }
}

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let color = self.color.unwrap_or(look.palette.muted.hsla());
        let moving = KoanTheme::motion_allowed(cx);
        let announce = self.announce.clone();
        let mark = self.mark(color, moving, window, cx);
        match announce {
            Some((id, label)) => super::announced(div().id(id), label, crate::theme::Tone::Muted)
                .flex_none()
                .child(mark)
                .into_any_element(),
            None => mark,
        }
    }
}

impl Spinner {
    fn mark(self, color: Hsla, moving: bool, window: &mut Window, cx: &mut App) -> AnyElement {
        if SHADED {
            let Some(field) = baked(window, cx) else {
                return reserving(div().size(self.side).flex_none(), self.reserve);
            };
            let side = f32::from(self.side) * window.scale_factor();
            let animated = Animated::new();
            animated.set_all(
                0,
                &[
                    if moving { 1.0 } else { 0.0 },
                    0.0,
                    field.length * side,
                    field.half_brush * side,
                    field.range * side,
                ],
            );
            animated.set_color(8, color);
            // Every spinner turns in step, from the first one drawn.
            animated.segment(
                0,
                0.0,
                1.0,
                *START.get_or_init(Instant::now),
                TURN,
                Ease::Linear,
                Repeat::Loop,
            );
            let mark = AnimatedQuad::new(&ENSO, &animated)
                .image(field.image.clone())
                .decorative()
                .size(self.side)
                .flex_none();
            return reserving(mark, self.reserve);
        }
        let stroke = stroke(self.side);
        let mark = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let range = if moving {
                    let start = *START.get_or_init(Instant::now);
                    let phase = (start.elapsed().as_secs_f32() / TURN.as_secs_f32()).fract();
                    shown(phase, stroke.quads.len())
                } else {
                    0..stroke.quads.len()
                };
                let size = size(stroke.brush, stroke.brush);
                let round = stroke.brush / 2.0;
                let end = range.end.min(stroke.quads.len());
                for offset in &stroke.quads[range.start.min(end)..end] {
                    window.paint_quad(
                        fill(Bounds::new(bounds.origin + *offset, size), color).corner_radii(round),
                    );
                }
                if moving {
                    window.request_animation_frame();
                }
            },
        )
        .size(self.side)
        .flex_none();
        reserving(mark, self.reserve)
    }
}

/// `mark`, centred in `room` where one is held.
fn reserving(mark: impl IntoElement, room: Option<Size<Pixels>>) -> AnyElement {
    match room {
        Some(room) => div()
            .w(room.width)
            .h(room.height)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(mark)
            .into_any_element(),
        None => mark.into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn the_stroke_is_laid_out_inside_its_square() {
        let stroke = lay_out(24.0);
        assert!(stroke.quads.len() > 50);
        let brush = f32::from(stroke.brush);
        for q in &stroke.quads {
            let (x, y) = (f32::from(q.x), f32::from(q.y));
            assert!(x >= -0.01 && y >= -0.01, "{x},{y}");
            assert!(x + brush <= 24.01 && y + brush <= 24.01, "{x},{y}");
        }
    }

    #[test]
    fn the_field_runs_along_the_stroke() {
        let field = bake_field();
        assert!(
            field.length > 1.0,
            "an ensō is longer than its square's side"
        );
        let bytes = field.image.as_bytes(0).unwrap();
        let at = |x: u32, y: u32| {
            let i = ((y * FIELD + x) * 4) as usize;
            (bytes[i + 2], bytes[i + 1])
        };
        // Some texels sit on the brush's line, and the arc runs the whole way.
        let on: Vec<_> = (0..FIELD * FIELD)
            .map(|i| at(i % FIELD, i / FIELD))
            .filter(|(_, away)| *away < 8)
            .collect();
        assert!(on.len() > 100);
        assert!(on.iter().any(|(along, _)| *along < 16));
        assert!(on.iter().any(|(along, _)| *along > 240));
        // The middle of the circle is off the brush.
        assert!(at(FIELD / 2, FIELD / 2).1 > 128);
    }

    #[test]
    fn a_loop_draws_the_mark_then_undraws_it() {
        assert_eq!(shown(0.0, 100), 0..0);
        assert_eq!(shown(0.5, 100), 0..100);
        assert!(shown(0.999, 100).len() <= 1);
        assert!(shown(0.25, 100).end > 20 && shown(0.25, 100).end < 80);
    }
}
