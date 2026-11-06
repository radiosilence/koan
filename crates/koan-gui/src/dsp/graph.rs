//! What a profile or a device's chain does to the sound, drawn on a
//! log-frequency axis on the GPU, and for an EQ that can be edited, the
//! editor: a band's handle dragged for frequency and gain, option-dragged
//! for Q, a graphic curve painted with a brush.
//!
//! Every curve comes from the core; this draws them. Each curve's path is
//! tessellated once, at the plot's origin, when the curve, the plot's size
//! or the scale changes, and moved into place as it is painted. A drag moves
//! its handle at once and asks the core for a preview, one at a time and
//! always the newest; when it arrives only the curves it changed are built
//! again.
//!
//! The EQ view is drawn at the level the profile plays at: its curves moved
//! by the preamp, a dashed rule at the preamp and any flat gain, and the
//! plot centred there, so two profiles of one shape draw alike however each
//! states its gain.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use koan_kit::app::*;
use koan_runtime::{DspBand, DspBrush, DspPoint, DspResponse};
// painted and hit-tested by hand: drawing koan-kit has no component for.
use koan_kit::raw::{DispatchPhase, Path, PathBuilder, canvas, fill};
// text measured or shaped at runtime.
use koan_kit::raw::{Font, FontWeight, TextRun, font};
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{Hsla, TextAlign, point, px, size};

use super::model::{Commit, Done, Dsp, Ended, GRAPH_RATE, Outcome};
use crate::components::Segmented;
use crate::theme::{FONT_FAMILY, KoanStyled, Look, Space, Tone, TypeRole, label, observe_accent};

/// How a curve is drawn, the same in its stage's block and on the graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub tone: Tone,
    /// Of the tone's colour.
    pub opacity: f32,
    pub width: f32,
    pub dash: &'static [f32],
}

impl Stroke {
    pub const CORRECTION: Stroke = Stroke::solid(Tone::Accent, 1.2);
    pub const TOTAL: Stroke = Stroke::solid(Tone::Strong, 2.0);
    /// The chain's right channel, where it differs from the left the total
    /// draws.
    pub const RIGHT: Stroke = Stroke {
        dash: &[3.0, 2.0],
        ..Stroke::solid(Tone::Muted, 1.2)
    };

    const fn solid(tone: Tone, width: f32) -> Stroke {
        Stroke {
            tone,
            opacity: 1.0,
            width,
            dash: &[],
        }
    }

    /// Each EQ of a chain in ink, with a dash of its own.
    pub fn eq(index: usize) -> Stroke {
        const DASHES: [&[f32]; 4] = [&[5.0, 3.0], &[2.0, 2.0], &[8.0, 3.0, 2.0, 3.0], &[1.0, 4.0]];
        Stroke {
            dash: DASHES[index % DASHES.len()],
            ..Stroke::solid(Tone::Ink, 1.2)
        }
    }

    pub fn color(&self, look: &Look) -> Hsla {
        look.color(self.tone).opacity(self.opacity)
    }
}

/// A stage of a chain: its name, its curve, and how its block draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: SharedString,
    pub db: Vec<f64>,
    pub stroke: Stroke,
}

/// A band's point: its index among the profile's filters, where it is, and
/// its Q.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handle {
    pub index: usize,
    pub hz: f64,
    pub db: f64,
    pub q: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Eq,
    Measured,
}

/// The EQ a graph edits: its name, its filters as last read, and for one
/// with bands for each channel, the channel being edited.
#[derive(Clone)]
pub struct Editing {
    pub dsp: Entity<Dsp>,
    pub name: String,
    pub bands: Vec<DspBand>,
    pub channel: u16,
}

/// A drag under way: what it started on, and the newest step, drawn now
/// and saved through the model's lane for edit `edit`.
struct Drag {
    edit: u64,
    index: usize,
    kind: Grab,
    /// The filter as it was when the drag began.
    band: DspBand,
    started: Point<Pixels>,
    /// Where Option went down in this drag, and the Q then: a Q drag
    /// measures from there.
    alt_from: Option<(Pixels, f64)>,
    step: Option<Commit>,
    saved_at: Instant,
}

/// Where an edit made on the graph is.
enum Edit {
    Idle,
    Dragging(Box<Drag>),
    /// Let go: its last step is being saved, and the preview stays drawn.
    Settling(u64),
    /// Saved: the first response read at or after `stamp` replaces the
    /// preview.
    Settled((u64, u64)),
}

#[derive(Clone)]
enum Grab {
    Band { kind: String },
    Paint,
}

pub struct Graph {
    /// The response as last saved, and the one an edit under way draws in
    /// its place.
    saved: Rc<DspResponse>,
    live: Option<Rc<DspResponse>>,
    parts: Vec<Part>,
    /// The flat line a device playing untouched draws, said in words.
    flat_note: Option<SharedString>,
    shown: Shown,
    height: Pixels,
    editing: Option<Editing>,
    pub brush: DspBrush,
    edit: Edit,
    /// What the painted curve holds so far.
    painted: Option<Vec<DspPoint>>,
    /// A preview is being drawn; the newest step asked meanwhile waits.
    previewing: bool,
    /// Filters read again while a drag was held, applied once it ends.
    queued: Option<Option<Editing>>,
    cache: Rc<RefCell<Cache>>,
    _subscriptions: Vec<Subscription>,
}

/// One frequency's place on the axis.
fn x_of(hz: f64, width: f32) -> f32 {
    ((hz.clamp(20.0, 20_000.0) / 20.0).log10() / 3.0) as f32 * width
}

fn hz_at(x: f32, width: f32) -> f64 {
    20.0 * 10f64.powf(3.0 * (x / width).clamp(0.0, 1.0) as f64)
}

