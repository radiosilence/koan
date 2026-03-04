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
//! 3. **Output phase** — lock `SharedAnalysisOutput` briefly, memcpy the
//!    computed spectrum/peaks/VU into it, release.  The TUI thread is blocked
//!    for at most one memcpy of 48-element Vec slices.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use realfft::RealFftPlanner;

use super::viz::{
    AnalysisOutput, NUM_BARS, RawVizSnapshot, SharedAnalysisOutput, VizBuffer, VizFrame,
    VizSnapshot,
};
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
        Self {
            window: hann_window(),
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

        let norm = 2.0 / FFT_SIZE as f32;
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

        // ── Interpolate empty bars ───────────────────────────────────────────
        for i in 0..NUM_BARS {
            if self.bar_counts[i] == 0 {
                let left = if i > 0 { self.spectrum[i - 1] } else { 0.0 };
                let right = if i + 1 < NUM_BARS {
                    self.spectrum[i + 1]
                } else {
                    0.0
                };
                self.spectrum[i] = (left + right) * 0.5;
            }
        }

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
/// Call `VizAnalyzer::spawn` to start the analysis thread. Drop the returned
/// handle (or let it go out of scope) to request graceful shutdown; the thread
/// exits within one analysis interval.
///
/// The latest analysis results are always available via `output()` or via the
/// `VizSnapshot` passed to `spawn_with_snapshot`.
pub struct VizAnalyzer {
    output: SharedAnalysisOutput,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl VizAnalyzer {
    /// Spawn the background analysis thread.
    ///
    /// * `viz_buffer` — the shared sample ring-buffer written by the decode thread.
    /// * `cfg`        — visualizer configuration (scale, decay times, fps).
    pub fn spawn(viz_buffer: Arc<VizBuffer>, cfg: &VisualizerConfig) -> Self {
        Self::spawn_inner(viz_buffer, cfg, None)
    }

    /// Spawn the background analysis thread, also writing results to `snapshot`.
    ///
    /// Each analysis pass writes a `VizFrame` to `snapshot` (write lock <1us)
    /// in addition to updating `SharedAnalysisOutput`.
    pub fn spawn_with_snapshot(
        viz_buffer: Arc<VizBuffer>,
        cfg: &VisualizerConfig,
        snapshot: Arc<VizSnapshot>,
    ) -> Self {
        Self::spawn_inner(viz_buffer, cfg, Some(snapshot))
    }

    fn spawn_inner(
        viz_buffer: Arc<VizBuffer>,
        cfg: &VisualizerConfig,
        snapshot: Option<Arc<VizSnapshot>>,
    ) -> Self {
        let output: SharedAnalysisOutput = Arc::new(Mutex::new(AnalysisOutput::default()));
        let running = Arc::new(AtomicBool::new(true));

        let scale = FrequencyScale::parse(&cfg.scale);
        let amplitude_scale = AmplitudeScale::parse(&cfg.amplitude_scale);
        let bar_half_life = cfg.bar_decay_ms as f32 / 1000.0;
        let peak_half_life = cfg.peak_decay_ms as f32 / 1000.0;
        let interval = Duration::from_millis(1000 / cfg.fps.max(1) as u64);

        let output_clone = Arc::clone(&output);
        let running_clone = Arc::clone(&running);

        let handle = thread::Builder::new()
            .name("viz-analyzer".into())
            .spawn(move || {
                analysis_loop(
                    viz_buffer,
                    output_clone,
                    snapshot,
                    running_clone,
                    scale,
                    amplitude_scale,
                    bar_half_life,
                    peak_half_life,
                    interval,
                );
            })
            .expect("failed to spawn viz-analyzer thread");

        Self {
            output,
            running,
            handle: Some(handle),
        }
    }

    /// Clone the latest analysis output for rendering.
    ///
    /// Acquires the output lock for the duration of a `Clone` — typically a
    /// handful of `memcpy`s over 48-element `Vec`s.
    pub fn output(&self) -> AnalysisOutput {
        self.output.lock().clone()
    }

    /// Shared reference to the raw output mutex (for callers that prefer to
    /// lock once and read multiple fields without cloning).
    pub fn shared_output(&self) -> SharedAnalysisOutput {
        Arc::clone(&self.output)
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

#[allow(clippy::too_many_arguments)]
fn analysis_loop(
    viz_buffer: Arc<VizBuffer>,
    output: SharedAnalysisOutput,
    snapshot: Option<Arc<VizSnapshot>>,
    running: Arc<AtomicBool>,
    scale: FrequencyScale,
    amplitude_scale: AmplitudeScale,
    bar_half_life: f32,
    peak_half_life: f32,
    interval: Duration,
) {
    let mut state = AnalysisState::new(scale, bar_half_life, peak_half_life, amplitude_scale);

    while running.load(Ordering::Relaxed) {
        let start = Instant::now();

        // ── Phase 1: snapshot (lock held briefly) ────────────────────────────
        let snap: RawVizSnapshot = viz_buffer.snapshot_with_meta();

        // ── Phase 2: compute (no lock held) ──────────────────────────────────
        state.analyze(
            &snap.samples,
            snap.channels.max(1) as usize,
            snap.sample_rate as f32,
        );

        // ── Phase 3a: publish to SharedAnalysisOutput (Mutex, <1us) ──────────
        {
            let mut out = output.lock();
            out.spectrum.copy_from_slice(&state.spectrum);
            out.peaks.copy_from_slice(&state.peaks);
            out.vu_levels = state.vu_levels;
        }

        // ── Phase 3b: publish to VizSnapshot (RwLock write, <1us) ────────────
        if let Some(ref snap_out) = snapshot {
            snap_out.write(VizFrame {
                spectrum: state.spectrum,
                vu_levels: state.vu_levels,
                timestamp: Instant::now(),
            });
        }

        // ── Sleep for the remainder of the interval ───────────────────────────
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

    #[test]
    fn analyzer_spawns_and_shuts_down() {
        let buf = VizBuffer::new();
        let cfg = make_cfg();
        let mut analyzer = VizAnalyzer::spawn(buf, &cfg);
        // Let it run for one cycle.
        std::thread::sleep(Duration::from_millis(100));
        analyzer.shutdown();
        // output() must still work after shutdown.
        let out = analyzer.output();
        assert_eq!(out.spectrum.len(), NUM_BARS);
        assert_eq!(out.peaks.len(), NUM_BARS);
    }

    #[test]
    fn analyzer_produces_nonzero_output_for_sine() {
        let buf = VizBuffer::new();
        let sample_rate = 44100u32;
        let channels = 2u16;
        let num_frames = 4096;
        let mut samples = Vec::with_capacity(num_frames * 2);
        for i in 0..num_frames {
            let t = i as f32 / sample_rate as f32;
            let val = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
            samples.push(val);
            samples.push(val);
        }
        buf.push_samples(&samples, channels, sample_rate);

        let cfg = make_cfg();
        let mut analyzer = VizAnalyzer::spawn(Arc::clone(&buf), &cfg);
        // Wait for at least two analysis passes.
        std::thread::sleep(Duration::from_millis(150));

        let out = analyzer.output();
        analyzer.shutdown();

        let max_bar = out.spectrum.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            max_bar > 0.05,
            "expected nonzero spectrum for 440 Hz sine, max = {}",
            max_bar
        );
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
