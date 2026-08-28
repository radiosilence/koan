//! Background FFT analysis thread for the visualizer.
//!
//! `VizAnalyzer` owns the FFT state and runs on a dedicated thread, decoupling
//! heavy computation from both the audio decode thread and the TUI render thread.
//!
//! # Lock discipline
//!
//! The analysis loop follows a strict two-phase discipline to minimise lock
//! contention:
//!
//! 1. **Input phase** — lock `VizBuffer` briefly, memcpy samples + metadata,
//!    release immediately.  The decode thread is never blocked for longer than
//!    a single copy.
//! 2. **Compute phase** — run windowing, FFT, bin→bar accumulation *without*
//!    holding any lock.
//! 3. **Output phase** — take the `VizSnapshot` write lock briefly, swap in the
//!    finished frame, release.  The TUI thread is blocked for at most one
//!    ~200-byte memcpy.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use realfft::RealFftPlanner;

use super::viz::{NUM_BARS, RawVizSnapshot, VizBuffer, VizFrame, VizSnapshot, WAVEFORM_SAMPLES};
use crate::config::VisualizerConfig;

// ── FFT constants ────────────────────────────────────────────────────────────

/// FFT window size: 2048 samples (~46ms at 44.1kHz).
const FFT_SIZE: usize = 2048;

/// Minimum frequency (Hz) included in spectrum bars.
const MIN_FREQ: f32 = 20.0;

/// Maximum frequency (Hz) included in spectrum bars.
const MAX_FREQ: f32 = 18_000.0;

/// dB floor: magnitudes below this map to 0.0.
const DB_FLOOR: f32 = -80.0;

/// dB ceiling: magnitudes at or above this map to 1.0.
const DB_CEIL: f32 = 0.0;

/// How long the analyser keeps working after the last frame anyone read.
/// Generous enough that a reader drawing slower than we analyse never trips it.
const IDLE_AFTER: Duration = Duration::from_secs(1);

/// How often a stood-down analyser looks for a reader coming back.
const IDLE_POLL: Duration = Duration::from_millis(250);

// ── Frequency scale ──────────────────────────────────────────────────────────

/// Frequency scale used to map FFT bins to spectrum bars.
#[derive(Debug, Clone, Copy, Default)]
pub enum FrequencyScale {
    /// Bark psychoacoustic scale — 24 critical bands, best for perceiving music.
    #[default]
    Bark,
    /// Mel perceptual pitch scale.
    Mel,
    /// Logarithmic — equal spacing per octave.
    Log,
    /// Linear — equal Hz per bar.
    Linear,
}

impl FrequencyScale {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bark" => Self::Bark,
            "mel" => Self::Mel,
            "log" | "logarithmic" => Self::Log,
            "linear" => Self::Linear,
            _ => Self::default(),
        }
    }

    /// Map a frequency in Hz to a normalised 0.0..1.0 position on this scale.
    fn normalize(&self, freq: f32) -> f32 {
        match self {
            Self::Bark => {
                let bark = |f: f32| 26.81 / (1.0 + 1960.0 / f) - 0.53;
                let b = bark(freq);
                let b_min = bark(MIN_FREQ);
                let b_max = bark(MAX_FREQ);
                (b - b_min) / (b_max - b_min)
            }
            Self::Mel => {
                let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
                let m = mel(freq);
                let m_min = mel(MIN_FREQ);
                let m_max = mel(MAX_FREQ);
                (m - m_min) / (m_max - m_min)
            }
            Self::Log => {
                let log_min = MIN_FREQ.ln();
                let log_max = MAX_FREQ.ln();
                (freq.ln() - log_min) / (log_max - log_min)
            }
            Self::Linear => (freq - MIN_FREQ) / (MAX_FREQ - MIN_FREQ),
        }
    }
}

// ── Amplitude scale ─────────────────────────────────────────────────────────

