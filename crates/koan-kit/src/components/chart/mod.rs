//! A chart: axes, series, rules and points over a plot, drawn by the GPU
//! from keyframes, and interactive by pointer and by touch.
//!
//! A page declares what to draw each time it renders: a `Chart` with an x
//! and a y `Axis` (linear or logarithmic, ticks chosen or given), `Series`
//! (lines, dashed or solid, and areas from a line to a level or to another
//! line), horizontal `Rule`s with an optional label, and `ChartPoint`s that
//! can be dragged within ranges. It compares what it is given with what it
//! drew and writes only what changed, as keyframes:
//!
//! - **Data.** A series whose values change gets an image holding where it
//!   is drawn now and its new values (`data.rs`), and its phase track moves
//!   from one to the other: eased (`DataMotion::Ease`), interpolated over a
//!   sample interval and so drawn one sample behind (`DataMotion::Interpolate`),
//!   or drawn current and carried on along its last rate until the next
//!   sample corrects it (`DataMotion::Extrapolate`). Samples arriving at their
//!   own rate are drawn at the display's.
//! - **Axes.** A range change eases each axis's affine map; every mark
//!   reads it as tracks, so the series, the grid, the points and the labels
//!   move together with no frame drawn.
//! - **Points.** A point moved by its owner settles where it was put; one
//!   being dragged stands under the pointer.
//!
//! Only layout runs on the CPU: the ticks, which labels fit (`scale::thin`)
//! and the images, made again when the data, the plot's size or the range
//! changes. A chart at rest presents nothing, and under Reduce Motion every
//! change lands at once. Where the renderer draws no shaders the chart
//! paints its resting state as paths.

mod data;
mod legend;
mod program;
pub mod scale;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::*;

pub use legend::{Legend, Swatch};
pub use scale::{Map, Scale, Tick};

use self::data::{Keyframe, Span, decimated, lengths, sample};
use super::{Animated, Ease, KoanTouchDrag as _, Repeat, SHADED, ShaderProgram, Track};
use crate::theme::{FONT_FAMILY, KoanTheme, Look, Space, Tone, TypeRole};

/// How long a rescale and a settling point take.
const GLIDE: Duration = Duration::from_millis(300);
/// How near the pointer must come to take a point, and a finger.
const REACH: f32 = 24.0;
const TOUCH_REACH: f32 = 36.0;

/// How a series meets new values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DataMotion {
    /// Drawn at once.
    Instant,
    /// Eased from where it is drawn, over the length given.
    Ease(Duration),
    /// A sample every interval, each reached as the next arrives: drawn one
    /// interval behind, exactly through every sample.
    Interpolate(Duration),
    /// A sample every interval, drawn as it lands and carried on along the
    /// rate of the last two for one more interval; the next corrects it.
    Extrapolate(Duration),
}

impl Default for DataMotion {
    fn default() -> Self {
        DataMotion::Ease(Duration::from_millis(250))
    }
}

/// Where an axis puts its ticks.
#[derive(Clone, Debug, PartialEq)]
pub enum Ticks {
    /// Round values: about one per 60 points across, or 40 up, for a
    /// linear axis; 1, 2 and 5 of each decade for a logarithmic one.
    Auto,
    /// Every multiple of this step.
    Every(f64),
    /// These values.
    At(Vec<f64>),
}

type Format = Rc<dyn Fn(f64) -> SharedString>;

/// One of a chart's axes: its scale, its range, and its ticks.
#[derive(Clone)]
pub struct Axis {
    scale: Scale,
    lo: f64,
    hi: f64,
    ticks: Ticks,
    format: Option<Format>,
    grid: bool,
    labels: bool,
}

impl Axis {
    pub fn linear(lo: f64, hi: f64) -> Self {
        Self::new(Scale::Linear, lo, hi)
    }

    pub fn log(lo: f64, hi: f64) -> Self {
        Self::new(Scale::Log, lo, hi)
    }

    fn new(scale: Scale, lo: f64, hi: f64) -> Self {
        Self {
            scale,
            lo,
            hi,
            ticks: Ticks::Auto,
            format: None,
            grid: true,
            labels: true,
        }
    }

    pub fn ticks(mut self, ticks: Ticks) -> Self {
        self.ticks = ticks;
        self
    }

    /// How a tick's label reads; by default its value to the tick step's
    /// precision, thousands as `k` on a logarithmic axis.
    pub fn format(mut self, format: impl Fn(f64) -> String + 'static) -> Self {
        self.format = Some(Rc::new(move |v| format(v).into()));
        self
    }

    /// No rule across the plot at each tick.
    pub fn no_grid(mut self) -> Self {
        self.grid = false;
        self
    }

    pub fn no_labels(mut self) -> Self {
        self.labels = false;
        self
    }

    fn map(&self) -> Map {
        Map::new(self.scale, self.lo, self.hi)
    }

    fn ticks_for(&self, length: f32, per: f32) -> Vec<Tick> {
        let (lo, hi) = (self.lo.min(self.hi), self.lo.max(self.hi));
        match &self.ticks {
            Ticks::Auto => match self.scale {
                Scale::Log => scale::log(lo, hi),
                Scale::Linear => scale::linear(lo, hi, ((length / per) as usize).max(2)),
            },
            Ticks::Every(step) => scale::every(lo, hi, *step),
            Ticks::At(values) => values
                .iter()
                .filter(|v| **v >= lo && **v <= hi)
                .map(|v| Tick { value: *v, rank: 0 })
                .collect(),
        }
    }

    fn label(&self, v: f64, ticks: &[Tick]) -> SharedString {
        if let Some(format) = &self.format {
            return format(v);
        }
        if self.scale == Scale::Log {
            return if v >= 1000.0 {
                trim(v / 1000.0, 2) + "k"
            } else {
                trim(v, 2)
            }
            .into();
        }
        let step = ticks
            .windows(2)
            .map(|w| (w[1].value - w[0].value).abs())
            .fold(f64::INFINITY, f64::min);
        let decimals = if step.is_finite() && step > 0.0 {
            (-step.log10().floor()).max(0.0) as usize
        } else {
            0
        };
        format!("{v:.decimals$}").replace('-', "−").into()
    }
}

