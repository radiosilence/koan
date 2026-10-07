//! squig.link's auto-EQ: the parametric bands its graph tool fits to bring a
//! measurement to a target, reproduced step for step so that the same
//! measurement and target give the same bands, to the hertz and the tenth of
//! a decibel, as a preset exported from the site.
//!
//! Ported from the Squiglink Lab graph tool (github.com/squiglink/lab,
//! `equalizer.js` and `graphtool.js`, after CrinGraph by Marshall Lochbaum),
//! which is published under the BSD Zero Clause License. The arithmetic is
//! kept in the original's order: the fit rounds its bands to a coarse grid
//! between passes, so a different but equivalent formula can move a band.
//!
//! What the site does, in order:
//!
//! - Both curves are interpolated, linearly in hertz, onto 1/48-octave
//!   points from 20 Hz to 20 kHz.
//! - Each is levelled to 60 phon: a loudness model (ISO 226:2003) is run over
//!   the curve as the graph draws it, smoothed, and the offset found that
//!   brings its total loudness there.
//! - Up to `MAX_BANDS` peaking bands are fitted between 20 Hz and 6 kHz, Q
//!   0.1 to 10, gain ±40 dB: the widest misses of more than 1 dB first,
//!   none above 7 kHz, then the misses of more than 0.5 dB left, each batch
//!   refined by a coordinate search over frequency, Q and gain, and all of
//!   them refined together at the end.
//! - The preamp is the negative of the bands' highest gain on those points.
//!
//! The solver fits peaking bands only: the site's shelves are for bands
//! edited by hand.

use crate::config::{EqFilter, EqFilterKind};

/// The rate the site works out its bands' responses at.
const RATE: f64 = 48_000.0;
/// The band limits of the site's "Auto EQ" constraints.
const FREQ_RANGE: (f64, f64) = (20.0, 6_000.0);
const Q_RANGE: (f64, f64) = (0.1, 10.0);
const GAIN_RANGE: (f64, f64) = (-40.0, 40.0);
/// The first batch leaves the treble alone.
const TREBLE_START_FROM: f64 = 7_000.0;
/// The site's band cap with none set: its `extraEQBandsMax`.
pub const MAX_BANDS: usize = 20;
/// Each refinement pass: how many steps either way of frequency, Q and gain
/// are tried, and the size of a step.
const DELTAS: [[f64; 6]; 3] = [
    [10.0, 10.0, 10.0, 5.0, 0.1, 0.5],
    [10.0, 10.0, 10.0, 2.0, 0.1, 0.2],
    [10.0, 10.0, 10.0, 1.0, 0.1, 0.1],
];
/// The loudness each curve is levelled to, in phon.
const NORM_PHON: f64 = 60.0;
/// The graph's default smoothing, which the levelling sees.
const SMOOTH: f64 = 5.0 * 0.01;

/// The bands squig.link's auto-EQ fits, and the preamp it gives them.
#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    pub filters: Vec<EqFilter>,
    pub preamp_db: f64,
}

/// One band as the solver holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    freq: f64,
    q: f64,
    gain: f64,
}

type Fr = Vec<(f64, f64)>;

/// What squig.link's auto-EQ makes of a headphone measured as `measurement`
/// for `target`, both as (Hz, dB) points on any grid and at any level. A
/// measurement the site calibrates should be calibrated already.
pub fn fit(measurement: &[(f64, f64)], target: &[(f64, f64)]) -> Fit {
    let freqs = f_values();
    let phone = levelled(&interp(&freqs, measurement));
    let target = levelled(&interp(&freqs, target));
    let bands = autoeq(&phone, &target, MAX_BANDS);
    let preamp_db = -gains(&freqs, &bands)
        .into_iter()
        .fold(f64::NEG_INFINITY, f64::max);
    Fit {
        filters: bands
            .iter()
            .map(|b| EqFilter {
                kind: EqFilterKind::Peaking,
                freq: b.freq,
                gain_db: b.gain,
                q: b.q,
                channels: Vec::new(),
            })
            .collect(),
        preamp_db,
    }
}

/// 1/48-octave points from 20 Hz, the first at or past 20 kHz the last.
fn f_values() -> Vec<f64> {
    let step = 2f64.powf(1.0 / 48.0);
    let mut f = vec![20.0];
    while *f.last().unwrap() < 20_000.0 {
        let next = f.last().unwrap() * step;
        f.push(next);
    }
    f
}

