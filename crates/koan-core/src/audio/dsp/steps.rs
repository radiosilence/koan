//! A profile's filters — bands, delays, mixes and graphic curves — run in the
//! order they are listed, at the output rate, ahead of convolution.
//!
//! Bands and delays act on each channel alone and could run in any order; a
//! mix makes channels of others, so what runs before it differs from what
//! runs after. The steps are resolved for a rate and channel count once per
//! session (`plan`), and consecutive bands share one pass over the samples.

use std::collections::VecDeque;
use std::f64::consts::{LN_10, PI, TAU};

use biquad::{Biquad, Coefficients, DirectForm2Transposed, Hertz, Type};
use fft_convolver::FFTConvolver;
use realfft::RealFftPlanner;

use crate::config::{DspFilter, EqFilter, EqFilterKind, GraphicEq, Mix};

/// A profile's filters at one rate, for one channel count.
pub(super) enum Planned {
    /// Each channel's biquads, in order.
    Bands(Vec<Vec<Coefficients<f64>>>),
    /// Each channel's delay, in whole frames.
    Delay(Vec<usize>),
    /// `rows[o]`: the `(input, gain)` pairs output channel `o` is made of.
    Mix(Vec<Vec<(usize, f64)>>),
    /// Each channel's curve, where it has one.
    Graphic(Vec<Option<GraphicEq>>),
}

fn applies(channels: &[u16], c: usize) -> bool {
    channels.is_empty() || channels.contains(&(c as u16))
}

pub(super) fn plan(filters: &[DspFilter], rate: u32, channels: usize) -> Vec<Planned> {
    let mut out = Vec::new();
    for f in filters {
        match f {
            DspFilter::Band(b) => {
                let c = coefficients(b, rate);
                let per = (0..channels)
                    .map(|ch| c.filter(|_| applies(&b.channels, ch)).into_iter().collect())
                    .collect();
                push_bands(&mut out, per);
            }
            DspFilter::Delay(d) => {
                let frames = (d.ms * rate as f64 / 1000.0 + d.samples).max(0.0);
                let (whole, allpass) = split_delay(frames, d.subsample);
                if whole > 0 {
                    out.push(Planned::Delay(
                        (0..channels)
                            .map(|ch| if applies(&d.channels, ch) { whole } else { 0 })
                            .collect(),
                    ));
                }
                if let Some(a) = allpass {
                    let per = (0..channels)
                        .map(|ch| applies(&d.channels, ch).then_some(a).into_iter().collect())
                        .collect();
                    push_bands(&mut out, per);
                }
            }
            DspFilter::Mix(m) => out.push(Planned::Mix(mix_rows(m, channels))),
            DspFilter::Graphic(g) => out.push(Planned::Graphic(
                (0..channels)
                    .map(|ch| applies(&g.channels, ch).then(|| g.clone()))
                    .collect(),
            )),
        }
    }
    out
}

fn push_bands(out: &mut Vec<Planned>, per: Vec<Vec<Coefficients<f64>>>) {
    if per.iter().all(Vec::is_empty) {
        return;
    }
    match out.last_mut() {
        Some(Planned::Bands(prev)) => {
            for (p, n) in prev.iter_mut().zip(per) {
                p.extend(n);
            }
        }
        _ => out.push(Planned::Bands(per)),
    }
}

/// A mix as rows for `channels`: inputs the stream lacks contribute nothing,
/// outputs it lacks are dropped, and channels the mix does not name pass.
fn mix_rows(m: &Mix, channels: usize) -> Vec<Vec<(usize, f64)>> {
    (0..channels)
        .map(|o| match m.outputs.get(o) {
            Some(row) => row
                .iter()
                .map(|&(i, g)| (i as usize, g))
                .filter(|&(i, _)| i < channels)
                .collect(),
            None => vec![(o, 1.0)],
        })
        .collect()
}

