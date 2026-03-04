use std::time::Instant;

use koan_core::audio::viz::VizBuffer;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use realfft::RealFftPlanner;

use super::theme::Theme;

/// FFT window size: 2048 samples (~46ms at 44.1kHz).
const FFT_SIZE: usize = 2048;

/// Number of spectrum bars to produce.
const NUM_BARS: usize = 48;

/// Minimum frequency (Hz).
const MIN_FREQ: f32 = 20.0;

/// Maximum frequency (Hz).
const MAX_FREQ: f32 = 18_000.0;

/// Eighth-block characters for sub-cell vertical resolution (8 levels per cell).
const EIGHTH_BLOCKS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Bark scale critical band edges (Hz). 24 bands covering 20Hz–15.5kHz.
#[allow(dead_code)]
const BARK_EDGES: &[f32] = &[
    20.0, 100.0, 200.0, 300.0, 400.0, 510.0, 630.0, 770.0, 920.0, 1080.0, 1270.0, 1480.0,
    1720.0, 2000.0, 2320.0, 2700.0, 3150.0, 3700.0, 4400.0, 5300.0, 6400.0, 7700.0, 9500.0,
    12000.0, 15500.0,
];

/// Frequency scale for spectrum analysis.
#[derive(Debug, Clone, Copy, Default)]
pub enum FrequencyScale {
    /// Bark psychoacoustic scale — 24 critical bands, best for perceiving music.
    #[default]
    Bark,
    /// Mel scale — perceptual pitch scale, popular in audio/ML.
    Mel,
    /// Logarithmic — equal spacing per octave.
    Log,
    /// Linear — equal Hz per bar, mostly for curiosity.
    Linear,
}