/// `fr` at each of `fv`, interpolated linearly in hertz and held past either
/// end. `fv` rises.
fn interp(fv: &[f64], fr: &[(f64, f64)]) -> Fr {
    let mut i = 0;
    fv.iter()
        .map(|&f| {
            while i + 1 < fr.len() {
                let (f0, v0) = fr[i];
                let (f1, v1) = fr[i + 1];
                if i == 0 && f < f0 {
                    return (f, v0);
                } else if f >= f0 && f < f1 {
                    return (f, v0 + (v1 - v0) * (f - f0) / (f1 - f0));
                }
                i += 1;
            }
            (f, fr.last().map_or(0.0, |p| p.1))
        })
        .collect()
}

/// `fr` moved to 60 phon, as the graph levels a curve.
fn levelled(fr: &Fr) -> Fr {
    let ys: Vec<f64> = fr.iter().map(|p| p.1).collect();
    let freqs: Vec<f64> = fr.iter().map(|p| p.0).collect();
    let offset = find_offset(&freqs, &smooth(&freqs, &ys), NORM_PHON);
    fr.iter().map(|&(f, v)| (f, v + offset)).collect()
}

// --- Smoothing ---------------------------------------------------------------

/// The graph's smoothing: a smoothing spline over log frequency, stiffer
/// towards the treble. `freqs` has at least five points.
fn smooth(freqs: &[f64], y: &[f64]) -> Vec<f64> {
    let x: Vec<f64> = freqs.iter().map(|f| f.ln()).collect();
    let n = x.len();
    let h: Vec<f64> = x.windows(2).map(|w| w[1] - w[0]).collect();
    let d = |i: usize| SMOOTH * (1.0f64 / 80.0).powf((i as f64 / n as f64).powf(2.0));

    let rh: Vec<f64> = h.iter().map(|d| 1.0 / d).collect();
    let g: [Vec<f64>; 3] = [
        rh[..rh.len() - 1].to_vec(),
        rh.windows(2).map(|w| -(w[1] + w[0])).collect(),
        rh[1..].to_vec(),
    ];
    let dv: Vec<f64> = (0..=rh.len()).map(d).collect();
    let dg: Vec<Vec<f64>> = g
        .iter()
        .enumerate()
        .map(|(j, r)| r.iter().enumerate().map(|(i, e)| e * dv[i + j]).collect())
        .collect();
    let d2: Vec<f64> = dv.iter().map(|e| e * e).collect();
    let h6: Vec<f64> = h.iter().map(|d| d / 6.0).collect();
    let mut m: [Vec<f64>; 3] = [
        h6.windows(2).map(|w| 2.0 * (w[1] + w[0])).collect(),
        h6[1..h6.len() - 1].to_vec(),
        vec![0.0; h6.len() - 3],
    ];
    for k in 0..3 {
        for i in 0..3 - k {
            let gk = &dg[k + i];
            for j in 0..dg[i].len() - k {
                m[k][j] += dg[i][j + k] * gk[j];
            }
        }
    }

    // Diagonal LDL decomposition of M. Past the end of M's shorter
    // diagonals the original reads `undefined`, which arithmetic makes NaN;
    // those entries are never read back.
    let at = |v: &Vec<f64>, i: usize| v.get(i).copied().unwrap_or(f64::NAN);
    let mut md = vec![m[0][0]];
    let mut ml: [Vec<f64>; 2] = [vec![m[1][0] / m[0][0]], vec![m[2][0] / m[0][0]]];
    for j in 1..m[0].len() {
        let back = md.len().min(2);
        let p: Vec<f64> = (0..back)
            .map(|i| md[md.len() - 1 - i] * ml[i][j - 1 - i])
            .collect();
        let a: Vec<f64> = (0..3)
            .map(|k| {
                let sum: f64 = p
                    .iter()
                    .take(2usize.saturating_sub(k))
                    .enumerate()
                    .map(|(i, pi)| pi * ml[k + i][j - 1 - i])
                    .filter(|v| !v.is_nan())
                    .sum();
                at(&m[k], j) - sum
            })
            .collect();
        md.push(a[0]);
        for (k, l) in ml.iter_mut().enumerate() {
            l.push(a[k + 1] / a[0]);
        }
    }

    let nn = g[0].len();
    let mut gy = vec![0.0; nn];
    for (j, r) in g.iter().enumerate() {
        for (i, e) in r.iter().enumerate() {
            gy[i] += e * y[i + j];
        }
    }
    for i in 0..nn {
        let yi = gy[i];
        for (k, l) in ml.iter().enumerate() {
            let j = i + k + 1;
            if j < nn {
                gy[j] -= l[i] * yi;
            }
        }
        gy[i] /= md[i];
    }
    for i in (0..nn).rev() {
        let yi = gy[i];
        for (k, l) in ml.iter().enumerate() {
            if let Some(j) = i.checked_sub(k + 1) {
                gy[j] -= l[j] * yi;
            }
        }
    }
    let mut u = y.to_vec();
    for (j, r) in g.iter().enumerate() {
        for (i, e) in r.iter().enumerate() {
            u[i + j] -= e * d2[i + j] * gy[i];
        }
    }
    u
}

