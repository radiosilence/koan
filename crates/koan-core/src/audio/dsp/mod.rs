//! Equalisation and convolution, between the decoder and the ring buffer.
//!
//! The chain runs on the decode thread, so the render callback stays what it
//! was: atomics and a ring buffer. With no profile for the output device there
//! is no chain at all — samples reach the ring untouched, and bit-perfect stays
//! something that can be checked.
//!
//! Order: preamp, resampling (only to reach an impulse response's rate), the
//! parametric bands at the output rate, then convolution. Running the bands
//! after resampling means their coefficients belong to the session rather than
//! the track, so a gapless change of source rate keeps the filters' state.
//!
//! Stages that delay the audio — the resampler, and a linear-phase response's
//! pre-ringing — have that delay trimmed from the front of the session and
//! flushed at its end. Output frame `n` is then input frame `n` at the output
//! rate, which is what the timeline counts and the playhead reads.

pub mod autoeq;

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use biquad::{Biquad, Coefficients, DirectForm2Transposed, Hertz, Type};
use fft_convolver::FFTConvolver;
use realfft::RealFftPlanner;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use thiserror::Error;

use crate::config::{DspProfile, EqFilter, EqFilterKind};

#[derive(Debug, Error)]
pub enum DspError {
    #[error("{}: {reason}", path.display())]
    Impulse { path: PathBuf, reason: String },
}

/// What is being done to the audio, for the format badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DspStatus {
    pub profile: String,
    /// Parametric bands are running.
    pub eq: bool,
    /// The rate the impulse response in use was designed at, which is the
    /// rate the output runs at. A source at another rate was resampled to
    /// reach it.
    pub convolution_rate: Option<u32>,
}

/// An impulse response at the rate it was designed for: one channel for every
/// output channel, or one per channel.
#[derive(Debug, Clone)]
pub struct Impulse {
    pub rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl Impulse {
    /// The frame the response peaks at — its group delay, for a linear-phase
    /// filter. One figure for every channel, so they stay aligned.
    fn delay(&self) -> usize {
        let mut best = (0, 0.0f32);
        for ch in &self.channels {
            for (i, &v) in ch.iter().enumerate() {
                if v.abs() > best.1 {
                    best = (i, v.abs());
                }
            }
        }
        best.0
    }

    fn for_channel(&self, c: usize) -> &[f32] {
        &self.channels[if self.channels.len() == 1 { 0 } else { c }]
    }

    fn fits(&self, channels: usize) -> bool {
        self.channels.len() == 1 || self.channels.len() == channels
    }
}

/// A profile ready to run, its impulse responses read from disk.
#[derive(Debug, Clone)]
pub struct Setup {
    pub name: String,
    preamp_db: Option<f64>,
    filters: Vec<EqFilter>,
    impulses: BTreeMap<u32, Impulse>,
}

impl Setup {
    /// `None` for a profile that would leave the audio as it is.
    pub fn load(profile: &DspProfile, base: &Path) -> Result<Option<Self>, DspError> {
        let mut impulses = BTreeMap::new();
        for path in &profile.impulses {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                base.join(path)
            };
            let impulse = read_impulse(&path).map_err(|reason| DspError::Impulse {
                path: path.clone(),
                reason,
            })?;
            impulses.insert(impulse.rate, impulse);
        }
        if profile.filters.is_empty()
            && impulses.is_empty()
            && profile.preamp_db.unwrap_or(0.0) == 0.0
        {
            return Ok(None);
        }
        Ok(Some(Self {
            name: profile.name.clone(),
            preamp_db: profile.preamp_db,
            filters: profile.filters.clone(),
            impulses,
        }))
    }

    #[cfg(test)]
    pub(crate) fn new(filters: Vec<EqFilter>, impulses: Vec<Impulse>) -> Self {
        Self {
            name: "test".into(),
            preamp_db: None,
            filters,
            impulses: impulses.into_iter().map(|i| (i.rate, i)).collect(),
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

    pub fn status(&self, source: u32) -> DspStatus {
        DspStatus {
            profile: self.name.clone(),
            eq: !self.filters.is_empty(),
            convolution_rate: (!self.impulses.is_empty()).then(|| self.output_rate(source)),
        }
    }
}

fn read_impulse(path: &Path) -> Result<Impulse, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut reader = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let track = reader
        .default_track(TrackType::Audio)
        .ok_or("no audio track")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("no audio track")?;
    let rate = params.sample_rate.ok_or("no sample rate")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &Default::default())
        .map_err(|e| e.to_string())?;

