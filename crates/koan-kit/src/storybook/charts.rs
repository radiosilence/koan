//! The charts page: a frequency response with points to drag, a time series
//! with a filled band, a series easing between data sets while its axis
//! rescales, a live feed at 15 Hz drawn at the display's rate, both ways
//! it can meet the present, and the response again at a phone's width.

use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::*;

use crate::components::{
    Axis, Base, Button, ButtonKind, Chart, ChartDrag, ChartPoint, DataMotion, Legend, Rule,
    SectionHeader, Series, Swatch, Ticks,
};
use crate::theme::{KoanStyled, Look, Space, Tone, TypeRole};

/// How often the live feed samples.
const FEED: Duration = Duration::from_millis(66);
/// Samples across the live feed and the response.
const BINS: usize = 48;
const RESPONSE: usize = 240;

/// A bump on the response: centre frequency, gain, width in octaves.
#[derive(Clone, Copy)]
struct Bump {
    hz: f64,
    db: f64,
    octaves: f64,
}

pub struct Charts {
    bumps: [Bump; 3],
    freqs: Arc<[f64]>,
    held: Option<u64>,
    set: usize,
    wide: bool,
    tick: u64,
    live: Vec<f64>,
    /// The feed runs while the page is drawn, and stops once it is not.
    drawn: bool,
    feeding: bool,
}

impl Charts {
    pub fn new() -> Self {
        let freqs: Arc<[f64]> = (0..RESPONSE)
            .map(|i| 20.0 * 1000f64.powf(i as f64 / (RESPONSE - 1) as f64))
            .collect();
        Self {
            bumps: [
                Bump {
                    hz: 90.0,
                    db: 4.5,
                    octaves: 1.6,
                },
                Bump {
                    hz: 1200.0,
                    db: -3.0,
                    octaves: 1.0,
                },
                Bump {
                    hz: 7000.0,
                    db: 2.5,
                    octaves: 1.2,
                },
            ],
            freqs,
            held: None,
            set: 0,
            wide: false,
            tick: 0,
            live: vec![0.0; BINS],
            drawn: false,
            feeding: false,
        }
    }

    fn response(&self) -> Vec<f64> {
        self.freqs
            .iter()
            .map(|hz| {
                self.bumps
                    .iter()
                    .map(|b| {
                        let d = (hz / b.hz).log2() / b.octaves;
                        b.db * (-d * d * 2.0).exp()
                    })
                    .sum()
            })
            .collect()
    }

    fn drag(&mut self, d: &ChartDrag, cx: &mut Context<Self>) {
        let Some(id) = d.point else { return };
        if let Some(b) = self.bumps.get_mut(id as usize) {
            b.hz = d.x;
            b.db = d.y;
        }
        self.held = Some(id);
        cx.notify();
    }

    /// Samples the feed every `FEED` while the page is drawn.
    fn feed(&mut self, cx: &mut Context<Self>) {
        if self.feeding {
            return;
        }
        self.feeding = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FEED).await;
                let going = this
                    .update(cx, |c, cx| {
                        if !std::mem::take(&mut c.drawn) {
                            c.feeding = false;
                            return false;
                        }
                        c.tick += 1;
                        let t = c.tick as f64 * FEED.as_secs_f64();
                        c.live = (0..BINS)
                            .map(|i| {
                                let x = i as f64 / BINS as f64;
                                let swell = (t * 1.3 + x * 5.0).sin() * 0.5 + 0.5;
                                let beat = (t * 4.1).sin().max(0.0) * (1.0 - x);
                                let noise = ((i as f64 * 12.9898 + t * 78.233).sin() * 43_758.545)
                                    .fract()
                                    .abs();
                                0.15 + 0.5 * swell * (1.0 - x * 0.6) + 0.3 * beat + 0.08 * noise
                            })
                            .collect();
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
    }
}

impl Default for Charts {
    fn default() -> Self {
        Self::new()
    }
}

/// The data sets the easing example moves between: a hundred days of
/// something, each set larger than the last.
fn data_set(set: usize) -> (Vec<f64>, Vec<f64>) {
    let xs: Vec<f64> = (0..100).map(|i| i as f64).collect();
    let ys = xs
        .iter()
        .map(|x| match set % 3 {
            0 => 20.0 + 8.0 * (x / 9.0).sin() + x * 0.1,
            1 => 45.0 + 25.0 * (x / 17.0).cos() - x * 0.2,
            _ => 70.0 + 50.0 * (x / 6.0).sin() * (x / 40.0).cos(),
        })
        .collect();
    (xs, ys)
}

impl Render for Charts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        self.drawn = true;
        self.feed(cx);
        let accent = look.accent.hsla();
        let ink = look.color(Tone::Ink);
        let muted = look.color(Tone::Muted);

