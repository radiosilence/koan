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

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use koan_ffi::{DspBand, DspPoint};

use super::menu::{Item, Trigger, menu};
use super::model::{Commit, Dsp};
use super::sheet::note;
use crate::components::TextField;
use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole};

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
    /// The text the field was last given, and the filter as last read,
    /// which a save names.
    shown: Rc<RefCell<String>>,
    band: Rc<RefCell<DspBand>>,
    _subscription: Subscription,
}

/// The fields of one profile's table, kept between renders and given the
/// filters' figures when they change and the field is not being typed in.
pub struct Fields {
    dsp: Entity<Dsp>,
    profile: String,
    fields: HashMap<(usize, Part), Field>,
}

impl Fields {
    pub fn new(dsp: Entity<Dsp>, profile: String) -> Self {
        Self {
            dsp,
            profile,
            fields: HashMap::new(),
        }
    }

    pub fn rename(&mut self, profile: String) {
        if profile != self.profile {
            self.profile = profile;
            self.fields.clear();
        }
    }

    /// Drops the fields of filters that are gone.
    pub fn keep(&mut self, bands: &[DspBand]) {
        self.fields
            .retain(|(i, part), _| match (bands.get(*i), part) {
                (Some(b), Part::PointHz(p) | Part::PointDb(p)) => {
                    b.kind == "graphic" && *p < b.curve.len()
                }
                (Some(b), _) => b.kind != "graphic",
                (None, _) => false,
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn field<V: 'static>(
        &mut self,
        index: usize,
        part: Part,
        value: f64,
        band: &DspBand,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let text = figure(value, part.digits());
        let key = (index, part);
        if !self.fields.contains_key(&key) {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(text.clone()));
            let (dsp, profile) = (self.dsp.clone(), self.profile.clone());
            let shown = Rc::new(RefCell::new(text.clone()));
            let held = Rc::new(RefCell::new(band.clone()));
            let sub = {
                let (shown, held) = (shown.clone(), held.clone());
                cx.subscribe(&input, move |_, input, e: &InputEvent, cx| {
                    if !matches!(e, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        return;
                    }
                    let text = input.read(cx).value().to_string();
                    let Some(v) = typed(&text, &shown.borrow()) else {
                        return;
                    };
                    *shown.borrow_mut() = text.trim().to_owned();
                    let band = held.borrow().clone();
                    commit(&dsp, &profile, index, &band, part, v, cx);
                })
            };
            self.fields.insert(
                key,
                Field {
                    input,
                    shown,
                    band: held,
                    _subscription: sub,
                },
            );
        }
        let field = self.fields.get_mut(&key).expect("made above");
        *field.band.borrow_mut() = band.clone();
        let focused =
            gpui_kit::Focusable::focus_handle(field.input.read(cx), cx).is_focused(window);
        if *field.shown.borrow() != text && !focused {
            *field.shown.borrow_mut() = text.clone();
            field
                .input
                .update(cx, |i, cx| i.set_value(text, window, cx));
        }
        div()
            .w(px(width))
            .flex_none()
            .child(TextField::new(&field.input).bare())
            .into_any_element()
    }
}

/// Saves one typed figure, the band's other figures as they were, naming the
/// band as it was read.
fn commit(
    dsp: &Entity<Dsp>,
    profile: &str,
    index: usize,
    band: &DspBand,
    part: Part,
    v: f64,
    cx: &mut App,
) {
    let step = match part {
        Part::Freq | Part::Gain | Part::Q => Commit::Band {
            name: profile.to_owned(),
            index: index as u32,
            expected: band.clone(),
            kind: band.kind.clone(),
            freq: if part == Part::Freq { v } else { band.freq },
            gain_db: if part == Part::Gain { v } else { band.gain_db },
            q: if part == Part::Q { v } else { band.q },
        },
        Part::PointHz(p) | Part::PointDb(p) => {
            let mut points = band.curve.clone();
            let Some(point) = points.get_mut(p) else {
                return;
            };
            match part {
                Part::PointHz(_) => point.hz = v,
                _ => point.db = v,
            }
            Commit::Curve {
                name: profile.to_owned(),
                index: index as u32,
                expected: band.clone(),
                points,
            }
        }
    };
    dsp.update(cx, |d, cx| {
        let edit = d.next_edit();
        d.edit(edit, step, true, Rc::new(|_, _| {}), cx);
    });
}

/// A band's type changed from its menu: the figures as they were.
fn retype(
    dsp: &Entity<Dsp>,
    profile: &str,
    index: usize,
    band: &DspBand,
    kind: &str,
    cx: &mut App,
) {
    let step = Commit::Band {
        name: profile.to_owned(),
        index: index as u32,
        expected: band.clone(),
        kind: kind.to_owned(),
        freq: band.freq,
        gain_db: band.gain_db,
        q: band.q,
    };
    dsp.update(cx, |d, cx| {
        let edit = d.next_edit();
        d.edit(edit, step, true, Rc::new(|_, _| {}), cx);
    });
}

const NUMBER_W: f32 = 22.0;
const HZ_W: f32 = 72.0;
const DB_W: f32 = 56.0;
const Q_W: f32 = 50.0;

/// The table: a header, then a row per filter on the channel shown, each
/// with its index among all the filters.
#[allow(clippy::too_many_arguments)]
pub fn table<V: 'static>(
    fields: &mut Fields,
    bands: &[DspBand],
    read_only: bool,
    channel: Option<u16>,
    open_curve: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    remove: Rc<dyn Fn(usize, &mut Window, &mut App)>,
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
    let cell = |text: &str, w: Option<f32>, right: bool| {
        div()
            .when_some(w, |d, w| d.w(px(w)).flex_none())
            .when(w.is_none(), |d| d.flex_1())
            .when(right, |d| d.flex().justify_end())
            .child(text.to_owned())
    };
    let mut rows: Vec<AnyElement> = Vec::new();
    if !bands.is_empty() {
        rows.push(
            div()
                .flex()
                .gap(px(8.0))
                .text_role(TypeRole::Fine, &look)
                .text_color(look.text_color(Tone::Muted))
                .child(cell("#", Some(NUMBER_W), false))
                .child(cell("Type", None, false))
                .child(cell("Hz", Some(HZ_W), true))
                .child(cell("dB", Some(DB_W), true))
                .child(cell("Q", Some(Q_W), true))
                .into_any_element(),
        );
    }
    for (position, (index, band)) in shown.into_iter().enumerate() {
        let kind = kinds.iter().find(|k| k.id == band.kind);
        let number = div()
            .w(px(NUMBER_W))
            .flex_none()
            .text_color(look.text_color(Tone::Muted))
            .child(format!("{}", position + 1));
        let row = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .py(px(2.0))
            .text_role(TypeRole::Meta, &look)
            .text_color(look.palette.ink.hsla())
            .child(number);
        let row = match (kind, read_only) {
            (Some(kind), false) => {
                let items = kinds
                    .iter()
                    .map(|k| {
                        let (dsp, profile, band, id) =
                            (dsp.clone(), profile.clone(), band.clone(), k.id.clone());
                        Item::choice(k.name.clone(), k.id == band.kind, move |_, cx| {
                            if id != band.kind {
                                retype(&dsp, &profile, index, &band, &id, cx);
                            }
                        })
                    })
                    .chain([Item::Rule, {
                        let remove = remove.clone();
                        Item::destructive("Remove", move |window, cx| remove(index, window, cx))
                    }])
                    .collect();
                row.child(div().flex_1().child(menu(
                    ("band-kind", index),
                    kind.short.clone(),
                    Trigger::Word,
                    items,
                    window,
                    cx,
                )))
                .child(fields.field(index, Part::Freq, band.freq, band, HZ_W, window, cx))
                .child(fields.field(index, Part::Gain, band.gain_db, band, DB_W, window, cx))
                .child(fields.field(
                    index,
                    Part::Q,
                    band.q,
                    band,
                    Q_W,
                    window,
                    cx,
                ))
            }
            _ if band.kind == "graphic" && !read_only => {
                let open = open_curve.clone();
                row.child(
                    div()
                        .id(("band-curve", index))
                        .flex_1()
                        .flex()
                        .justify_between()
                        .cursor_pointer()
                        .on_click(move |_, window, cx| open(index, window, cx))
                        .child(kind_name(band))
                        .child(
                            div()
                                .text_color(look.text_color(Tone::Muted))
                                .child(values(band)),
                        ),
                )
            }
            _ => row.child(
                div()
                    .flex_1()
                    .flex()
                    .justify_between()
                    .child(kind_name(band))
                    .child(
                        div()
                            .text_color(look.text_color(Tone::Muted))
                            .child(values(band)),
                    ),
            ),
        };
        rows.push(row.into_any_element());
    }
    div().flex().flex_col().children(rows).into_any_element()
}

