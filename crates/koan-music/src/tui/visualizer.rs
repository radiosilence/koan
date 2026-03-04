use koan_core::audio::viz::VizBuffer;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use realfft::RealFftPlanner;

use super::theme::Theme;

/// FFT window size: 2048 samples (~46ms at 44.1kHz).
const FFT_SIZE: usize = 2048;

/// Number of spectrum bars to produce.
const NUM_BARS: usize = 64;

/// Minimum frequency for the logarithmic mapping (Hz).
const MIN_FREQ: f32 = 20.0;

/// Maximum frequency for the logarithmic mapping (Hz).
const MAX_FREQ: f32 = 20_000.0;

/// Smoothing decay factor: bars fall at this rate per tick.
/// Higher = slower fall. At 20fps, 0.85 gives ~150ms to half-value.
const DECAY_FACTOR: f32 = 0.85;

/// Peak hold decay factor (slower than bars for the "sticky peak" effect).
const PEAK_DECAY: f32 = 0.95;

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
}

impl VisualizerState {
    pub fn new() -> Self {
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
        }
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

        // Map FFT bins to logarithmic frequency bars.
        let bin_hz = sample_rate / FFT_SIZE as f32;
        let log_min = MIN_FREQ.ln();
        let log_max = MAX_FREQ.ln();

        // Save current spectrum as previous for smoothing.
        std::mem::swap(&mut self.spectrum, &mut self.prev_spectrum);

        // Reset spectrum bars to zero before accumulating.
        for bar in self.spectrum.iter_mut() {
            *bar = 0.0;
        }

        // Count how many bins contribute to each bar (for averaging).
        let mut bar_counts = vec![0u32; NUM_BARS];

        let num_bins = self.fft_output.len();
        for bin_idx in 0..num_bins {
            let freq = bin_idx as f32 * bin_hz;
            if !(MIN_FREQ..=MAX_FREQ).contains(&freq) {
                continue;
            }

            let log_freq = freq.ln();
            let normalized = (log_freq - log_min) / (log_max - log_min);
            let bar_idx = ((normalized * NUM_BARS as f32) as usize).min(NUM_BARS - 1);

            // Magnitude in dB (normalized by FFT size for proper scaling).
            let c = self.fft_output[bin_idx];
            let magnitude = (c.re * c.re + c.im * c.im).sqrt() / (FFT_SIZE as f32 / 2.0);
            let db = if magnitude > 0.0 {
                20.0 * magnitude.log10()
            } else {
                DB_FLOOR
            };
            let level = ((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR))
                .clamp(0.0, 1.0)
                .powf(0.4);

            // Take the max of all bins mapping to this bar.
            if level > self.spectrum[bar_idx] {
                self.spectrum[bar_idx] = level;
            }
            bar_counts[bar_idx] += 1;
        }

        // Interpolate bars that got no FFT bins (gaps in log mapping).
        for i in 0..NUM_BARS {
            if bar_counts[i] == 0 {
                let left = if i > 0 { self.spectrum[i - 1] } else { 0.0 };
                let right = if i + 1 < NUM_BARS {
                    self.spectrum[i + 1]
                } else {
                    0.0
                };
                self.spectrum[i] = (left + right) * 0.5;
            }
        }

        // Apply smoothing: bar falls at DECAY_FACTOR per tick, jumps up instantly.
        for i in 0..NUM_BARS {
            let decayed = self.prev_spectrum[i] * DECAY_FACTOR;
            self.spectrum[i] = self.spectrum[i].max(decayed);

            // Update peak hold (slower decay).
            if self.spectrum[i] > self.peaks[i] {
                self.peaks[i] = self.spectrum[i];
            } else {
                self.peaks[i] *= PEAK_DECAY;
            }
        }
    }

    /// Apply decay smoothing without new FFT input (called when paused).
    ///
    /// Feeds silence into the smoothing pass so bars gracefully fall to zero.
    pub fn decay_to_zero(&mut self) {
        for i in 0..NUM_BARS {
            self.spectrum[i] *= DECAY_FACTOR;
            self.peaks[i] *= PEAK_DECAY;
        }
        for v in self.vu_levels.iter_mut() {
            *v *= DECAY_FACTOR;
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

            // Bar height in half-cells for sub-cell resolution using ▄ half blocks.
            let half_cells = (bar_val * height * 2.0).round() as usize;

            // Peak position in half-cells from bottom.
            let peak_half = (peak_val * height * 2.0).round() as usize;

            // Render from bottom to top.
            for row in 0..area.height {
                let cell_from_bottom = (area.height - 1 - row) as usize;
                let y = area.y + row;

                // Each cell covers 2 half-cells: bottom = cell*2, top = cell*2+1
                let bottom_half = cell_from_bottom * 2;
                let top_half = bottom_half + 1;
                let has_bottom = bottom_half < half_cells;
                let has_top = top_half < half_cells;

                // Color based on position relative to total height.
                let pos_ratio = cell_from_bottom as f32 / height;
                let style = if pos_ratio < 0.33 {
                    self.theme.spectrum_low
                } else if pos_ratio < 0.66 {
                    self.theme.spectrum_mid
                } else {
                    self.theme.spectrum_high
                };

                if has_bottom && has_top {
                    buf[(x, y)].set_char('█').set_style(style);
                } else if has_bottom {
                    // Only lower half filled — use ▄
                    buf[(x, y)].set_char('▄').set_style(style);
                } else {
                    // Check for peak marker in this cell.
                    let peak_cell = peak_half / 2;
                    if peak_cell == cell_from_bottom && peak_half > half_cells {
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

        // Now push silence and update several times.
        viz.push_samples(&vec![0.0; num_frames * 2], 2, sample_rate);
        for _ in 0..20 {
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