    let mut interleaved = Vec::new();
    let mut packet_samples: Vec<f32> = Vec::new();
    let mut channels = 0;
    while let Some(packet) = reader.next_packet().map_err(|e| e.to_string())? {
        if packet.track_id != track_id {
            continue;
        }
        let decoded = decoder.decode(&packet).map_err(|e| e.to_string())?;
        channels = decoded.spec().channels().count();
        decoded.copy_to_vec_interleaved(&mut packet_samples);
        interleaved.extend_from_slice(&packet_samples);
    }
    if channels == 0 || interleaved.is_empty() {
        return Err("empty".into());
    }
    let channels = (0..channels)
        .map(|c| {
            interleaved
                .iter()
                .skip(c)
                .step_by(channels)
                .copied()
                .collect()
        })
        .collect();
    Ok(Impulse { rate, channels })
}

/// The processing for one session, built for the first track's format.
pub struct Chain {
    channels: usize,
    out_rate: u32,
    gain: f32,
    resample: Option<Resample>,
    /// `filters × channels`, channel-major.
    eq: Vec<DirectForm2Transposed<f64>>,
    convolve: Option<Convolve>,
    out: Vec<f32>,
}

impl Chain {
    pub fn new(setup: &Setup, source_rate: u32, channels: u16) -> Self {
        let channels = channels as usize;
        let out_rate = setup.output_rate(source_rate);
        let impulse = setup
            .impulses
            .get(&out_rate)
            .filter(|i| {
                let fits = i.fits(channels);
                if !fits {
                    log::warn!(
                        "dsp: impulse at {out_rate}Hz has {} channels, the stream {channels}; convolution skipped",
                        i.channels.len()
                    );
                }
                fits
            });

        let coefficients: Vec<_> = setup
            .filters
            .iter()
            .filter_map(|f| coefficients(f, out_rate))
            .collect();
        let preamp_db = setup.preamp_db.unwrap_or_else(|| {
            -20.0 * peak_gain(&coefficients, impulse, out_rate).log10().max(0.0)
        });
        log::info!(
            "dsp: '{}' at {out_rate}Hz — {} bands, preamp {preamp_db:.2} dB{}",
            setup.name,
            coefficients.len(),
            impulse.map_or(String::new(), |i| format!(
                ", {} taps, {} frames delay",
                i.channels[0].len(),
                i.delay()
            ))
        );

        Self {
            channels,
            out_rate,
            gain: 10f64.powf(preamp_db / 20.0) as f32,
            resample: Resample::new(source_rate, out_rate, channels),
            eq: (0..channels)
                .flat_map(|_| coefficients.iter().map(|&c| DirectForm2Transposed::new(c)))
                .collect(),
            convolve: impulse.map(|i| Convolve::new(i, channels)),
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
        self.out.clear();
        if self.resample.as_ref().map_or(self.out_rate, |r| r.in_rate) == source_rate {
            return &self.out;
        }
        let mut tail = Vec::new();
        if let Some(mut r) = self.resample.take() {
            r.flush(&mut tail);
        }
        self.resample = Resample::new(source_rate, self.out_rate, self.channels);
        self.run_tail(tail);
        &self.out
    }

    /// Process interleaved `input`. Returns what is ready to be written, and
    /// how many samples of output time `input` amounts to — the figure the
    /// timeline counts, which with a resampler is not what came out this call.
    pub fn process(&mut self, input: &[f32]) -> (&[f32], u64) {
        self.out.clear();
        let length = match self.resample.as_mut() {
            Some(r) => {
                r.run(input, &mut self.out);
                r.counted() * self.channels as u64
            }
            None => {
                self.out.extend_from_slice(input);
                input.len() as u64
            }
        };
        self.post();
        (&self.out, length)
    }

    /// What the chain still holds at the end of a session.
    pub fn flush(&mut self) -> &[f32] {
        self.out.clear();
        let mut tail = Vec::new();
        if let Some(r) = self.resample.as_mut() {
            r.flush(&mut tail);
        }
        self.run_tail(tail);
        if let Some(c) = self.convolve.as_mut() {
            c.flush(&mut self.out);
        }
        &self.out
    }

    fn run_tail(&mut self, tail: Vec<f32>) {
        self.out = tail;
        self.post();
    }

    /// Gain, bands and convolution over `self.out`, at the output rate.
    fn post(&mut self) {
        let n = self.eq.len() / self.channels.max(1);
        if self.gain != 1.0 || n > 0 {
            for frame in self.out.chunks_exact_mut(self.channels) {
                for (c, s) in frame.iter_mut().enumerate() {
                    let mut x = (*s * self.gain) as f64;
                    for f in &mut self.eq[c * n..(c + 1) * n] {
                        x = f.run(x);
                    }
                    *s = x as f32;
                }
            }
        }
        if let Some(c) = self.convolve.as_mut() {
            c.run(&mut self.out);
        }
    }
}

fn coefficients(f: &EqFilter, rate: u32) -> Option<Coefficients<f64>> {
    let kind = match f.kind {
        EqFilterKind::Peaking => Type::PeakingEQ(f.gain_db),
        EqFilterKind::LowShelf => Type::LowShelf(f.gain_db),
        EqFilterKind::HighShelf => Type::HighShelf(f.gain_db),
        EqFilterKind::LowPass => Type::LowPass,
        EqFilterKind::HighPass => Type::HighPass,
    };
    Hertz::from_hz(f.freq)
        .and_then(|f0| Coefficients::from_params(kind, Hertz::from_hz(rate as f64)?, f0, f.q))
        .inspect_err(|e| {
            log::warn!(
                "dsp: {:?} at {}Hz skipped at {rate}Hz: {e:?}",
                f.kind,
                f.freq
            );
        })
        .ok()
}

/// The magnitude of a biquad's response at `w` radians per sample.
fn magnitude(c: &Coefficients<f64>, w: f64) -> f64 {
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let num = (c.b0 + c.b1 * c1 + c.b2 * c2).hypot(c.b1 * s1 + c.b2 * s2);
    let den = (1.0 + c.a1 * c1 + c.a2 * c2).hypot(c.a1 * s1 + c.a2 * s2);
    num / den
}

/// The largest gain, linear, the bands and the response apply at any
/// frequency. A preamp of its inverse keeps a full-scale sine at that frequency
/// at full scale, as AutoEQ's `Preamp` line does.
fn peak_gain(bands: &[Coefficients<f64>], impulse: Option<&Impulse>, rate: u32) -> f64 {
    let eq = |w: f64| bands.iter().map(|c| magnitude(c, w)).product::<f64>();
    let Some(impulse) = impulse else {
        let (lo, hi) = (10f64.ln(), (rate as f64 * 0.499).ln());
        return (0..=4096)
            .map(|i| {
                let f = (lo + (hi - lo) * i as f64 / 4096.0).exp();
                eq(std::f64::consts::TAU * f / rate as f64)
            })
            .fold(0.0, f64::max);
    };
    let size = impulse.channels[0].len().next_power_of_two().max(8192);
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(size);
    let mut spectrum = fft.make_output_vec();
    let mut peak = 0.0f64;
    for ch in &impulse.channels {
        let mut input = fft.make_input_vec();
        for (d, &s) in input.iter_mut().zip(ch) {
            *d = s as f64;
        }
        if fft.process(&mut input, &mut spectrum).is_err() {
            continue;
        }
        for (k, bin) in spectrum.iter().enumerate() {
            let w = std::f64::consts::TAU * k as f64 / size as f64;
            peak = peak.max(bin.norm() * eq(w));
        }
    }
    peak
}

/// Sample-rate conversion to an impulse response's rate.
struct Resample {
    inner: Fft<f32>,
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    /// Interleaved input not yet a whole chunk.
    pending: Vec<f32>,
    scratch: Vec<f32>,
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
        let inner = Fft::<f32>::new(
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

    fn run(&mut self, input: &[f32], dst: &mut Vec<f32>) {
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
    fn flush(&mut self, dst: &mut Vec<f32>) {
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
    fn chunk(&mut self, need: usize, partial: Option<usize>, dst: &mut Vec<f32>) -> usize {
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

/// FIR convolution, one convolver per channel.
struct Convolve {
    convolvers: Vec<FFTConvolver<f32>>,
    channels: usize,
    delay: usize,
    /// Output frames of the response's delay still to drop.
    skip: usize,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl Convolve {
    fn new(impulse: &Impulse, channels: usize) -> Self {
        let convolvers = (0..channels)
            .map(|c| {
                let mut conv = FFTConvolver::default();
                // Only a block size of zero is refused.
                let _ = conv.init(1024, impulse.for_channel(c));
                conv
            })
            .collect();
        let delay = impulse.delay();
        Self {
            convolvers,
            channels,
            delay,
            skip: delay,
            input: Vec::new(),
            output: Vec::new(),
        }
    }

    fn run(&mut self, buf: &mut Vec<f32>) {
        let ch = self.channels;
        let frames = buf.len() / ch;
        self.input.resize(frames, 0.0);
        self.output.resize(frames, 0.0);
        for (c, conv) in self.convolvers.iter_mut().enumerate() {
            for (d, s) in self.input.iter_mut().zip(buf.iter().skip(c).step_by(ch)) {
                *d = *s;
            }
            if conv.process(&self.input, &mut self.output).is_err() {
                continue;
            }
            for (d, s) in buf.iter_mut().skip(c).step_by(ch).zip(&self.output) {
                *d = *s;
            }
        }
        let drop = self.skip.min(frames);
        self.skip -= drop;
        buf.drain(..drop * ch);
    }

    /// The response's delay worth of silence, which brings out the last of
    /// the audio.
    fn flush(&mut self, dst: &mut Vec<f32>) {
        let mut tail = vec![0.0; self.delay * self.channels];
        self.run(&mut tail);
        dst.extend_from_slice(&tail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(kind: EqFilterKind, freq: f64, gain_db: f64, q: f64) -> EqFilter {
        EqFilter {
            kind,
            freq,
            gain_db,
            q,
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

    #[test]
    fn a_profile_that_changes_nothing_is_no_chain() {
        let profile = DspProfile {
            name: "flat".into(),
            ..Default::default()
        };
        assert!(Setup::load(&profile, Path::new("/")).unwrap().is_none());
    }

    #[test]
    fn a_peaking_band_boosts_its_frequency_and_the_preamp_pays_for_it() {
        let setup = Setup::new(vec![band(EqFilterKind::Peaking, 1000.0, 6.0, 1.0)], vec![]);
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
        let setup = Setup::new(vec![band(EqFilterKind::Peaking, 30000.0, 6.0, 1.0)], vec![]);
        let mut chain = Chain::new(&setup, 44100, 2);
        let input = sine(44100, 1000.0, 4410, 2, 0.5);
        let (out, _) = run_all(&mut chain, &input, 1024);
        assert_eq!(out, input);
    }

    fn delayed_impulse(rate: u32, delay: usize) -> Impulse {
        let mut ir = vec![0.0; delay * 2 + 1];
        ir[delay] = 1.0;
        Impulse {
            rate,
            channels: vec![ir],
        }
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
        let setup = Setup::new(
            vec![],
            vec![Impulse {
                rate: 48000,
                channels: vec![ir],
            }],
        );
        let mut chain = Chain::new(&setup, 48000, 1);
        let (out, _) = run_all(&mut chain, &[0.5; 4800], 480);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-4));
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
        let setup = Setup::load(&profile, &dir).unwrap().unwrap();
        let impulse = &setup.impulses[&96000];
        assert_eq!(impulse.channels.len(), 2);
        assert_eq!(impulse.channels[0], vec![0.0, 0.5, 0.0, 0.0]);
        assert_eq!(impulse.delay(), 1);

        let missing = DspProfile {
            impulses: vec!["nope.wav".into()],
            ..profile
        };
        assert!(Setup::load(&missing, &dir).is_err());
    }
}