/// Amplitude scale applied to FFT magnitudes before display.
#[derive(Debug, Clone, Copy, Default)]
pub enum AmplitudeScale {
    /// A-weighted + gentle gamma — bars reflect perceived loudness with quiet boost.
    Perceptual,
    /// Pure A-weighting (IEC 61672), linear mapping after.
    #[default]
    AWeight,
    /// Square root — gentle boost to quiet bands.
    Sqrt,
    /// Linear — raw dB-normalized magnitude, no correction.
    Linear,
}

impl AmplitudeScale {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "perceptual" => Self::Perceptual,
            "aweight" | "a-weight" | "a_weight" => Self::AWeight,
            "sqrt" => Self::Sqrt,
            "linear" => Self::Linear,
            _ => Self::default(),
        }
    }

    /// Apply the amplitude curve to a 0.0..1.0 normalized level.
    fn apply(self, level: f32) -> f32 {
        match self {
            Self::Perceptual => level.powf(0.4),
            Self::AWeight => level,
            Self::Sqrt => level.sqrt(),
            Self::Linear => level,
        }
    }
}

/// A-weighting correction in dB for a given frequency (IEC 61672-1).
///
/// Returns the dB offset to add to a magnitude before normalization.
/// At 1kHz the correction is 0dB; bass and extreme treble are attenuated.
fn a_weight_db(freq: f32) -> f32 {
    let f2 = freq * freq;
    let f4 = f2 * f2;

    let num = 12194.0_f32.powi(2) * f4;
    let denom = (f2 + 20.6_f32.powi(2))
        * ((f2 + 107.7_f32.powi(2)) * (f2 + 737.9_f32.powi(2))).sqrt()
        * (f2 + 12194.0_f32.powi(2));

    if denom == 0.0 {
        return DB_FLOOR;
    }

    // R_A(f) relative to 1kHz reference
    let ra = num / denom;
    // A-weighting: 20*log10(R_A) + 2.00 dB offset (IEC 61672 normalization)
    20.0 * ra.log10() + 2.0
}