/// Whole frames, and the fraction left as a first-order Thiran allpass. The
/// allpass is most accurate with a delay between one half and one and a half
/// frames, so it takes one whole frame with the fraction where it can.
fn split_delay(frames: f64, subsample: bool) -> (usize, Option<Coefficients<f64>>) {
    let whole = frames.floor();
    let frac = frames - whole;
    if !subsample || frac < 1e-9 {
        return (frames.round() as usize, None);
    }
    let (whole, d) = if whole >= 1.0 {
        (whole - 1.0, 1.0 + frac)
    } else {
        (0.0, frac)
    };
    let a = (1.0 - d) / (1.0 + d);
    let allpass = Coefficients {
        b0: a,
        b1: 1.0,
        b2: 0.0,
        a1: a,
        a2: 0.0,
    };
    (whole as usize, Some(allpass))
}

pub(super) fn coefficients(f: &EqFilter, rate: u32) -> Option<Coefficients<f64>> {
    let kind = match f.kind {
        EqFilterKind::Gain => {
            return Some(Coefficients {
                a1: 0.0,
                a2: 0.0,
                b0: 10f64.powf(f.gain_db / 20.0),
                b1: 0.0,
                b2: 0.0,
            });
        }
        EqFilterKind::LowShelfFirstOrder
        | EqFilterKind::HighShelfFirstOrder
        | EqFilterKind::LowPassFirstOrder
        | EqFilterKind::HighPassFirstOrder
        | EqFilterKind::AllPassFirstOrder => return first_order(f, rate),
        EqFilterKind::Notch => Type::Notch,
        EqFilterKind::BandPass => Type::BandPass,
        EqFilterKind::AllPass => Type::AllPass,
        EqFilterKind::Peaking => Type::PeakingEQ(f.gain_db),
        EqFilterKind::LowShelf => Type::LowShelf(f.gain_db),
        EqFilterKind::HighShelf => Type::HighShelf(f.gain_db),
        EqFilterKind::LowPass => Type::LowPass,
        EqFilterKind::HighPass => Type::HighPass,
    };
    Hertz::from_hz(f.freq)
        .and_then(|f0| Coefficients::from_params(kind, Hertz::from_hz(rate as f64)?, f0, f.q))
        .inspect_err(|e| skipped(f, rate, &format!("{e:?}")))
        .ok()
}

fn skipped(f: &EqFilter, rate: u32, why: &str) {
    log::warn!(
        "dsp: {:?} at {}Hz skipped at {rate}Hz: {why}",
        f.kind,
        f.freq
    );
}

/// First-order sections by the bilinear transform, the corner prewarped. The
/// shelves are `(√A·s + A) / (√A·s + 1)` and its mirror: half their gain, in
/// dB, at the corner.
fn first_order(f: &EqFilter, rate: u32) -> Option<Coefficients<f64>> {
    if !(f.freq > 0.0 && f.freq < rate as f64 / 2.0) {
        skipped(f, rate, "not between 0 and half the sample rate");
        return None;
    }
    let k = (PI * f.freq / rate as f64).tan();
    let a = 10f64.powf(f.gain_db / 20.0);
    let r = a.sqrt();
    // (b0, b1, a0, a1) before normalising.
    let (b0, b1, a0, a1) = match f.kind {
        EqFilterKind::LowPassFirstOrder => (k, k, k + 1.0, k - 1.0),
        EqFilterKind::HighPassFirstOrder => (1.0, -1.0, k + 1.0, k - 1.0),
        EqFilterKind::AllPassFirstOrder => (k - 1.0, k + 1.0, k + 1.0, k - 1.0),
        EqFilterKind::LowShelfFirstOrder => (a * k + r, a * k - r, k + r, k - r),
        EqFilterKind::HighShelfFirstOrder => (a + r * k, r * k - a, 1.0 + r * k, r * k - 1.0),
        _ => unreachable!("only first-order kinds come here"),
    };
    Some(Coefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: 0.0,
        a1: a1 / a0,
        a2: 0.0,
    })
}