impl FrequencyScale {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bark" => Self::Bark,
            "mel" => Self::Mel,
            "log" | "logarithmic" => Self::Log,
            "linear" => Self::Linear,
            _ => Self::default(),
        }
    }

    /// Map a frequency (Hz) to a normalized 0.0..1.0 position on this scale.
    fn normalize(&self, freq: f32) -> f32 {
        match self {
            Self::Bark => {
                // Bark formula: z = 26.81 / (1 + 1960/f) - 0.53
                let bark = |f: f32| 26.81 / (1.0 + 1960.0 / f) - 0.53;
                let b = bark(freq);
                let b_min = bark(MIN_FREQ);
                let b_max = bark(MAX_FREQ);
                (b - b_min) / (b_max - b_min)
            }
            Self::Mel => {
                // Mel formula: m = 2595 * log10(1 + f/700)
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

/// Default half-life for bar decay in seconds.
const DEFAULT_BAR_HALF_LIFE: f32 = 0.08;

/// Default half-life for peak decay in seconds.
const DEFAULT_PEAK_HALF_LIFE: f32 = 0.35;

/// dB floor for normalization: magnitudes below this map to 0.0.
const DB_FLOOR: f32 = -80.0;

/// dB ceiling for normalization: magnitudes at or above this map to 1.0.
const DB_CEIL: f32 = 0.0;

/// Precomputed Hann window coefficients for the FFT window size.
fn hann_window() -> Vec<f32> {
    (0..FFT_SIZE)
        .map(|i| {
            let t = std::f32::consts::PI * 2.0 * i as f32 / FFT_SIZE as f32;
            0.5 * (1.0 - t.cos())
        })
        .collect()
}

/// Pre-compute which bar index each FFT bin maps to for a given sample rate and scale.
/// Returns None for bins outside [MIN_FREQ, MAX_FREQ].
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

/// Processed visualizer data, ready for rendering.
pub struct VisualizerState {
    /// Current spectrum bar values (0.0..1.0), one per bar.
    pub spectrum: Vec<f32>,
    /// Previous frame's spectrum for smoothing/decay.
    prev_spectrum: Vec<f32>,
    /// Peak hold values (slowly decaying maxima).
    pub peaks: Vec<f32>,
    /// RMS levels for VU meters: [left, right].
    pub vu_levels: [f32; 2],
    /// Precomputed Hann window.
    window: Vec<f32>,
    /// FFT scratch buffer (windowed time-domain samples).
    fft_input: Vec<f32>,
    /// FFT output buffer (complex frequency-domain).
    fft_output: Vec<realfft::num_complex::Complex<f32>>,
    /// RealFft planner (cached for reuse).
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    /// Pre-computed bin→bar mapping (rebuilt when sample rate changes).
    bin_to_bar: Vec<Option<usize>>,
    /// Last seen sample rate (to detect changes and rebuild mapping).
    last_sample_rate: f32,
    /// Reusable scratch: how many bins contributed to each bar.
    bar_counts: Vec<u32>,
    /// Last update timestamp for time-based decay.
    last_update: Instant,
    /// Frequency scale for bin→bar mapping.
    scale: FrequencyScale,
    /// Bar decay half-life in seconds (configurable).
    bar_half_life: f32,
    /// Peak decay half-life in seconds (configurable).
    peak_half_life: f32,
}

impl VisualizerState {
    pub fn new() -> Self {
        Self::with_config(FrequencyScale::default(), DEFAULT_BAR_HALF_LIFE, DEFAULT_PEAK_HALF_LIFE)
    }

    pub fn from_config(cfg: &koan_core::config::VisualizerConfig) -> Self {
        let scale = FrequencyScale::from_str(&cfg.scale);
        let bar_half_life = cfg.bar_decay_ms as f32 / 1000.0;
        let peak_half_life = cfg.peak_decay_ms as f32 / 1000.0;
        Self::with_config(scale, bar_half_life, peak_half_life)
    }

    pub fn with_config(scale: FrequencyScale, bar_half_life: f32, peak_half_life: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let fft_input = fft.make_input_vec();
        let fft_output = fft.make_output_vec();

        Self {
            spectrum: vec![0.0; NUM_BARS],
            prev_spectrum: vec![0.0; NUM_BARS],
            peaks: vec![0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            window: hann_window(),
            fft_input,
            fft_output,
            fft,
            bin_to_bar: Vec::new(),
            last_sample_rate: 0.0,
            bar_counts: vec![0u32; NUM_BARS],
            last_update: Instant::now(),
            scale,
            bar_half_life,
            peak_half_life,
        }
    }

    /// Compute decay factors from elapsed time since last update.
    fn decay_factors(&mut self) -> (f32, f32) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;
        // decay = 0.5^(dt / half_life)
        let bar_decay = 0.5f32.powf(dt / self.bar_half_life);
        let peak_decay = 0.5f32.powf(dt / self.peak_half_life);
        (bar_decay, peak_decay)
    }

    /// Number of bars in the spectrum.
    /// Used by the spectrum rendering widget (Phase 3).
    #[allow(dead_code)]
    pub fn num_bars(&self) -> usize {
        NUM_BARS
    }

    /// Snapshot the viz buffer, run FFT, and update spectrum bars.
    ///
    /// Called once per tick (~20fps) from `handle_tick()`.
    pub fn update_spectrum(&mut self, viz_buffer: &VizBuffer) {
        let snapshot = viz_buffer.snapshot();
        let channels = viz_buffer.channels().max(1) as usize;
        let sample_rate = viz_buffer.sample_rate() as f32;

        if snapshot.is_empty() || sample_rate <= 0.0 {
            return;
        }

        // Compute VU levels (RMS per channel) from the snapshot.
        self.update_vu(&snapshot, channels);

        // Mix to mono: average all channels for FFT input.
        // Take the most recent FFT_SIZE frames from the snapshot.
        let total_frames = snapshot.len() / channels;
        let frames_to_use = total_frames.min(FFT_SIZE);
        let frame_start = total_frames - frames_to_use;

        for i in 0..FFT_SIZE {
            if i < frames_to_use {
                let frame_idx = frame_start + i;
                let sample_start = frame_idx * channels;
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    if sample_start + ch < snapshot.len() {
                        sum += snapshot[sample_start + ch];
                    }
                }
                self.fft_input[i] = (sum / channels as f32) * self.window[i];
            } else {
                // Zero-pad if we have fewer samples than FFT_SIZE.
                self.fft_input[i] = 0.0;
            }
        }

        // Run the FFT.
        if self
            .fft
            .process(&mut self.fft_input, &mut self.fft_output)
            .is_err()
        {
            return;
        }

        // Rebuild bin→bar lookup when sample rate changes.
        if (sample_rate - self.last_sample_rate).abs() > 0.5 {
            self.bin_to_bar = build_bin_to_bar(sample_rate, self.scale);
            self.last_sample_rate = sample_rate;
        }

        // Save current spectrum as previous for smoothing.
        std::mem::swap(&mut self.spectrum, &mut self.prev_spectrum);

        // Reset spectrum and counts.
        for bar in self.spectrum.iter_mut() {
            *bar = 0.0;
        }
        for c in self.bar_counts.iter_mut() {
            *c = 0;
        }

        // Map FFT bins to bars using pre-computed lookup (no log/division per bin).
        let norm = 2.0 / FFT_SIZE as f32;
        let db_range_inv = 1.0 / (DB_CEIL - DB_FLOOR);
        let num_bins = self.fft_output.len().min(self.bin_to_bar.len());
        for bin_idx in 0..num_bins {
            let bar_idx = match self.bin_to_bar[bin_idx] {
                Some(b) => b,
                None => continue,
            };

            let c = self.fft_output[bin_idx];
            let mag_sq = c.re * c.re + c.im * c.im;
            // Use 10*log10(mag_sq) instead of 20*log10(sqrt(mag_sq)) — avoids sqrt.
            let magnitude = mag_sq.sqrt() * norm;
            let db = if magnitude > 0.0 {
                20.0 * magnitude.log10()
            } else {
                DB_FLOOR
            };
            let level = ((db - DB_FLOOR) * db_range_inv).clamp(0.0, 1.0).powf(0.4);

            if level > self.spectrum[bar_idx] {
                self.spectrum[bar_idx] = level;
            }
            self.bar_counts[bar_idx] += 1;
        }

        // Interpolate bars that got no FFT bins (gaps in log mapping).
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

        // Apply time-based smoothing: decay rate is independent of frame rate.
        let (bar_decay, peak_decay) = self.decay_factors();
        for i in 0..NUM_BARS {
            let decayed = self.prev_spectrum[i] * bar_decay;
            self.spectrum[i] = self.spectrum[i].max(decayed);

            // Update peak hold (slower decay).
            if self.spectrum[i] > self.peaks[i] {
                self.peaks[i] = self.spectrum[i];
            } else {
                self.peaks[i] *= peak_decay;
            }
        }
    }

    /// Apply decay smoothing without new FFT input (called when paused).
    ///
    /// Feeds silence into the smoothing pass so bars gracefully fall to zero.
    pub fn decay_to_zero(&mut self) {
        let (bar_decay, peak_decay) = self.decay_factors();
        for i in 0..NUM_BARS {
            self.spectrum[i] *= bar_decay;
            self.peaks[i] *= peak_decay;
        }
        for v in self.vu_levels.iter_mut() {
            *v *= bar_decay;
        }
    }

    /// Compute RMS levels per channel for VU meters.
    fn update_vu(&mut self, snapshot: &[f32], channels: usize) {
        if channels == 0 || snapshot.is_empty() {
            self.vu_levels = [0.0; 2];
            return;
        }

        // Use the last ~2048 frames for VU calculation.
        let total_frames = snapshot.len() / channels;
        let frames_to_use = total_frames.min(2048);
        let frame_start = total_frames - frames_to_use;

        let mut sum_sq = [0.0f64; 2];
        let vu_channels = channels.min(2);

        for frame in 0..frames_to_use {
            let idx = (frame_start + frame) * channels;
            for ch in 0..vu_channels {
                if idx + ch < snapshot.len() {
                    let s = snapshot[idx + ch] as f64;
                    sum_sq[ch] += s * s;
                }
            }
        }

        for (ch, &sq) in sum_sq.iter().enumerate().take(vu_channels) {
            let rms = (sq / frames_to_use as f64).sqrt() as f32;
            // Convert to dB and normalize to 0.0..1.0.
            let db = if rms > 0.0 {
                20.0 * rms.log10()
            } else {
                DB_FLOOR
            };
            self.vu_levels[ch] = ((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR)).clamp(0.0, 1.0);
        }

        // Mono: duplicate left to right.
        if vu_channels == 1 {
            self.vu_levels[1] = self.vu_levels[0];
        }
    }
}

/// 80s hi-fi LED-segment spectrum analyzer widget.
pub struct SpectrumWidget<'a> {
    state: &'a VisualizerState,
    theme: &'a Theme,
}

impl<'a> SpectrumWidget<'a> {
    pub fn new(state: &'a VisualizerState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for SpectrumWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let num_bands = self.state.spectrum.len();
        if num_bands == 0 {
            return;
        }

        let height = area.height as f32;

        // Each bar is 1 column wide with a 1-column gap: bar, gap, bar, gap...
        // This gives the retro LED-segment look.
        let num_display_bars = (area.width as usize).div_ceil(2);
        if num_display_bars == 0 {
            return;
        }

        for bar_idx in 0..num_display_bars {
            let x = area.x + (bar_idx as u16) * 2;
            if x >= area.x + area.width {
                break;
            }

            // Map this display bar to spectrum band(s).
            let (bar_val, peak_val) = if num_display_bars <= num_bands {
                // Downsample: average bands in this bucket.
                let start = bar_idx * num_bands / num_display_bars;
                let end = ((bar_idx + 1) * num_bands / num_display_bars).max(start + 1);
                let count = end - start;
                let bv = self.state.spectrum[start..end].iter().sum::<f32>() / count as f32;
                let pv = self.state.peaks[start..end].iter().sum::<f32>() / count as f32;
                (bv, pv)
            } else {
                // Upsample: interpolate between adjacent bands.
                let t = if num_display_bars > 1 {
                    bar_idx as f32 * (num_bands - 1) as f32 / (num_display_bars - 1) as f32
                } else {
                    0.0
                };
                let lo = t.floor() as usize;
                let hi = (lo + 1).min(num_bands - 1);
                let frac = t - lo as f32;
                let bv = self.state.spectrum[lo] * (1.0 - frac) + self.state.spectrum[hi] * frac;
                let pv = self.state.peaks[lo] * (1.0 - frac) + self.state.peaks[hi] * frac;
                (bv, pv)
            };

            // Bar height in eighth-cells for sub-cell resolution (8 levels per cell).
            let eighths = (bar_val * height * 8.0).round() as usize;

            // Peak position in eighths from bottom.
            let peak_eighths = (peak_val * height * 8.0).round() as usize;

            // Render from bottom to top.
            for row in 0..area.height {
                let cell_from_bottom = (area.height - 1 - row) as usize;
                let y = area.y + row;

                // How many eighths fall within this cell?
                let cell_base = cell_from_bottom * 8;
                let fill = eighths.saturating_sub(cell_base).min(8);

                // Color based on position relative to total height.
                let pos_ratio = cell_from_bottom as f32 / height;
                let style = if pos_ratio < 0.33 {
                    self.theme.spectrum_low
                } else if pos_ratio < 0.66 {
                    self.theme.spectrum_mid
                } else {
                    self.theme.spectrum_high
                };

                if fill > 0 {
                    buf[(x, y)]
                        .set_char(EIGHTH_BLOCKS[fill])
                        .set_style(style);
                } else {
                    // Check for peak marker in this cell.
                    let peak_cell = peak_eighths / 8;
                    if peak_cell == cell_from_bottom && peak_eighths > eighths {
                        buf[(x, y)]
                            .set_char('▔')
                            .set_style(self.theme.spectrum_peak);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visualizer_state_initializes() {
        let state = VisualizerState::new();
        assert_eq!(state.spectrum.len(), NUM_BARS);
        assert_eq!(state.peaks.len(), NUM_BARS);
        assert_eq!(state.vu_levels, [0.0, 0.0]);
    }

    #[test]
    fn update_spectrum_with_silence() {
        let mut state = VisualizerState::new();
        let viz = VizBuffer::new();

        // Buffer is all zeros (silence).
        state.update_spectrum(&viz);

        // All bars should be zero or near-zero.
        for &bar in &state.spectrum {
            assert!(bar <= 0.01, "expected near-zero, got {}", bar);
        }
    }

    #[test]
    fn update_spectrum_with_sine_wave() {
        let mut state = VisualizerState::new();
        let viz = VizBuffer::new();

        // Generate a 440Hz sine wave (stereo, 44100Hz).
        let sample_rate = 44100u32;
        let channels = 2u16;
        let num_frames = 4096;
        let mut samples = Vec::with_capacity(num_frames * channels as usize);
        for i in 0..num_frames {
            let t = i as f32 / sample_rate as f32;
            let val = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
            samples.push(val); // left
            samples.push(val); // right
        }
        viz.push_samples(&samples, channels, sample_rate);

        state.update_spectrum(&viz);

        // At least one bar should be non-zero (the bar containing 440Hz).
        let max_bar = state.spectrum.iter().cloned().fold(0.0f32, f32::max);
        assert!(max_bar > 0.1, "expected some energy, max bar = {}", max_bar);

        // VU levels should be non-zero.
        assert!(state.vu_levels[0] > 0.0);
        assert!(state.vu_levels[1] > 0.0);
    }

    #[test]
    fn smoothing_decays_over_time() {
        let mut state = VisualizerState::new();
        let viz = VizBuffer::new();

        // Push a loud sine wave.
        let sample_rate = 44100u32;
        let num_frames = 4096;
        let mut samples = Vec::with_capacity(num_frames * 2);
        for i in 0..num_frames {
            let t = i as f32 / sample_rate as f32;
            let val = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            samples.push(val);
            samples.push(val);
        }
        viz.push_samples(&samples, 2, sample_rate);

        state.update_spectrum(&viz);
        let initial_max = state.spectrum.iter().cloned().fold(0.0f32, f32::max);

        // Push silence and simulate 20 frames at ~60fps (16ms apart).
        // We fake elapsed time by rewinding last_update before each call.
        viz.push_samples(&vec![0.0; num_frames * 2], 2, sample_rate);
        let frame_dt = std::time::Duration::from_millis(16);
        for _ in 0..20 {
            state.last_update = Instant::now() - frame_dt;
            state.update_spectrum(&viz);
        }

        let final_max = state.spectrum.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            final_max < initial_max * 0.1,
            "expected significant decay: initial={}, final={}",
            initial_max,
            final_max
        );
    }

    #[test]
    fn hann_window_is_correct_size() {
        let w = hann_window();
        assert_eq!(w.len(), FFT_SIZE);
        // Hann window: 0 at edges, 1 at center.
        assert!(w[0].abs() < 0.001);
        assert!((w[FFT_SIZE / 2] - 1.0).abs() < 0.001);
    }
}