/// Pre-compute A-weighting corrections for each FFT bin.
fn build_a_weight_table(sample_rate: f32) -> Vec<f32> {
    let bin_hz = sample_rate / FFT_SIZE as f32;
    let num_bins = FFT_SIZE / 2 + 1;
    (0..num_bins)
        .map(|bin_idx| {
            let freq = bin_idx as f32 * bin_hz;
            if freq < 1.0 {
                DB_FLOOR // DC bin — silence
            } else {
                a_weight_db(freq)
            }
        })
        .collect()
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Precomputed Hann window coefficients.
fn hann_window() -> Vec<f32> {
    (0..FFT_SIZE)
        .map(|i| {
            let t = std::f32::consts::PI * 2.0 * i as f32 / FFT_SIZE as f32;
            0.5 * (1.0 - t.cos())
        })
        .collect()
}

/// Build the bin→bar lookup table for a given sample rate and scale.
/// Returns `None` for bins outside [MIN_FREQ, MAX_FREQ].
fn build_bin_to_bar(sample_rate: f32, scale: FrequencyScale) -> Vec<Option<usize>> {
    let bin_hz = sample_rate / FFT_SIZE as f32;
    let num_bins = FFT_SIZE / 2 + 1;
    (0..num_bins)
        .map(|bin_idx| {
            let freq = bin_idx as f32 * bin_hz;
            if !(MIN_FREQ..=MAX_FREQ).contains(&freq) {
                return None;
            }
            let normalized = scale.normalize(freq);
            Some(((normalized * NUM_BARS as f32) as usize).min(NUM_BARS - 1))
        })
        .collect()
}

// ── Internal analysis state ──────────────────────────────────────────────────

/// All mutable state owned by the analysis thread — not shared.
struct AnalysisState {
    /// Precomputed Hann window.
    window: Vec<f32>,
    /// Magnitude scale that maps a windowed bin back to signal amplitude.
    /// Derived from the window's coherent gain, so it stays correct if the
    /// window function changes.
    fft_norm: f32,
    /// FFT scratch: time-domain input (windowed mono).
    fft_input: Vec<f32>,
    /// FFT scratch: frequency-domain output.
    fft_output: Vec<realfft::num_complex::Complex<f32>>,
    /// Cached FFT plan.
    fft: Arc<dyn realfft::RealToComplex<f32>>,
    /// Bin→bar lookup (rebuilt on sample-rate change).
    bin_to_bar: Vec<Option<usize>>,
    /// Last seen sample rate — detects changes.
    last_sample_rate: f32,
    /// Reusable counts per bar (how many bins mapped to each bar).
    bar_counts: [u32; NUM_BARS],
    /// Smoothed spectrum from previous frame (for decay).
    prev_spectrum: [f32; NUM_BARS],
    /// Current spectrum (written each pass, then moved to output).
    spectrum: [f32; NUM_BARS],
    /// Peak hold values.
    peaks: [f32; NUM_BARS],
    /// VU levels [left, right].
    vu_levels: [f32; 2],
    /// Timestamp of the previous analysis pass (for decay timing).
    last_update: Instant,
    /// Frequency scale for bin→bar mapping.
    scale: FrequencyScale,
    /// Bar decay half-life in seconds.
    bar_half_life: f32,
    /// Peak decay half-life in seconds.
    peak_half_life: f32,
    /// Amplitude scale for magnitude mapping.
    amplitude_scale: AmplitudeScale,
    /// Pre-computed A-weighting correction per FFT bin (dB).
    a_weight_table: Vec<f32>,
    /// Rolling average of low-band energy for beat detection.
    /// Tracks the mean of the bottom ~4 bars over recent frames.
    beat_avg: f32,
    /// Current beat energy output (0.0..1.0), decays each frame.
    beat_energy: f32,
}

impl AnalysisState {
    fn new(
        scale: FrequencyScale,
        bar_half_life: f32,
        peak_half_life: f32,
        amplitude_scale: AmplitudeScale,
    ) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let fft_input = fft.make_input_vec();
        let fft_output = fft.make_output_vec();
        let window = hann_window();
        let fft_norm = 2.0 / window.iter().sum::<f32>();
        Self {
            window,
            fft_norm,
            fft_input,
            fft_output,
            fft,
            bin_to_bar: Vec::new(),
            last_sample_rate: 0.0,
            bar_counts: [0u32; NUM_BARS],
            prev_spectrum: [0.0; NUM_BARS],
            spectrum: [0.0; NUM_BARS],
            peaks: [0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            last_update: Instant::now(),
            scale,
            bar_half_life,
            peak_half_life,
            amplitude_scale,
            a_weight_table: Vec::new(),
            beat_avg: 0.0,
            beat_energy: 0.0,
        }
    }

    /// Compute time-based decay factors from elapsed time since last pass.
    fn decay_factors(&mut self) -> (f32, f32) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;
        let bar_decay = 0.5f32.powf(dt / self.bar_half_life);
        let peak_decay = 0.5f32.powf(dt / self.peak_half_life);
        (bar_decay, peak_decay)
    }

    /// Run a full analysis pass on the given snapshot.
    ///
    /// No lock is held during this call.
    fn analyze(&mut self, samples: &[f32], channels: usize, sample_rate: f32) {
        if samples.is_empty() || sample_rate <= 0.0 || channels == 0 {
            self.decay_silence();
            return;
        }

        // ── VU (RMS per channel) ────────────────────────────────────────────
        self.compute_vu(samples, channels);

        // ── Mix to mono + apply Hann window ────────────────────────────────
        let total_frames = samples.len() / channels;
        let frames_to_use = total_frames.min(FFT_SIZE);
        let frame_start = total_frames - frames_to_use;

        for i in 0..FFT_SIZE {
            if i < frames_to_use {
                let frame_idx = frame_start + i;
                let sample_start = frame_idx * channels;
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    if sample_start + ch < samples.len() {
                        sum += samples[sample_start + ch];
                    }
                }
                self.fft_input[i] = (sum / channels as f32) * self.window[i];
            } else {
                self.fft_input[i] = 0.0;
            }
        }

        // ── FFT ─────────────────────────────────────────────────────────────
        if self
            .fft
            .process(&mut self.fft_input, &mut self.fft_output)
            .is_err()
        {
            self.decay_silence();
            return;
        }

        // ── Rebuild bin→bar + A-weight table on sample-rate change ──────────
        if (sample_rate - self.last_sample_rate).abs() > 0.5 {
            self.bin_to_bar = build_bin_to_bar(sample_rate, self.scale);
            self.a_weight_table = build_a_weight_table(sample_rate);
            self.last_sample_rate = sample_rate;
        }

        // ── Accumulate bins into bars ────────────────────────────────────────
        std::mem::swap(&mut self.spectrum, &mut self.prev_spectrum);
        for bar in self.spectrum.iter_mut() {
            *bar = 0.0;
        }
        for c in self.bar_counts.iter_mut() {
            *c = 0;
        }

        let norm = self.fft_norm;
        let db_range_inv = 1.0 / (DB_CEIL - DB_FLOOR);
        let num_bins = self.fft_output.len().min(self.bin_to_bar.len());

        for bin_idx in 0..num_bins {
            let bar_idx = match self.bin_to_bar[bin_idx] {
                Some(b) => b,
                None => continue,
            };
            let c = self.fft_output[bin_idx];
            let magnitude = (c.re * c.re + c.im * c.im).sqrt() * norm;
            let mut db = if magnitude > 0.0 {
                20.0 * magnitude.log10()
            } else {
                DB_FLOOR
            };
            // Apply A-weighting if using perceptual or aweight scale.
            if matches!(
                self.amplitude_scale,
                AmplitudeScale::Perceptual | AmplitudeScale::AWeight
            ) && let Some(&aw) = self.a_weight_table.get(bin_idx)
            {
                db += aw;
            }
            let level = ((db - DB_FLOOR) * db_range_inv).clamp(0.0, 1.0);
            let level = self.amplitude_scale.apply(level);
            if level > self.spectrum[bar_idx] {
                self.spectrum[bar_idx] = level;
            }
            self.bar_counts[bar_idx] += 1;
        }

        self.fill_empty_bars();

        // ── Time-based smoothing + peak hold ────────────────────────────────
        let (bar_decay, peak_decay) = self.decay_factors();
        for i in 0..NUM_BARS {
            let decayed = self.prev_spectrum[i] * bar_decay;
            self.spectrum[i] = self.spectrum[i].max(decayed);

            if self.spectrum[i] > self.peaks[i] {
                self.peaks[i] = self.spectrum[i];
            } else {
                self.peaks[i] *= peak_decay;
            }
        }

        // ── Beat detection (low-band transient) ─────────────────────────────
        // Sum the bottom ~6 bars (sub-bass through upper bass) as the beat signal.
        let beat_bands = NUM_BARS.min(6);
        let low_energy: f32 = self.spectrum[..beat_bands].iter().sum::<f32>() / beat_bands as f32;

        // Slow EMA — alpha 0.02 gives ~50 frame memory at 60fps (~0.8s).
        // This tracks the ambient bass level, not individual beats.
        const BEAT_AVG_ALPHA: f32 = 0.02;
        self.beat_avg = self.beat_avg * (1.0 - BEAT_AVG_ALPHA) + low_energy * BEAT_AVG_ALPHA;

        // Beat = how far current energy exceeds the rolling average, normalized.
        // The spike is scaled so that a 2x surge = 1.0 output.
        let beat_spike = if self.beat_avg > 0.005 {
            let excess = (low_energy - self.beat_avg).max(0.0);
            (excess / self.beat_avg.max(0.05)).clamp(0.0, 1.0)
        } else {
            // No meaningful baseline yet — use raw energy as bootstrap.
            (low_energy * 3.0).clamp(0.0, 1.0)
        };

        // Beat energy: rise instantly, decay slower than bars for a visible pulse.
        // Using sqrt of bar_decay gives roughly double the half-life.
        self.beat_energy = beat_spike.max(self.beat_energy * bar_decay.sqrt());
    }

    /// Fill bars that no FFT bin landed in.
    ///
    /// At high sample rates a 2048-point FFT spaces bins ~94 Hz apart, leaving
    /// whole runs of the bottom Bark bars with no bin at all. Each run is
    /// interpolated across its two *measured* neighbours in one pass, so a
    /// synthesised bar is never used as an endpoint for the next one.
    fn fill_empty_bars(&mut self) {
        let mut i = 0;
        while i < NUM_BARS {
            if self.bar_counts[i] != 0 {
                i += 1;
                continue;
            }
            let mut end = i;
            while end < NUM_BARS && self.bar_counts[end] == 0 {
                end += 1;
            }

            match (i.checked_sub(1), (end < NUM_BARS).then_some(end)) {
                (Some(left), Some(right)) => {
                    let (lo, hi) = (self.spectrum[left], self.spectrum[right]);
                    let span = (right - left) as f32;
                    for (n, bar) in (i..end).enumerate() {
                        let t = (n + 1) as f32 / span;
                        self.spectrum[bar] = lo + (hi - lo) * t;
                    }
                }
                // A run at either edge has one measured neighbour; extend it
                // rather than fading the outermost bar toward an imaginary zero.
                (Some(left), None) => {
                    let value = self.spectrum[left];
                    self.spectrum[i..end].fill(value);
                }
                (None, Some(right)) => {
                    let value = self.spectrum[right];
                    self.spectrum[i..end].fill(value);
                }
                (None, None) => self.spectrum.fill(0.0),
            }

            i = end;
        }
    }

    /// Apply decay-to-silence (called when paused or no audio).
    fn decay_silence(&mut self) {
        let (bar_decay, peak_decay) = self.decay_factors();
        for i in 0..NUM_BARS {
            self.spectrum[i] *= bar_decay;
            self.peaks[i] *= peak_decay;
        }
        for v in self.vu_levels.iter_mut() {
            *v *= bar_decay;
        }
        self.beat_energy *= bar_decay;
    }

    /// Compute RMS VU levels per channel from the snapshot.
    fn compute_vu(&mut self, samples: &[f32], channels: usize) {
        let total_frames = samples.len() / channels;
        let frames_to_use = total_frames.min(2048);
        let frame_start = total_frames - frames_to_use;
        let vu_channels = channels.min(2);
        let mut sum_sq = [0.0f64; 2];

        for frame in 0..frames_to_use {
            let idx = (frame_start + frame) * channels;
            for ch in 0..vu_channels {
                if idx + ch < samples.len() {
                    let s = samples[idx + ch] as f64;
                    sum_sq[ch] += s * s;
                }
            }
        }

        let db_range = DB_CEIL - DB_FLOOR;
        for (ch, &sq) in sum_sq.iter().enumerate().take(vu_channels) {
            let rms = (sq / frames_to_use as f64).sqrt() as f32;
            let db = if rms > 0.0 {
                20.0 * rms.log10()
            } else {
                DB_FLOOR
            };
            self.vu_levels[ch] = ((db - DB_FLOOR) / db_range).clamp(0.0, 1.0);
        }

        if vu_channels == 1 {
            self.vu_levels[1] = self.vu_levels[0];
        }
    }
}