/// The magnitude of a biquad's response at `w` radians per sample.
pub(super) fn magnitude(c: &Coefficients<f64>, w: f64) -> f64 {
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let num = (c.b0 + c.b1 * c1 + c.b2 * c2).hypot(c.b1 * s1 + c.b2 * s2);
    let den = (1.0 + c.a1 * c1 + c.a2 * c2).hypot(c.a1 * s1 + c.a2 * s2);
    num / den
}

/// A bound on the gain from each input channel to each output at `w` radians
/// per sample: `m[o][i]`. Magnitudes multiply along a channel and add through
/// a mix, which is the most the steps can do whatever the phases.
pub(super) fn gain_matrix(plan: &[Planned], channels: usize, w: f64, rate: u32) -> Vec<Vec<f64>> {
    let mut m: Vec<Vec<f64>> = (0..channels)
        .map(|o| {
            (0..channels)
                .map(|i| if i == o { 1.0 } else { 0.0 })
                .collect()
        })
        .collect();
    let scale = |m: &mut Vec<Vec<f64>>, c: usize, g: f64| {
        for v in &mut m[c] {
            *v *= g;
        }
    };
    for step in plan {
        match step {
            Planned::Bands(per) => {
                for (c, bands) in per.iter().enumerate() {
                    let g: f64 = bands.iter().map(|b| magnitude(b, w)).product();
                    scale(&mut m, c, g);
                }
            }
            Planned::Delay(_) => {}
            Planned::Mix(rows) => {
                m = rows
                    .iter()
                    .map(|row| {
                        (0..channels)
                            .map(|i| row.iter().map(|&(j, g)| g.abs() * m[j][i]).sum())
                            .collect()
                    })
                    .collect();
            }
            Planned::Graphic(curves) => {
                let hz = w * rate as f64 / TAU;
                for (c, curve) in curves.iter().enumerate() {
                    if let Some(g) = curve {
                        scale(&mut m, c, 10f64.powf(curve_db(&sorted(g), hz) / 20.0));
                    }
                }
            }
        }
    }
    m
}

/// The curve's points in order of frequency, past any that are not numbers:
/// a curve from a config edited by hand, or a file, makes no response that is
/// not a number either.
fn sorted(g: &GraphicEq) -> Vec<(f64, f64)> {
    let mut points: Vec<(f64, f64)> = g
        .points
        .iter()
        .copied()
        .filter(|(f, d)| *f > 0.0 && f.is_finite() && d.is_finite())
        .collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    points
}

/// The curve's gain at `hz`: interpolated against log frequency, held past
/// either end.
fn curve_db(points: &[(f64, f64)], hz: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 0.0;
    };
    if hz <= first.0 {
        return first.1;
    }
    if hz >= last.0 {
        return last.1;
    }
    let i = points.partition_point(|p| p.0 <= hz);
    let ((f0, g0), (f1, g1)) = (points[i - 1], points[i]);
    g0 + (g1 - g0) * (hz / f0).ln() / (f1 / f0).ln()
}