        let response = |id: &'static str, this: &Self, cx: &mut Context<Self>| {
            let entity = cx.entity();
            let step = entity.clone();
            let release = entity.clone();
            Chart::new(
                id,
                Axis::log(20.0, 20_000.0),
                Axis::linear(-12.0, 12.0).ticks(Ticks::Every(6.0)),
            )
            .height(px(220.0))
            .rules([Rule::horizontal("zero", 0.0)])
            .series([
                Series::line("response", this.freqs.clone(), this.response())
                    .width(2.0)
                    .color(accent)
                    .motion(DataMotion::Ease(Duration::from_millis(120))),
            ])
            .points(this.bumps.iter().enumerate().map(|(i, b)| {
                ChartPoint::new(i as u64, b.hz, b.db)
                    .held(this.held == Some(i as u64))
                    .ranges((20.0, 20_000.0), (-12.0, 12.0))
            }))
            .on_press(move |d, _, cx| entity.update(cx, |c, cx| c.drag(d, cx)))
            .on_step(move |d, _, cx| step.update(cx, |c, cx| c.drag(d, cx)))
            .on_release(move |_, _, cx| {
                release.update(cx, |c, cx| {
                    c.held = None;
                    cx.notify();
                })
            })
            .legend(
                Legend::new()
                    .key("Response", Swatch::line(accent))
                    .key("No change", Swatch::line(look.palette.rule.hsla()).thin()),
            )
        };

        // A time series with its spread as a band.
        let days: Arc<[f64]> = (0..120).map(|i| i as f64).collect();
        let mean: Vec<f64> = days
            .iter()
            .map(|d| 50.0 + 18.0 * (d / 14.0).sin() + d * 0.15)
            .collect();
        let spread = |sign: f64| -> Arc<[f64]> {
            mean.iter()
                .zip(days.iter())
                .map(|(m, d)| m + sign * (6.0 + 4.0 * (d / 5.0).cos().abs()))
                .collect()
        };
        let (high, low) = (spread(1.0), spread(-1.0));
        let band = Chart::new(
            "band",
            Axis::linear(0.0, 119.0).format(|d| format!("day {}", d as i64)),
            Axis::linear(0.0, 100.0),
        )
        .height(px(200.0))
        .series([
            Series::area("spread", days.clone(), high)
                .base(Base::Line(low))
                .color(accent.opacity(0.2)),
            Series::line("mean", days.clone(), mean.clone())
                .width(1.5)
                .color(accent),
            Series::line("target", days.clone(), vec![60.0; days.len()])
                .dash(&[4.0, 3.0])
                .color(muted),
        ])
        .legend(
            Legend::new()
                .key("Mean", Swatch::line(accent))
                .key("Spread", Swatch::line(accent.opacity(0.4)))
                .key("Target", Swatch::line(muted).dashed(true)),
        );

        // Easing between data sets, and the axis rescaling to fit.
        let (xs, ys) = data_set(self.set);
        let top = if self.wide {
            150.0
        } else {
            ys.iter().fold(0.0f64, |a, b| a.max(*b)).ceil() + 5.0
        };
        let easing = Chart::new("easing", Axis::linear(0.0, 99.0), Axis::linear(0.0, top))
            .height(px(200.0))
            .series([
                Series::area("fill", xs.clone(), ys.clone()).color(ink.opacity(0.08)),
                Series::line("set", xs, ys)
                    .width(2.0)
                    .color(ink)
                    .motion(DataMotion::Ease(Duration::from_millis(600))),
            ]);
        let entity = cx.entity();
        let next = entity.clone();
        let controls = div()
            .flex()
            .gap(look.space(Space::S))
            .child(
                Button::new("chart-next", ButtonKind::Standard)
                    .label("Next data set")
                    .on_click(move |_, _, cx| {
                        next.update(cx, |c, cx| {
                            c.set += 1;
                            cx.notify();
                        })
                    }),
            )
            .child(
                Button::new("chart-rescale", ButtonKind::Standard)
                    .label(if self.wide {
                        "Fit the data"
                    } else {
                        "Rescale to 150"
                    })
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |c, cx| {
                            c.wide = !c.wide;
                            cx.notify();
                        })
                    }),
            );

        // The live feed, two ways.
        let bins: Arc<[f64]> = (0..BINS).map(|i| i as f64).collect();
        let live = Chart::new(
            "live",
            Axis::linear(0.0, (BINS - 1) as f64).no_labels(),
            Axis::linear(0.0, 1.2).ticks(Ticks::Every(0.5)),
        )
        .height(px(160.0))
        .series([
            Series::area("live-fill", bins.clone(), self.live.clone())
                .color(accent.opacity(0.15))
                .motion(DataMotion::Interpolate(FEED)),
            Series::line("interpolated", bins.clone(), self.live.clone())
                .width(2.0)
                .color(accent)
                .motion(DataMotion::Interpolate(FEED)),
            Series::line("extrapolated", bins, self.live.clone())
                .width(1.2)
                .dash(&[3.0, 2.0])
                .color(ink)
                .motion(DataMotion::Extrapolate(FEED)),
        ])
        .legend(
            Legend::new()
                .key("Interpolated: a sample behind", Swatch::line(accent))
                .key("Extrapolated: current", Swatch::line(ink).dashed(true)),
        );

        let note = |text: &'static str| {
            div()
                .text_role(TypeRole::Meta, &look)
                .text_color(look.text_color(Tone::Muted))
                .child(text)
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(look.space(Space::L))
            .child(SectionHeader::new("A response, its points dragged"))
            .child(note(
                "Each point drags within the plot; the curve follows as the GPU eases it.",
            ))
            .child(response("response", self, cx))
            .child(SectionHeader::new("A time series with a band"))
            .child(band)
            .child(SectionHeader::new("Easing between data sets"))
            .child(controls)
            .child(easing)
            .child(SectionHeader::new("A live feed at 15 Hz"))
            .child(note(
                "Samples arrive every 66 ms; each is a keyframe the GPU draws at the display's rate.",
            ))
            .child(live)
            .child(SectionHeader::new("At a phone's width"))
            .child(
                div()
                    .w(px(340.0))
                    .p(look.space(Space::M))
                    .border_1()
                    .border_color(look.palette.rule.hsla())
                    .child(response("response-phone", self, cx)),
            )
    }
}
