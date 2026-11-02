//! A profile's filters as a dense table, its parametric bands editable in
//! place: number, type, frequency, gain and Q, the figures right-aligned. A
//! graphic curve opens a table of its points; delays and mixes are shown as
//! they are.
//!
//! A figure is shown to as many places as it is typed to, and saved only
//! when the text in its field was changed: a field left as shown never
//! writes back a rounded value, so a band keeps every digit its file gave it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use koan_ffi::{DspBand, DspPoint};
use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, SharedString,
    Subscription, Window, div,
};
use koan_kit::app::{InteractiveElement, StatefulInteractiveElement, Styled};

use super::menu::{Item, Trigger, menu};
use super::model::{Commit, Dsp};
use crate::components::sheet::note;
use crate::components::{Align, Column, DenseRow, DenseTable, TextField};
use crate::theme::{Look, Space, Tone};

/// A figure of a band's row, or of a curve's point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Freq,
    Gain,
    Q,
    PointHz(usize),
    PointDb(usize),
}

impl Part {
    /// How many places a figure is shown and typed to.
    fn digits(self) -> usize {
        match self {
            Part::Freq => 0,
            Part::Gain | Part::PointHz(_) | Part::PointDb(_) => 1,
            Part::Q => 2,
        }
    }
}

/// `v` to at most `digits` places, trailing zeros dropped.
pub fn figure(v: f64, digits: usize) -> String {
    let s = format!("{v:.digits$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".into() } else { s }
}