// --- Levelling to a loudness ---------------------------------------------------

/// ISO 226:2003's equal-loudness parameters, by frequency.
const ISO226_F: [f64; 29] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0,
    8000.0, 10000.0, 12500.0,
];
const ISO226_A_F: [f64; 29] = [
    0.532, 0.506, 0.48, 0.455, 0.432, 0.409, 0.387, 0.367, 0.349, 0.33, 0.315, 0.301, 0.288, 0.276,
    0.267, 0.259, 0.253, 0.25, 0.246, 0.244, 0.243, 0.243, 0.243, 0.242, 0.242, 0.245, 0.254,
    0.271, 0.301,
];
const ISO226_L_U: [f64; 29] = [
    -31.6, -27.2, -23.0, -19.1, -15.9, -13.0, -10.3, -8.1, -6.2, -4.5, -3.1, -2.0, -1.1, -0.4, 0.0,
    0.3, 0.5, 0.0, -2.7, -4.1, -1.0, 1.7, 2.5, 1.2, -2.1, -7.1, -11.2, -10.7, -3.1,
];
const ISO226_T_F: [f64; 29] = [
    78.5, 68.7, 59.5, 51.1, 44.0, 37.5, 31.5, 26.5, 22.1, 17.9, 14.4, 11.4, 8.6, 6.2, 4.4, 3.0,
    2.2, 2.4, 3.5, 1.7, -1.3, -4.2, -6.0, -5.4, -1.5, 6.0, 12.6, 13.9, 12.3,
];

