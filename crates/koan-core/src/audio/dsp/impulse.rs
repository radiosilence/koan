//! Impulse responses and the convolution stage that runs them.
//!
//! A response is a set of routes, as Convolver's `.cfg` describes one: each
//! route convolves a weighted mix of input channels with its own response and
//! adds the result, weighted, into output channels. A plain WAV is the common
//! case of that — channel `n` of the file convolving channel `n` of the audio —
//! and crossfeed or per-speaker corrections are the rest.

use std::collections::VecDeque;
use std::fs::File;
use std::path::Path;

use fft_convolver::FFTConvolver;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// One response convolving a mix of inputs into a set of outputs.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub ir: Vec<f32>,
    /// `(channel, weight)` summed into the convolver's input.
    pub inputs: Vec<(usize, f32)>,
    /// `(channel, weight)` the convolver's output is added into.
    pub outputs: Vec<(usize, f32)>,
}

/// An impulse response at the rate it was designed for.
#[derive(Debug, Clone, PartialEq)]
pub struct Impulse {
    pub rate: u32,
    /// The channel count the routes are laid out for. `None` is one response
    /// applied to every channel alike: a mono WAV.
    pub channels: Option<usize>,
    pub routes: Vec<Route>,
    /// Per channel, in frames, before and after convolution.
    pub in_delays: Vec<usize>,
    pub out_delays: Vec<usize>,
}

impl Impulse {
    /// A file's channels: one channel for all, or channel `n` for channel `n`.
    pub fn from_channels(rate: u32, mut channels: Vec<Vec<f32>>) -> Self {
        if channels.len() == 1 {
            return Self {
                rate,
                channels: None,
                routes: vec![Route {
                    ir: channels.remove(0),
                    inputs: vec![(0, 1.0)],
                    outputs: vec![(0, 1.0)],
                }],
                in_delays: Vec::new(),
                out_delays: Vec::new(),
            };
        }
        let n = channels.len();
        Self {
            rate,
            channels: Some(n),
            routes: channels
                .into_iter()
                .enumerate()
                .map(|(c, ir)| Route {
                    ir,
                    inputs: vec![(c, 1.0)],
                    outputs: vec![(c, 1.0)],
                })
                .collect(),
            in_delays: Vec::new(),
            out_delays: Vec::new(),
        }
    }

    pub fn fits(&self, channels: usize) -> bool {
        self.channels.is_none_or(|n| n == channels)
    }

    /// The routes for a stream of `channels`, a shared response copied to each.
    pub(crate) fn routes_for(&self, channels: usize) -> Vec<Route> {
        match self.channels {
            Some(_) => self.routes.clone(),
            None => (0..channels)
                .map(|c| Route {
                    ir: self.routes[0].ir.clone(),
                    inputs: vec![(c, 1.0)],
                    outputs: vec![(c, 1.0)],
                })
                .collect(),
        }
    }

    /// Each channel convolved by its own response, with nothing mixed or
    /// delayed: what a plain WAV can hold.
    pub fn as_channels(&self) -> Option<Vec<&[f32]>> {
        if !self
            .in_delays
            .iter()
            .chain(&self.out_delays)
            .all(|&d| d == 0)
        {
            return None;
        }
        if self.channels.is_none() {
            return Some(vec![&self.routes[0].ir]);
        }
        let mut out = vec![None; self.channels?];
        for r in &self.routes {
            let ([(i, wi)], [(o, wo)]) = (&r.inputs[..], &r.outputs[..]) else {
                return None;
            };
            if i != o || *wi != 1.0 || *wo != 1.0 || out[*i].is_some() {
                return None;
            }
            out[*i] = Some(&r.ir[..]);
        }
        out.into_iter().collect()
    }

    /// Whether any route feeds one channel into another, as crossfeed does.
    pub fn mixes(&self) -> bool {
        self.routes.iter().any(|r| {
            r.inputs.len() > 1
                || r.outputs.len() > 1
                || r.inputs.first().map(|i| i.0) != r.outputs.first().map(|o| o.0)
        })
    }

    /// The frame the responses peak at — the group delay, for a linear-phase
    /// filter. One figure for every route, so the channels stay aligned.
    pub fn delay(&self) -> usize {
        let mut best = (0, 0.0f32);
        for r in &self.routes {
            for (i, &v) in r.ir.iter().enumerate() {
                if v.abs() > best.1 {
                    best = (i, v.abs());
                }
            }
        }
        best.0
    }

    pub fn taps(&self) -> usize {
        self.routes.iter().map(|r| r.ir.len()).max().unwrap_or(0)
    }
}

/// An audio file's channels, at its own rate. WAV, AIFF, FLAC and ALAC — what
/// Roon takes, and whatever else symphonia reads.
pub fn read_audio(path: &Path) -> Result<(u32, Vec<Vec<f32>>), String> {
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
    let planar = (0..channels)
        .map(|c| {
            interleaved
                .iter()
                .skip(c)
                .step_by(channels)
                .copied()
                .collect()
        })
        .collect();
    Ok((rate, planar))
}