const HZ_TICKS: [f64; 10] = [
    20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0,
];

/// The left margin for the dB labels, and the foot for the Hz labels.
const AXIS_W: f32 = 30.0;
const AXIS_H: f32 = 16.0;
/// A handle's radius, and how near the pointer must come to take it.
const HANDLE: f32 = 4.0;
const HANDLE_HELD: f32 = 6.0;
const REACH: f32 = 24.0;
/// How often a drag's edit is saved, and so played, while it goes on.
const SAVE_EVERY: Duration = Duration::from_millis(250);
/// A drag away from every handle paints only once it has travelled this
/// far, so a click does not.
const PAINT_SLOP: f32 = 4.0;

impl Graph {
    pub fn new(response: DspResponse, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![observe_accent(cx)];
        // A window put away under a drag never sees its button come up:
        // the drag ends there.
        subscriptions.push(
            cx.observe_window_activation(window, |g: &mut Graph, window, cx| {
                if !window.is_window_active() {
                    g.let_go(cx);
                }
            }),
        );
        subscriptions.push(cx.on_release(|graph: &mut Graph, cx| graph.release(cx)));
        Self {
            shown: Shown::Eq,
            saved: Rc::new(response),
            live: None,
            parts: Vec::new(),
            flat_note: None,
            height: px(220.0),
            editing: None,
            brush: DspBrush::Narrow,
            edit: Edit::Idle,
            painted: None,
            previewing: false,
            queued: None,
            cache: Rc::new(RefCell::new(Cache::default())),
            _subscriptions: subscriptions,
        }
    }

    /// A flat chain, drawn: no change anywhere.
    pub fn flat_response() -> DspResponse {
        let freqs: Vec<f64> = (0..120)
            .map(|i| 20.0 * 1000f64.powf(i as f64 / 119.0))
            .collect();
        DspResponse {
            total: vec![0.0; freqs.len()],
            freqs,
            bands: Vec::new(),
            layers: Vec::new(),
            measurement: None,
            target: None,
            predicted: None,
            preamp_db: 0.0,
            gain_db: 0.0,
            correction: None,
            tuning: None,
            original: None,
            right: None,
            right_gain_db: 0.0,
        }
    }

    pub fn open_on(mut self, shown: Shown) -> Self {
        self.shown = shown;
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }

    /// A response read as the `Dsp` stood at `stamp`. While an edit is under
    /// way its preview stays; once it has been saved, the first response
    /// read at or after the save replaces it, whatever it holds.
    pub fn set_response(
        &mut self,
        response: DspResponse,
        stamp: (u64, u64),
        cx: &mut Context<Self>,
    ) {
        self.saved = Rc::new(response);
        let replaces = match self.edit {
            Edit::Idle => true,
            Edit::Settled(at) => stamp >= at,
            Edit::Dragging(_) | Edit::Settling(_) => false,
        };
        if replaces {
            self.edit = Edit::Idle;
            self.live = None;
            self.painted = None;
        }
        cx.notify();
    }

    pub fn set_parts(&mut self, parts: Vec<Part>, cx: &mut Context<Self>) {
        if parts != self.parts {
            self.parts = parts;
            cx.notify();
        }
    }

    pub fn set_flat_note(&mut self, note: Option<SharedString>, cx: &mut Context<Self>) {
        if note != self.flat_note {
            self.flat_note = note;
            cx.notify();
        }
    }

    /// Makes the graph an editor of `editing`'s bands, or with none a
    /// drawing. Filters read again while an edit is under way wait for it
    /// to end, so the handles are not re-indexed under the pointer.
    pub fn set_editing(&mut self, editing: Option<Editing>, cx: &mut Context<Self>) {
        if matches!(self.edit, Edit::Dragging(_) | Edit::Settling(_)) {
            self.queued = Some(editing);
            return;
        }
        self.editing = editing;
        cx.notify();
    }

