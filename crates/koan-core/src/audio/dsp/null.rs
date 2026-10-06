//! Null tests: the chain's output against references written apart from it.
//!
//! Each case runs an impulse, a logarithmic sweep and noise through `Chain`,
//! the code that plays, and subtracts what an independent implementation of
//! the same filters makes of them. Biquads are worked from the Audio EQ
//! Cookbook and run in direct form I, where the chain uses the `biquad`
//! crate in transposed direct form II; first-order sections are the bilinear
//! transform of their analogue prototypes; delays and mixes are exact;
//! graphic curves and impulse responses are summed tap by tap, where the
//! chain convolves by partitioned FFT.
//!
//! What remains is rounding: the 64-bit arithmetic both sides do differently,
//! and the 32-bit floats the chain hands the ring buffer, about −150 dBFS for
//! the levels here. Every case must stay below `BOUND_DBFS`. A filter that
//! changed what it does — a wrong formula, a coefficient for the wrong rate, a
//! stage out of order, state lost between packets — leaves a residual tens of
//! dB above it.

use std::f64::consts::{PI, TAU};

use super::{Chain, Setup};
use crate::config::{Delay, DspFilter, EqFilter, EqFilterKind, GraphicEq, Mix};

const RATE: u32 = 48000;
const FRAMES: usize = 8192;

/// The largest difference allowed, in dB below full scale. A biquad can do no
/// better than the 32-bit output, about −150 dBFS here; the FIR and
/// convolution cases sum thousands of products a sample in another order,
/// which costs about 10 dB of that, and still clear this with room.
const BOUND_DBFS: f64 = -120.0;

/// Packets of an odd length, so a filter's state has to survive every split.
const PACKET: usize = 997;

/// Deterministic noise in [-1, 1).
fn noise(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
}

/// Interleaved test signals at half scale, each channel a different one so a
/// mix or a per-channel filter shows up: an impulse, a 20 Hz–20 kHz
/// logarithmic sweep, and noise.
fn signals(channels: usize) -> Vec<(&'static str, Vec<f32>)> {
    let (f0, f1) = (20.0f64, 20000.0f64);
    let secs = FRAMES as f64 / RATE as f64;
    let k = (f1 / f0).ln();
    let sweep = |n: usize| {
        let t = n as f64 / RATE as f64;
        0.5 * (TAU * f0 * secs / k * ((t / secs * k).exp() - 1.0)).sin()
    };
    let mut seed = 0x6b6f616e;
    let noise: Vec<f64> = (0..FRAMES * channels)
        .map(|_| 0.5 * noise(&mut seed))
        .collect();
    let impulse = (0..FRAMES * channels)
        .map(|i| if i / channels == 0 { 0.5 } else { 0.0 })
        .collect();
    let swept = (0..FRAMES * channels)
        .map(|i| {
            let (n, c) = (i / channels, i % channels);
            // The other channels sweep down, from where the first ends.
            (if c == 0 {
                sweep(n)
            } else {
                sweep(FRAMES - 1 - n)
            }) as f32
        })
        .collect();
    vec![
        ("impulse", impulse),
        ("sweep", swept),
        ("noise", noise.iter().map(|&v| v as f32).collect()),
    ]
}

/// The chain's output for `filters` at unity preamp, `input` handed over in
/// packets.
fn chain(filters: &[DspFilter], channels: usize, input: &[f32]) -> Vec<f64> {
    let setup = Setup::new(filters.to_vec(), vec![]).with_preamp(0.0);
    run(&setup, channels, input)
}

fn run(setup: &Setup, channels: usize, input: &[f32]) -> Vec<f64> {
    let mut c = Chain::new(setup, RATE, channels as u16);
    let mut out = Vec::with_capacity(input.len());
    for p in input.chunks(PACKET * channels) {
        out.extend(c.process(p).0.iter().map(|&s| s as f64));
    }
    out.extend(c.flush().iter().map(|&s| s as f64));
    out.truncate(input.len());
    out
}

/// The largest difference between the two, in dB below full scale.
fn residual_dbfs(got: &[f64], want: &[f64]) -> f64 {
    assert_eq!(got.len(), want.len());
    let worst = got
        .iter()
        .zip(want)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f64::max);
    20.0 * worst.max(1e-20).log10()
}

/// Run `filters` and `reference` over every signal and hold the residual to
/// `BOUND_DBFS`.
fn null(
    name: &str,
    filters: &[DspFilter],
    channels: usize,
    reference: impl Fn(&[f64]) -> Vec<f64>,
) {
    for (signal, input) in signals(channels) {
        let wide: Vec<f64> = input.iter().map(|&s| s as f64).collect();
        let db = residual_dbfs(&chain(filters, channels, &input), &reference(&wide));
        eprintln!("{name}, {signal}: {db:.1} dBFS");
        assert!(db < BOUND_DBFS, "{name}, {signal}: residual {db:.1} dBFS");
    }
}