/// What typing `text` into a field showing `shown` asks: nothing when it is
/// unchanged or not a number.
pub fn typed(text: &str, shown: &str) -> Option<f64> {
    let text = text.trim().replace('−', "-");
    if text == shown {
        return None;
    }
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

struct Field {
    input: Entity<InputState>,
    /// The text the field was last given, and the row's reset it last
    /// took.
    shown: Rc<RefCell<String>>,
    reset: u64,
    _subscription: Subscription,
}

/// What a row holds between renders: the filter as it last read or saved
/// it, which every figure of the row saves against, and the edit those
/// saves chain through.
struct RowState {
    band: DspBand,
    edit: Option<u64>,
    /// The row's newest save, numbered, and whether it is still under way.
    /// Saves of one edit chain, and only the newest is answered, so it is
    /// the one waited on: it carries every figure typed before it.
    latest: u64,
    saving: bool,
    /// The stamp the row's last save was read back at: until the page has
    /// read the filters at or after it, what it read is older than the row.
    settled: Option<(u64, u64)>,
    /// A save was refused as stale: the row takes the filters as next read,
    /// at or after the refusal, even while it is being typed in.
    stale: bool,
    /// Moves each time the row is put back, so a field being typed in is
    /// put back too.
    reset: u64,
}

struct Row {
    state: Rc<RefCell<RowState>>,
    fields: HashMap<Part, Field>,
}

/// The fields of one profile's table, kept between renders. A row takes
/// the filters as read only when nobody is typing in it, none of its saves
/// is under way and the read is at least as new as its last save; a save
/// refused because the filter changed puts the row back at once.
pub struct Fields {
    dsp: Entity<Dsp>,
    profile: String,
    rows: HashMap<usize, Row>,
}

impl Fields {
    pub fn new(dsp: Entity<Dsp>, profile: String) -> Self {
        Self {
            dsp,
            profile,
            rows: HashMap::new(),
        }
    }

    pub fn rename(&mut self, profile: String) {
        if profile != self.profile {
            self.profile = profile;
            self.rows.clear();
        }
    }

    /// Drops the rows of filters that are gone.
    pub fn keep(&mut self, bands: &[DspBand]) {
        self.rows.retain(|i, row| {
            let Some(b) = bands.get(*i) else {
                return false;
            };
            row.fields.retain(|part, _| match part {
                Part::PointHz(p) | Part::PointDb(p) => b.kind == "graphic" && *p < b.curve.len(),
                _ => b.kind != "graphic",
            });
            true
        });
    }

    /// Takes `band`, the filter as read at `stamp`, into its row when the
    /// row may take it, and gives back the row.
    fn row(
        &mut self,
        index: usize,
        band: &DspBand,
        stamp: (u64, u64),
        window: &Window,
        cx: &mut App,
    ) -> Rc<RefCell<RowState>> {
        let row = self.rows.entry(index).or_insert_with(|| Row {
            state: Rc::new(RefCell::new(RowState {
                band: band.clone(),
                edit: None,
                latest: 0,
                saving: false,
                settled: None,
                stale: false,
                reset: 0,
            })),
            fields: HashMap::new(),
        });
        let typing = row.fields.values().any(|f| {
            koan_kit::app::Focusable::focus_handle(f.input.read(cx), cx).is_focused(window)
        });
        let state = row.state.clone();
        let mut s = state.borrow_mut();
        let read_since = s.settled.is_none_or(|at| stamp >= at);
        let take = if s.stale {
            if read_since {
                s.stale = false;
                s.reset += 1;
            }
            read_since
        } else {
            !typing && !s.saving && read_since
        };
        if take {
            s.band = band.clone();
            s.settled = None;
            if let Some(id) = s.edit.take() {
                self.dsp.update(cx, |d, _| d.forget(id));
            }
        }
        drop(s);
        state
    }

    #[allow(clippy::too_many_arguments)]
    fn field<V: 'static>(
        &mut self,
        index: usize,
        part: Part,
        band: &DspBand,
        stamp: (u64, u64),
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let state = self.row(index, band, stamp, window, cx);
        let (dsp, profile) = (self.dsp.clone(), self.profile.clone());
        let row = self.rows.get_mut(&index).expect("made by row");
        let (text, reset) = {
            let s = state.borrow();
            (figure(value_of(&s.band, part), part.digits()), s.reset)
        };
        if let std::collections::hash_map::Entry::Vacant(vacant) = row.fields.entry(part) {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(text.clone()));
            let shown = Rc::new(RefCell::new(text.clone()));
            let sub = {
                let (shown, state) = (shown.clone(), state.clone());
                cx.subscribe_in(
                    &input,
                    window,
                    move |_, input, e: &InputEvent, window, cx| {
                        if !matches!(e, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                            return;
                        }
                        let text = input.read(cx).value().trim().to_owned();
                        let Some(v) = typed(&text, &shown.borrow()) else {
                            // Not a number: the field goes back to what it showed.
                            let back = shown.borrow().clone();
                            if text != back {
                                input.update(cx, |i, cx| i.set_value(back, window, cx));
                            }
                            return;
                        };
                        *shown.borrow_mut() = text;
                        commit(
                            &dsp,
                            &profile,
                            index,
                            &state,
                            move |band| set(band, part, v),
                            cx,
                        );
                    },
                )
            };
            vacant.insert(Field {
                input,
                shown,
                reset,
                _subscription: sub,
            });
        }
        let field = row.fields.get_mut(&part).expect("made above");
        let focused =
            koan_kit::app::Focusable::focus_handle(field.input.read(cx), cx).is_focused(window);
        if field.reset != reset || (*field.shown.borrow() != text && !focused) {
            field.reset = reset;
            *field.shown.borrow_mut() = text.clone();
            field
                .input
                .update(cx, |i, cx| i.set_value(text, window, cx));
        }
        // Its column's width, from the table.
        div()
            .w_full()
            .child(TextField::new(&field.input).bare())
            .into_any_element()
    }
}

fn value_of(band: &DspBand, part: Part) -> f64 {
    match part {
        Part::Freq => band.freq,
        Part::Gain => band.gain_db,
        Part::Q => band.q,
        Part::PointHz(p) => band.curve.get(p).map_or(0.0, |p| p.hz),
        Part::PointDb(p) => band.curve.get(p).map_or(0.0, |p| p.db),
    }
}

fn set(band: &mut DspBand, part: Part, v: f64) {
    match part {
        Part::Freq => band.freq = v,
        Part::Gain => band.gain_db = v,
        Part::Q => band.q = v,
        Part::PointHz(p) => {
            if let Some(point) = band.curve.get_mut(p) {
                point.hz = v;
            }
        }
        Part::PointDb(p) => {
            if let Some(point) = band.curve.get_mut(p) {
                point.db = v;
            }
        }
    }
}