    /// Where band `index`'s handle was last drawn, in the window: for a
    /// test or a measurement to press on.
    pub fn handle_position(&self, index: usize) -> Option<Point<Pixels>> {
        self.cache
            .borrow()
            .handles
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, p)| *p)
    }

    fn shown_response(&self) -> &Rc<DspResponse> {
        self.live.as_ref().unwrap_or(&self.saved)
    }

    fn measured(&self) -> bool {
        self.saved.measurement.is_some()
    }

    fn showing_eq(&self) -> bool {
        self.shown == Shown::Eq || !self.measured()
    }

    /// The graphic curve a drag away from the handles paints: the first on
    /// the channel being edited.
    fn curve_index(&self) -> Option<usize> {
        let e = self.editing.as_ref()?;
        e.bands.iter().position(|b| {
            b.kind == "graphic" && (b.channels.is_empty() || b.channels.contains(&e.channel))
        })
    }

    /// The bands with a handle: the core's draggable types, on the channel
    /// drawn.
    fn handles(&self, cx: &App) -> Vec<Handle> {
        let Some(e) = &self.editing else {
            return Vec::new();
        };
        let bounds = &e.dsp.read(cx).bounds;
        let mut handles: Vec<Handle> = e
            .bands
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                bounds.kinds.iter().any(|k| k.draggable && k.id == b.kind)
                    && (b.channels.is_empty() || b.channels.contains(&e.channel))
            })
            .map(|(index, b)| Handle {
                index,
                hz: b.freq,
                db: b.gain_db,
                q: b.q,
            })
            .collect();
        // The band held follows the pointer, not the filters last read.
        if let Edit::Dragging(drag) = &self.edit
            && let Drag {
                index,
                step:
                    Some(Commit::Band {
                        freq, gain_db, q, ..
                    }),
                ..
            } = drag.as_ref()
            && let Some(h) = handles.iter_mut().find(|h| h.index == *index)
        {
            (h.hz, h.db, h.q) = (*freq, *gain_db, *q);
        }
        handles
    }

    /// The level the EQ view's curves sit at: the preamp, and any flat gain
    /// among the filters, which `total` holds.
    fn level(&self, r: &DspResponse) -> f64 {
        if self.showing_eq() {
            r.preamp_db + self.gain(r)
        } else {
            0.0
        }
    }

    /// For an EQ with bands for each channel, the right's gain while the
    /// right is edited.
    fn gain(&self, r: &DspResponse) -> f64 {
        match (&self.editing, &r.right) {
            (Some(e), Some(_)) if e.channel == 1 => r.right_gain_db,
            _ => r.gain_db,
        }
    }

    /// What is drawn, by slot: the curves, their strokes or fills, and how
    /// far each is moved.
    fn curves(&self, r: &DspResponse) -> Vec<Curve> {
        let mut out = Vec::new();
        if !self.showing_eq() {
            let mut add = |slot: &'static str, db: &Option<Vec<f64>>, stroke: Stroke| {
                if let Some(db) = db {
                    out.push(Curve::line(slot, 0, db.clone(), stroke, 0.0));
                }
            };
            add(
                "measurement",
                &r.measurement,
                Stroke::solid(Tone::Muted, 1.2),
            );
            add(
                "target",
                &r.target,
                Stroke {
                    opacity: 0.55,
                    dash: &[4.0, 3.0],
                    ..Stroke::solid(Tone::Ink, 1.2)
                },
            );
            add("predicted", &r.predicted, Stroke::solid(Tone::Accent, 2.0));
            return out;
        }
        let level = self.level(r);
        let shift = r.preamp_db;
        for (i, band) in r.bands.iter().enumerate() {
            out.push(Curve::area(i, band.db.clone(), level));
        }
        // An EQ with bands for each channel: the channel edited is the
        // total, the other beside it.
        let (total, other) = match (&self.editing, &r.right) {
            (Some(e), Some(right)) if e.channel == 1 => {
                (right.clone(), Some(("left", r.total.clone())))
            }
            (_, Some(right)) => (r.total.clone(), Some(("right", right.clone()))),
            _ => (r.total.clone(), None),
        };
        for (i, part) in self.parts.iter().enumerate() {
            out.push(Curve::line("part", i, part.db.clone(), part.stroke, shift));
        }
        if let Some((_, db)) = other {
            out.push(Curve::line("other", 0, db, Stroke::eq(1), shift));
        }
        if self.parts.is_empty()
            && let (Some(c), Some(t)) = (&r.correction, &r.tuning)
        {
            let correction = Stroke {
                opacity: 0.7,
                dash: &[4.0, 3.0],
                ..Stroke::CORRECTION
            };
            out.push(Curve::line("correction", 0, c.clone(), correction, shift));
            out.push(Curve::line(
                "tuning",
                0,
                t.clone(),
                Stroke::solid(Tone::Ink, 1.2),
                shift,
            ));
        }
        if let Some(original) = &r.original {
            let stroke = Stroke {
                dash: &[4.0, 3.0],
                ..Stroke::solid(Tone::Muted, 1.2)
            };
            out.push(Curve::line("original", 0, original.clone(), stroke, shift));
        }
        let stroke = if self.parts.is_empty() {
            Stroke::solid(Tone::Accent, 2.0)
        } else {
            Stroke::TOTAL
        };
        out.push(Curve::line("total", 0, total, stroke, shift));
        out
    }

    /// The dB range drawn: the next 6 dB past whatever is drawn, at least
    /// ±6, so a small correction is not drawn as a large one. The EQ view is
    /// centred on its level.
    fn domain(&self, r: &DspResponse, curves: &[Curve], handles: &[Handle]) -> (f64, f64) {
        let level = self.level(r);
        let values = curves
            .iter()
            .flat_map(|c| c.db.iter().map(move |v| v + c.shift))
            .chain(handles.iter().map(|h| h.db + level))
            .chain([0.0, level]);
        if self.showing_eq() {
            let reach = values.map(|v| (v - level).abs()).fold(0.0, f64::max);
            let half = (reach / 6.0).ceil().max(1.0) * 6.0;
            return (level - half, level + half);
        }
        let (lo, hi) = values.fold((0.0f64, 0.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
        (
            ((lo / 6.0).floor() * 6.0).min(-6.0),
            ((hi / 6.0).ceil() * 6.0).max(6.0),
        )
    }

    fn release(&mut self, cx: &mut App) {
        // A page closed under a drag: its last step is saved by the model,
        // which outlives the page. One closed while settling has nothing
        // more to send.
        if let (Edit::Dragging(drag), Some(e)) =
            (std::mem::replace(&mut self.edit, Edit::Idle), &self.editing)
            && let Some(step) = drag.step
        {
            // Nobody is left to hear how it ended, so the model forgets it
            // once it lands.
            let dsp = e.dsp.downgrade();
            let edit = drag.edit;
            e.dsp.update(cx, |model, cx| {
                model.edit(
                    edit,
                    step,
                    true,
                    Rc::new(move |_, cx| {
                        dsp.update(cx, |dsp, _| dsp.forget(edit)).ok();
                    }),
                    cx,
                )
            });
        }
    }
}

/// A curve as drawn: a line, or a band's area from the level to it.
#[derive(Clone, Debug, PartialEq)]
struct Curve {
    slot: (&'static str, usize),
    db: Vec<f64>,
    stroke: Option<Stroke>,
    shift: f64,
}

impl Curve {
    fn line(slot: &'static str, i: usize, db: Vec<f64>, stroke: Stroke, shift: f64) -> Self {
        Curve {
            slot: (slot, i),
            db,
            stroke: Some(stroke),
            shift,
        }
    }

    fn area(i: usize, db: Vec<f64>, level: f64) -> Self {
        Curve {
            slot: ("band", i),
            db,
            stroke: None,
            shift: level,
        }
    }
}

/// Paths built at the plot's origin, relative to the level the curves sit
/// at, kept while their curve, the plot's size, the scale and the range
/// around the level are unchanged: a preamp that moves with an edit moves
/// the level and nothing else, so it rebuilds nothing.
#[derive(Default)]
struct Cache {
    plot: Option<(Size, f32, (f64, f64), usize)>,
    paths: Vec<(Curve, Option<Rc<Path<Pixels>>>)>,
    /// Where each handle was last drawn, in the window.
    handles: Vec<(usize, Point<Pixels>)>,
}

type Size = koan_kit::app::Size<Pixels>;

/// A curve's path and how it is drawn.
type Drawn = (Option<Stroke>, Rc<Path<Pixels>>);

impl Cache {
    fn paths(
        &mut self,
        curves: Vec<Curve>,
        freqs: &[f64],
        plot: Size,
        scale: f32,
        (lo, hi): (f64, f64),
        level: f64,
    ) -> Vec<Drawn> {
        let domain = (lo - level, hi - level);
        let key = (plot, scale, domain, freqs.len());
        if self.plot != Some(key) {
            self.plot = Some(key);
            self.paths.clear();
        }
        let mut kept = std::mem::take(&mut self.paths);
        self.paths = curves
            .into_iter()
            .map(|mut curve| {
                curve.shift -= level;
                if let Some(at) = kept.iter().position(|(c, _)| c == &curve) {
                    return kept.swap_remove(at);
                }
                let path = build(&curve, freqs, plot, domain).map(Rc::new);
                (curve, path)
            })
            .collect();
        self.paths
            .iter()
            .filter_map(|(c, p)| p.clone().map(|p| (c.stroke, p)))
            .collect()
    }
}

fn y_of(db: f64, height: f32, (lo, hi): (f64, f64)) -> f32 {
    ((hi - db) / (hi - lo)) as f32 * height
}

fn build(curve: &Curve, freqs: &[f64], plot: Size, domain: (f64, f64)) -> Option<Path<Pixels>> {
    let (w, h) = (f32::from(plot.width), f32::from(plot.height));
    let n = curve.db.len().min(freqs.len());
    if n < 2 {
        return None;
    }
    let at = |i: usize, db: f64| point(px(x_of(freqs[i], w)), px(y_of(db, h, domain)));
    match curve.stroke {
        Some(stroke) => {
            let mut b = PathBuilder::stroke(px(stroke.width));
            if !stroke.dash.is_empty() {
                let dash: Vec<Pixels> = stroke.dash.iter().map(|d| px(*d)).collect();
                b = b.dash_array(&dash);
            }
            b.move_to(at(0, curve.db[0] + curve.shift));
            for i in 1..n {
                b.line_to(at(i, curve.db[i] + curve.shift));
            }
            b.build().ok()
        }
        None => {
            let mut b = PathBuilder::fill();
            b.move_to(at(0, curve.shift));
            for i in 0..n {
                b.line_to(at(i, curve.shift + curve.db[i]));
            }
            b.line_to(at(n - 1, curve.shift));
            b.close();
            b.build().ok()
        }
    }
}

/// A path built at the origin, moved to `by`.
fn moved(path: &Path<Pixels>, by: Point<Pixels>) -> Path<Pixels> {
    let mut p = path.clone();
    p.bounds.origin += by;
    for v in &mut p.vertices {
        v.xy_position += by;
    }
    p
}

/// What prepaint hands paint.
struct Frame {
    plot: Bounds<Pixels>,
    domain: (f64, f64),
    level: f64,
    paths: Vec<Drawn>,
    // painted and hit-tested by hand: drawing koan-kit has no component for.
    hitbox: koan_kit::raw::Hitbox,
    handles: Vec<Handle>,
    held: Option<usize>,
    showing_eq: bool,
}

impl Render for Graph {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let response = self.shown_response().clone();
        let handles = if self.showing_eq() {
            self.handles(cx)
        } else {
            Vec::new()
        };
        let curves = self.curves(&response);
        let domain = self.domain(&response, &curves, &handles);
        let level = self.level(&response);
        let held = match &self.edit {
            Edit::Dragging(d) if matches!(d.kind, Grab::Band { .. }) => Some(d.index),
            _ => None,
        };
        let showing_eq = self.showing_eq();
        let cache = self.cache.clone();
        let this = cx.weak_entity();
        let editable = self.editing.is_some() && showing_eq;
        let height = self.height;
        let measured = self.measured();
        let shown = self.shown;

        let chart = canvas(
            {
                let response = response.clone();
                move |bounds, window, _cx| {
                    let plot = Bounds::new(
                        bounds.origin + point(px(AXIS_W), px(0.0)),
                        size(
                            bounds.size.width - px(AXIS_W),
                            bounds.size.height - px(AXIS_H),
                        ),
                    );
                    let geometry = Geometry {
                        plot,
                        domain,
                        level,
                    };
                    cache.borrow_mut().handles =
                        handles.iter().map(|h| (h.index, geometry.at(h))).collect();
                    let paths = cache.borrow_mut().paths(
                        curves,
                        &response.freqs,
                        plot.size,
                        window.scale_factor(),
                        domain,
                        level,
                    );
                    // painted and hit-tested by hand: drawing koan-kit has no component for.
                    let hitbox = window.insert_hitbox(plot, koan_kit::raw::HitboxBehavior::Normal);
                    Frame {
                        plot,
                        domain,
                        level,
                        paths,
                        hitbox,
                        handles,
                        held,
                        showing_eq,
                    }
                }
            },
            move |bounds, frame, window, cx| {
                paint_chart(bounds, &frame, &look, window, cx);
                if editable {
                    listen(this, &frame, window);
                }
            },
        )
        .w_full()
        .h(height);

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(look.space(Space::M))
            .when(measured, |d| {
                d.child(
                    Segmented::new(
                        "graph-shown",
                        ["EQ", "Measured"],
                        (shown == Shown::Measured) as usize,
                    )
                    .on_change({
                        let this = cx.weak_entity();
                        move |i, _, cx| {
                            this.update(cx, |g, cx| {
                                g.shown = if *i == 1 { Shown::Measured } else { Shown::Eq };
                                cx.notify();
                            })
                            .ok();
                        }
                    }),
                )
            })
            .child(
                div()
                    .relative()
                    .child(chart)
                    .children(self.flat_note.clone().map(|note| {
                        div()
                            .absolute()
                            .top(look.space(Space::Xl))
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .text_role(TypeRole::Meta, &look)
                            .text_color(look.text_color(Tone::Muted))
                            .child(label(note))
                    })),
            )
            .child(self.legend(&response, &look))
            .when(self.curve_index().is_some() && showing_eq, |d| {
                d.child(self.brushes(cx))
            })
    }
}

impl Graph {
    fn legend(&self, r: &DspResponse, look: &Look) -> impl IntoElement {
        let mut keys: Vec<(SharedString, Stroke, bool)> = Vec::new();
        let app = |s: &str| -> SharedString { label(s) };
        if self.showing_eq() {
            if !self.parts.is_empty() {
                for p in &self.parts {
                    keys.push((p.name.clone(), p.stroke, false));
                }
                keys.push((app("Total"), Stroke::TOTAL, false));
            } else if r.correction.is_some() && r.tuning.is_some() {
                keys.push((
                    app("Correction"),
                    Stroke {
                        opacity: 0.7,
                        dash: &[3.0, 2.0],
                        ..Stroke::CORRECTION
                    },
                    false,
                ));
                keys.push((app("Tuning"), Stroke::solid(Tone::Ink, 2.0), false));
                keys.push((app("Total"), Stroke::solid(Tone::Accent, 2.0), false));
            } else {
                keys.push((app("EQ"), Stroke::solid(Tone::Accent, 2.0), false));
            }
            if r.right.is_some() {
                let other = match &self.editing {
                    Some(e) if e.channel == 1 => "Left",
                    _ => "Right",
                };
                keys.push((app(other), Stroke::eq(1), false));
            }
            if r.original.is_some() {
                keys.push((
                    app("Original"),
                    Stroke {
                        dash: &[3.0, 2.0],
                        ..Stroke::solid(Tone::Muted, 2.0)
                    },
                    false,
                ));
            }
            keys.push((app("No change"), Stroke::solid(Tone::Muted, 1.0), true));
            if !r.bands.is_empty() {
                keys.push((
                    app("Each band"),
                    Stroke {
                        opacity: 0.3,
                        ..Stroke::solid(Tone::Muted, 2.0)
                    },
                    false,
                ));
            }
        } else {
            keys.push((app("Measured"), Stroke::solid(Tone::Muted, 2.0), false));
            keys.push((
                app("Target"),
                Stroke {
                    opacity: 0.55,
                    dash: &[3.0, 2.0],
                    ..Stroke::solid(Tone::Ink, 2.0)
                },
                false,
            ));
            keys.push((app("Corrected"), Stroke::solid(Tone::Accent, 2.0), false));
        }
        let preamp = format!(
            "{} {}",
            label("Preamp"),
            db_label(r.preamp_db + self.gain(r))
        );
        div()
            .flex()
            .flex_wrap()
            .gap(look.space(Space::M))
            .text_role(TypeRole::Fine, look)
            .text_color(look.text_color(Tone::Muted))
            .children(keys.into_iter().map(|(name, stroke, thin)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(swatch(stroke, thin, look))
                    .child(name)
            }))
            .child(preamp)
    }

    fn brushes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = match self.brush {
            DspBrush::Point => 0,
            DspBrush::Narrow => 1,
            DspBrush::Wide => 2,
        };
        let this = cx.weak_entity();
        Segmented::new("graph-brush", ["Point", "Narrow", "Wide"], selected).on_change(
            move |i, _, cx| {
                this.update(cx, |g, cx| {
                    g.brush = [DspBrush::Point, DspBrush::Narrow, DspBrush::Wide][*i];
                    cx.notify();
                })
                .ok();
            },
        )
    }
}