fn trim(v: f64, most: usize) -> String {
    let s = format!("{v:.most$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    s.replace('-', "−")
}

#[derive(Clone, Debug, PartialEq)]
enum Mark {
    Line { width: f32, dash: Vec<f32> },
    Area { base: Base },
}

/// Where an area is filled to.
#[derive(Clone, Debug, PartialEq)]
pub enum Base {
    /// A level on the y axis.
    Level(f64),
    /// Another line, one value for each of the series' samples.
    Line(Arc<[f64]>),
}

/// A series of values against x, drawn as a line or as an area.
#[derive(Clone)]
pub struct Series {
    id: SharedString,
    xs: Arc<[f64]>,
    ys: Arc<[f64]>,
    mark: Mark,
    color: Option<Hsla>,
    motion: DataMotion,
}

impl Series {
    /// A line through `ys` at `xs`, which rise. Solid, 1.5 points, in ink.
    pub fn line(
        id: impl Into<SharedString>,
        xs: impl Into<Arc<[f64]>>,
        ys: impl Into<Arc<[f64]>>,
    ) -> Self {
        Self {
            id: id.into(),
            xs: xs.into(),
            ys: ys.into(),
            mark: Mark::Line {
                width: 1.5,
                dash: Vec::new(),
            },
            color: None,
            motion: DataMotion::default(),
        }
    }

    /// The area between the line through `ys` and the level 0.
    pub fn area(
        id: impl Into<SharedString>,
        xs: impl Into<Arc<[f64]>>,
        ys: impl Into<Arc<[f64]>>,
    ) -> Self {
        Self {
            mark: Mark::Area {
                base: Base::Level(0.0),
            },
            ..Self::line(id, xs, ys)
        }
    }

    /// A line's width, in points.
    pub fn width(mut self, points: f32) -> Self {
        if let Mark::Line { width, .. } = &mut self.mark {
            *width = points;
        }
        self
    }

    /// A line's dash: on and off lengths in points, up to two pairs. Empty
    /// is solid.
    pub fn dash(mut self, pattern: &[f32]) -> Self {
        if let Mark::Line { dash, .. } = &mut self.mark {
            *dash = pattern.iter().take(4).copied().collect();
        }
        self
    }

    /// An area filled to `base` rather than to 0.
    pub fn base(mut self, base: Base) -> Self {
        if let Mark::Area { base: b } = &mut self.mark {
            *b = base;
        }
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn motion(mut self, motion: DataMotion) -> Self {
        self.motion = motion;
        self
    }
}

/// A level marked across the plot, with an optional label at its left.
#[derive(Clone)]
pub struct Rule {
    id: SharedString,
    value: f64,
    color: Option<Hsla>,
    width: f32,
    dash: Vec<f32>,
    label: Option<SharedString>,
    below: bool,
}

impl Rule {
    /// A hairline across the plot at `value` on the y axis, in the theme's
    /// rule colour.
    pub fn horizontal(id: impl Into<SharedString>, value: f64) -> Self {
        Self {
            id: id.into(),
            value,
            color: None,
            width: 0.0,
            dash: Vec::new(),
            label: None,
            below: false,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Its width in points, rather than a hairline.
    pub fn width(mut self, points: f32) -> Self {
        self.width = points;
        self
    }

    pub fn dash(mut self, pattern: &[f32]) -> Self {
        self.dash = pattern.iter().take(4).copied().collect();
        self
    }

    /// Said at the plot's left, just above the rule, or below it.
    pub fn label(mut self, label: impl Into<SharedString>, below: bool) -> Self {
        self.label = Some(label.into());
        self.below = below;
        self
    }
}

/// A point on the plot, which a drag can move within its ranges.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartPoint {
    pub id: u64,
    pub x: f64,
    pub y: f64,
    color: Option<Hsla>,
    radius: f32,
    held_radius: f32,
    held: bool,
    x_range: Option<(f64, f64)>,
    y_range: Option<(f64, f64)>,
}

impl ChartPoint {
    /// A disc of 4 points' radius, in the accent.
    pub fn new(id: u64, x: f64, y: f64) -> Self {
        Self {
            id,
            x,
            y,
            color: None,
            radius: 4.0,
            held_radius: 6.0,
            held: false,
            x_range: None,
            y_range: None,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Its radius at rest and while held, in points.
    pub fn radius(mut self, rest: f32, held: f32) -> Self {
        self.radius = rest;
        self.held_radius = held;
        self
    }

    /// Drawn as held: larger, as under a finger.
    pub fn held(mut self, held: bool) -> Self {
        self.held = held;
        self
    }

    /// The values a drag of it may reach; the plot's ranges otherwise.
    pub fn ranges(mut self, x: (f64, f64), y: (f64, f64)) -> Self {
        self.x_range = Some(x);
        self.y_range = Some(y);
        self
    }
}

/// Where a drag on the plot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragPhase {
    Press,
    Step,
    Release,
}

/// A drag on the plot: on a point (taken within reach of the press) or on
/// the plot itself, with the values under the pointer, held to the point's
/// ranges or the plot's.
#[derive(Clone, Copy, Debug)]
pub struct ChartDrag {
    pub phase: DragPhase,
    pub point: Option<u64>,
    pub x: f64,
    pub y: f64,
    pub position: Point<Pixels>,
    /// Where the press was.
    pub start: Point<Pixels>,
    pub modifiers: Modifiers,
}

type Handler = Rc<dyn Fn(&ChartDrag, &mut Window, &mut App)>;

/// Where each point was laid out, in the window: for a test or a
/// measurement to press on.
pub type PointPlaces = Rc<RefCell<Vec<(u64, Point<Pixels>)>>>;

/// The chart. See the module's description.
#[derive(IntoElement)]
pub struct Chart {
    id: ElementId,
    x: Axis,
    y: Axis,
    series: Vec<Series>,
    rules: Vec<Rule>,
    points: Vec<ChartPoint>,
    height: Pixels,
    legend: Option<Legend>,
    gutter: Option<Pixels>,
    on_press: Option<Handler>,
    on_step: Option<Handler>,
    on_release: Option<Handler>,
    places: Option<PointPlaces>,
}

impl Chart {
    pub fn new(id: impl Into<ElementId>, x: Axis, y: Axis) -> Self {
        Self {
            id: id.into(),
            x,
            y,
            series: Vec::new(),
            rules: Vec::new(),
            points: Vec::new(),
            height: px(220.0),
            legend: None,
            gutter: None,
            on_press: None,
            on_step: None,
            on_release: None,
            places: None,
        }
    }

    /// Drawn in order, each over the last.
    pub fn series(mut self, series: impl IntoIterator<Item = Series>) -> Self {
        self.series.extend(series);
        self
    }

    /// Drawn under the series.
    pub fn rules(mut self, rules: impl IntoIterator<Item = Rule>) -> Self {
        self.rules.extend(rules);
        self
    }

    /// Drawn over the series.
    pub fn points(mut self, points: impl IntoIterator<Item = ChartPoint>) -> Self {
        self.points.extend(points);
        self
    }

    /// The plot's height with its x labels; it is as wide as its container.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }

    pub fn legend(mut self, legend: Legend) -> Self {
        self.legend = Some(legend);
        self
    }

    /// The y labels' column, rather than as wide as its widest label.
    pub fn y_gutter(mut self, width: Pixels) -> Self {
        self.gutter = Some(width);
        self
    }

    /// A press on the plot, on a point within reach or on the plot itself.
    pub fn on_press(mut self, f: impl Fn(&ChartDrag, &mut Window, &mut App) + 'static) -> Self {
        self.on_press = Some(Rc::new(f));
        self
    }

    /// Each move of a drag. A point dragged is drawn where its owner puts
    /// it on the next render, so the owner moves it.
    pub fn on_step(mut self, f: impl Fn(&ChartDrag, &mut Window, &mut App) + 'static) -> Self {
        self.on_step = Some(Rc::new(f));
        self
    }

    /// The drag's end: let go, or the button seen up.
    pub fn on_release(mut self, f: impl Fn(&ChartDrag, &mut Window, &mut App) + 'static) -> Self {
        self.on_release = Some(Rc::new(f));
        self
    }

    pub fn report_points(mut self, places: PointPlaces) -> Self {
        self.places = Some(places);
        self
    }
}

/// An axis's map easing from one range to another.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    from: Map,
    to: Map,
    start: Instant,
    length: Duration,
}

impl Glide {
    fn still(map: Map, now: Instant) -> Self {
        Glide {
            from: map,
            to: map,
            start: now,
            length: Duration::ZERO,
        }
    }

    fn progress(&self, now: Instant) -> f64 {
        if self.length.is_zero() {
            return 1.0;
        }
        let track = Track {
            from: 0.0,
            to: 1.0,
            start: 0.0,
            length: self.length.as_secs_f32(),
            curve: super::curve(Ease::QuickOut),
            repeat: Repeat::Once,
        };
        track.at(now.saturating_duration_since(self.start).as_secs_f32()) as f64
    }

    fn at(&self, now: Instant) -> Map {
        self.from.lerp(&self.to, self.progress(now))
    }

    /// Moving to `target`, from where it is drawn now; whether it changed.
    fn retarget(this: &mut Option<Glide>, target: Map, now: Instant, moving: bool) -> bool {
        match this {
            Some(g) if g.to == target => false,
            Some(g) => {
                let from = g.at(now);
                *g = Glide {
                    from,
                    to: target,
                    start: now,
                    length: if moving { GLIDE } else { Duration::ZERO },
                };
                true
            }
            None => {
                *this = Some(Glide::still(target, now));
                true
            }
        }
    }

    /// Writes tracks `k` and `k + 1` of `animated` as this glide, framed
    /// for values stored as shares of `span`.
    fn write(&self, animated: &Animated, k: usize, span: Span) {
        let (a0, b0) = self.from.framed(span.lo, span.hi);
        let (a1, b1) = self.to.framed(span.lo, span.hi);
        if self.length.is_zero() {
            animated.hold(k, a1 as f32);
            animated.hold(k + 1, b1 as f32);
        } else {
            for (i, (f, t)) in [(a0, a1), (b0, b1)].into_iter().enumerate() {
                animated.segment(
                    k + i,
                    f as f32,
                    t as f32,
                    self.start,
                    self.length,
                    Ease::QuickOut,
                    Repeat::Once,
                );
            }
        }
    }
}

/// What a series' image and tracks were last written from.
struct SeriesState {
    id: SharedString,
    animated: Animated,
    image: Option<Arc<RenderImage>>,
    keyframe: Keyframe,
    spans: (Span, Span, Span),
    source: (Arc<[f64]>, Arc<[f64]>, Option<Arc<[f64]>>),
    /// The axes, spans and plot the tracks and lengths were written for.
    written: Option<(Glide, Glide, Span, Span)>,
    laid: Option<(Size<Pixels>, f32, Map, Map)>,
    area: bool,
    seen: bool,
}

struct PointState {
    id: u64,
    animated: Animated,
    value: (f64, f64),
    radius: f32,
    written: Option<(Glide, Glide)>,
    seen: bool,
}

struct LabelState {
    animated: Animated,
    origin: Point<Pixels>,
    seen: bool,
}

struct Pressed {
    point: Option<u64>,
    start: Point<Pixels>,
}

#[derive(Default)]
struct State {
    x: Option<Glide>,
    y: Option<Glide>,
    series: Vec<SeriesState>,
    grid: Option<(Animated, Arc<RenderImage>, Vec<u64>, Span, Span)>,
    points: Vec<PointState>,
    labels: HashMap<(u8, u64), LabelState>,
    pressed: Option<Pressed>,
    scales: Option<(Scale, Scale)>,
}

/// A mark as painted.
struct Quad {
    program: &'static ShaderProgram,
    animated: Animated,
    image: Option<Arc<RenderImage>>,
    bounds: Bounds<Pixels>,
    fallback: Fallback,
}

/// A mark at rest, for a renderer with no shaders.
enum Fallback {
    Line(Vec<Point<Pixels>>, Pixels, Vec<Pixels>, Hsla),
    Area(Vec<Point<Pixels>>, Hsla),
    Rules(Vec<Bounds<Pixels>>, Hsla),
    Dot(Point<Pixels>, Pixels, Hsla),
}

struct Label {
    line: ShapedLine,
    origin: Point<Pixels>,
    animated: Animated,
    strip: Bounds<Pixels>,
}

/// What prepaint hands paint.
struct Frame {
    plot: Bounds<Pixels>,
    quads: Vec<Quad>,
    labels: Vec<Label>,
    line_height: Pixels,
    hitbox: Option<Hitbox>,
    /// Each draggable point at rest, in the window, with its ranges.
    targets: Vec<(u64, Point<Pixels>, (f64, f64), (f64, f64))>,
}

/// The axes and marks, owned, as prepaint takes them.
struct Spec {
    x: Axis,
    y: Axis,
    series: Vec<Series>,
    rules: Vec<Rule>,
    points: Vec<ChartPoint>,
    gutter: Option<Pixels>,
    interactive: bool,
    places: Option<PointPlaces>,
}

impl RenderOnce for Chart {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| State::default());
        let interactive =
            self.on_press.is_some() || self.on_step.is_some() || self.on_release.is_some();
        let handlers = [self.on_press, self.on_step, self.on_release];
        let spec = Spec {
            x: self.x,
            y: self.y,
            series: self.series,
            rules: self.rules,
            points: self.points,
            gutter: self.gutter,
            interactive,
            places: self.places,
        };
        let plot = canvas(
            {
                let state = state.clone();
                move |bounds, window, cx| {
                    let motion = KoanTheme::motion_allowed(cx);
                    state.update(cx, |s, cx| {
                        s.prepare(spec, bounds, &look, motion, window, cx)
                    })
                }
            },
            move |_, frame, window, cx| {
                paint(&frame, window, cx);
                if interactive {
                    listen(state, &frame, handlers, look.touch, window);
                }
            },
        )
        .w_full()
        .h(self.height);
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(look.space(Space::M))
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(plot)
                    .when(interactive, |d| d.koan_touch_drag(&look)),
            )
            .children(self.legend)
    }
}

