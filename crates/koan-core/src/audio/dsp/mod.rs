//! Equalisation and convolution, between the decoder and the ring buffer.
//!
//! The chain runs on the decode thread, so the render callback stays what it
//! was: atomics and a ring buffer. With no profile for the output device there
//! is no chain at all — samples reach the ring untouched, and bit-perfect stays
//! something that can be checked.
//!
//! Order: preamp, resampling (only to reach an impulse response's rate), the
//! profile's filters at the output rate in the order listed (`steps`), then
//! convolution. Running the filters after resampling means their coefficients
//! belong to the session rather than the track, so a gapless change of source
//! rate keeps the filters' state.
//!
//! Stages that delay the audio — the resampler, and a linear-phase response's
//! pre-ringing — have that delay trimmed from the front of the session and
//! flushed at its end. Output frame `n` is then input frame `n` at the output
//! rate, which is what the timeline counts and the playhead reads.

pub mod apo;
pub mod autoeq;
pub mod camilla;
pub mod convolver;
pub mod import;
pub mod impulse;
pub mod profiles;
pub mod raw;
mod steps;
pub mod targets;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use realfft::RealFftPlanner;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use thiserror::Error;

use impulse::{Convolve, read_audio};
pub use impulse::{Impulse, Route};

use steps::{Planned, Steps, gain_matrix, plan};

use crate::config::{DspFilter, DspProfile};

#[derive(Debug, Error)]
pub enum DspError {
    #[error("{}: {reason}", path.display())]
    Impulse { path: PathBuf, reason: String },
    /// A target the profile was made for or moved to that cannot be read.
    #[error("no target called {0}")]
    Target(String),
    /// A layer that cannot be played: missing, a layer of itself, or one
    /// with impulse responses.
    #[error("{0}")]
    Layer(String),
}

/// How deep layers may nest: a stack of stacks of stacks, and so on.
const MAX_LAYER_DEPTH: usize = 8;
/// The most filters a stack may expand to. A layer played twice, or a
/// diamond of stacks sharing layers, copies its filters each time; past this
/// the stack is refused rather than grown without bound.
const MAX_CHAIN_FILTERS: usize = 256;
/// The most layers a stack may visit while resolving, however few filters
/// they hold: a diamond of empty stacks costs time if not memory.
const MAX_LAYER_VISITS: usize = 1024;

/// The level `filters` leave channel 0 at, in dB, at each of `freqs`, as
/// the DSP runs them at `rate`: the same plan, and the same gain the derived
/// preamp is worked out from. A graphic curve counts as its design, which
/// its minimum-phase FIR follows; delays change no level.
pub fn response(filters: &[DspFilter], freqs: &[f64], rate: u32) -> Vec<f64> {
    let plan = steps::plan(filters, rate, 2);
    freqs
        .iter()
        .map(|&hz| {
            let w = std::f64::consts::TAU * hz / rate as f64;
            let gain: f64 = steps::gain_matrix(&plan, 2, w, rate)[0].iter().sum();
            20.0 * gain.max(1e-6).log10()
        })
        .collect()
}

/// The filters `profile` plays: each layer that is on, in order, as that
/// layer plays it, then its own, then the step moving it to another target.
/// `stack` holds the profiles being resolved, which a cycle would come back
/// to. A layer is EQ alone: impulse responses belong to the profile that
/// plays them, and two stacks of responses would make one profile's rates
/// another's. Refused past `MAX_LAYER_DEPTH`, `MAX_CHAIN_FILTERS` or
/// `MAX_LAYER_VISITS`, so no stack, however edited, can grow the chain
/// without bound.
pub fn chain(
    profile: &DspProfile,
    all: &[DspProfile],
    stack: &mut Vec<String>,
) -> Result<Vec<DspFilter>, DspError> {
    let mut visits = 0;
    resolve(profile, all, stack, &mut visits)
}