/// A key's line: the stroke as drawn, short.
fn swatch(stroke: Stroke, thin: bool, look: &Look) -> impl IntoElement {
    let color = stroke.color(look);
    let height = if thin { look.hairline } else { px(2.0) };
    if stroke.dash.is_empty() {
        return div().w(px(14.0)).h(height).bg(color).into_any_element();
    }
    div()
        .flex()
        .gap(px(2.0))
        .children((0..3).map(move |_| div().w(px(3.0)).h(height).bg(color)))
        .into_any_element()
}

pub fn db_label(db: f64) -> String {
    if db.abs() < 0.05 {
        "0.0 dB".into()
    } else {
        format!("{db:+.1} dB").replace('-', "−")
    }
}

fn hz_label(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{}k", (hz / 1000.0) as i64)
    } else {
        format!("{}", hz as i64)
    }
}

fn paint_chart(bounds: Bounds<Pixels>, f: &Frame, look: &Look, window: &mut Window, cx: &mut App) {
    let plot = f.plot;
    let (w, h) = (f32::from(plot.size.width), f32::from(plot.size.height));
    let rule = look.palette.rule.hsla();
    let muted = look.palette.muted.hsla();
    let text = look.text_color(Tone::Muted);
    let font_size = look.font_size(TypeRole::Fine);
    let line_height = look.line_height(TypeRole::Fine);
    let mono = Font {
        weight: FontWeight::NORMAL,
        ..font(FONT_FAMILY)
    };
    let label_at = |s: String,
                    origin: Point<Pixels>,
                    align: TextAlign,
                    width: Pixels,
                    window: &mut Window,
                    cx: &mut App| {
        let run = TextRun {
            len: s.len(),
            font: mono.clone(),
            color: text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(s.into(), font_size, &[run], None);
        line.paint(origin, line_height, align, Some(width), window, cx)
            .ok();
    };

    // The grid: a line at each tick, and the axes' labels. A label is left
    // out where it would run into the one before, as on a phone's narrow
    // graph, where each is also kept inside the plot's width.
    let mut labelled_to: Option<Pixels> = None;
    for hz in HZ_TICKS {
        let x = plot.origin.x + px(x_of(hz, w));
        window.paint_quad(fill(
            Bounds::new(
                point(x, plot.origin.y),
                size(look.hairline, plot.size.height),
            ),
            rule.opacity(0.5),
        ));
        let tick = hz_label(hz);
        let width = window
            .text_system()
            .shape_line(
                tick.clone().into(),
                font_size,
                &[TextRun {
                    len: tick.len(),
                    font: mono.clone(),
                    color: text,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width;
        let centred = f32::from(x - width / 2.0);
        let left = Pixels::from(if look.touch {
            centred
                .min(f32::from(plot.origin.x + plot.size.width - width))
                .max(f32::from(plot.origin.x))
        } else {
            centred
        });
        if labelled_to.is_some_and(|to| left < to + look.space(Space::S)) {
            continue;
        }
        labelled_to = Some(left + width);
        label_at(
            tick,
            point(left, plot.origin.y + plot.size.height + px(2.0)),
            TextAlign::Left,
            width + look.hairline,
            window,
            cx,
        );
    }
    let (lo, hi) = f.domain;
    let stride = if hi - lo > 30.0 { 10.0 } else { 6.0 };
    let mut db = (lo / stride).ceil() * stride;
    while db <= hi + 1e-9 {
        let y = plot.origin.y + px(y_of(db, h, f.domain));
        window.paint_quad(fill(
            Bounds::new(
                point(plot.origin.x, y),
                size(plot.size.width, look.hairline),
            ),
            rule.opacity(0.5),
        ));
        label_at(
            format!("{}", db.round() as i64),
            point(bounds.origin.x, y - line_height / 2.0),
            TextAlign::Right,
            px(AXIS_W - 6.0),
            window,
            cx,
        );
        db += stride;
    }

    if f.showing_eq {
        // No change, and the level the curves sit at.
        let zero = plot.origin.y + px(y_of(0.0, h, f.domain));
        window.paint_quad(fill(
            Bounds::new(
                point(plot.origin.x, zero),
                size(plot.size.width, look.hairline),
            ),
            rule,
        ));
        if f.level.abs() >= 0.05 {
            let y = px(y_of(f.level, h, f.domain));
            let mut b = PathBuilder::stroke(px(1.0)).dash_array(&[px(4.0), px(3.0)]);
            b.move_to(point(px(0.0), y));
            b.line_to(point(plot.size.width, y));
            if let Ok(path) = b.build() {
                window.paint_path(moved(&path, plot.origin), muted);
            }
            label_at(
                format!("{} {}", label("Preamp"), db_label(f.level)),
                point(
                    plot.origin.x + px(4.0),
                    plot.origin.y
                        + y
                        + if f.level < 0.0 {
                            px(2.0)
                        } else {
                            -line_height - px(2.0)
                        },
                ),
                TextAlign::Left,
                plot.size.width,
                window,
                cx,
            );
        }
    }

    window.with_content_mask(
        // painted and hit-tested by hand: drawing koan-kit has no component for.
        Some(koan_kit::raw::ContentMask { bounds: plot }),
        |window| {
            for (stroke, path) in &f.paths {
                let color = match stroke {
                    Some(stroke) => stroke.color(look),
                    None => muted.opacity(0.15),
                };
                window.paint_path(moved(path, plot.origin), color);
            }
        },
    );

    let accent = look.accent.hsla();
    for handle in &f.handles {
        let r = if f.held == Some(handle.index) {
            HANDLE_HELD
        } else {
            HANDLE
        };
        let c = point(
            plot.origin.x + px(x_of(handle.hz, w)),
            plot.origin.y + px(y_of(f.level + handle.db, h, f.domain)),
        );
        window.paint_quad(
            fill(
                Bounds::new(c - point(px(r), px(r)), size(px(2.0 * r), px(2.0 * r))),
                accent,
            )
            .corner_radii(px(r)),
        );
    }
}

/// The drag's listeners, live while the graph is painted: a press on the
/// plot starts one; moves and the release are heard wherever the pointer
/// is, so a drag let go outside the graph still ends.
fn listen(this: WeakEntity<Graph>, f: &Frame, window: &mut Window) {
    let plot = f.plot;
    let (domain, level) = (f.domain, f.level);
    let handles = f.handles.clone();
    let hitbox = f.hitbox.clone();
    let geometry = Geometry {
        plot,
        domain,
        level,
    };
    {
        let this = this.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || e.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let near = nearest(&handles, e.position, &geometry);
            this.update(cx, |g, cx| g.grab(near, e.position, cx)).ok();
        });
    }
    {
        let this = this.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            this.update(cx, |g, cx| {
                if matches!(g.edit, Edit::Dragging(_)) {
                    if e.pressed_button != Some(MouseButton::Left) {
                        g.let_go(cx);
                    } else {
                        g.step(e.position, e.modifiers, &geometry, cx);
                    }
                }
            })
            .ok();
        });
    }
    window.on_mouse_event(move |e: &MouseUpEvent, phase, _window, cx| {
        if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
            return;
        }
        this.update(cx, |g, cx| g.let_go(cx)).ok();
    });
}