fn mono() -> Font {
    Font {
        weight: FontWeight::NORMAL,
        ..font(FONT_FAMILY)
    }
}

fn shape(text: SharedString, look: &Look, window: &Window) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: mono(),
        color: look.text_color(Tone::Muted),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text, look.font_size(TypeRole::Fine), &[run], None)
}

fn key_of(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

impl State {
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &mut self,
        spec: Spec,
        bounds: Bounds<Pixels>,
        look: &Look,
        motion: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        let now = Animated::now();
        let scale = window.scale_factor();
        self.scales = Some((spec.x.scale, spec.y.scale));
        let line_height = look.line_height(TypeRole::Fine);
        let x_gutter = if spec.x.labels {
            line_height + px(2.0)
        } else {
            px(0.0)
        };
        let height = (bounds.size.height - x_gutter).max(px(1.0));

        let y_ticks = spec.y.ticks_for(f32::from(height), 40.0);
        let y_lines: Vec<(Tick, ShapedLine)> = if spec.y.labels {
            y_ticks
                .iter()
                .map(|t| (*t, shape(spec.y.label(t.value, &y_ticks), look, window)))
                .collect()
        } else {
            Vec::new()
        };
        let gutter = spec.gutter.unwrap_or_else(|| {
            let widest = y_lines
                .iter()
                .map(|(_, l)| l.width)
                .fold(px(0.0), Pixels::max);
            if widest > px(0.0) {
                widest + look.space(Space::S)
            } else {
                px(0.0)
            }
        });
        let plot = Bounds::new(
            bounds.origin + point(gutter, px(0.0)),
            size((bounds.size.width - gutter).max(px(1.0)), height),
        );
        let (w, h) = (f32::from(plot.size.width), f32::from(plot.size.height));
        let (xmap, ymap) = (spec.x.map(), spec.y.map());
        let x_moved = Glide::retarget(&mut self.x, xmap, now, motion);
        let y_moved = Glide::retarget(&mut self.y, ymap, now, motion);
        let (xg, yg) = (self.x.unwrap(), self.y.unwrap());
        let ease_len = if motion { GLIDE } else { Duration::ZERO };

        let to_px = |x: f64, y: f64| {
            point(
                plot.origin.x + px((xmap.unit(spec.x.scale.transform(x)) as f32) * w),
                plot.origin.y + px((1.0 - ymap.unit(spec.y.scale.transform(y)) as f32) * h),
            )
        };
        let mut quads = Vec::new();

        // The grid.
        let x_ticks = spec.x.ticks_for(w, 60.0);
        if spec.x.grid || spec.y.grid {
            let xs: Vec<f64> = if spec.x.grid {
                x_ticks
                    .iter()
                    .map(|t| spec.x.scale.transform(t.value))
                    .collect()
            } else {
                Vec::new()
            };
            let ys: Vec<f64> = if spec.y.grid {
                y_ticks
                    .iter()
                    .map(|t| spec.y.scale.transform(t.value))
                    .collect()
            } else {
                Vec::new()
            };
            let key: Vec<u64> = xs
                .iter()
                .chain([&f64::NAN])
                .chain(&ys)
                .map(|v| v.to_bits())
                .collect();
            let xspan = Span::of([xmap.at(0.0), xmap.at(1.0)]);
            let yspan = Span::of([ymap.at(0.0), ymap.at(1.0)]);
            let stale = self.grid.as_ref().is_none_or(|g| g.2 != key);
            if stale {
                let n = xs.len().max(ys.len()).max(1);
                let mut bgra = vec![0u8; n * 2 * 4];
                for (row, (values, span)) in [(&xs, xspan), (&ys, yspan)].iter().enumerate() {
                    for (i, v) in values.iter().enumerate() {
                        let at = (row * n + i) * 4;
                        bgra[at..at + 4].copy_from_slice(&data::encode(span.share(*v)));
                    }
                }
                let image = data::image(n as u32, 2, bgra);
                let animated = match self.grid.take() {
                    Some((animated, old, ..)) => {
                        cx.drop_image(old, Some(window));
                        animated
                    }
                    None => Animated::new(),
                };
                self.grid = Some((animated, image, key, xspan, yspan));
            }
            let (animated, image, _, gx, gy) = self.grid.as_mut().unwrap();
            animated.attach(true, window, cx);
            if stale {
                // Its ticks are stored as shares of the range it was made for.
                *gx = xspan;
                *gy = yspan;
            }
            if stale || x_moved || y_moved {
                xg.write(animated, 1, *gx);
                yg.write(animated, 3, *gy);
            }
            let color = look.palette.rule.hsla().opacity(0.5);
            animated.set_color(0, color);
            animated.set_all(
                4,
                &[
                    f32::from(look.hairline) * scale,
                    xs.len() as f32,
                    ys.len() as f32,
                    xs.len().max(ys.len()).max(1) as f32,
                ],
            );
            let mut rules: Vec<Bounds<Pixels>> = x_ticks
                .iter()
                .filter(|_| spec.x.grid)
                .map(|t| {
                    let p = to_px(t.value, spec.y.lo);
                    Bounds::new(
                        point(p.x, plot.origin.y),
                        size(look.hairline, plot.size.height),
                    )
                })
                .collect();
            rules.extend(y_ticks.iter().filter(|_| spec.y.grid).map(|t| {
                let p = to_px(spec.x.lo, t.value);
                Bounds::new(
                    point(plot.origin.x, p.y),
                    size(plot.size.width, look.hairline),
                )
            }));
            quads.push(Quad {
                program: &program::GRID,
                animated: animated.clone(),
                image: Some(image.clone()),
                bounds: plot,
                fallback: Fallback::Rules(rules, color),
            });
        }

        // Rules are lines across the plot, wider than any range it eases
        // through, and drawn under the series.
        let (tx0, tx1) = (xmap.at(0.0), xmap.at(1.0));
        let reach = (tx1 - tx0).abs().max(1e-9);
        let across: Arc<[f64]> = [tx0 - reach, tx1 + reach].into();
        let mut marks: Vec<(Series, bool)> = Vec::new();
        let mut rule_labels = Vec::new();
        for rule in &spec.rules {
            let color = rule.color.unwrap_or(look.palette.rule.hsla());
            let width = if rule.width > 0.0 {
                rule.width
            } else {
                f32::from(look.hairline)
            };
            marks.push((
                Series::line(
                    SharedString::from(format!("rule:{}", rule.id)),
                    across.clone(),
                    vec![rule.value; 2],
                )
                .width(width)
                .dash(&rule.dash)
                .color(color)
                .motion(DataMotion::Ease(GLIDE)),
                true,
            ));
            if let Some(label) = &rule.label {
                rule_labels.push((rule.id.clone(), rule.value, label.clone(), rule.below));
            }
        }
        marks.extend(spec.series.into_iter().map(|s| (s, false)));

        for state in &mut self.series {
            state.seen = false;
        }
        for (series, pre) in marks {
            let i = match self.series.iter().position(|s| s.id == series.id) {
                Some(i) => i,
                None => {
                    self.series.push(SeriesState {
                        id: series.id.clone(),
                        animated: Animated::new(),
                        image: None,
                        keyframe: Keyframe::default(),
                        spans: (Span::of([]), Span::of([]), Span::of([])),
                        source: (Arc::from([]), Arc::from([]), None),
                        written: None,
                        laid: None,
                        area: false,
                        seen: false,
                    });
                    self.series.len() - 1
                }
            };
            let s = &mut self.series[i];
            s.seen = true;
            s.animated.attach(true, window, cx);
            let color = series.color.unwrap_or(look.color(Tone::Ink));
            let base_line = match &series.mark {
                Mark::Area {
                    base: Base::Line(b),
                } => Some(b.clone()),
                _ => None,
            };
            let area = matches!(series.mark, Mark::Area { .. });
            let same = |a: &Arc<[f64]>, b: &Arc<[f64]>| Arc::ptr_eq(a, b) || a[..] == b[..];
            let changed = s.image.is_none()
                || s.area != area
                || !same(&s.source.0, &series.xs)
                || !same(&s.source.1, &series.ys)
                || match (&s.source.2, &base_line) {
                    (Some(a), Some(b)) => !same(a, b),
                    (None, None) => false,
                    _ => true,
                };
            let laid = (plot.size, scale, xmap, ymap);
            let dashed = matches!(&series.mark, Mark::Line { dash, .. } if !dash.is_empty());
            let relaid = s.laid != Some(laid);
            let xt = |x: f64| if pre { x } else { spec.x.scale.transform(x) };
            let yt = |y: f64| spec.y.scale.transform(y);

            if changed {
                let n = series.xs.len().min(series.ys.len());
                let picked = decimated(n, w * scale);
                let xs: Vec<f64> = picked.iter().map(|&i| xt(series.xs[i])).collect();
                let ys: Vec<f64> = picked.iter().map(|&i| yt(series.ys[i])).collect();
                let base: Vec<f64> = match &base_line {
                    Some(b) => picked
                        .iter()
                        .map(|&i| yt(b.get(i).copied().unwrap_or(0.0)))
                        .collect(),
                    None => vec![0.0; xs.len()],
                };
                let old = std::mem::take(&mut s.keyframe);
                let first = s.image.is_none() || old.len() == 0 || s.area != area;
                let phase = if first {
                    1.0
                } else {
                    s.animated.at(0, now) as f64
                };
                let (motion_kind, interval) = match series.motion {
                    _ if first || !motion => (DataMotion::Instant, Duration::ZERO),
                    m @ (DataMotion::Ease(d)
                    | DataMotion::Interpolate(d)
                    | DataMotion::Extrapolate(d)) => (m, d),
                    DataMotion::Instant => (DataMotion::Instant, Duration::ZERO),
                };
                let from: Vec<f64> = match motion_kind {
                    DataMotion::Instant => ys.clone(),
                    DataMotion::Extrapolate(_) => {
                        xs.iter().map(|x| sample(&old.xs, &old.to, *x)).collect()
                    }
                    _ => {
                        let drawn = old.drawn(phase);
                        xs.iter().map(|x| sample(&old.xs, &drawn, *x)).collect()
                    }
                };
                let base_from: Vec<f64> = match motion_kind {
                    _ if base_line.is_none() => base.clone(),
                    DataMotion::Instant => base.clone(),
                    DataMotion::Extrapolate(_) => xs
                        .iter()
                        .map(|x| sample(&old.xs, &old.aux_to, *x))
                        .collect(),
                    _ => {
                        let drawn = old.drawn_aux(phase);
                        xs.iter().map(|x| sample(&old.xs, &drawn, *x)).collect()
                    }
                };
                s.keyframe = Keyframe {
                    xs,
                    from,
                    to: ys,
                    aux_from: base_from,
                    aux_to: base,
                };
                match motion_kind {
                    DataMotion::Instant => s.animated.hold(0, 1.0),
                    DataMotion::Ease(d) => {
                        s.animated
                            .segment(0, 0.0, 1.0, now, d, Ease::QuickOut, Repeat::Once)
                    }
                    DataMotion::Interpolate(_) => {
                        s.animated
                            .segment(0, 0.0, 1.0, now, interval, Ease::Linear, Repeat::Once)
                    }
                    DataMotion::Extrapolate(_) => {
                        s.animated
                            .segment(0, 1.0, 2.0, now, interval, Ease::Linear, Repeat::Once)
                    }
                }
                s.source = (series.xs.clone(), series.ys.clone(), base_line.clone());
                s.area = area;
            }

            // A dashed line's lengths are in device pixels as laid out, so
            // they are measured again when the plot or the range changes.
            let mut total = 0.0;
            if changed || (dashed && relaid) {
                let k = &mut s.keyframe;
                let xspan = Span::of(k.xs.iter().copied());
                let yspan = if area {
                    let level = match &series.mark {
                        Mark::Area {
                            base: Base::Level(v),
                        } => Some(yt(*v)),
                        _ => None,
                    };
                    k.y_span(
                        &k.aux_from
                            .iter()
                            .chain(&k.aux_to)
                            .copied()
                            .filter(|_| base_line.is_some())
                            .chain(level)
                            .collect::<Vec<_>>(),
                    )
                } else {
                    k.y_span(&[])
                };
                let aux = if area {
                    yspan
                } else if dashed {
                    let at = |ys: &[f64]| -> Vec<f64> {
                        let points: Vec<(f64, f64)> =
                            k.xs.iter()
                                .zip(ys)
                                .map(|(x, y)| {
                                    (
                                        xmap.unit(*x) * (w * scale) as f64,
                                        (1.0 - ymap.unit(*y)) * (h * scale) as f64,
                                    )
                                })
                                .collect();
                        lengths(&points)
                    };
                    let (from, to) = (at(&k.from), at(&k.to));
                    k.aux_from = from;
                    k.aux_to = to;
                    total = k
                        .aux_from
                        .last()
                        .copied()
                        .unwrap_or(0.0)
                        .max(k.aux_to.last().copied().unwrap_or(0.0))
                        .max(1.0);
                    Span { lo: 0.0, hi: total }
                } else {
                    Span { lo: 0.0, hi: 1.0 }
                };
                if let Some(old) = s.image.take() {
                    cx.drop_image(old, Some(window));
                }
                s.image = Some(k.image(xspan, yspan, aux));
                s.spans = (xspan, yspan, aux);
                s.laid = Some(laid);
            } else if dashed {
                total = s.spans.2.hi;
            }
            let (xspan, yspan, _) = s.spans;
            let key = (xg, yg, xspan, yspan);
            if s.written != Some(key) {
                xg.write(&s.animated, 1, xspan);
                yg.write(&s.animated, 3, yspan);
                s.written = Some(key);
            }
            s.animated.set_color(0, color);
            let n = s.keyframe.len() as f32;
            let k = &s.keyframe;
            let resting: Vec<Point<Pixels>> =
                k.xs.iter()
                    .zip(&k.to)
                    .map(|(x, y)| {
                        point(
                            plot.origin.x + px(xmap.unit(*x) as f32 * w),
                            plot.origin.y + px((1.0 - ymap.unit(*y) as f32) * h),
                        )
                    })
                    .collect();
            let fallback = match &series.mark {
                Mark::Line { width, dash } => {
                    let mut d = [0.0f32; 4];
                    for (slot, v) in d.iter_mut().zip(dash) {
                        *slot = v * scale;
                    }
                    s.animated
                        .set_all(4, &[width * scale / 2.0, n, total as f32]);
                    s.animated.set_all(8, &d);
                    Fallback::Line(
                        resting,
                        px(*width),
                        dash.iter().map(|d| px(*d)).collect(),
                        color,
                    )
                }
                Mark::Area { base } => {
                    let (row, level) = match base {
                        Base::Level(v) => (0.0, yspan.share(yt(*v)) as f32),
                        Base::Line(_) => (1.0, 0.0),
                    };
                    s.animated.set_all(5, &[n, row, level]);
                    let mut polygon = resting.clone();
                    match base {
                        Base::Level(v) => {
                            let y = plot.origin.y + px((1.0 - ymap.unit(yt(*v)) as f32) * h);
                            if let (Some(first), Some(last)) = (resting.first(), resting.last()) {
                                polygon.push(point(last.x, y));
                                polygon.push(point(first.x, y));
                            }
                        }
                        Base::Line(_) => {
                            for (x, y) in k.xs.iter().zip(&k.aux_to).rev() {
                                polygon.push(point(
                                    plot.origin.x + px(xmap.unit(*x) as f32 * w),
                                    plot.origin.y + px((1.0 - ymap.unit(*y) as f32) * h),
                                ));
                            }
                        }
                    }
                    Fallback::Area(polygon, color)
                }
            };
            quads.push(Quad {
                program: if area { &program::AREA } else { &program::LINE },
                animated: s.animated.clone(),
                image: s.image.clone(),
                bounds: plot,
                fallback,
            });
        }
        let mut dropped = Vec::new();
        self.series.retain_mut(|s| {
            if !s.seen
                && let Some(image) = s.image.take()
            {
                dropped.push(image);
            }
            s.seen
        });
        for image in dropped {
            cx.drop_image(image, Some(window));
        }

        // Points.
        let pad = spec
            .points
            .iter()
            .map(|p| p.radius.max(p.held_radius))
            .fold(0.0f32, f32::max)
            + 1.0;
        let dot_bounds = Bounds::new(
            plot.origin - point(px(pad), px(pad)),
            plot.size + size(px(2.0 * pad), px(2.0 * pad)),
        );
        for p in &mut self.points {
            p.seen = false;
        }
        let dragged = self.pressed.as_ref().and_then(|p| p.point);
        let mut targets = Vec::new();
        for point_spec in &spec.points {
            let ux = xmap.unit(spec.x.scale.transform(point_spec.x)) as f32;
            let uy = ymap.unit(spec.y.scale.transform(point_spec.y)) as f32;
            let radius = if point_spec.held {
                point_spec.held_radius
            } else {
                point_spec.radius
            } * scale;
            let fresh = !self.points.iter().any(|p| p.id == point_spec.id);
            if fresh {
                self.points.push(PointState {
                    id: point_spec.id,
                    animated: Animated::new(),
                    value: (point_spec.x, point_spec.y),
                    radius,
                    written: None,
                    seen: false,
                });
            }
            let p = self
                .points
                .iter_mut()
                .find(|p| p.id == point_spec.id)
                .unwrap();
            p.seen = true;
            p.animated.attach(true, window, cx);
            let value = (point_spec.x, point_spec.y);
            if fresh || dragged == Some(p.id) {
                p.animated.hold(0, ux);
                p.animated.hold(1, uy);
            } else if p.value != value {
                p.animated.to(0, ux, ease_len, Ease::QuickOut);
                p.animated.to(1, uy, ease_len, Ease::QuickOut);
            } else if p.written != Some((xg, yg)) {
                // Moved by the axes alone: along their glide exactly.
                let tx = spec.x.scale.transform(value.0);
                let ty = spec.y.scale.transform(value.1);
                for (k, glide, t) in [(0, xg, tx), (1, yg, ty)] {
                    let (f, to) = (glide.from.unit(t) as f32, glide.to.unit(t) as f32);
                    if glide.length.is_zero() {
                        p.animated.hold(k, to);
                    } else {
                        p.animated.segment(
                            k,
                            f,
                            to,
                            glide.start,
                            glide.length,
                            Ease::QuickOut,
                            Repeat::Once,
                        );
                    }
                }
            }
            if fresh {
                p.animated.hold(2, radius);
            } else if p.radius != radius {
                p.animated.to(
                    2,
                    radius,
                    ease_len.min(Duration::from_millis(150)),
                    Ease::QuickOut,
                );
            }
            p.value = value;
            p.radius = radius;
            p.written = Some((xg, yg));
            let color = point_spec.color.unwrap_or(look.accent.hsla());
            p.animated.set_color(0, color);
            p.animated.set(4, pad * scale);
            let at = to_px(point_spec.x, point_spec.y);
            quads.push(Quad {
                program: &program::DOT,
                animated: p.animated.clone(),
                image: None,
                bounds: dot_bounds,
                fallback: Fallback::Dot(at, px(radius / scale), color),
            });
            let range = |given: Option<(f64, f64)>, axis: &Axis| {
                given.unwrap_or((axis.lo.min(axis.hi), axis.lo.max(axis.hi)))
            };
            targets.push((
                point_spec.id,
                at,
                range(point_spec.x_range, &spec.x),
                range(point_spec.y_range, &spec.y),
            ));
        }
        self.points.retain(|p| p.seen);
        if let Some(places) = &spec.places {
            *places.borrow_mut() = targets.iter().map(|(id, at, ..)| (*id, *at)).collect();
        }

        // Labels, laid out at rest and thinned to fit.
        for label in self.labels.values_mut() {
            label.seen = false;
        }
        let mut labels = Vec::new();
        let gap = f32::from(look.space(Space::S));
        if spec.x.labels {
            let lines: Vec<(Tick, ShapedLine)> = x_ticks
                .iter()
                .map(|t| (*t, shape(spec.x.label(t.value, &x_ticks), look, window)))
                .collect();
            let lefts: Vec<f32> = lines
                .iter()
                .map(|(t, l)| {
                    let centre = f32::from(to_px(t.value, spec.y.lo).x - l.width / 2.0);
                    centre
                        .min(f32::from(bounds.origin.x + bounds.size.width - l.width))
                        .max(f32::from(bounds.origin.x))
                })
                .collect();
            let spans: Vec<(f32, f32)> = lines
                .iter()
                .zip(&lefts)
                .map(|((_, l), left)| (*left, left + f32::from(l.width)))
                .collect();
            let ranks: Vec<u8> = lines.iter().map(|(t, _)| t.rank).collect();
            let kept = scale::thin(&spans, &ranks, gap);
            let strip = Bounds::new(
                point(bounds.origin.x, plot.origin.y + plot.size.height),
                size(bounds.size.width, x_gutter),
            );
            for (((tick, line), left), keep) in lines.into_iter().zip(lefts).zip(kept) {
                if keep {
                    let origin = point(px(left), plot.origin.y + plot.size.height + px(2.0));
                    let from = xg.from.unit(spec.x.scale.transform(tick.value)) as f32 * w;
                    let from = point(plot.origin.x + px(from) - line.width / 2.0, origin.y);
                    labels.push(self.label(
                        (0, tick.value.to_bits()),
                        line,
                        origin,
                        strip,
                        (x_moved, from),
                        (xg, scale),
                    ));
                }
            }
        }
        if spec.y.labels {
            let tops: Vec<f32> = y_lines
                .iter()
                .map(|(t, _)| f32::from(to_px(spec.x.lo, t.value).y - line_height / 2.0))
                .collect();
            let spans: Vec<(f32, f32)> = tops
                .iter()
                .map(|t| (*t, t + f32::from(line_height)))
                .collect();
            let ranks: Vec<u8> = y_lines.iter().map(|(t, _)| t.rank).collect();
            let kept = scale::thin(&spans, &ranks, 0.0);
            let strip = Bounds::new(bounds.origin, size(gutter, bounds.size.height));
            let right = plot.origin.x - look.space(Space::S);
            for (((tick, line), top), keep) in y_lines.into_iter().zip(tops).zip(kept) {
                if keep {
                    let origin = point(right - line.width, px(top));
                    let from = (1.0 - yg.from.unit(spec.y.scale.transform(tick.value)) as f32) * h;
                    let from = point(origin.x, plot.origin.y + px(from) - line_height / 2.0);
                    labels.push(self.label(
                        (1, tick.value.to_bits()),
                        line,
                        origin,
                        strip,
                        (y_moved, from),
                        (yg, scale),
                    ));
                }
            }
        }
        for (id, value, text, below) in rule_labels {
            let line = shape(text, look, window);
            let y = to_px(spec.x.lo, value).y;
            let origin = point(
                plot.origin.x + px(4.0),
                if below {
                    y + px(2.0)
                } else {
                    y - line_height - px(2.0)
                },
            );
            labels.push(self.label(
                (2, key_of(&id)),
                line,
                origin,
                plot,
                (false, origin),
                (
                    Glide {
                        length: ease_len,
                        ..Glide::still(ymap, now)
                    },
                    scale,
                ),
            ));
        }
        self.labels.retain(|_, l| l.seen);
        for label in &labels {
            label.animated.attach(true, window, cx);
        }

        let hitbox = spec
            .interactive
            .then(|| window.insert_hitbox(dot_bounds, HitboxBehavior::Normal));
        Frame {
            plot,
            quads,
            labels,
            line_height,
            hitbox,
            targets,
        }
    }

    /// Label `key` at `origin`: moved from where it was drawn when its
    /// place changed, eased as the axis is (`glide`); one new while its
    /// axis moves (`axis.0`) fades in from where the old range put it
    /// (`axis.1`).
    fn label(
        &mut self,
        key: (u8, u64),
        line: ShapedLine,
        origin: Point<Pixels>,
        strip: Bounds<Pixels>,
        axis: (bool, Point<Pixels>),
        (glide, scale): (Glide, f32),
    ) -> Label {
        let now = Animated::now();
        let moving = !glide.length.is_zero() && now < glide.start + glide.length;
        let (start, length) = if moving {
            (glide.start, glide.length)
        } else {
            (now, Duration::ZERO)
        };
        let fresh = !self.labels.contains_key(&key);
        let state = self.labels.entry(key).or_insert_with(|| {
            let animated = Animated::new();
            super::layer::standing(&animated);
            LabelState {
                animated,
                origin,
                seen: false,
            }
        });
        state.seen = true;
        let animated = &state.animated;
        let shift = |k: usize, from: f32| {
            if length.is_zero() || from.abs() < 0.01 {
                animated.hold(k, 0.0);
            } else {
                animated.segment(k, from, 0.0, start, length, Ease::QuickOut, Repeat::Once);
            }
        };
        if fresh {
            if axis.0 && !length.is_zero() {
                shift(1, f32::from(axis.1.x - origin.x) * scale);
                shift(2, f32::from(axis.1.y - origin.y) * scale);
                animated.segment(0, 0.0, 1.0, start, length, Ease::QuickOut, Repeat::Once);
            }
        } else if state.origin != origin {
            let drawn = (animated.at(1, now), animated.at(2, now));
            shift(1, drawn.0 + f32::from(state.origin.x - origin.x) * scale);
            shift(2, drawn.1 + f32::from(state.origin.y - origin.y) * scale);
        }
        state.origin = origin;
        Label {
            line,
            origin,
            animated: animated.clone(),
            strip,
        }
    }
}