fn resolve(
    profile: &DspProfile,
    all: &[DspProfile],
    stack: &mut Vec<String>,
    visits: &mut usize,
) -> Result<Vec<DspFilter>, DspError> {
    if stack.contains(&profile.name) {
        return Err(DspError::Layer(format!(
            "{} is a layer of itself, through {}",
            profile.name,
            stack.join(" → ")
        )));
    }
    if stack.len() >= MAX_LAYER_DEPTH {
        return Err(DspError::Layer(format!(
            "{} nests layers more than {MAX_LAYER_DEPTH} deep",
            stack[0]
        )));
    }
    *visits += 1;
    if *visits > MAX_LAYER_VISITS {
        return Err(DspError::Layer(format!(
            "{} reaches its layers more than {MAX_LAYER_VISITS} times over",
            stack.first().unwrap_or(&profile.name)
        )));
    }
    stack.push(profile.name.clone());
    let mut out = Vec::new();
    let too_many = |stack: &[String]| {
        DspError::Layer(format!(
            "{} comes to more than {MAX_CHAIN_FILTERS} filters with its layers",
            stack[0]
        ))
    };
    // A group plays one member: the first switched on, or with none on, the
    // first of all. A stack plays each layer switched on.
    let playing: Vec<&crate::config::DspLayer> = if profile.group {
        profile
            .layers
            .iter()
            .find(|l| l.on)
            .or(profile.layers.first())
            .into_iter()
            .collect()
    } else {
        profile.layers.iter().filter(|l| l.on).collect()
    };
    for layer in playing {
        let p = all
            .iter()
            .find(|p| p.name == layer.profile)
            .ok_or_else(|| {
                DspError::Layer(format!(
                    "{} has no profile {} to layer",
                    profile.name, layer.profile
                ))
            })?;
        // A stack's layers play together, and only one response can: a
        // group's member plays alone, its responses as the group's own.
        if !profile.group && !responses(p, all).is_empty() {
            return Err(DspError::Layer(format!(
                "{} has impulse responses, and only EQ can be a layer",
                p.name
            )));
        }
        out.extend(resolve(p, all, stack, visits)?);
        if out.len() > MAX_CHAIN_FILTERS {
            return Err(too_many(stack));
        }
    }
    out.extend(profile.filters.iter().cloned());
    // Another target than the one the correction was made for: their
    // difference, after the correction.
    if let Some(t) = &profile.target
        && let Some(chosen) = t.chosen.as_ref().filter(|c| **c != t.made_for)
    {
        let curve = |id: &str| targets::choice_curve(id).ok_or(DspError::Target(id.into()));
        let (from, to) = (curve(&t.made_for)?, curve(chosen)?);
        out.push(DspFilter::Graphic(targets::difference(&from, &to)));
    }
    if out.len() > MAX_CHAIN_FILTERS && stack.len() > 1 {
        return Err(too_many(stack));
    }
    stack.pop();
    Ok(out)
}

/// The member of the group `profile` that plays: the first switched on, or
/// with none on, the first.
pub fn playing<'a>(profile: &DspProfile, all: &'a [DspProfile]) -> Option<&'a DspProfile> {
    let layer = profile
        .layers
        .iter()
        .find(|l| l.on)
        .or(profile.layers.first())?;
    all.iter().find(|p| p.name == layer.profile)
}