#[derive(Clone, Copy)]
struct Geometry {
    plot: Bounds<Pixels>,
    domain: (f64, f64),
    level: f64,
}

impl Geometry {
    fn at(&self, h: &Handle) -> Point<Pixels> {
        let (w, ht) = (
            f32::from(self.plot.size.width),
            f32::from(self.plot.size.height),
        );
        point(
            self.plot.origin.x + px(x_of(h.hz, w)),
            self.plot.origin.y + px(y_of(self.level + h.db, ht, self.domain)),
        )
    }

    /// The frequency and the gain under `p`, as a band adds them to the
    /// level.
    fn value(&self, p: Point<Pixels>) -> (f64, f64) {
        let (w, ht) = (
            f32::from(self.plot.size.width),
            f32::from(self.plot.size.height),
        );
        let x = f32::from(p.x - self.plot.origin.x);
        let y = f32::from(p.y - self.plot.origin.y).clamp(0.0, ht);
        let (lo, hi) = self.domain;
        let db = hi - (y / ht) as f64 * (hi - lo);
        (hz_at(x, w), db - self.level)
    }
}

fn nearest(handles: &[Handle], p: Point<Pixels>, g: &Geometry) -> Option<Handle> {
    handles
        .iter()
        .map(|h| {
            let at = g.at(h);
            let (dx, dy) = (f32::from(at.x - p.x), f32::from(at.y - p.y));
            (*h, (dx * dx + dy * dy).sqrt())
        })
        .filter(|(_, d)| *d < REACH)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(h, _)| h)
}