/// A fixed delay, in frames.
struct DelayLine(VecDeque<f64>);

impl DelayLine {
    fn new(frames: usize) -> Option<Self> {
        (frames > 0).then(|| Self(std::iter::repeat_n(0.0, frames).collect()))
    }

    fn run(&mut self, samples: &mut [f64]) {
        for s in samples {
            self.0.push_back(*s);
            *s = self.0.pop_front().expect("never empty");
        }
    }
}

/// A route ready to run.
struct Running {
    conv: FFTConvolver<f64>,
    inputs: Vec<(usize, f32)>,
    outputs: Vec<(usize, f32)>,
}

/// The convolution stage for one session.
pub(crate) struct Convolve {
    routes: Vec<Running>,
    channels: usize,
    in_delays: Vec<Option<DelayLine>>,
    out_delays: Vec<Option<DelayLine>>,
    delay: usize,
    /// Output frames of the responses' delay still to drop.
    skip: usize,
    planar_in: Vec<Vec<f64>>,
    planar_out: Vec<Vec<f64>>,
    mix: Vec<f64>,
    wet: Vec<f64>,
}

impl Convolve {
    pub(crate) fn new(impulse: &Impulse, channels: usize) -> Self {
        let routes = impulse
            .routes_for(channels)
            .into_iter()
            .map(|r| {
                let mut conv = FFTConvolver::default();
                // Work goes with the number of partitions, so a long response
                // takes larger ones: 262k taps at 192 kHz is 32 of 8192 rather
                // than 256 of 1024. The decoder runs a second ahead of the
                // output, so a block's latency costs nothing. Only a block
                // size of zero is refused.
                let block = (r.ir.len().next_power_of_two() / 32).clamp(1024, 16384);
                let ir: Vec<f64> = r.ir.iter().map(|&t| t as f64).collect();
                let _ = conv.init(block, &ir);
                Running {
                    conv,
                    inputs: r.inputs,
                    outputs: r.outputs,
                }
            })
            .collect();
        let delays = |d: &[usize]| {
            (0..channels)
                .map(|c| DelayLine::new(d.get(c).copied().unwrap_or(0)))
                .collect()
        };
        let delay = impulse.delay();
        Self {
            routes,
            channels,
            in_delays: delays(&impulse.in_delays),
            out_delays: delays(&impulse.out_delays),
            delay,
            skip: delay,
            planar_in: vec![Vec::new(); channels],
            planar_out: vec![Vec::new(); channels],
            mix: Vec::new(),
            wet: Vec::new(),
        }
    }

    pub(crate) fn run(&mut self, buf: &mut Vec<f64>) {
        let ch = self.channels;
        let frames = buf.len() / ch;
        for (c, plane) in self.planar_in.iter_mut().enumerate() {
            plane.clear();
            plane.extend(buf.iter().skip(c).step_by(ch));
            if let Some(d) = self.in_delays[c].as_mut() {
                d.run(plane);
            }
        }
        for plane in &mut self.planar_out {
            plane.clear();
            plane.resize(frames, 0.0);
        }
        self.mix.resize(frames, 0.0);
        self.wet.resize(frames, 0.0);
        for Running {
            conv,
            inputs,
            outputs,
        } in &mut self.routes
        {
            self.mix.fill(0.0);
            for &(c, w) in inputs.iter() {
                if let Some(plane) = self.planar_in.get(c) {
                    for (m, s) in self.mix.iter_mut().zip(plane) {
                        *m += w as f64 * s;
                    }
                }
            }
            if conv.process(&self.mix, &mut self.wet).is_err() {
                continue;
            }
            for &(c, w) in outputs.iter() {
                if let Some(plane) = self.planar_out.get_mut(c) {
                    for (o, s) in plane.iter_mut().zip(&self.wet) {
                        *o += w as f64 * s;
                    }
                }
            }
        }
        for (c, plane) in self.planar_out.iter_mut().enumerate() {
            if let Some(d) = self.out_delays[c].as_mut() {
                d.run(plane);
            }
            for (d, s) in buf.iter_mut().skip(c).step_by(ch).zip(plane.iter()) {
                *d = *s;
            }
        }
        let drop = self.skip.min(frames);
        self.skip -= drop;
        buf.drain(..drop * ch);
    }

    /// The responses' delay worth of silence, which brings out the last of
    /// the audio. A channel delayed on purpose loses that much of its end, so
    /// the session stays exactly as long as what was fed.
    pub(crate) fn flush(&mut self, dst: &mut Vec<f64>) {
        let mut tail = vec![0.0; self.delay * self.channels];
        self.run(&mut tail);
        dst.extend_from_slice(&tail);
    }
}