fn paint(frame: &Frame, window: &mut Window, cx: &mut App) {
    for quad in &frame.quads {
        if SHADED {
            quad.animated
                .paint(quad.program, quad.image.clone(), quad.bounds, window);
            continue;
        }
        match &quad.fallback {
            Fallback::Line(points, width, dash, color) => {
                window.with_content_mask(Some(ContentMask { bounds: frame.plot }), |window| {
                    let mut b = PathBuilder::stroke(*width);
                    if !dash.is_empty() {
                        b = b.dash_array(dash);
                    }
                    for (i, p) in points.iter().enumerate() {
                        if i == 0 {
                            b.move_to(*p);
                        } else {
                            b.line_to(*p);
                        }
                    }
                    if let Ok(path) = b.build() {
                        window.paint_path(path, *color);
                    }
                });
            }
            Fallback::Area(points, color) => {
                window.with_content_mask(Some(ContentMask { bounds: frame.plot }), |window| {
                    let mut b = PathBuilder::fill();
                    for (i, p) in points.iter().enumerate() {
                        if i == 0 {
                            b.move_to(*p);
                        } else {
                            b.line_to(*p);
                        }
                    }
                    b.close();
                    if let Ok(path) = b.build() {
                        window.paint_path(path, *color);
                    }
                });
            }
            Fallback::Rules(rules, color) => {
                for r in rules {
                    window.paint_quad(fill(*r, *color));
                }
            }
            Fallback::Dot(at, r, color) => {
                window.paint_quad(
                    fill(
                        Bounds::new(*at - point(*r, *r), size(*r * 2.0, *r * 2.0)),
                        *color,
                    )
                    .corner_radii(*r),
                );
            }
        }
    }
    let mut settles: Option<Duration> = None;
    for label in &frame.labels {
        let (line, origin, height) = (&label.line, label.origin, frame.line_height);
        if SHADED && label.animated.moving() {
            label
                .animated
                .paint_layer(&super::LAYER, label.strip, window, cx, |window, cx| {
                    line.paint(origin, height, TextAlign::Left, None, window, cx)
                        .ok();
                });
            if let Some(wait) = label.animated.settled_in() {
                settles = Some(settles.map_or(wait, |s| s.max(wait)));
            }
        } else {
            line.paint(origin, height, TextAlign::Left, None, window, cx)
                .ok();
        }
    }
    // A label painted as a layer while it moved is painted plainly once it
    // lands: its view is drawn again then, as `AnimatedLayer`'s is.
    if let Some(wait) = settles {
        let view = window.current_view();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(wait).await;
            cx.update(|cx| cx.notify(view));
        })
        .detach();
    }
}