impl Graph {
    /// A press on the plot: the handle under it, or the start of a stroke
    /// where there is a curve to paint. Nothing starts while the last edit
    /// is still being saved.
    fn grab(&mut self, near: Option<Handle>, at: Point<Pixels>, cx: &mut Context<Self>) {
        if matches!(self.edit, Edit::Dragging(_) | Edit::Settling(_)) {
            return;
        }
        let Some(e) = &self.editing else { return };
        let (index, kind) = match near {
            Some(h) => {
                let Some(band) = e.bands.get(h.index) else {
                    return;
                };
                (
                    h.index,
                    Grab::Band {
                        kind: band.kind.clone(),
                    },
                )
            }
            None => match self.curve_index() {
                Some(index) => (index, Grab::Paint),
                None => return,
            },
        };
        let band = e.bands[index].clone();
        let edit = e.dsp.update(cx, |dsp, _| dsp.next_edit());
        self.painted = None;
        self.edit = Edit::Dragging(Box::new(Drag {
            edit,
            index,
            kind,
            band,
            started: at,
            alt_from: None,
            step: None,
            saved_at: Instant::now(),
        }));
        cx.notify();
    }

    /// A step of the drag: the handle moved, or the curve painted, drawn at
    /// once and previewed, and saved if a quarter second has passed since
    /// the last save. A drag held still saves nothing until it moves again
    /// or is let go.
    fn step(
        &mut self,
        at: Point<Pixels>,
        modifiers: Modifiers,
        g: &Geometry,
        cx: &mut Context<Self>,
    ) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        let Edit::Dragging(drag) = &mut self.edit else {
            return;
        };
        let bounds = e.dsp.read(cx).bounds.clone();
        let (hz, db) = g.value(at);
        let (hz, db) = (
            hz.clamp(bounds.hz_min, bounds.hz_max),
            db.clamp(bounds.db_min, bounds.db_max),
        );
        let step = match drag.kind.clone() {
            Grab::Band { kind } => {
                let (freq, gain_db, q) = match &drag.step {
                    Some(Commit::Band {
                        freq, gain_db, q, ..
                    }) => (*freq, *gain_db, *q),
                    _ => (drag.band.freq, drag.band.gain_db, drag.band.q),
                };
                // Option-drag widens or narrows the band where it is: up for
                // a higher Q, measured from where Option went down.
                let (freq, gain_db, q) = if modifiers.alt {
                    let (from_y, from_q) = *drag.alt_from.get_or_insert((at.y, q));
                    let dy = f32::from(at.y - from_y) as f64;
                    (freq, gain_db, koan_runtime::dsp_drag_q(from_q, dy))
                } else {
                    drag.alt_from = None;
                    (hz, db, q)
                };
                Commit::Band {
                    name: e.name.clone(),
                    index: drag.index as u32,
                    expected: drag.band.clone(),
                    kind,
                    freq,
                    gain_db,
                    q,
                }
            }
            Grab::Paint => {
                let d = at - drag.started;
                let travelled = (f32::from(d.x).powi(2) + f32::from(d.y).powi(2)).sqrt();
                if drag.step.is_none() && travelled < PAINT_SLOP {
                    return;
                }
                let from = self
                    .painted
                    .clone()
                    .unwrap_or_else(|| drag.band.curve.clone());
                let points = koan_runtime::dsp_paint_curve(from, hz, db, self.brush);
                self.painted = Some(points.clone());
                Commit::Curve {
                    name: e.name.clone(),
                    index: drag.index as u32,
                    expected: drag.band.clone(),
                    points,
                }
            }
        };
        drag.step = Some(step.clone());
        if drag.saved_at.elapsed() >= SAVE_EVERY {
            drag.saved_at = Instant::now();
            let id = drag.edit;
            let done = self.on_ended(id, cx);
            e.dsp
                .update(cx, |dsp, cx| dsp.edit(id, step, false, done, cx));
        }
        self.preview(cx);
        cx.notify();
    }

    /// The drag's end, once only: let go, or the button seen up. Its last
    /// step is saved; once that lands the preview gives way to what was
    /// saved.
    fn let_go(&mut self, cx: &mut Context<Self>) {
        let Edit::Dragging(drag) = std::mem::replace(&mut self.edit, Edit::Idle) else {
            return;
        };
        let (Some(step), Some(e)) = (drag.step, &self.editing) else {
            self.apply_queued();
            cx.notify();
            return;
        };
        self.edit = Edit::Settling(drag.edit);
        let id = drag.edit;
        let done = self.on_ended(id, cx);
        e.dsp
            .update(cx, |dsp, cx| dsp.edit(id, step, true, done, cx));
        cx.notify();
    }

    fn on_ended(&self, id: u64, cx: &mut Context<Self>) -> Done {
        let this = cx.weak_entity();
        Rc::new(move |ended: Ended, cx: &mut App| {
            this.update(cx, |g, cx| g.ended(id, ended, cx)).ok();
        })
    }

    /// An edit's saves ended: its last one landed, or one was refused or
    /// failed, which ends a drag still held; the rest of that drag moves
    /// nothing.
    fn ended(&mut self, id: u64, ended: Ended, cx: &mut Context<Self>) {
        let ours = match &self.edit {
            Edit::Settling(e) => *e == id,
            Edit::Dragging(d) => d.edit == id && ended.outcome != Outcome::Saved,
            _ => false,
        };
        if !ours {
            return;
        }
        if let Some(e) = &self.editing {
            e.dsp.update(cx, |d, _| d.forget(id));
        }
        self.apply_queued();
        if ended.outcome == Outcome::Saved {
            self.edit = Edit::Settled(ended.stamp);
        } else {
            // Refused or failed: what is drawn goes back to what is saved,
            // which the page reads again.
            self.edit = Edit::Idle;
            self.live = None;
            self.painted = None;
        }
        cx.notify();
    }

    fn apply_queued(&mut self) {
        if let Some(editing) = self.queued.take() {
            self.editing = editing;
        }
    }

    /// Draws the newest step through the core, one preview at a time; one
    /// landing after its drag has ended is dropped.
    fn preview(&mut self, cx: &mut Context<Self>) {
        if self.previewing {
            return;
        }
        let Some(e) = &self.editing else { return };
        let Edit::Dragging(drag) = &self.edit else {
            return;
        };
        let Some(step) = drag.step.clone() else {
            return;
        };
        self.previewing = true;
        let engine = e.dsp.read(cx).engine();
        let id = drag.edit;
        cx.spawn(async move |this, cx| {
            let drawn = match step.clone() {
                Commit::Band {
                    name,
                    index,
                    kind,
                    freq,
                    gain_db,
                    q,
                    ..
                } => {
                    engine
                        .dsp_preview_band(name, index, kind, freq, gain_db, q, GRAPH_RATE)
                        .await
                }
                Commit::Curve {
                    name,
                    index,
                    points,
                    ..
                } => {
                    engine
                        .dsp_preview_curve(name, index, points, GRAPH_RATE)
                        .await
                }
            };
            this.update(cx, |g, cx| {
                g.previewing = false;
                let Edit::Dragging(drag) = &g.edit else {
                    return;
                };
                // A preview for an older drag: the drag now under way may
                // have a step waiting on it.
                if drag.edit != id {
                    g.preview(cx);
                    return;
                }
                if let Some(drawn) = drawn {
                    g.live = Some(Rc::new(drawn));
                    cx.notify();
                }
                if drag.step.as_ref().is_some_and(|s| !same_step(s, &step)) {
                    g.preview(cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

fn same_step(a: &Commit, b: &Commit) -> bool {
    match (a, b) {
        (
            Commit::Band {
                freq,
                gain_db,
                q,
                kind,
                ..
            },
            Commit::Band {
                freq: f2,
                gain_db: g2,
                q: q2,
                kind: k2,
                ..
            },
        ) => freq == f2 && gain_db == g2 && q == q2 && kind == k2,
        (Commit::Curve { points, .. }, Commit::Curve { points: p2, .. }) => {
            points.len() == p2.len()
                && points
                    .iter()
                    .zip(p2)
                    .all(|(a, b)| a.hz == b.hz && a.db == b.db)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axis_is_logarithmic_from_20_hz_to_20_khz() {
        assert_eq!(x_of(20.0, 300.0), 0.0);
        assert!((x_of(20_000.0, 300.0) - 300.0).abs() < 1e-3);
        assert!((x_of(632.455_5, 300.0) - 150.0).abs() < 1e-2);
        for hz in [25.0, 440.0, 9000.0] {
            assert!((hz_at(x_of(hz, 300.0), 300.0) - hz).abs() / hz < 1e-5);
        }
    }

    #[test]
    fn labels_say_the_level_and_the_tick() {
        assert_eq!(db_label(0.01), "0.0 dB");
        assert_eq!(db_label(-3.25), "−3.2 dB");
        assert_eq!(db_label(2.0), "+2.0 dB");
        assert_eq!(hz_label(2000.0), "2k");
        assert_eq!(hz_label(50.0), "50");
    }
}