/// The graph's free-field correction, at 1/48 octave from 19.48 Hz, before
/// its 7 dB offset.
const FREE_FIELD: [f64; 480] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0725, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.0896, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.0967, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0886, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.0656, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.024, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.045, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.029, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1524, 0.2, 0.2, 0.2386, 0.3395, 0.4,
    0.437, 0.5, 0.5287, 0.6225, 0.7, 0.7063, 0.7962, 0.8, 0.8941, 0.9, 0.9863, 1.0, 1.0729, 1.1,
    1.1544, 1.2, 1.2504, 1.3, 1.3, 1.3, 1.3, 1.3163, 1.4, 1.4, 1.4, 1.4, 1.4017, 1.4846, 1.5, 1.5,
    1.5748, 1.6, 1.6, 1.653, 1.7, 1.7, 1.7487, 1.8, 1.8341, 1.9, 1.9, 1.9229, 2.0, 2.0, 2.0, 2.1,
    2.1, 2.1897, 2.2, 2.2, 2.2674, 2.3, 2.3, 2.3567, 2.4, 2.4, 2.4446, 2.5, 2.5262, 2.6, 2.6234,
    2.7149, 2.8, 2.8038, 2.9011, 2.9969, 3.0913, 3.1845, 3.2762, 3.3757, 3.4649, 3.5617, 3.657,
    3.751, 3.8, 3.8432, 3.9332, 4.0, 4.0, 4.0, 4.0121, 4.1, 4.1, 4.1, 4.0079, 4.0, 4.0, 4.0, 4.0,
    3.9334, 3.9, 3.9, 3.9, 3.8541, 3.8, 3.8, 3.768, 3.7, 3.6761, 3.6, 3.6, 3.5927, 3.5, 3.5, 3.5,
    3.5, 3.5, 3.5761, 3.6, 3.6, 3.6604, 3.7, 3.7514, 3.8, 3.8, 3.8349, 3.9, 3.9218, 4.0199, 4.1123,
    4.2076, 4.3016, 4.3985, 4.6816, 5.0515, 5.4222, 5.8036, 6.1097, 6.4656, 6.8461, 7.3316, 7.9083,
    8.4305, 8.9369, 9.5105, 10.0759, 10.6024, 11.0027, 11.4847, 12.0482, 12.5152, 12.8994, 13.2776,
    13.7381, 14.1303, 14.5168, 14.8858, 15.273, 15.6547, 15.9731, 16.2596, 16.542, 16.7857,
    17.0111, 17.2325, 17.3532, 17.522, 17.6, 17.6, 17.6, 17.6, 17.5044, 17.41, 17.3145, 17.2205,
    17.1255, 17.0318, 16.9373, 16.784, 16.6459, 16.4536, 16.2578, 16.1234, 15.967, 15.8736,
    15.7552, 15.566, 15.3879, 15.2881, 15.0958, 14.9064, 14.8099, 14.6287, 14.5201, 14.3477,
    14.2307, 14.0709, 13.9399, 13.7916, 13.6514, 13.5552, 13.4604, 13.367, 13.2718, 13.1766,
    13.0812, 12.9743, 12.7916, 12.6975, 12.602, 12.5078, 12.3247, 12.0547, 11.7686, 11.4154,
    11.1009, 10.9385, 10.7344, 10.3998, 10.0163, 9.6382, 9.2957, 8.9799, 8.6248, 8.3404, 8.0424,
    7.674, 7.3851, 7.0061, 6.5307, 6.1484, 5.7696, 5.4662, 5.1084, 4.7302, 4.3498, 3.971, 3.6455,
    3.4075, 3.1343, 2.7917, 2.5376, 2.3484, 2.1585, 1.9849, 1.9107, 2.0, 2.0, 2.0, 2.0894, 2.1844,
    2.2787, 2.374, 2.6057, 2.8265, 3.0161, 3.2057, 3.3954, 3.5851, 3.8122, 4.0967, 4.354, 4.5651,
    4.8509, 5.1459, 5.5259, 5.9041, 6.1881, 6.5643, 6.8561, 7.1418, 7.4251, 7.7093, 8.0593, 8.3192,
    8.4541, 8.5493, 8.6437, 8.7, 8.7336, 8.8, 8.8, 8.8, 8.8, 8.7926, 8.7, 8.7, 8.6079, 8.5133, 8.5,
    8.4237, 8.1863, 7.968, 7.7786, 7.4219, 6.948, 6.4299, 5.8212, 5.1563, 4.4634, 3.7042, 2.8897,
    1.9005, 1.2368, 0.5651, -0.2856, -0.8593, -2.9,
];

/// One frequency's loudness model.
struct Loudness {
    a: f64,
    k: f64,
    c: f64,
    free_field: f64,
}

fn loudness_model(fv: &[f64]) -> Vec<Loudness> {
    let mut i = 0;
    fv.iter()
        .map(|&f| {
            if i < ISO226_F.len() && f >= ISO226_F[i] {
                i += 1;
            }
            let i0 = i.saturating_sub(1);
            let i1 = i.min(ISO226_F.len() - 1);
            let g = |v: &[f64; 29]| {
                if i0 == i1 {
                    v[i0]
                } else {
                    let (l0, l1, lf) = (ISO226_F[i0].ln(), ISO226_F[i1].ln(), f.ln());
                    let l = (lf - l0) / (l1 - l0);
                    v[i0] + l * (v[i1] - v[i0])
                }
            };
            let a = g(&ISO226_A_F);
            let m = a * (4f64.log10() - 10.0 + g(&ISO226_L_U) / 10.0);
            let k = (0.005076 / 10f64.powf(m)) - 10f64.powf(a * g(&ISO226_T_F) / 10.0);
            let c = 10f64.powf(9.4 + 4.0 * m) / fv.len() as f64;
            let ffi = (0.5 + 48.0 * (f / 19.4806).log2()).floor();
            let ffi = ffi.clamp(0.0, 479.0) as usize;
            Loudness {
                a,
                k,
                c,
                free_field: FREE_FIELD[ffi] - 7.0,
            }
        })
        .collect()
}