/// The minimum-phase response with the curve's magnitude, by the folded real
/// cepstrum. Minimum phase is what the parametric filters a curve is sampled
/// from have: no pre-ringing, and no delay to trim. The FFT is about a second
/// long, so bins are about 1 Hz apart; the response is cut where what is left
/// of it is 120 dB down.
pub(super) fn graphic_fir(g: &GraphicEq, rate: u32) -> Vec<f64> {
    let points = sorted(g);
    let n = (rate as usize).next_power_of_two().max(16384);
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(n);
    let inverse = planner.plan_fft_inverse(n);

    let mut spectrum = forward.make_output_vec();
    for (k, bin) in spectrum.iter_mut().enumerate() {
        let hz = k as f64 * rate as f64 / n as f64;
        *bin = (curve_db(&points, hz) * LN_10 / 20.0).into();
    }
    let mut cepstrum = inverse.make_output_vec();
    if inverse.process(&mut spectrum, &mut cepstrum).is_err() {
        return vec![1.0];
    }
    for (i, c) in cepstrum.iter_mut().enumerate() {
        *c *= match i {
            0 => 1.0,
            i if i < n / 2 => 2.0,
            i if i == n / 2 => 1.0,
            _ => 0.0,
        } / n as f64;
    }
    if forward.process(&mut cepstrum, &mut spectrum).is_err() {
        return vec![1.0];
    }
    for bin in &mut spectrum {
        *bin = bin.exp();
    }
    let last = spectrum.len() - 1;
    spectrum[0].im = 0.0;
    spectrum[last].im = 0.0;
    let mut h = inverse.make_output_vec();
    if inverse.process(&mut spectrum, &mut h).is_err() {
        return vec![1.0];
    }
    h.truncate(n / 2);
    for v in &mut h {
        *v /= n as f64;
    }
    let total: f64 = h.iter().map(|v| v * v).sum();
    let mut tail = 0.0;
    let mut len = h.len();
    while len > 1 {
        tail += h[len - 1] * h[len - 1];
        if tail > total * 1e-12 {
            break;
        }
        len -= 1;
    }
    h.truncate(len);
    h
}

enum Stage {
    Bands(Vec<Vec<DirectForm2Transposed<f64>>>),
    /// Per channel; empty for one not delayed.
    Delay(Vec<VecDeque<f64>>),
    Mix {
        rows: Vec<Vec<(usize, f64)>>,
        frame: Vec<f64>,
    },
    Fir {
        convs: Vec<Option<FFTConvolver<f64>>>,
        plane: Vec<f64>,
        wet: Vec<f64>,
    },
}

/// The steps for one session, running over interleaved frames.
pub(super) struct Steps {
    stages: Vec<Stage>,
    channels: usize,
}