/// A second-order section as the cookbook writes it, `a0` not divided out.
#[derive(Clone, Copy)]
struct Section {
    b: [f64; 3],
    a: [f64; 3],
}

impl Section {
    /// Robert Bristow-Johnson's Audio EQ Cookbook, with `alpha` from Q
    /// throughout, shelves included; the band-pass is the constant skirt
    /// gain one.
    fn cookbook(kind: EqFilterKind, f0: f64, gain_db: f64, q: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = TAU * f0 / RATE as f64;
        let (cos, sin) = (w0.cos(), w0.sin());
        let alpha = sin / (2.0 * q);
        let sq = 2.0 * a.sqrt() * alpha;
        let (b, a) = match kind {
            EqFilterKind::LowPass => (
                [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            EqFilterKind::HighPass => (
                [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            EqFilterKind::BandPass => (
                [sin / 2.0, 0.0, -sin / 2.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            EqFilterKind::Notch => (
                [1.0, -2.0 * cos, 1.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            EqFilterKind::AllPass => (
                [1.0 - alpha, -2.0 * cos, 1.0 + alpha],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            EqFilterKind::Peaking => (
                [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
                [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
            ),
            EqFilterKind::LowShelf => (
                [
                    a * ((a + 1.0) - (a - 1.0) * cos + sq),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - sq),
                ],
                [
                    (a + 1.0) + (a - 1.0) * cos + sq,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - sq,
                ],
            ),
            EqFilterKind::HighShelf => (
                [
                    a * ((a + 1.0) + (a - 1.0) * cos + sq),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - sq),
                ],
                [
                    (a + 1.0) - (a - 1.0) * cos + sq,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - sq,
                ],
            ),
            other => unreachable!("{other:?} is not a cookbook biquad"),
        };
        Self { b, a }
    }

    /// The bilinear transform of `(n1·s + n0) / (d1·s + d0)`, `s` normalised
    /// to the corner and the corner prewarped.
    fn bilinear(f0: f64, n: [f64; 2], d: [f64; 2]) -> Self {
        let k = (PI * f0 / RATE as f64).tan();
        let ([n0, n1], [d0, d1]) = (n, d);
        Self {
            b: [n0 * k + n1, n0 * k - n1, 0.0],
            a: [d0 * k + d1, d0 * k - d1, 0.0],
        }
    }

    /// First-order sections from their prototypes: the shelves have half
    /// their gain, in dB, at the corner.
    fn first_order(kind: EqFilterKind, f0: f64, gain_db: f64) -> Self {
        let g = 10f64.powf(gain_db / 20.0);
        let r = g.sqrt();
        let (n, d) = match kind {
            EqFilterKind::LowPassFirstOrder => ([1.0, 0.0], [1.0, 1.0]),
            EqFilterKind::HighPassFirstOrder => ([0.0, 1.0], [1.0, 1.0]),
            EqFilterKind::AllPassFirstOrder => ([1.0, -1.0], [1.0, 1.0]),
            // (√g·s + g) / (√g·s + 1)
            EqFilterKind::LowShelfFirstOrder => ([g, r], [1.0, r]),
            // (g·s + √g) / (s + √g)
            EqFilterKind::HighShelfFirstOrder => ([r, g], [r, 1.0]),
            other => unreachable!("{other:?} is not first order"),
        };
        Self::bilinear(f0, n, d)
    }

    /// Direct form I over one channel, dividing by `a0` as it goes.
    fn run(&self, x: &mut [f64]) {
        let ([b0, b1, b2], [a0, a1, a2]) = (self.b, self.a);
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for s in x {
            let y = (b0 * *s + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2) / a0;
            (x2, x1, y2, y1) = (x1, *s, y1, y);
            *s = y;
        }
    }
}

fn band(kind: EqFilterKind, freq: f64, gain_db: f64, q: f64) -> EqFilter {
    EqFilter {
        kind,
        freq,
        gain_db,
        q,
        channels: vec![],
    }
}

/// Each channel of interleaved `x`, through `f`.
fn per_channel(x: &[f64], channels: usize, mut f: impl FnMut(usize, &mut Vec<f64>)) -> Vec<f64> {
    let mut out = x.to_vec();
    for c in 0..channels {
        let mut plane: Vec<f64> = x.iter().skip(c).step_by(channels).copied().collect();
        f(c, &mut plane);
        for (d, s) in out.iter_mut().skip(c).step_by(channels).zip(plane) {
            *d = s;
        }
    }
    out
}

/// `x` convolved with `ir`, tap by tap, cut to `x`'s length.
fn convolve(x: &[f64], ir: &[f64]) -> Vec<f64> {
    (0..x.len())
        .map(|n| {
            ir.iter()
                .take(n + 1)
                .enumerate()
                .map(|(k, h)| h * x[n - k])
                .sum()
        })
        .collect()
}

#[test]
fn second_order_bands_null_against_the_cookbook() {
    use EqFilterKind::*;
    let cases = [
        ("peak +9 dB at 1 kHz", band(Peaking, 1000.0, 9.0, 1.4)),
        (
            "peak −12 dB at 60 Hz, narrow",
            band(Peaking, 60.0, -12.0, 8.0),
        ),
        ("low shelf +6 dB at 105 Hz", band(LowShelf, 105.0, 6.0, 0.7)),
        (
            "high shelf −4 dB at 8 kHz",
            band(HighShelf, 8000.0, -4.0, 0.7),
        ),
        ("low pass at 18 kHz", band(LowPass, 18000.0, 0.0, 0.707)),
        ("high pass at 25 Hz", band(HighPass, 25.0, 0.0, 0.707)),
        ("notch at 3 kHz", band(Notch, 3000.0, 0.0, 4.0)),
        ("band pass at 500 Hz", band(BandPass, 500.0, 0.0, 2.0)),
        ("all pass at 2 kHz", band(AllPass, 2000.0, 0.0, 0.9)),
    ];
    for (name, b) in cases {
        let s = Section::cookbook(b.kind, b.freq, b.gain_db, b.q);
        null(name, &[DspFilter::Band(b)], 2, |x| {
            per_channel(x, 2, |_, p| s.run(p))
        });
    }
}

#[test]
fn first_order_bands_null_against_their_prototypes() {
    use EqFilterKind::*;
    let cases = [
        (
            "first-order low shelf +5 dB at 200 Hz",
            band(LowShelfFirstOrder, 200.0, 5.0, 0.7),
        ),
        (
            "first-order high shelf −3 dB at 4 kHz",
            band(HighShelfFirstOrder, 4000.0, -3.0, 0.7),
        ),
        (
            "first-order low pass at 12 kHz",
            band(LowPassFirstOrder, 12000.0, 0.0, 0.7),
        ),
        (
            "first-order high pass at 40 Hz",
            band(HighPassFirstOrder, 40.0, 0.0, 0.7),
        ),
        (
            "first-order all pass at 700 Hz",
            band(AllPassFirstOrder, 700.0, 0.0, 0.7),
        ),
    ];
    for (name, b) in cases {
        let s = Section::first_order(b.kind, b.freq, b.gain_db);
        null(name, &[DspFilter::Band(b)], 2, |x| {
            per_channel(x, 2, |_, p| s.run(p))
        });
    }
}

/// A correction as AutoEQ writes one: ten bands in a row, each channel alike,
/// then a band on the right channel alone.
#[test]
fn a_parametric_profile_nulls_band_by_band() {
    use EqFilterKind::*;
    let mut bands = vec![
        band(LowShelf, 105.0, 5.5, 0.7),
        band(Peaking, 180.0, -2.8, 0.9),
        band(Peaking, 650.0, 1.2, 1.1),
        band(Peaking, 1400.0, 2.1, 1.4),
        band(Peaking, 2600.0, -1.6, 3.0),
        band(Peaking, 3200.0, -4.0, 2.2),
        band(Peaking, 4800.0, 2.4, 4.0),
        band(Peaking, 6100.0, 3.4, 3.0),
        band(Peaking, 9000.0, -3.1, 5.0),
        band(HighShelf, 10000.0, -2.5, 0.7),
    ];
    bands.push(EqFilter {
        channels: vec![1],
        ..band(Peaking, 2000.0, -6.0, 1.0)
    });
    let filters: Vec<DspFilter> = bands.iter().cloned().map(DspFilter::Band).collect();
    null("ten bands and one on the right", &filters, 2, |x| {
        per_channel(x, 2, |c, p| {
            for b in bands
                .iter()
                .filter(|b| b.channels.is_empty() || b.channels.contains(&(c as u16)))
            {
                Section::cookbook(b.kind, b.freq, b.gain_db, b.q).run(p);
            }
        })
    });
}

#[test]
fn delays_null_against_a_shift_and_a_thiran_allpass() {
    let whole = DspFilter::Delay(Delay {
        ms: 1.0,
        channels: vec![1],
        ..Default::default()
    });
    null("48-frame delay on the right", &[whole], 2, |x| {
        per_channel(x, 2, |c, p| {
            if c == 1 {
                p.splice(0..0, std::iter::repeat_n(0.0, 48));
                p.truncate(FRAMES);
            }
        })
    });

    // 3.4 frames: two whole, then a first-order Thiran allpass of 1.4.
    let fractional = DspFilter::Delay(Delay {
        samples: 3.4,
        subsample: true,
        ..Default::default()
    });
    let d: f64 = 1.4;
    let a = (1.0 - d) / (1.0 + d);
    let thiran = Section {
        b: [a, 1.0, 0.0],
        a: [1.0, a, 0.0],
    };
    null("3.4-frame delay", &[fractional], 2, |x| {
        per_channel(x, 2, |_, p| {
            p.splice(0..0, [0.0, 0.0]);
            p.truncate(FRAMES);
            thiran.run(p);
        })
    });
}

#[test]
fn a_mix_nulls_against_its_matrix() {
    let m = [[0.7, 0.3], [-0.25, 1.1]];
    let mix = DspFilter::Mix(Mix {
        outputs: vec![
            vec![(0, m[0][0]), (1, m[0][1])],
            vec![(0, m[1][0]), (1, m[1][1])],
        ],
    });
    null("a 2×2 mix", &[mix], 2, |x| {
        x.chunks(2)
            .flat_map(|f| {
                [
                    m[0][0] * f[0] + m[0][1] * f[1],
                    m[1][0] * f[0] + m[1][1] * f[1],
                ]
            })
            .collect()
    });
}

/// The curve's minimum-phase response, convolved tap by tap: what the
/// partitioned convolver must reproduce. That the response has the curve's
/// shape is `a_graphic_curve_becomes_a_minimum_phase_response_with_its_shape`.
#[test]
fn a_graphic_curve_nulls_against_its_response_summed_tap_by_tap() {
    let g = GraphicEq {
        points: vec![
            (20.0, 3.0),
            (80.0, 2.0),
            (300.0, -1.5),
            (1000.0, 0.0),
            (3000.0, -3.0),
            (6000.0, 1.0),
            (16000.0, -4.0),
        ],
        channels: vec![],
    };
    let ir = super::steps::graphic_fir(&g, RATE);
    null("a graphic curve", &[DspFilter::Graphic(g)], 2, |x| {
        per_channel(x, 2, |_, p| *p = convolve(p, &ir))
    });
}

/// A convolution with a response per channel, peaking at its first tap so
/// there is no delay to trim, after a band: the order the chain runs them.
#[test]
fn a_band_then_convolution_nulls_against_both_by_hand() {
    let mut seed = 99;
    let irs: Vec<Vec<f32>> = (0..2)
        .map(|c| {
            let mut ir: Vec<f32> = (0..2048)
                .map(|i| {
                    (noise(&mut seed) * 0.3 * (-(i as f64) / (300.0 + 200.0 * c as f64)).exp())
                        as f32
                })
                .collect();
            ir[0] = 1.0;
            ir
        })
        .collect();
    let peak = band(EqFilterKind::Peaking, 2500.0, -5.0, 2.0);
    let setup = Setup::new(
        vec![DspFilter::Band(peak.clone())],
        vec![super::Impulse::from_channels(RATE, irs.clone())],
    )
    .with_preamp(0.0);
    let s = Section::cookbook(peak.kind, peak.freq, peak.gain_db, peak.q);
    for (signal, input) in signals(2) {
        let wide: Vec<f64> = input.iter().map(|&v| v as f64).collect();
        let want = per_channel(&wide, 2, |c, p| {
            s.run(p);
            let ir: Vec<f64> = irs[c].iter().map(|&v| v as f64).collect();
            *p = convolve(p, &ir);
        });
        let db = residual_dbfs(&run(&setup, 2, &input), &want);
        eprintln!("band then convolution, {signal}: {db:.1} dBFS");
        assert!(
            db < BOUND_DBFS,
            "band then convolution, {signal}: residual {db:.1} dBFS"
        );
    }
}

/// The bound means something: a band a tenth of a decibel from the reference
/// leaves a residual far above it.
#[test]
fn a_band_slightly_off_does_not_null() {
    let b = band(EqFilterKind::Peaking, 1000.0, 3.0, 1.0);
    let off = Section::cookbook(b.kind, b.freq, b.gain_db + 0.1, b.q);
    let (_, input) = signals(2).remove(2);
    let wide: Vec<f64> = input.iter().map(|&v| v as f64).collect();
    let want = per_channel(&wide, 2, |_, p| off.run(p));
    let db = residual_dbfs(&chain(&[DspFilter::Band(b)], 2, &input), &want);
    assert!(db > -60.0, "0.1 dB off still nulls to {db:.1} dBFS");
}