/// The offset, in dB, that brings the curve `fr` on `fv` to `target` phon.
fn find_offset(fv: &[f64], fr: &[f64], target: f64) -> f64 {
    let par = loudness_model(fv);
    let l10 = 10f64.ln() / 10.0;
    let step = |o: f64| {
        let (mut v, mut d) = (0.0, 0.0);
        for (p, &y) in par.iter().zip(fr) {
            let v0 = (l10 * (y + o - p.free_field)).exp();
            let mut ds = l10 * v0;
            let v1 = p.k + v0.powf(p.a);
            ds *= p.a * v0.powf(p.a - 1.0);
            v += p.c * v1.powf(4.0);
            ds *= p.c * 4.0 * v1.powf(3.0);
            d += ds;
        }
        (v.ln() - target * l10) * (v / d)
    };
    let mut x = 0.0;
    for _ in 0..1000 {
        let dx = step(x);
        x -= dx;
        if dx.abs() <= 0.01 || !dx.is_finite() {
            break;
        }
    }
    x
}

// --- Bands -------------------------------------------------------------------

/// RBJ's peaking biquad as the site writes it: `[a0, a1, a2, b0, b1, b2]`,
/// normalised by a0.
fn peaking(b: &Band) -> [f64; 6] {
    let freq = (b.freq / RATE).clamp(1e-6, 1.0);
    let q = b.q.clamp(1e-4, 1000.0);
    let gain = b.gain.clamp(-40.0, 40.0);
    let w0 = 2.0 * std::f64::consts::PI * freq;
    let (sin, cos) = (w0.sin(), w0.cos());
    let a = 10f64.powf(gain / 40.0);
    let alpha = sin / (2.0 * q);
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos;
    let a2 = 1.0 - alpha / a;
    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos;
    let b2 = 1.0 - alpha * a;
    [1.0, a1 / a0, a2 / a0, b0 / a0, b1 / a0, b2 / a0]
}

/// The bands' summed gain, in dB, at each of `freqs`. A band with no
/// frequency, gain or Q is left out, as the site leaves it out.
fn gains(freqs: &[f64], bands: &[Band]) -> Vec<f64> {
    let coeffs: Vec<[f64; 6]> = bands
        .iter()
        .filter(|b| b.freq != 0.0 && b.gain != 0.0 && b.q != 0.0)
        .map(peaking)
        .collect();
    let mut out = vec![0.0; freqs.len()];
    for [a0, a1, a2, b0, b1, b2] in coeffs {
        for (g, &f) in out.iter_mut().zip(freqs) {
            let w = 2.0 * std::f64::consts::PI * f / RATE;
            let phi = 4.0 * (w / 2.0).sin().powf(2.0);
            *g += 10.0
                * ((b0 + b1 + b2).powf(2.0)
                    + (b0 * b2 * phi - (b1 * (b0 + b2) + 4.0 * b0 * b2)) * phi)
                    .log10()
                - 10.0
                    * ((a0 + a1 + a2).powf(2.0)
                        + (a0 * a2 * phi - (a1 * (a0 + a2) + 4.0 * a0 * a2)) * phi)
                        .log10();
        }
    }
    out
}

fn apply(fr: &Fr, bands: &[Band]) -> Fr {
    let freqs: Vec<f64> = fr.iter().map(|p| p.0).collect();
    let g = gains(&freqs, bands);
    fr.iter().zip(g).map(|(&(f, v), g)| (f, v + g)).collect()
}

/// The mean miss, misses under 0.1 dB counting as none.
fn distance(fr1: &Fr, fr2: &Fr) -> f64 {
    let sum: f64 = fr1
        .iter()
        .zip(fr2)
        .map(|(a, b)| (a.1 - b.1).abs())
        .map(|d| if d >= 0.1 { d } else { 0.0 })
        .sum();
    sum / fr1.len() as f64
}