/// Saves a change to a row's filter, its other figures as the row holds
/// them, naming the filter the row last read or saved, through the row's
/// edit. The row takes the change at once, so the next figure typed
/// carries it, and takes the filter as stored once the last save under way
/// lands: held to the ranges, so the row never shows what the save changed.
fn commit(
    dsp: &Entity<Dsp>,
    profile: &str,
    index: usize,
    state: &Rc<RefCell<RowState>>,
    change: impl FnOnce(&mut DspBand),
    cx: &mut App,
) {
    let (expected, band, id, seq) = {
        let mut s = state.borrow_mut();
        let expected = s.band.clone();
        let mut band = expected.clone();
        change(&mut band);
        s.band = band.clone();
        s.latest += 1;
        s.saving = true;
        let id = match s.edit {
            Some(id) => id,
            None => {
                let id = dsp.update(cx, |d, _| d.next_edit());
                s.edit = Some(id);
                id
            }
        };
        (expected, band, id, s.latest)
    };
    let step = if band.kind == "graphic" {
        Commit::Curve {
            name: profile.to_owned(),
            index: index as u32,
            expected,
            points: band.curve.clone(),
        }
    } else {
        Commit::Band {
            name: profile.to_owned(),
            index: index as u32,
            expected,
            kind: band.kind.clone(),
            freq: band.freq,
            gain_db: band.gain_db,
            q: band.q,
        }
    };
    let row = state.clone();
    let done: super::model::Done = Rc::new(move |e: super::model::Ended, _| {
        let mut s = row.borrow_mut();
        if s.edit != Some(id) || s.latest != seq {
            return;
        }
        s.saving = false;
        s.settled = Some(e.stamp);
        match (e.outcome, e.band) {
            (super::model::Outcome::Saved, Some(stored)) => {
                // The save held a figure to its range: the row shows that.
                if stored != s.band {
                    s.band = stored;
                    s.reset += 1;
                }
            }
            _ => {
                s.edit = None;
                s.stale = true;
            }
        }
    });
    dsp.update(cx, |d, cx| d.edit(id, step, true, done, cx));
}

const NUMBER_W: f32 = 22.0;
const HZ_W: f32 = 72.0;
const DB_W: f32 = 56.0;
const Q_W: f32 = 50.0;

/// What a row's control does to filter `index`.
pub type RowAction = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Removing a row: the filter's index, its place in the rows shown, and the
/// filter as the row shows it, which the removal names.
pub type RemoveAction = Rc<dyn Fn(usize, usize, DspBand, &mut Window, &mut App)>;