// ── VizAnalyzer (public API) ─────────────────────────────────────────────────

/// Background FFT analysis engine.
///
/// Call `VizAnalyzer::spawn_with_snapshot` to start the analysis thread. Drop
/// the returned handle (or let it go out of scope) to request graceful
/// shutdown; the thread exits within one analysis interval.
pub struct VizAnalyzer {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl VizAnalyzer {
    /// Spawn the background analysis thread, writing each pass to `snapshot`.
    ///
    /// * `viz_buffer`     — the delay line written by the decode thread.
    /// * `cfg`            — visualizer configuration (scale, decay times, fps).
    /// * `snapshot`       — where each finished `VizFrame` is published.
    /// * `samples_played` — the engine's played counter, used to read the delay
    ///   line at the position currently reaching the DAC.
    pub fn spawn_with_snapshot(
        viz_buffer: Arc<VizBuffer>,
        cfg: &VisualizerConfig,
        snapshot: Arc<VizSnapshot>,
        samples_played: Arc<AtomicU64>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));

        let scale = FrequencyScale::parse(&cfg.scale);
        let amplitude_scale = AmplitudeScale::parse(&cfg.amplitude_scale);
        let bar_half_life = cfg.bar_decay_ms as f32 / 1000.0;
        let peak_half_life = cfg.peak_decay_ms as f32 / 1000.0;
        // The configured rate is the starting one. A client drawing on a
        // display can set its own — see `VizSnapshot::set_fps`.
        snapshot.set_fps(cfg.fps);