/// A band for each span where `fr` misses `target` by `threshold` or more,
/// centred within `FREQ_RANGE`.
fn candidates(fr: &Fr, target: &Fr, threshold: f64) -> Vec<Band> {
    let mut state = 0.0;
    let mut start: Option<usize> = None;
    let mut out = Vec::new();
    for (i, &(f, v0)) in fr.iter().enumerate() {
        let delta = v0 - target[i].1;
        let next = if delta.abs() < threshold {
            0.0
        } else {
            delta / delta.abs()
        };
        if next == state {
            continue;
        }
        if let Some(s) = start {
            if state != 0.0 {
                let lo = fr[s].0;
                let center = (lo * f).sqrt();
                let gain =
                    interp(&[center], &target[s..i])[0].1 - interp(&[center], &fr[s..i])[0].1;
                let q = center / (f - lo);
                if center >= FREQ_RANGE.0 && center <= FREQ_RANGE.1 {
                    out.push(Band {
                        freq: center,
                        q,
                        gain,
                    });
                }
            }
            start = None;
        } else {
            start = Some(i);
        }
        state = next;
    }
    out
}

/// The step frequencies are rounded to and moved by, by decade.
fn freq_unit(freq: f64) -> f64 {
    if freq < 100.0 {
        1.0
    } else if freq < 1000.0 {
        10.0
    } else if freq < 10_000.0 {
        100.0
    } else {
        1000.0
    }
}

/// Bands rounded down as the site shows them, and held within its limits.
fn strip(bands: &[Band]) -> Vec<Band> {
    bands
        .iter()
        .map(|b| {
            let snapped = if !b.freq.is_finite() || b.freq <= 0.0 {
                FREQ_RANGE.0
            } else {
                (b.freq - b.freq % freq_unit(b.freq)).floor()
            };
            Band {
                freq: snapped.max(FREQ_RANGE.0).min(FREQ_RANGE.1),
                q: ((b.q * 10.0).floor() / 10.0).max(Q_RANGE.0).min(Q_RANGE.1),
                gain: ((b.gain * 10.0).floor() / 10.0)
                    .max(GAIN_RANGE.0)
                    .min(GAIN_RANGE.1),
            }
        })
        .collect()
}

/// One refinement pass over `bands`, forwards then backwards, then close
/// bands merged and bands that do not help dropped.
fn optimize(fr: &Fr, target: &Fr, bands: &[Band], iteration: usize) -> Vec<Band> {
    let mut bands = bands.to_vec();
    for backwards in [false, true] {
        bands = strip(&bands);
        let [max_df, max_dq, max_dg, step_df, step_dq, step_dg] = DELTAS[iteration];
        let order: Vec<usize> = if backwards {
            (0..bands.len()).rev().collect()
        } else {
            (0..bands.len()).collect()
        };
        for i in order {
            let f = bands[i];
            let others: Vec<Band> = bands
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, b)| *b)
                .collect();
            let fr1 = apply(fr, &others);
            let mut best = f;
            let mut best_distance = distance(&apply(&fr1, &[f]), target);
            let mut test = |df: f64, dq: f64, dg: f64| {
                let freq = f.freq + df * freq_unit(f.freq) * step_df;
                let q = f.q + dq * step_dq;
                let gain = f.gain + dg * step_dg;
                if freq < FREQ_RANGE.0
                    || freq > FREQ_RANGE.1
                    || q < Q_RANGE.0
                    || q > Q_RANGE.1
                    || gain < GAIN_RANGE.0
                    || gain > GAIN_RANGE.1
                {
                    return false;
                }
                let candidate = Band { freq, q, gain };
                let d = distance(&apply(&fr1, &[candidate]), target);
                if d < best_distance {
                    best = candidate;
                    best_distance = d;
                    return true;
                }
                false
            };
            let mut df = -max_df;
            while df < max_df {
                // The smallest Q first.
                let mut dq = max_dq - 1.0;
                while dq >= -max_dq {
                    let mut dg = 1.0;
                    while dg < max_dg && test(df, dq, dg) {
                        dg += 1.0;
                    }
                    let mut dg = -1.0;
                    while dg >= -max_dg && test(df, dq, dg) {
                        dg -= 1.0;
                    }
                    dq -= 1.0;
                }
                df += 1.0;
            }
            bands[i] = best;
        }
    }
    bands.sort_by(|a, b| a.freq.total_cmp(&b.freq));
    let mut i = 0;
    while i + 1 < bands.len() {
        let (f1, f2) = (bands[i], bands[i + 1]);
        if (f1.freq - f2.freq).abs() <= freq_unit(f1.freq) && (f1.q - f2.q).abs() <= 0.1 {
            bands[i].gain += f2.gain;
            bands.remove(i + 1);
        } else {
            i += 1;
        }
    }
    let mut best_distance = distance(&apply(fr, &bands), target);
    let mut i = 0;
    while i < bands.len() {
        if bands[i].gain.abs() <= 0.1 {
            bands.remove(i);
            continue;
        }
        let without: Vec<Band> = bands
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i)
            .map(|(_, b)| *b)
            .collect();
        let d = distance(&apply(fr, &without), target);
        if d < best_distance {
            bands = without;
            best_distance = d;
        } else {
            i += 1;
        }
    }
    bands
}