/// The drag's listeners, live while the chart is painted: a press on the
/// plot starts one; moves and the release are heard wherever the pointer
/// is, so a drag let go outside the chart still ends.
fn listen(
    state: Entity<State>,
    frame: &Frame,
    handlers: [Option<Handler>; 3],
    touch: bool,
    window: &mut Window,
) {
    let Some(hitbox) = frame.hitbox.clone() else {
        return;
    };
    let plot = frame.plot;
    let targets = Rc::new(frame.targets.clone());
    let [press, step, release] = handlers;
    let value = {
        let targets = targets.clone();
        let state = state.clone();
        Rc::new(move |p: Point<Pixels>, point: Option<u64>, cx: &App| {
            let s = state.read(cx);
            let (xg, yg) = (s.x?, s.y?);
            let ux = (f32::from(p.x - plot.origin.x) / f32::from(plot.size.width)).clamp(0.0, 1.0);
            let uy = 1.0
                - (f32::from(p.y - plot.origin.y) / f32::from(plot.size.height)).clamp(0.0, 1.0);
            let (tx, ty) = (xg.to.at(ux as f64), yg.to.at(uy as f64));
            let target = point.and_then(|id| targets.iter().find(|t| t.0 == id));
            Some((tx, ty, target.map(|t| (t.2, t.3))))
        })
    };
    let emit = {
        let state = state.clone();
        let value = value.clone();
        move |phase: DragPhase,
              p: Point<Pixels>,
              modifiers: Modifiers,
              handler: &Option<Handler>,
              window: &mut Window,
              cx: &mut App| {
            let Some((point, start)) = state.read(cx).pressed.as_ref().map(|d| (d.point, d.start))
            else {
                return;
            };
            let Some((tx, ty, ranges)) = value(p, point, cx) else {
                return;
            };
            let (sx, sy) = state
                .read(cx)
                .scales
                .unwrap_or((Scale::Linear, Scale::Linear));
            let (mut x, mut y) = (sx.invert(tx), sy.invert(ty));
            if let Some(((x0, x1), (y0, y1))) = ranges {
                x = x.clamp(x0.min(x1), x0.max(x1));
                y = y.clamp(y0.min(y1), y0.max(y1));
            }
            if let Some(handler) = handler {
                handler(
                    &ChartDrag {
                        phase,
                        point,
                        x,
                        y,
                        position: p,
                        start,
                        modifiers,
                    },
                    window,
                    cx,
                );
            }
        }
    };
    let emit = Rc::new(emit);
    {
        let state = state.clone();
        let emit = emit.clone();
        let reach = if touch { TOUCH_REACH } else { REACH };
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || e.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let places: Vec<(f32, f32)> = targets
                .iter()
                .map(|t| (f32::from(t.1.x), f32::from(t.1.y)))
                .collect();
            let near = scale::nearest(
                &places,
                (f32::from(e.position.x), f32::from(e.position.y)),
                reach,
            )
            .map(|i| targets[i].0);
            state.update(cx, |s, _| {
                s.pressed = Some(Pressed {
                    point: near,
                    start: e.position,
                })
            });
            emit(
                DragPhase::Press,
                e.position,
                e.modifiers,
                &press,
                window,
                cx,
            );
        });
    }
    {
        let state = state.clone();
        let emit = emit.clone();
        let release = release.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || state.read(cx).pressed.is_none() {
                return;
            }
            if e.pressed_button != Some(MouseButton::Left) {
                emit(
                    DragPhase::Release,
                    e.position,
                    e.modifiers,
                    &release,
                    window,
                    cx,
                );
                state.update(cx, |s, _| s.pressed = None);
            } else {
                emit(DragPhase::Step, e.position, e.modifiers, &step, window, cx);
            }
        });
    }
    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble
            || e.button != MouseButton::Left
            || state.read(cx).pressed.is_none()
        {
            return;
        }
        emit(
            DragPhase::Release,
            e.position,
            e.modifiers,
            &release,
            window,
            cx,
        );
        state.update(cx, |s, _| s.pressed = None);
    });
}