impl Steps {
    pub(super) fn new(plan: Vec<Planned>, rate: u32, channels: usize) -> Self {
        let stages = plan
            .into_iter()
            .map(|step| match step {
                Planned::Bands(per) => Stage::Bands(
                    per.iter()
                        .map(|b| b.iter().map(|&c| DirectForm2Transposed::new(c)).collect())
                        .collect(),
                ),
                Planned::Delay(frames) => Stage::Delay(
                    frames
                        .into_iter()
                        .map(|n| std::iter::repeat_n(0.0, n).collect())
                        .collect(),
                ),
                Planned::Mix(rows) => Stage::Mix {
                    rows,
                    frame: vec![0.0; channels],
                },
                Planned::Graphic(curves) => Stage::Fir {
                    convs: curves
                        .iter()
                        .map(|g| {
                            let ir = graphic_fir(g.as_ref()?, rate);
                            let mut conv = FFTConvolver::default();
                            conv.init(1024, &ir).ok()?;
                            Some(conv)
                        })
                        .collect(),
                    plane: Vec::new(),
                    wet: Vec::new(),
                },
            })
            .collect();
        Self { stages, channels }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    pub(super) fn run(&mut self, buf: &mut [f64]) {
        let ch = self.channels;
        for stage in &mut self.stages {
            match stage {
                Stage::Bands(per) => {
                    for frame in buf.chunks_exact_mut(ch) {
                        for (s, bands) in frame.iter_mut().zip(per.iter_mut()) {
                            for f in bands {
                                *s = f.run(*s);
                            }
                        }
                    }
                }
                Stage::Delay(lines) => {
                    for frame in buf.chunks_exact_mut(ch) {
                        for (s, line) in frame.iter_mut().zip(lines.iter_mut()) {
                            if !line.is_empty() {
                                line.push_back(*s);
                                *s = line.pop_front().expect("never empty");
                            }
                        }
                    }
                }
                Stage::Mix { rows, frame } => {
                    for f in buf.chunks_exact_mut(ch) {
                        for (o, row) in frame.iter_mut().zip(rows.iter()) {
                            *o = row.iter().map(|&(i, g)| g * f[i]).sum();
                        }
                        f.copy_from_slice(frame);
                    }
                }
                Stage::Fir { convs, plane, wet } => {
                    for (c, conv) in convs.iter_mut().enumerate() {
                        let Some(conv) = conv else { continue };
                        plane.clear();
                        plane.extend(buf.iter().skip(c).step_by(ch));
                        wet.resize(plane.len(), 0.0);
                        if conv.process(plane, wet).is_err() {
                            continue;
                        }
                        for (d, s) in buf.iter_mut().skip(c).step_by(ch).zip(wet.iter()) {
                            *d = *s;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Delay;

    fn band(kind: EqFilterKind, freq: f64, gain_db: f64) -> EqFilter {
        EqFilter {
            kind,
            freq,
            gain_db,
            q: 0.7,
            channels: vec![],
        }
    }

    fn db_at(c: &Coefficients<f64>, hz: f64, rate: f64) -> f64 {
        20.0 * magnitude(c, TAU * hz / rate).log10()
    }

    #[test]
    fn first_order_shelves_reach_their_gain_and_half_it_at_the_corner() {
        let rate = 48000.0;
        let low = coefficients(&band(EqFilterKind::LowShelfFirstOrder, 200.0, 6.0), 48000).unwrap();
        assert!((db_at(&low, 1.0, rate) - 6.0).abs() < 0.05);
        assert!((db_at(&low, 200.0, rate) - 3.0).abs() < 0.05);
        assert!(db_at(&low, 20000.0, rate).abs() < 0.1);

        let high = coefficients(
            &band(EqFilterKind::HighShelfFirstOrder, 2000.0, -4.0),
            48000,
        )
        .unwrap();
        assert!(db_at(&high, 1.0, rate).abs() < 0.01);
        assert!((db_at(&high, 2000.0, rate) + 2.0).abs() < 0.05);
        assert!((db_at(&high, 23900.0, rate) + 4.0).abs() < 0.05);
    }

    #[test]
    fn first_order_passes_fall_6_db_an_octave() {
        let rate = 48000.0;
        let lp = coefficients(&band(EqFilterKind::LowPassFirstOrder, 100.0, 0.0), 48000).unwrap();
        assert!((db_at(&lp, 100.0, rate) + 3.01).abs() < 0.05);
        let slope = db_at(&lp, 1600.0, rate) - db_at(&lp, 3200.0, rate);
        assert!((slope - 6.0).abs() < 0.2, "{slope}");
        let hp = coefficients(&band(EqFilterKind::HighPassFirstOrder, 100.0, 0.0), 48000).unwrap();
        assert!((db_at(&hp, 100.0, rate) + 3.01).abs() < 0.05);
        let ap = coefficients(&band(EqFilterKind::AllPassFirstOrder, 100.0, 0.0), 48000).unwrap();
        for hz in [10.0, 100.0, 1000.0, 20000.0] {
            assert!(db_at(&ap, hz, rate).abs() < 1e-9);
        }
    }

    fn run(filters: &[DspFilter], channels: usize, input: &[f64]) -> Vec<f64> {
        let mut steps = Steps::new(plan(filters, 48000, channels), 48000, channels);
        let mut buf = input.to_vec();
        steps.run(&mut buf);
        buf
    }

    #[test]
    fn a_delay_holds_its_channels_back() {
        let d = DspFilter::Delay(Delay {
            samples: 2.0,
            channels: vec![1],
            ..Default::default()
        });
        let out = run(&[d], 2, &[1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(out, vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_subsample_delay_moves_a_sine_by_its_fraction() {
        let (rate, hz) = (48000.0, 1000.0);
        let d = DspFilter::Delay(Delay {
            samples: 2.25,
            subsample: true,
            ..Default::default()
        });
        let input: Vec<f64> = (0..4800)
            .map(|i| (TAU * hz * i as f64 / rate).sin())
            .collect();
        let out = run(&[d], 1, &input);
        let err = (2400..4800)
            .map(|i| (out[i] - (TAU * hz * (i as f64 - 2.25) / rate).sin()).abs())
            .fold(0.0, f64::max);
        assert!(err < 1e-3, "{err}");
    }

    #[test]
    fn a_mix_reads_every_input_before_writing() {
        let swap = DspFilter::Mix(Mix {
            outputs: vec![vec![(1, 1.0)], vec![(0, 0.5), (1, 0.5)]],
        });
        assert_eq!(run(&[swap], 2, &[0.2, 0.6]), vec![0.6, 0.4]);
        // Channels past the mix pass; inputs the stream lacks add nothing.
        let partial = DspFilter::Mix(Mix {
            outputs: vec![vec![(0, 1.0), (5, 1.0)]],
        });
        assert_eq!(run(&[partial], 2, &[0.2, 0.6]), vec![0.2, 0.6]);
    }

    #[test]
    fn a_band_before_a_mix_is_not_one_after() {
        let gain = DspFilter::Band(EqFilter {
            channels: vec![0],
            ..band(EqFilterKind::Gain, 1000.0, -6.0206)
        });
        let mono = DspFilter::Mix(Mix {
            outputs: vec![vec![(0, 0.5), (1, 0.5)], vec![(0, 0.5), (1, 0.5)]],
        });
        let before = run(&[gain.clone(), mono.clone()], 2, &[1.0, 1.0]);
        let after = run(&[mono, gain], 2, &[1.0, 1.0]);
        assert!((before[0] - 0.75).abs() < 1e-4 && (before[1] - 0.75).abs() < 1e-4);
        assert!((after[0] - 0.5).abs() < 1e-4 && (after[1] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_graphic_curve_becomes_a_minimum_phase_response_with_its_shape() {
        let rate = 48000;
        let g = GraphicEq {
            points: vec![(20.0, 0.0), (500.0, 0.0), (1000.0, -6.0), (2000.0, 0.0)],
            channels: vec![],
        };
        let ir = graphic_fir(&g, rate);
        // Minimum phase: the energy is at the front.
        let peak = ir
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert!(peak < 8, "peak at {peak}");
        let response = |hz: f64| {
            let w = TAU * hz / rate as f64;
            let (re, im) = ir.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, &h)| {
                (re + h * (w * n as f64).cos(), im - h * (w * n as f64).sin())
            });
            20.0 * f64::hypot(re, im).log10()
        };
        assert!(response(200.0).abs() < 0.05, "{}", response(200.0));
        assert!(
            (response(1000.0) + 6.0).abs() < 0.05,
            "{}",
            response(1000.0)
        );
        assert!((response(707.1) + 3.0).abs() < 0.1, "{}", response(707.1));
        assert!(response(8000.0).abs() < 0.05);
    }

    #[test]
    fn the_gain_bound_follows_a_mix() {
        let boost = DspFilter::Band(EqFilter {
            channels: vec![1],
            ..band(EqFilterKind::Gain, 1000.0, 6.0206)
        });
        let sum = DspFilter::Mix(Mix {
            outputs: vec![vec![(0, 1.0), (1, 1.0)]],
        });
        let p = plan(&[boost, sum], 48000, 2);
        let m = gain_matrix(&p, 2, 0.1, 48000);
        assert!((m[0][0] - 1.0).abs() < 1e-4 && (m[0][1] - 2.0).abs() < 1e-4);
        assert!((m[1][1] - 2.0).abs() < 1e-4 && m[1][0] == 0.0);
    }
}