/// Each refinement pass in turn.
fn refine(fr: &Fr, target: &Fr, mut bands: Vec<Band>) -> Vec<Band> {
    for iteration in 0..DELTAS.len() {
        bands = optimize(fr, target, &bands, iteration);
    }
    bands
}

/// The widest of `bands`, at most `n`, in frequency order.
fn widest(mut bands: Vec<Band>, n: usize) -> Vec<Band> {
    bands.sort_by(|a, b| a.q.total_cmp(&b.q));
    bands.truncate(n);
    bands.sort_by(|a, b| a.freq.total_cmp(&b.freq));
    bands
}

fn autoeq(fr: &Fr, target: &Fr, max_bands: usize) -> Vec<Band> {
    let first_batch = (max_bands / 2).saturating_sub(1).max(1);
    let first: Vec<Band> = candidates(fr, target, 1.0)
        .into_iter()
        .filter(|c| c.freq <= TREBLE_START_FROM)
        .collect();
    let first = refine(fr, target, widest(first, first_batch));
    let second_fr = apply(fr, &first);
    let second = widest(
        candidates(&second_fr, target, 0.5),
        max_bands.saturating_sub(first.len()),
    );
    let second = refine(&second_fr, target, second);
    let all = [first, second].concat();
    strip(&refine(fr, target, all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::dsp::targets::{at, points};
    use crate::config::DspFilter;

    const LEFT: &str = include_str!("testdata/aful-8s-super-review-L.txt");
    const RIGHT: &str = include_str!("testdata/aful-8s-super-review-R.txt");
    const CAL: &str = include_str!("testdata/squig-ief-2023-cal.txt");

    fn amplitude_mean(a: f64, b: f64) -> f64 {
        20.0 * ((10f64.powf(a / 20.0) + 10f64.powf(b / 20.0)) / 2.0).log10()
    }

    /// The AFUL Performer 8S as squig.link (Super* Review) hands it to its
    /// auto-EQ: each channel on the graph's points, the site's IEF 2023
    /// calibration subtracted, the two averaged by amplitude.
    fn as_squig_has_it() -> Fr {
        let fv = f_values();
        let cal = interp(&fv, &points(CAL));
        let side = |text| interp(&fv, &points(text));
        let (l, r) = (side(LEFT), side(RIGHT));
        fv.iter()
            .enumerate()
            .map(|(i, &f)| (f, amplitude_mean(l[i].1 - cal[i].1, r[i].1 - cal[i].1)))
            .collect()
    }

    /// The same measurement as koan keeps one fetched from squig.link: the
    /// channels averaged on their own frequencies, then calibrated.
    fn as_koan_has_it() -> Fr {
        let (l, r, cal) = (points(LEFT), points(RIGHT), points(CAL));
        l.iter()
            .map(|&(f, db)| (f, amplitude_mean(db, at(&r, f)) - at(&cal, f)))
            .collect()
    }

    const PRESETS: [(&str, &str, &str); 2] = [
        (
            "Harman IE 2019",
            include_str!("testdata/squig-harman-ie-2019-target.txt"),
            include_str!("testdata/aful-8s-squig-harman-2019.txt"),
        ),
        (
            "Super 22",
            include_str!("testdata/squig-super-22-target.txt"),
            include_str!("testdata/aful-8s-squig-super-22.txt"),
        ),
    ];

    /// The left channel's bands of a squig.link export, and its preamp,
    /// which a per-channel export carries as a gain on the channel.
    fn exported(text: &str) -> (Vec<EqFilter>, f64) {
        let parsed = crate::audio::dsp::apo::parse(text).unwrap();
        let left: Vec<EqFilter> = parsed
            .filters
            .into_iter()
            .filter_map(|f| match f {
                DspFilter::Band(b) if b.channels.is_empty() || b.channels == [0] => Some(b),
                _ => None,
            })
            .collect();
        let preamp = left
            .iter()
            .filter(|b| b.kind == EqFilterKind::Gain)
            .map(|b| b.gain_db)
            .sum::<f64>()
            + parsed.preamp_db.unwrap_or(0.0);
        let bands = left
            .into_iter()
            .filter(|b| b.kind != EqFilterKind::Gain)
            .collect();
        (bands, preamp)
    }

    fn rms_db(ours: &[EqFilter], theirs: &[EqFilter]) -> f64 {
        let grid: Vec<f64> = crate::audio::dsp::targets::grid()
            .into_iter()
            .filter(|hz| (20.0..=10_000.0).contains(hz))
            .collect();
        let play = |f: &[EqFilter]| {
            let filters: Vec<DspFilter> = f.iter().cloned().map(DspFilter::Band).collect();
            crate::audio::dsp::response(&filters, &grid, 48_000)
        };
        let (a, b) = (play(ours), play(theirs));
        let sum: f64 = a.iter().zip(&b).map(|(x, y)| (x - y) * (x - y)).sum();
        (sum / grid.len() as f64).sqrt()
    }

    fn assert_matches(name: &str, fit: &Fit, preset: &str) {
        let (theirs, preamp) = exported(preset);
        let rms = rms_db(&fit.filters, &theirs);
        println!(
            "{name}: {} bands, preamp {:.2} dB against {preamp:.1}, RMS {rms:.4} dB 20 Hz-10 kHz",
            fit.filters.len(),
            fit.preamp_db
        );
        assert_eq!(fit.filters.len(), theirs.len(), "{name}: {:?}", fit.filters);
        for (o, t) in fit.filters.iter().zip(&theirs) {
            assert_eq!(o.kind, t.kind, "{name}");
            assert_eq!(o.freq, t.freq, "{name}: {o:?} against {t:?}");
            assert!(
                (o.gain_db - t.gain_db).abs() < 0.05,
                "{name}: {o:?} against {t:?}"
            );
            assert!((o.q - t.q).abs() < 0.0005, "{name}: {o:?} against {t:?}");
        }
        assert!(
            (fit.preamp_db - preamp).abs() < 0.05,
            "{name}: preamp {}",
            fit.preamp_db
        );
        assert!(rms <= 0.3, "{name}: {rms:.3} dB RMS");
    }

    /// Given the measurement as squig.link has it, the fit is the site's
    /// preset, band for band, to the precision the site exports.
    #[test]
    fn reproduces_squiglinks_presets() {
        let measured = as_squig_has_it();
        for (name, target, preset) in PRESETS {
            assert_matches(name, &fit(&measured, &points(target)), preset);
        }
    }

    /// Given the measurement as koan keeps one fetched from the site, the
    /// fit is still the site's preset.
    #[test]
    fn reproduces_squiglinks_presets_from_koans_copy() {
        let measured = as_koan_has_it();
        for (name, target, preset) in PRESETS {
            assert_matches(name, &fit(&measured, &points(target)), preset);
        }
    }

    /// Refitted from the copy a saved correction keeps, levelled and on
    /// AutoEQ's grid to the hundredth of a decibel, as when its target is
    /// changed, the fit is still the site's preset.
    #[test]
    fn reproduces_squiglinks_presets_from_the_kept_measurement() {
        use crate::audio::dsp::targets;
        let text: String = as_koan_has_it()
            .iter()
            .map(|(f, db)| format!("{f},{db}\n"))
            .collect();
        let read = targets::covering(&text, "A measurement").unwrap();
        let kept = targets::parse(&targets::on_grid(&read));
        for (name, target, preset) in PRESETS {
            assert_matches(name, &fit(&kept, &points(target)), preset);
        }
    }

    /// The graph's points run from 20 Hz to the first at or past 20 kHz.
    #[test]
    fn points_are_a_48th_of_an_octave_apart() {
        let fv = f_values();
        assert_eq!(fv[0], 20.0);
        assert!(*fv.last().unwrap() >= 20_000.0);
        assert!(fv[fv.len() - 2] < 20_000.0);
        assert_eq!(fv.len(), 480);
    }
}