/// The table: a header, then a row per filter on the channel shown, each
/// with its index among all the filters.
#[allow(clippy::too_many_arguments)]
pub fn table<V: 'static>(
    fields: &mut Fields,
    bands: &[DspBand],
    read_only: bool,
    channel: Option<u16>,
    stamp: (u64, u64),
    open_curve: RowAction,
    remove: RemoveAction,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let look = Look::of(window, cx);
    let dsp = fields.dsp.clone();
    let profile = fields.profile.clone();
    let kinds = dsp.read(cx).bounds.kinds.clone();
    let shown: Vec<(usize, &DspBand)> = bands
        .iter()
        .enumerate()
        .filter(|(_, b)| channel.is_none_or(|c| b.channels.is_empty() || b.channels.contains(&c)))
        .collect();
    let columns = [
        Column::fixed(NUMBER_W, Align::Start),
        Column::flex(),
        Column::fixed(HZ_W, Align::End),
        Column::fixed(DB_W, Align::End),
        Column::fixed(Q_W, Align::End),
    ];
    let mut table = DenseTable::new(columns);
    if !bands.is_empty() {
        table = table.header(["#", "Type", "Hz", "dB", "Q"]);
    }
    for (position, (index, band)) in shown.into_iter().enumerate() {
        let kind = kinds.iter().find(|k| k.id == band.kind);
        let number = div()
            .text_color(look.text_color(Tone::Muted))
            .child(format!("{}", position + 1));
        let summary = || {
            div()
                .w_full()
                .flex()
                .justify_between()
                .child(kind_name(band))
                .child(
                    div()
                        .text_color(look.text_color(Tone::Muted))
                        .child(values(band)),
                )
        };
        let row = DenseRow::new().cell(number);
        let row = match (kind, read_only) {
            (Some(kind), false) => {
                // A type chosen goes through the row's saves, as a typed
                // figure does.
                let state = fields.row(index, band, stamp, window, cx);
                let items = kinds
                    .iter()
                    .map(|k| {
                        let (dsp, profile, state, id) =
                            (dsp.clone(), profile.clone(), state.clone(), k.id.clone());
                        let current = state.borrow().band.kind.clone();
                        Item::choice(k.name.clone(), k.id == current, move |_, cx| {
                            if state.borrow().band.kind != id {
                                let id = id.clone();
                                commit(&dsp, &profile, index, &state, move |b| b.kind = id, cx);
                            }
                        })
                    })
                    .chain([Item::Rule, {
                        let (remove, band) = (remove.clone(), band.clone());
                        Item::destructive("Remove", move |window, cx| {
                            remove(index, position, band.clone(), window, cx)
                        })
                    }])
                    .collect();
                row.cell(menu(
                    ("band-kind", index),
                    kind.short.clone(),
                    Trigger::Word,
                    items,
                    window,
                    cx,
                ))
                .cell(fields.field(index, Part::Freq, band, stamp, window, cx))
                .cell(fields.field(index, Part::Gain, band, stamp, window, cx))
                .cell(fields.field(index, Part::Q, band, stamp, window, cx))
            }
            _ if band.kind == "graphic" && !read_only => {
                let open = open_curve.clone();
                row.span(
                    4,
                    summary()
                        .id(("band-curve", index))
                        .cursor_pointer()
                        .on_click(move |_, window, cx| open(index, window, cx)),
                )
            }
            _ => row.span(4, summary()),
        };
        // Delays, mixes and curves are removed from a menu of their own.
        let row = if kind.is_none() && !read_only {
            let (remove, band) = (remove.clone(), band.clone());
            let items = vec![Item::destructive("Remove", move |window, cx| {
                remove(index, position, band.clone(), window, cx)
            })];
            row.trailing(menu(
                ("band-options", index),
                "···",
                Trigger::Word,
                items,
                window,
                cx,
            ))
        } else {
            row
        };
        table = table.row(row);
    }
    table.into_any_element()
}

/// A graphic curve's points, each frequency and gain editable.
pub fn points<V: 'static>(
    fields: &mut Fields,
    index: usize,
    band: &DspBand,
    stamp: (u64, u64),
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let look = Look::of(window, cx);
    let table = DenseTable::new([
        Column::fixed(32.0, Align::Start),
        Column::flex(),
        Column::fixed(80.0, Align::End),
        Column::fixed(64.0, Align::End),
    ])
    .header(["#", "", "Hz", "dB"])
    .rows(
        band.curve
            .iter()
            .enumerate()
            .map(|(i, _): (usize, &DspPoint)| {
                DenseRow::new()
                    .cell(
                        div()
                            .text_color(look.text_color(Tone::Muted))
                            .child(format!("{}", i + 1)),
                    )
                    .cell(div())
                    .cell(fields.field(index, Part::PointHz(i), band, stamp, window, cx))
                    .cell(fields.field(index, Part::PointDb(i), band, stamp, window, cx))
            }),
    );
    div()
        .flex()
        .flex_col()
        .gap(look.space(Space::Xs))
        .child(table)
        .child(note(
            "Edits play at once. Frequency and gain are held to 10 Hz–22 kHz and ±30 dB; a point moved past another takes its place in order.",
            Tone::Muted,
            &look,
        ))
        .into_any_element()
}

/// A filter's type in words, and the channels it is on.
pub fn kind_name(band: &DspBand) -> String {
    let name = match band.kind.as_str() {
        "peaking" => "Peaking",
        "low_shelf" => "Low shelf",
        "high_shelf" => "High shelf",
        "low_pass" => "Low pass",
        "high_pass" => "High pass",
        "notch" => "Notch",
        "band_pass" => "Band pass",
        "all_pass" => "All pass",
        "low_shelf_first_order" => "Low shelf, 6 dB/oct",
        "high_shelf_first_order" => "High shelf, 6 dB/oct",
        "low_pass_first_order" => "Low pass, 6 dB/oct",
        "high_pass_first_order" => "High pass, 6 dB/oct",
        "all_pass_first_order" => "All pass, first order",
        "gain" => "Gain",
        "delay" => "Delay",
        "mix" => "Mix",
        "graphic" => "Graphic EQ",
        other => other,
    };
    if band.channels.is_empty() {
        name.to_owned()
    } else {
        let chans: Vec<String> = band.channels.iter().map(|c| channel(*c)).collect();
        format!("{name} · {}", chans.join(" "))
    }
}