/// A graphic curve's points, each frequency and gain editable.
pub fn points<V: 'static>(
    fields: &mut Fields,
    index: usize,
    band: &DspBand,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let look = Look::of(window, cx);
    let header = div()
        .flex()
        .gap(px(8.0))
        .text_role(TypeRole::Fine, &look)
        .text_color(look.text_color(Tone::Muted))
        .child(div().w(px(32.0)).child("#"))
        .child(div().flex_1())
        .child(div().w(px(80.0)).flex().justify_end().child("Hz"))
        .child(div().w(px(64.0)).flex().justify_end().child("dB"));
    let rows = band
        .curve
        .iter()
        .enumerate()
        .map(|(i, p): (usize, &DspPoint)| {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_role(TypeRole::Meta, &look)
                .text_color(look.palette.ink.hsla())
                .child(
                    div()
                        .w(px(32.0))
                        .text_color(look.text_color(Tone::Muted))
                        .child(format!("{}", i + 1)),
                )
                .child(div().flex_1())
                .child(fields.field(index, Part::PointHz(i), p.hz, band, 80.0, window, cx))
                .child(fields.field(index, Part::PointDb(i), p.db, band, 64.0, window, cx))
                .into_any_element()
        });
    div()
        .flex()
        .flex_col()
        .gap(look.space(Space::Xs))
        .child(header)
        .children(rows.collect::<Vec<_>>())
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
        assert_eq!(figure(0.707_106_78, 2), "0.71");
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