/// The impulse responses `profile` plays: its own, and for a group, those
/// of the member playing.
pub fn responses(profile: &DspProfile, all: &[DspProfile]) -> Vec<PathBuf> {
    fn walk(p: &DspProfile, all: &[DspProfile], depth: usize, out: &mut Vec<PathBuf>) {
        out.extend(p.impulses.iter().cloned());
        if p.group
            && depth < MAX_LAYER_DEPTH
            && let Some(m) = playing(p, all)
        {
            walk(m, all, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(profile, all, 0, &mut out);
    out
}

/// What is being done to the audio, for the format badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DspStatus {
    pub profile: String,
    /// Filters ahead of convolution are running: bands, delays, mixes or
    /// a graphic curve.
    pub eq: bool,
    /// The rate the impulse response in use was designed at, which is the
    /// rate the output runs at. A source at another rate was resampled to
    /// reach it.
    pub convolution_rate: Option<u32>,
}

/// A profile ready to run, its impulse responses read from disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub name: String,
    preamp_db: Option<f64>,
    filters: Vec<DspFilter>,
    /// By rate. More than one at a rate where they are for different channel
    /// counts.
    impulses: BTreeMap<u32, Vec<Impulse>>,
}

impl Setup {
    /// `None` for a profile that would leave the audio as it is.
    /// `all` is every profile, which a stack's layers are found among.
    pub fn load(
        profile: &DspProfile,
        all: &[DspProfile],
        base: &Path,
    ) -> Result<Option<Self>, DspError> {
        let mut impulses: BTreeMap<u32, Vec<Impulse>> = BTreeMap::new();
        for path in &responses(profile, all) {
            for impulse in load_impulses(path, base)? {
                impulses.entry(impulse.rate).or_default().push(impulse);
            }
        }
        let filters = chain(profile, all, &mut Vec::new())?;
        if filters.is_empty() && impulses.is_empty() && profile.preamp_db.unwrap_or(0.0) == 0.0 {
            return Ok(None);
        }
        Ok(Some(Self {
            name: profile.name.clone(),
            preamp_db: profile.preamp_db,
            filters,
            impulses,
        }))
    }

    /// The preamp at `rate` for `channels`: the profile's own, or the one
    /// derived from the peak gain of its bands and response there.
    pub fn preamp_db(&self, rate: u32, channels: usize) -> f64 {
        let impulse = self
            .impulses
            .get(&rate)
            .and_then(|at_rate| at_rate.iter().find(|i| i.fits(channels)));
        self.preamp_db.unwrap_or_else(|| {
            let plan = plan(&self.filters, rate, channels);
            -20.0 * peak_gain(&plan, impulse, channels, rate).log10().max(0.0)
        })
    }

    /// What the profile does at each of `freqs`, in dB, on the first of two
    /// channels, for a source at `rate`: its filters, then its impulse
    /// response at the rate it plays at. Routes add by magnitude, as for the
    /// derived preamp; the preamp itself is not included.
    pub fn response(&self, freqs: &[f64], rate: u32) -> Vec<f64> {
        let rate = self.output_rate(rate);
        let Some(impulse) = self
            .impulses
            .get(&rate)
            .and_then(|at_rate| at_rate.iter().find(|i| i.fits(2)))
        else {
            return response(&self.filters, freqs, rate);
        };
        let plan = plan(&self.filters, rate, 2);
        let routes = impulse.routes_for(2);
        let size = impulse.taps().next_power_of_two().max(8192);
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(size);
        let magnitudes: Vec<Vec<f64>> = routes
            .iter()
            .map(|r| {
                let mut input = fft.make_input_vec();
                for (d, &s) in input.iter_mut().zip(&r.ir) {
                    *d = s as f64;
                }
                let mut spectrum = fft.make_output_vec();
                if fft.process(&mut input, &mut spectrum).is_err() {
                    return vec![0.0; spectrum.len()];
                }
                spectrum.iter().map(|b| b.norm()).collect()
            })
            .collect();
        freqs
            .iter()
            .map(|&hz| {
                let w = std::f64::consts::TAU * hz / rate as f64;
                let gains: Vec<f64> = gain_matrix(&plan, 2, w, rate)
                    .iter()
                    .map(|row| row.iter().sum())
                    .collect();
                // Between the two bins either side of `hz`.
                let k = hz * size as f64 / rate as f64;
                let gain: f64 = routes
                    .iter()
                    .zip(&magnitudes)
                    .map(|(r, m)| {
                        let i = (k.floor() as usize).min(m.len() - 1);
                        let next = m[(i + 1).min(m.len() - 1)];
                        let at = m[i] + (next - m[i]) * (k - i as f64).clamp(0.0, 1.0);
                        let fed: f64 = r
                            .inputs
                            .iter()
                            .map(|&(c, g)| g.abs() as f64 * gains.get(c).copied().unwrap_or(1.0))
                            .sum();
                        r.outputs
                            .iter()
                            .filter(|&&(o, _)| o == 0)
                            .map(|&(_, g)| at * fed * g.abs() as f64)
                            .sum::<f64>()
                    })
                    .sum();
                20.0 * gain.max(1e-6).log10()
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn with_preamp(mut self, db: f64) -> Self {
        self.preamp_db = Some(db);
        self
    }

    #[cfg(test)]
    pub(crate) fn new(filters: Vec<DspFilter>, impulses: Vec<Impulse>) -> Self {
        Self {
            name: "test".into(),
            preamp_db: None,
            filters,
            impulses: impulses.into_iter().fold(BTreeMap::new(), |mut m, i| {
                m.entry(i.rate).or_insert_with(Vec::new).push(i);
                m
            }),
        }
    }

    /// The rate a source at `source` plays at: its own, unless convolution
    /// needs another. A response exported at the source's rate is used as it
    /// is; otherwise the nearest one, and the audio is resampled to it.
    /// Resampling the response instead would low-pass hi-res material at the
    /// response's Nyquist.
    pub fn output_rate(&self, source: u32) -> u32 {
        if self.impulses.is_empty() || self.impulses.contains_key(&source) {
            return source;
        }
        *self
            .impulses
            .keys()
            .min_by_key(|&&r| (r.abs_diff(source), std::cmp::Reverse(r)))
            .expect("not empty")
    }

    /// Rates there are responses for.
    pub fn rates(&self) -> Vec<u32> {
        self.impulses.keys().copied().collect()
    }

    pub fn status(&self, source: u32) -> DspStatus {
        DspStatus {
            profile: self.name.clone(),
            eq: !self.filters.is_empty(),
            convolution_rate: (!self.impulses.is_empty()).then(|| self.output_rate(source)),
        }
    }
}

/// The responses one entry of a profile's `impulses` names: a WAV (or any
/// audio file) at its own rate, or a Convolver `.cfg`.
pub fn load_impulses(path: &Path, base: &Path) -> Result<Vec<Impulse>, DspError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let read = if convolver::is_cfg(&path) {
        convolver::read(&path)
    } else {
        read_audio(&path).map(|(rate, channels)| vec![Impulse::from_channels(rate, channels)])
    };
    read.map_err(|reason| DspError::Impulse { path, reason })
}

/// The processing for one session, built for the first track's format.
///
/// Samples become 64-bit floats on the way in and 32-bit on the way out, and
/// nothing is rounded between: resampling, filters and convolution all run in
/// 64-bit, as CamillaDSP and Roon do. `null_test_against_direct_convolution`
/// measures what is left.
pub struct Chain {
    channels: usize,
    out_rate: u32,
    gain: f64,
    resample: Option<Resample>,
    steps: Steps,
    convolve: Option<Convolve>,
    input: Vec<f64>,
    work: Vec<f64>,
    out: Vec<f32>,
}

impl Chain {
    pub fn new(setup: &Setup, source_rate: u32, channels: u16) -> Self {
        let channels = channels as usize;
        let out_rate = setup.output_rate(source_rate);
        let impulse = setup.impulses.get(&out_rate).and_then(|at_rate| {
            let fitting = at_rate.iter().find(|i| i.fits(channels));
            if fitting.is_none() {
                log::warn!(
                    "dsp: no impulse at {out_rate}Hz for {channels} channels; convolution skipped"
                );
            }
            fitting
        });

        let preamp_db = setup.preamp_db(out_rate, channels);
        log::info!(
            "dsp: '{}' at {out_rate}Hz — {} filters, preamp {preamp_db:.2} dB{}",
            setup.name,
            setup.filters.len(),
            impulse.map_or(String::new(), |i| format!(
                ", {} routes of {} taps, {} frames delay",
                i.routes.len(),
                i.taps(),
                i.delay()
            ))
        );

        Self {
            channels,
            out_rate,
            gain: 10f64.powf(preamp_db / 20.0),
            resample: Resample::new(source_rate, out_rate, channels),
            steps: Steps::new(plan(&setup.filters, out_rate, channels), out_rate, channels),
            convolve: impulse.map(|i| Convolve::new(i, channels)),
            input: Vec::new(),
            work: Vec::new(),
            out: Vec::new(),
        }
    }

    pub fn output_rate(&self) -> u32 {
        self.out_rate
    }

    /// Carry the chain into a track at `source_rate`, the output rate
    /// unchanged. Returns the outgoing resampler's tail, which belongs to the
    /// track before and is still to be written.
    pub fn set_source_rate(&mut self, source_rate: u32) -> &[f32] {
        self.work.clear();
        if self.resample.as_ref().map_or(self.out_rate, |r| r.in_rate) != source_rate {
            if let Some(mut r) = self.resample.take() {
                r.flush(&mut self.work);
            }
            self.resample = Resample::new(source_rate, self.out_rate, self.channels);
            self.post();
        }
        self.emit()
    }

    /// Process interleaved `input`. Returns what is ready to be written, and
    /// how many samples of output time `input` amounts to — the figure the
    /// timeline counts, which with a resampler is not what came out this call.
    pub fn process(&mut self, input: &[f32]) -> (&[f32], u64) {
        self.input.clear();
        self.input.extend(input.iter().map(|&s| s as f64));
        self.work.clear();
        let length = match self.resample.as_mut() {
            Some(r) => {
                r.run(&self.input, &mut self.work);
                r.counted() * self.channels as u64
            }
            None => {
                self.work.extend_from_slice(&self.input);
                input.len() as u64
            }
        };
        self.post();
        (self.emit(), length)
    }

    /// What the chain still holds at the end of a session.
    pub fn flush(&mut self) -> &[f32] {
        self.work.clear();
        if let Some(r) = self.resample.as_mut() {
            r.flush(&mut self.work);
        }
        self.post();
        if let Some(c) = self.convolve.as_mut() {
            c.flush(&mut self.work);
        }
        self.emit()
    }

    /// Gain, filters and convolution over `self.work`, at the output rate.
    fn post(&mut self) {
        if self.gain != 1.0 {
            for s in &mut self.work {
                *s *= self.gain;
            }
        }
        if !self.steps.is_empty() {
            self.steps.run(&mut self.work);
        }
        if let Some(c) = self.convolve.as_mut() {
            c.run(&mut self.work);
        }
    }

    /// `self.work` as the ring buffer takes it.
    fn emit(&mut self) -> &[f32] {
        self.out.clear();
        self.out.extend(self.work.iter().map(|&s| s as f32));
        &self.out
    }
}

/// The largest gain, linear, the filters and the response apply to any channel
/// at any frequency. A preamp of its inverse keeps a full-scale sine at that
/// frequency at full scale, as AutoEQ's `Preamp` line does. Whatever sums into
/// one channel — a mix, routes into one output — is added by magnitude, which
/// bounds what it can reach.
fn peak_gain(plan: &[Planned], impulse: Option<&Impulse>, channels: usize, rate: u32) -> f64 {
    // Each channel's gain at `w` with every input at full scale, in phase.
    let channel_gains = |w: f64| -> Vec<f64> {
        gain_matrix(plan, channels.max(1), w, rate)
            .iter()
            .map(|row| row.iter().sum())
            .collect()
    };
    let Some(impulse) = impulse else {
        let (lo, hi) = (10f64.ln(), (rate as f64 * 0.499).ln());
        return (0..=4096)
            .flat_map(|i| {
                let f = (lo + (hi - lo) * i as f64 / 4096.0).exp();
                let w = std::f64::consts::TAU * f / rate as f64;
                channel_gains(w)
            })
            .fold(0.0, f64::max);
    };
    let routes = impulse.routes_for(channels);
    let size = impulse.taps().next_power_of_two().max(8192);
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(size);
    let mut spectrum = fft.make_output_vec();
    let mut per_output = vec![vec![0.0f64; spectrum.len()]; channels];
    let gains: Vec<Vec<f64>> = if plan.is_empty() {
        Vec::new()
    } else {
        (0..spectrum.len())
            .map(|k| channel_gains(std::f64::consts::TAU * k as f64 / size as f64))
            .collect()
    };
    for r in &routes {
        let mut input = fft.make_input_vec();
        for (d, &s) in input.iter_mut().zip(&r.ir) {
            *d = s as f64;
        }
        if fft.process(&mut input, &mut spectrum).is_err() {
            continue;
        }
        for (k, bin) in spectrum.iter().enumerate() {
            let fed: f64 = r
                .inputs
                .iter()
                .map(|&(c, g)| {
                    g.abs() as f64 * gains.get(k).and_then(|g| g.get(c)).map_or(1.0, |&g| g)
                })
                .sum();
            for &(o, g) in &r.outputs {
                if let Some(out) = per_output.get_mut(o) {
                    out[k] += bin.norm() * fed * g.abs() as f64;
                }
            }
        }
    }
    per_output.iter().flatten().copied().fold(0.0, f64::max)
}

/// Sample-rate conversion to an impulse response's rate.
struct Resample {
    inner: Fft<f64>,
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    /// Interleaved input not yet a whole chunk.
    pending: Vec<f64>,
    scratch: Vec<f64>,
    /// Output frames of the resampler's own delay still to drop.
    skip: usize,
    fed: u64,
    emitted: u64,
    counted: u64,
}

impl Resample {
    fn new(in_rate: u32, out_rate: u32, channels: usize) -> Option<Self> {
        if in_rate == out_rate {
            return None;
        }
        let inner = Fft::<f64>::new(
            in_rate as usize,
            out_rate as usize,
            1024,
            channels,
            FixedSync::Input,
        )
        .inspect_err(|e| log::error!("dsp: no resampler {in_rate}→{out_rate}Hz: {e}"))
        .ok()?;
        Some(Self {
            skip: inner.output_delay(),
            scratch: vec![0.0; inner.output_frames_max() * channels],
            inner,
            in_rate,
            out_rate,
            channels,
            pending: Vec::new(),
            fed: 0,
            emitted: 0,
            counted: 0,
        })
    }

    /// Output frames `fed` input frames amount to.
    fn target(&self) -> u64 {
        self.fed * self.out_rate as u64 / self.in_rate as u64
    }

    /// Output frames of time fed since the last call.
    fn counted(&mut self) -> u64 {
        let target = self.target();
        let new = target - self.counted;
        self.counted = target;
        new
    }

    fn run(&mut self, input: &[f64], dst: &mut Vec<f64>) {
        self.fed += (input.len() / self.channels) as u64;
        self.pending.extend_from_slice(input);
        loop {
            let need = self.inner.input_frames_next();
            if self.pending.len() / self.channels < need {
                break;
            }
            self.chunk(need, None, dst);
            self.pending.drain(..need * self.channels);
        }
    }

    /// Everything still held, padded with silence until the output reaches
    /// the length of what was fed.
    fn flush(&mut self, dst: &mut Vec<f64>) {
        let start = dst.len();
        let target = self.target();
        let mut partial = self.pending.len() / self.channels;
        while self.emitted < target {
            let need = self.inner.input_frames_next();
            self.pending.resize(need * self.channels, 0.0);
            if self.chunk(need, Some(partial), dst) == 0 {
                break;
            }
            partial = 0;
        }
        self.pending.clear();
        let over = (self.emitted - target) as usize * self.channels;
        dst.truncate((dst.len() - over).max(start));
        self.emitted = target;
    }

    /// Resample one chunk into `dst`. Returns the frames the resampler made,
    /// its delay included.
    fn chunk(&mut self, need: usize, partial: Option<usize>, dst: &mut Vec<f64>) -> usize {
        let ch = self.channels;
        let frames_out = self.scratch.len() / ch;
        let (Ok(input), Ok(mut output)) = (
            InterleavedSlice::new(&self.pending[..need * ch], ch, need),
            InterleavedSlice::new_mut(&mut self.scratch, ch, frames_out),
        ) else {
            return 0;
        };
        let indexing = partial.map(|p| Indexing::new().partial_len(p));
        match self
            .inner
            .process_into_buffer(&input, &mut output, indexing.as_ref())
        {
            Ok((_, produced)) => {
                let drop = self.skip.min(produced);
                self.skip -= drop;
                dst.extend_from_slice(&self.scratch[drop * ch..produced * ch]);
                self.emitted += (produced - drop) as u64;
                produced
            }
            Err(e) => {
                log::error!("dsp: resampling failed: {e}");
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EqFilter, EqFilterKind};

    fn band(kind: EqFilterKind, freq: f64, gain_db: f64, q: f64) -> EqFilter {
        EqFilter {
            kind,
            freq,
            gain_db,
            q,
            channels: vec![],
        }
    }

    fn sine(rate: u32, freq: f64, frames: usize, channels: usize, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = amp * (std::f64::consts::TAU * freq * i as f64 / rate as f64).sin() as f32;
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    fn rms(s: &[f32]) -> f64 {
        (s.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    }

    /// Run `input` through in packets, as the decoder hands it over.
    fn run_all(chain: &mut Chain, input: &[f32], packet: usize) -> (Vec<f32>, u64) {
        let mut out = Vec::new();
        let mut counted = 0;
        for p in input.chunks(packet) {
            let (o, n) = chain.process(p);
            out.extend_from_slice(o);
            counted += n;
        }
        out.extend_from_slice(chain.flush());
        (out, counted)
    }

    /// Deterministic noise in [-1, 1).
    fn noise(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((*seed >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }

    /// What is left after subtracting a textbook convolution — every output
    /// sample summed tap by tap in 64-bit — from the chain's. The partitioned
    /// FFT is the same sum reordered, so the residual is rounding: what the
    /// 64-bit chain adds, and the 32-bit floats it hands the ring buffer.
    #[test]
    fn null_test_against_direct_convolution() {
        let (rate, taps, frames, peak) = (48000, 4096, 8192, 200);
        let mut seed = 1;
        let mut ir: Vec<f32> = (0..taps)
            .map(|i| (noise(&mut seed) * 0.2 * (-(i as f64) / 600.0).exp()) as f32)
            .collect();
        ir[peak] = 1.0;
        let input: Vec<f32> = (0..frames * 2)
            .map(|_| (noise(&mut seed) * 0.25) as f32)
            .collect();
        let mut setup = Setup::new(vec![], vec![Impulse::from_channels(rate, vec![ir.clone()])]);
        setup.preamp_db = Some(0.0);
        let mut chain = Chain::new(&setup, rate, 2);
        let (out, _) = run_all(&mut chain, &input, 1152);
        assert_eq!(out.len(), input.len());

        let (mut err, mut sig) = (0.0f64, 0.0f64);
        for c in 0..2 {
            for n in 0..frames {
                // Output frame n is input frame n: the peak's delay is trimmed.
                let m = n + peak;
                let y: f64 = (0..taps)
                    .filter(|&k| k <= m && m - k < frames)
                    .map(|k| ir[k] as f64 * input[(m - k) * 2 + c] as f64)
                    .sum();
                err += (out[n * 2 + c] as f64 - y).powi(2);
                sig += y.powi(2);
            }
        }
        let db = 10.0 * (err / sig).log10();
        eprintln!("residual against direct convolution: {db:.1} dB");
        // 32-bit output rounding alone sits near -150 dB.
        assert!(db < -140.0, "residual {db:.1} dB");
    }

    /// The heaviest response there is: 262,145 taps a channel at 192 kHz, as
    /// the Harman 780 Roon pack ships. Run it in release:
    /// `cargo test --release -p koan-core --lib bench_long_response -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_long_response() {
        let (rate, taps, secs) = (192000, 262_145, 10);
        let mut seed = 7;
        let ir: Vec<Vec<f32>> = (0..2)
            .map(|_| {
                (0..taps)
                    .map(|i| (noise(&mut seed) * (-(i as f64) / 20000.0).exp()) as f32)
                    .collect()
            })
            .collect();
        let input: Vec<f32> = (0..rate as usize * secs * 2)
            .map(|_| (noise(&mut seed) * 0.25) as f32)
            .collect();
        let setup = Setup::new(vec![], vec![Impulse::from_channels(rate, ir)]);
        let started = std::time::Instant::now();
        let mut chain = Chain::new(&setup, rate, 2);
        let built = started.elapsed();
        let started = std::time::Instant::now();
        for p in input.chunks(4096 * 2) {
            chain.process(p);
        }
        let took = started.elapsed();
        eprintln!(
            "{taps} taps at {rate} Hz, stereo: built in {built:?}; {secs} s of audio in {took:?}, {:.1}x real time, {:.1}% of one core",
            secs as f64 / took.as_secs_f64(),
            100.0 * took.as_secs_f64() / secs as f64
        );
    }

    #[test]
    fn a_profile_that_changes_nothing_is_no_chain() {
        let profile = DspProfile {
            name: "flat".into(),
            ..Default::default()
        };
        assert!(
            Setup::load(&profile, &[], Path::new("/"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_peaking_band_boosts_its_frequency_and_the_preamp_pays_for_it() {
        let setup = Setup::new(
            vec![band(EqFilterKind::Peaking, 1000.0, 6.0, 1.0).into()],
            vec![],
        );
        let mut chain = Chain::new(&setup, 48000, 2);
        let input = sine(48000, 1000.0, 48000, 2, 0.5);
        let (out, counted) = run_all(&mut chain, &input, 4096);
        assert_eq!(out.len(), input.len());
        assert_eq!(counted, input.len() as u64);
        // +6 dB at the centre, -6 dB of derived preamp: unity.
        let ratio = rms(&out[24000..]) / rms(&input[24000..]);
        assert!((ratio - 1.0).abs() < 0.02, "ratio {ratio}");

        // Far from the band only the preamp is left.
        let mut chain = Chain::new(&setup, 48000, 2);
        let input = sine(48000, 100.0, 48000, 2, 0.5);
        let (out, _) = run_all(&mut chain, &input, 4096);
        let db = 20.0 * (rms(&out[24000..]) / rms(&input[24000..])).log10();
        assert!((db + 6.0).abs() < 0.2, "{db} dB");
    }

    #[test]
    fn a_band_past_nyquist_is_skipped_not_fatal() {
        let setup = Setup::new(
            vec![band(EqFilterKind::Peaking, 30000.0, 6.0, 1.0).into()],
            vec![],
        );
        let mut chain = Chain::new(&setup, 44100, 2);
        let input = sine(44100, 1000.0, 4410, 2, 0.5);
        let (out, _) = run_all(&mut chain, &input, 1024);
        assert_eq!(out, input);
    }

    fn delayed_impulse(rate: u32, delay: usize) -> Impulse {
        let mut ir = vec![0.0; delay * 2 + 1];
        ir[delay] = 1.0;
        Impulse::from_channels(rate, vec![ir])
    }

    /// The drawn response carries the impulse response, after the filters.
    #[test]
    fn a_response_includes_the_impulse() {
        let half = Impulse::from_channels(48000, vec![vec![0.5]]);
        let peak = DspFilter::Band(crate::config::EqFilter {
            kind: crate::config::EqFilterKind::Peaking,
            freq: 1000.0,
            gain_db: 3.0,
            q: 1.0,
            channels: vec![],
        });
        let setup = Setup::new(vec![peak], vec![half]);
        let db = setup.response(&[20.0, 1000.0], 48000);
        assert!((db[0] + 6.02).abs() < 0.05, "{db:?}");
        assert!((db[1] + 3.02).abs() < 0.1, "{db:?}");
    }

    #[test]
    fn convolution_delay_is_trimmed_and_the_tail_flushed() {
        let setup = Setup::new(vec![], vec![delayed_impulse(48000, 300)]);
        let mut chain = Chain::new(&setup, 48000, 2);
        let input = sine(48000, 440.0, 10000, 2, 0.5);
        let (out, counted) = run_all(&mut chain, &input, 1152);
        assert_eq!(out.len(), input.len());
        assert_eq!(counted, input.len() as u64);
        for (a, b) in out.iter().zip(&input) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn a_source_without_its_own_response_is_resampled_to_the_nearest() {
        let setup = Setup::new(
            vec![],
            vec![delayed_impulse(44100, 0), delayed_impulse(48000, 0)],
        );
        assert_eq!(setup.output_rate(44100), 44100);
        assert_eq!(setup.output_rate(96000), 48000);
        assert_eq!(setup.output_rate(88200), 48000);
        assert_eq!(setup.output_rate(22050), 44100);
        assert_eq!(
            setup.status(96000),
            DspStatus {
                profile: "test".into(),
                eq: false,
                convolution_rate: Some(48000)
            }
        );

        let mut chain = Chain::new(&setup, 96000, 2);
        assert_eq!(chain.output_rate(), 48000);
        let input = sine(96000, 1000.0, 96000, 2, 0.5);
        let (out, counted) = run_all(&mut chain, &input, 4096);
        assert_eq!(out.len(), 48000 * 2);
        assert_eq!(counted, 48000 * 2);
        let ratio = rms(&out[4800..43200]) / rms(&input[9600..86400]);
        assert!((ratio - 1.0).abs() < 0.02, "ratio {ratio}");
        // Delay trimmed: the output starts in phase with the input.
        let expect = sine(48000, 1000.0, 2000, 2, 0.5);
        let err = out[2000..4000]
            .iter()
            .zip(&expect[2000..4000])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 0.02, "max error {err}");
    }

    #[test]
    fn a_gapless_rate_change_keeps_the_output_rate_and_its_length() {
        let setup = Setup::new(vec![], vec![delayed_impulse(48000, 64)]);
        let mut chain = Chain::new(&setup, 44100, 2);
        let first = sine(44100, 500.0, 44100, 2, 0.5);
        let mut out = Vec::new();
        let mut counted = 0;
        for p in first.chunks(4096) {
            let (o, n) = chain.process(p);
            out.extend_from_slice(o);
            counted += n;
        }
        out.extend_from_slice(chain.set_source_rate(48000));
        let second = sine(48000, 500.0, 48000, 2, 0.5);
        for p in second.chunks(4096) {
            let (o, n) = chain.process(p);
            out.extend_from_slice(o);
            counted += n;
        }
        out.extend_from_slice(chain.flush());
        assert_eq!(counted, 96000 * 2);
        assert_eq!(out.len(), 96000 * 2);
    }

    #[test]
    fn the_preamp_covers_the_response_gain() {
        let mut ir = vec![0.0; 64];
        ir[0] = 2.0;
        let setup = Setup::new(vec![], vec![Impulse::from_channels(48000, vec![ir])]);
        let mut chain = Chain::new(&setup, 48000, 1);
        let (out, _) = run_all(&mut chain, &[0.5; 4800], 480);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-4));
    }

    #[test]
    fn bands_for_one_channel_leave_the_other_alone() {
        let mut b = band(EqFilterKind::Gain, 1000.0, -6.0, 1.0);
        b.channels = vec![1];
        let mut setup = Setup::new(vec![b.into()], vec![]);
        setup.preamp_db = Some(0.0);
        let mut chain = Chain::new(&setup, 48000, 2);
        let (out, _) = run_all(&mut chain, &[0.5, 0.5, 0.5, 0.5], 4);
        assert_eq!(out[0], 0.5);
        assert!((out[1] - 0.25).abs() < 1e-3);
    }

    #[test]
    fn a_route_can_feed_one_channel_into_the_other() {
        // Left passes; right is left at half, plus right.
        let impulse = Impulse {
            rate: 48000,
            channels: Some(2),
            routes: vec![
                Route {
                    ir: vec![1.0],
                    inputs: vec![(0, 1.0)],
                    outputs: vec![(0, 1.0)],
                },
                Route {
                    ir: vec![1.0],
                    inputs: vec![(0, 0.5), (1, 1.0)],
                    outputs: vec![(1, 1.0)],
                },
            ],
            in_delays: vec![],
            out_delays: vec![0, 1],
        };
        let mut setup = Setup::new(vec![], vec![impulse]);
        setup.preamp_db = Some(0.0);
        let mut chain = Chain::new(&setup, 48000, 2);
        let (out, _) = run_all(&mut chain, &[0.4, 0.2, 0.0, 0.0], 4);
        // The right output is a frame late, by its delay.
        assert_eq!(out, vec![0.4, 0.0, 0.0, 0.4]);
    }

    #[test]
    fn an_impulse_is_read_from_a_wav_at_its_own_rate() {
        let dir = std::env::temp_dir().join(format!("koan-dsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ir.wav");
        let frames: [i16; 8] = [0, 0, 16384, 16384, 0, 0, 0, 0];
        let mut wav = Vec::new();
        let data_len = (frames.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&96000u32.to_le_bytes());
        wav.extend_from_slice(&(96000u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for f in frames {
            wav.extend_from_slice(&f.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();

        let profile = DspProfile {
            name: "room".into(),
            impulses: vec!["ir.wav".into()],
            ..Default::default()
        };
        let setup = Setup::load(&profile, &[], &dir).unwrap().unwrap();
        let impulse = &setup.impulses[&96000][0];
        assert_eq!(impulse.channels, Some(2));
        assert_eq!(impulse.routes[0].ir, vec![0.0, 0.5, 0.0, 0.0]);
        assert_eq!(impulse.delay(), 1);

        let missing = DspProfile {
            impulses: vec!["nope.wav".into()],
            ..profile
        };
        assert!(Setup::load(&missing, &[], &dir).is_err());
    }
}
