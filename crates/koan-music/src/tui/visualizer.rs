use std::time::Instant;

use koan_core::audio::viz::VizSnapshot;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::theme::Theme;

/// Number of spectrum bars to produce (must match koan_core::audio::viz::NUM_BARS).
const NUM_BARS: usize = 48;

/// Eighth-block characters for sub-cell vertical resolution (8 levels per cell).
const EIGHTH_BLOCKS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Processed visualizer data, ready for rendering.
///
/// All FFT/analysis work is done on a dedicated thread in koan-core (VizAnalyzer).
/// This struct handles only decay smoothing, peak hold, and rendering on the UI thread.
///
/// Lock discipline: `update_from_snapshot` acquires the VizSnapshot RwLock for <1us
/// (clone of ~200 bytes), then does all decay/smoothing on local copies with no locks held.
pub struct VisualizerState {
    /// Current spectrum bar values (0.0..1.0), one per bar.
    pub spectrum: Vec<f32>,
    /// Previous frame's spectrum for decay smoothing.
    prev_spectrum: Vec<f32>,
    /// Peak hold values (slowly decaying maxima).
    pub peaks: Vec<f32>,
    /// RMS levels for VU meters: [left, right].
    pub vu_levels: [f32; 2],
    /// Last update timestamp for time-based decay.
    pub(crate) last_update: Instant,
    /// Bar decay half-life in seconds (configurable).
    bar_half_life: f32,
    /// Peak decay half-life in seconds (configurable).
    peak_half_life: f32,
}

impl VisualizerState {
    pub fn from_config(cfg: &koan_core::config::VisualizerConfig) -> Self {
        let bar_half_life = cfg.bar_decay_ms as f32 / 1000.0;
        let peak_half_life = cfg.peak_decay_ms as f32 / 1000.0;
        Self::with_config(bar_half_life, peak_half_life)
    }

    pub fn with_config(bar_half_life: f32, peak_half_life: f32) -> Self {
        Self {
            spectrum: vec![0.0; NUM_BARS],
            prev_spectrum: vec![0.0; NUM_BARS],
            peaks: vec![0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            last_update: Instant::now(),
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
    #[allow(dead_code)]
    pub fn num_bars(&self) -> usize {
        NUM_BARS
    }

    /// Read the latest analysis frame from VizSnapshot and apply decay/smoothing.
    ///
    /// The snapshot read is <1us (RwLock clone of ~200 bytes).
    /// All decay/smoothing runs on local copies — no locks held during computation.
    /// Called once per frame (~60fps) from `handle_tick()`.
    pub fn update_from_snapshot(&mut self, snapshot: &VizSnapshot) {
        // Acquire RwLock read, clone frame, release — total <1us.
        let frame = snapshot.read();

        // Save current spectrum as previous for smoothing.
        std::mem::swap(&mut self.spectrum, &mut self.prev_spectrum);

        // Compute time-based decay factors (no lock held).
        let (bar_decay, peak_decay) = self.decay_factors();

        let bars = self.spectrum.len().min(frame.spectrum.len());
        for i in 0..bars {
            let new_val = frame.spectrum[i];
            let decayed = self.prev_spectrum[i] * bar_decay;
            // Take the max of the fresh analysis value and the decayed previous value.
            self.spectrum[i] = new_val.max(decayed);

            // Peak hold: rise instantly, fall slowly with peak_decay.
            if self.spectrum[i] > self.peaks[i] {
                self.peaks[i] = self.spectrum[i];
            } else {
                self.peaks[i] *= peak_decay;
            }
        }

        // VU levels come directly from the analysis thread — copy as-is.
        self.vu_levels = frame.vu_levels;
    }

    /// Apply decay smoothing without new analysis input (called when paused/stopped).
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
                    buf[(x, y)].set_char(EIGHTH_BLOCKS[fill]).set_style(style);
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
    use koan_core::audio::viz::{VizFrame, VizSnapshot};

    #[test]
    fn visualizer_state_initializes() {
        let state = VisualizerState::with_config(0.045, 0.18);
        assert_eq!(state.spectrum.len(), NUM_BARS);
        assert_eq!(state.peaks.len(), NUM_BARS);
        assert_eq!(state.vu_levels, [0.0, 0.0]);
    }

    #[test]
    fn update_from_snapshot_with_silence() {
        let mut state = VisualizerState::with_config(0.045, 0.18);
        let snapshot = VizSnapshot::new();

        // Default snapshot has all zeros (silence).
        state.update_from_snapshot(&snapshot);

        for &bar in &state.spectrum {
            assert!(bar <= 0.01, "expected near-zero, got {}", bar);
        }
    }

    #[test]
    fn update_from_snapshot_with_signal() {
        let mut state = VisualizerState::with_config(0.045, 0.18);
        let snapshot = VizSnapshot::new();

        // Write a frame with some energy.
        let mut spectrum = vec![0.0f32; NUM_BARS];
        spectrum[10] = 0.8;
        spectrum[20] = 0.5;
        snapshot.write(VizFrame {
            spectrum,
            vu_levels: [0.6, 0.6],
            timestamp: Instant::now(),
        });

        state.update_from_snapshot(&snapshot);

        assert!(state.spectrum[10] > 0.5, "expected energy at bar 10");
        assert!(state.vu_levels[0] > 0.0, "expected non-zero VU");
    }

    #[test]
    fn decay_to_zero_reduces_bars() {
        let mut state = VisualizerState::with_config(0.045, 0.18);
        let snapshot = VizSnapshot::new();

        // Seed some energy.
        let spectrum = vec![1.0f32; NUM_BARS];
        snapshot.write(VizFrame {
            spectrum,
            vu_levels: [1.0, 1.0],
            timestamp: Instant::now(),
        });
        state.update_from_snapshot(&snapshot);

        let initial_max = state.spectrum.iter().cloned().fold(0.0f32, f32::max);
        assert!(initial_max > 0.5);

        // Decay many times — bars should approach zero.
        for _ in 0..100 {
            state.last_update = Instant::now() - std::time::Duration::from_millis(50);
            state.decay_to_zero();
        }

        let final_max = state.spectrum.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            final_max < 0.01,
            "expected near-zero after decay, got {}",
            final_max
        );
    }

    #[test]
    fn peak_hold_rises_and_falls() {
        let mut state = VisualizerState::with_config(0.045, 0.18);
        let snapshot = VizSnapshot::new();

        // Push a loud frame.
        let mut spectrum = vec![0.0f32; NUM_BARS];
        spectrum[5] = 0.9;
        snapshot.write(VizFrame {
            spectrum,
            vu_levels: [0.0; 2],
            timestamp: Instant::now(),
        });
        state.update_from_snapshot(&snapshot);
        let peak_after_signal = state.peaks[5];
        assert!(peak_after_signal > 0.5, "peak should rise with signal");

        // Push silence — peak should hold or slowly decay, not jump to zero.
        snapshot.write(VizFrame {
            spectrum: vec![0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            timestamp: Instant::now(),
        });
        state.last_update = Instant::now() - std::time::Duration::from_millis(16);
        state.update_from_snapshot(&snapshot);
        assert!(
            state.peaks[5] <= peak_after_signal,
            "peak should not grow after silence"
        );
        assert!(
            state.peaks[5] > 0.0,
            "peak should not instantly zero after one frame"
        );
    }
}