        let running_clone = Arc::clone(&running);

        let handle = thread::Builder::new()
            .name("viz-analyzer".into())
            .spawn(move || {
                analysis_loop(
                    viz_buffer,
                    snapshot,
                    samples_played,
                    running_clone,
                    scale,
                    amplitude_scale,
                    bar_half_life,
                    peak_half_life,
                );
            })
            .expect("failed to spawn viz-analyzer thread");

        Self {
            running,
            handle: Some(handle),
        }
    }

    /// Signal the background thread to stop and wait for it to exit.
    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for VizAnalyzer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ── Analysis thread loop ─────────────────────────────────────────────────────

/// Frames read from the delay line each pass: enough for both the FFT window
/// and the widest waveform the UI draws.
const WINDOW_FRAMES: usize = if FFT_SIZE > WAVEFORM_SAMPLES {
    FFT_SIZE
} else {
    WAVEFORM_SAMPLES
};

#[allow(clippy::too_many_arguments)]
fn analysis_loop(
    viz_buffer: Arc<VizBuffer>,
    snapshot: Arc<VizSnapshot>,
    samples_played: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    scale: FrequencyScale,
    amplitude_scale: AmplitudeScale,
    bar_half_life: f32,
    peak_half_life: f32,
) {
    let mut state = AnalysisState::new(scale, bar_half_life, peak_half_life, amplitude_scale);
    let mut snap = RawVizSnapshot::default();
    let mut last_reads = u64::MAX;
    let mut last_read_at = Instant::now();

    while running.load(Ordering::Relaxed) {
        let start = Instant::now();

        // ── Phase 0: is anyone looking? ──────────────────────────────────────
        // Nothing reading the snapshot means nothing to compute. The FFT, the
        // per-frame waveform allocation and the delay-line copy all go away
        // until a visualiser opens, which is the whole cost of this thread in
        // a client that never opens one.
        let reads = snapshot.reads();
        if reads != last_reads {
            last_reads = reads;
            last_read_at = start;
        } else if start.duration_since(last_read_at) > IDLE_AFTER {
            thread::sleep(IDLE_POLL);
            continue;
        }

        // ── Phase 1: read the delay line at the play head (lock held briefly) ─
        let played = samples_played.load(Ordering::Relaxed);
        viz_buffer.snapshot_at(played, WINDOW_FRAMES, &mut snap);

        // ── Phase 2: compute (no lock held) ──────────────────────────────────
        state.analyze(
            &snap.samples,
            snap.channels.max(1) as usize,
            snap.sample_rate as f32,
        );

        // ── Phase 3: publish to VizSnapshot (RwLock write, <1us) ─────────────
        // The tail of the window is the newest audible audio, which is what the
        // oscilloscope and lissajous modes draw.
        let interleaved_len = WAVEFORM_SAMPLES * snap.channels.max(1) as usize;
        let waveform_start = snap.samples.len().saturating_sub(interleaved_len);
        snapshot.write(VizFrame {
            spectrum: state.spectrum,
            peaks: state.peaks,
            vu_levels: state.vu_levels,
            beat_energy: state.beat_energy,
            timestamp: Instant::now(),
            waveform: snap.samples[waveform_start..].to_vec(),
        });

        // ── Sleep for the remainder of the interval ───────────────────────────
        // Read each pass, so a window moving to a 120Hz display is followed on
        // the next one rather than at the next track.
        let interval = snapshot.interval();
        let elapsed = start.elapsed();
        if elapsed < interval {
            thread::sleep(interval - elapsed);
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::viz::VizBuffer;
    use crate::config::VisualizerConfig;

    fn make_cfg() -> VisualizerConfig {
        VisualizerConfig::default()
    }

    /// Interleaved stereo sine, `frames` long, at `freq` Hz.
    fn sine(frames: usize, freq: f32, amplitude: f32, sample_rate: u32) -> Vec<f32> {
        let mut samples = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let val = (2.0 * std::f32::consts::PI * freq * t).sin() * amplitude;
            samples.push(val);
            samples.push(val);
        }
        samples
    }

    fn spawn_analyzer(
        buf: Arc<VizBuffer>,
        cfg: &VisualizerConfig,
        played: u64,
    ) -> (VizAnalyzer, Arc<VizSnapshot>) {
        let snapshot = VizSnapshot::new();
        let analyzer = VizAnalyzer::spawn_with_snapshot(
            buf,
            cfg,
            Arc::clone(&snapshot),
            Arc::new(AtomicU64::new(played)),
        );
        (analyzer, snapshot)
    }

    #[test]
    fn analyzer_spawns_and_shuts_down() {
        let buf = VizBuffer::new();
        let cfg = make_cfg();
        let (mut analyzer, snapshot) = spawn_analyzer(buf, &cfg, 0);
        // Let it run for one cycle.
        std::thread::sleep(Duration::from_millis(100));
        analyzer.shutdown();
        // The snapshot must still be readable after shutdown.
        let frame = snapshot.read();
        assert_eq!(frame.spectrum.len(), NUM_BARS);
        assert_eq!(frame.peaks.len(), NUM_BARS);
    }

    #[test]
    fn analyzer_produces_nonzero_output_for_sine() {
        let buf = VizBuffer::new();
        let sample_rate = 44100u32;
        let samples = sine(4096, 440.0, 0.5, sample_rate);
        buf.push_samples(&samples, 2, sample_rate);

        let cfg = make_cfg();
        // Everything pushed has been played, so the window sits at the head.
        let (mut analyzer, snapshot) = spawn_analyzer(Arc::clone(&buf), &cfg, samples.len() as u64);
        // Wait for at least two analysis passes.
        std::thread::sleep(Duration::from_millis(150));

        let frame = snapshot.read();
        analyzer.shutdown();

        let max_bar = frame.spectrum.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            max_bar > 0.05,
            "expected nonzero spectrum for 440 Hz sine, max = {}",
            max_bar
        );
    }

    #[test]
    fn analyzer_reads_the_delay_line_at_the_play_head() {
        let sample_rate = 44100u32;
        let buf = VizBuffer::new();
        // A second of silence is heard first; a tone is decoded far ahead of it.
        buf.push_samples(&vec![0.0; sample_rate as usize * 2], 2, sample_rate);
        buf.push_samples(&sine(4096, 440.0, 0.8, sample_rate), 2, sample_rate);

        let cfg = make_cfg();
        // The DAC is still inside the silence.
        let (mut analyzer, snapshot) = spawn_analyzer(Arc::clone(&buf), &cfg, sample_rate as u64);
        std::thread::sleep(Duration::from_millis(150));
        let frame = snapshot.read();
        analyzer.shutdown();

        let max_bar = frame.spectrum.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            max_bar < 0.05,
            "visualizer showed audio the DAC has not reached yet, max = {}",
            max_bar
        );
    }

    #[test]
    fn full_scale_sine_reads_zero_db() {
        // Linear amplitude scale so no A-weighting shifts the level, and a
        // bin-centred frequency so there is no scalloping loss to hide a
        // wrong window gain.
        let mut state =
            AnalysisState::new(FrequencyScale::Bark, 0.08, 0.35, AmplitudeScale::Linear);
        let sample_rate = 44100.0;
        let freq = 46.0 * sample_rate / FFT_SIZE as f32;
        let samples = sine(FFT_SIZE, freq, 1.0, sample_rate as u32);

        state.analyze(&samples, 2, sample_rate);

        // 0 dBFS maps to the top of the DB_FLOOR..DB_CEIL range.
        let max_bar = state.spectrum.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            max_bar > 0.98,
            "full-scale sine should reach the top of the widget, got {}",
            max_bar
        );
    }

    #[test]
    fn bark_bars_go_unmapped_at_high_sample_rates() {
        // 192 kHz over a 2048-point FFT is 93.75 Hz per bin — too coarse for
        // the bottom of the Bark scale, which is what makes interpolation
        // load-bearing rather than cosmetic.
        let mapping = build_bin_to_bar(192_000.0, FrequencyScale::Bark);
        let mut counts = [0u32; NUM_BARS];
        for bar in mapping.iter().flatten() {
            counts[*bar] += 1;
        }
        assert_eq!(
            counts[0], 0,
            "no bin reaches the lowest Bark bar at 192 kHz"
        );
        assert!(
            counts.iter().filter(|&&c| c == 0).count() > 3,
            "expected several unmapped bass bars, got {:?}",
            counts
        );
    }

    #[test]
    fn empty_bars_interpolate_without_sawtooth() {
        let mut state =
            AnalysisState::new(FrequencyScale::Bark, 0.08, 0.35, AmplitudeScale::Linear);
        // A rising bass ramp measured only on the bars a 192 kHz FFT reaches.
        for (n, &bar) in [1usize, 4, 6, 8].iter().enumerate() {
            state.bar_counts[bar] = 1;
            state.spectrum[bar] = 0.2 + 0.1 * n as f32;
        }
        for bar in 9..NUM_BARS {
            state.bar_counts[bar] = 1;
            state.spectrum[bar] = 0.5;
        }

        state.fill_empty_bars();

        // Bar 0 has no measured neighbour below it, so it takes bar 1's level
        // rather than half of it.
        assert!((state.spectrum[0] - state.spectrum[1]).abs() < 1e-6);
        for i in 0..8 {
            assert!(
                state.spectrum[i + 1] >= state.spectrum[i] - 1e-6,
                "sawtooth across interpolated bass: {:?}",
                &state.spectrum[..9]
            );
        }
    }

    #[test]
    fn analysis_state_decays_to_zero_on_silence() {
        // Use Linear amplitude scale — A-weighting can produce small residual
        // levels from FFT numerical noise at boosted frequencies.
        let mut state =
            AnalysisState::new(FrequencyScale::Bark, 0.08, 0.35, AmplitudeScale::Linear);

        // Seed some nonzero spectrum.
        for v in state.spectrum.iter_mut() {
            *v = 1.0;
        }
        for v in state.peaks.iter_mut() {
            *v = 1.0;
        }

        // Simulate 100 frames of silence with 100ms gaps (10s total).
        // peak_half_life = 350ms → need ~3.4 half-lives to reach < 0.1.
        // Use 100ms offsets so decay is guaranteed even on fast machines where
        // the real elapsed time between last_update and decay_factors() is tiny.
        let silence: Vec<f32> = vec![0.0; FFT_SIZE * 2];
        for _ in 0..100 {
            state.last_update = Instant::now() - Duration::from_millis(100);
            state.analyze(&silence, 2, 44100.0);
        }

        let max_spec = state.spectrum.iter().cloned().fold(0.0f32, f32::max);
        let max_peak = state.peaks.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            max_spec < 0.1,
            "spectrum should decay near zero, got {}",
            max_spec
        );
        assert!(
            max_peak < 0.1,
            "peaks should decay near zero, got {}",
            max_peak
        );
    }

    #[test]
    fn bin_to_bar_covers_audible_range() {
        let mapping = build_bin_to_bar(44100.0, FrequencyScale::Bark);
        let active_bins: Vec<usize> = mapping.iter().filter_map(|x| *x).collect();
        assert!(
            !active_bins.is_empty(),
            "at least some bins should map to bars"
        );
        let max_bar = *active_bins.iter().max().unwrap();
        assert!(max_bar < NUM_BARS, "bar index must be in range");
    }

    #[test]
    fn frequency_scale_bark_normalize_monotonic() {
        let scale = FrequencyScale::Bark;
        let freqs: Vec<f32> = vec![100.0, 500.0, 1000.0, 4000.0, 10000.0];
        let normed: Vec<f32> = freqs.iter().map(|&f| scale.normalize(f)).collect();
        for w in normed.windows(2) {
            assert!(w[1] > w[0], "Bark scale must be monotonically increasing");
        }
    }
}