fn channel(c: u16) -> String {
    match c {
        0 => "L".into(),
        1 => "R".into(),
        n => format!("Ch {}", n + 1),
    }
}

/// A filter's figures as one line, for the filters that are read rather
/// than typed into.
pub fn values(band: &DspBand) -> String {
    let gain = format!("{:+.1} dB", band.gain_db);
    match band.kind.as_str() {
        "gain" => return gain,
        "delay" => {
            let mut parts = Vec::new();
            if band.delay_ms != 0.0 {
                parts.push(format!("{:.2} ms", band.delay_ms));
            }
            if band.delay_samples != 0.0 {
                parts.push(format!("{:.2} samples", band.delay_samples));
            }
            return parts.join(" + ");
        }
        "mix" => {
            return band
                .mix
                .iter()
                .enumerate()
                .filter_map(|(o, out)| {
                    let s = &out.sources;
                    if s.len() == 1 && s[0].channel as usize == o && s[0].gain == 1.0 {
                        return None;
                    }
                    let terms: Vec<String> = s
                        .iter()
                        .map(|src| {
                            if src.gain == 1.0 {
                                channel(src.channel)
                            } else {
                                format!("{}×{}", src.gain, channel(src.channel))
                            }
                        })
                        .collect();
                    Some(format!(
                        "{} = {}",
                        channel(o as u16),
                        if terms.is_empty() {
                            "0".into()
                        } else {
                            terms.join(" + ")
                        }
                    ))
                })
                .collect::<Vec<_>>()
                .join("   ");
        }
        "graphic" => {
            let lo = band
                .curve
                .iter()
                .map(|p| p.db)
                .fold(f64::INFINITY, f64::min);
            let hi = band
                .curve
                .iter()
                .map(|p| p.db)
                .fold(f64::NEG_INFINITY, f64::max);
            let (lo, hi) = if band.curve.is_empty() {
                (0.0, 0.0)
            } else {
                (lo, hi)
            };
            return format!("{} points  {lo:+.1} to {hi:+.1} dB", band.curve.len());
        }
        _ => {}
    }
    let freq = if band.freq >= 1000.0 {
        format!("{:.1} kHz", band.freq / 1000.0)
    } else {
        format!("{:.0} Hz", band.freq)
    };
    let q = format!("Q {:.2}", band.q);
    if band.kind.ends_with("_first_order") {
        return if band.kind.contains("shelf") {
            format!("{freq}  {gain}")
        } else {
            freq
        };
    }
    if ["low_pass", "high_pass", "notch", "band_pass", "all_pass"].contains(&band.kind.as_str()) {
        format!("{freq}  {q}")
    } else {
        format!("{freq}  {gain}  {q}")
    }
}

pub fn footer(read_only: bool) -> SharedString {
    if read_only {
        "A correction plays as made. To change the sound, add a tuning on top; to edit these, make it a tuning under What it's for.".into()
    } else {
        "Edits play at once. Frequency, gain and Q are held to 10 Hz–22 kHz, ±30 dB and 0.1–20."
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_show_to_their_places_without_trailing_zeros() {
        assert_eq!(figure(1000.0, 0), "1000");
        assert_eq!(figure(3.25, 1), "3.2");
        assert_eq!(figure(3.0, 1), "3");
        assert_eq!(figure(0.712_345, 2), "0.71");
        assert_eq!(figure(-0.04, 1), "0");
    }

    /// A field left as shown saves nothing, so a band of 0.7071 keeps its
    /// Q when only its gain is typed.
    #[test]
    fn an_untouched_figure_is_not_saved() {
        assert_eq!(typed("0.71", "0.71"), None);
        assert_eq!(typed(" 0.71 ", "0.71"), None);
        assert_eq!(typed("0.8", "0.71"), Some(0.8));
        assert_eq!(typed("−3", "0"), Some(-3.0));
        assert_eq!(typed("abc", "0"), None);
        assert_eq!(typed("inf", "0"), None);
    }
}
