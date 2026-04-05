use std::time::Instant;

use koan_core::audio::viz::VizSnapshot;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

use super::theme::Theme;

/// Number of spectrum bars to produce (must match koan_core::audio::viz::NUM_BARS).
const NUM_BARS: usize = 48;

/// Eighth-block characters for sub-cell vertical resolution (8 levels per cell).
const EIGHTH_BLOCKS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

// ── Palette ─────────────────────────────────────────────────────────────────

/// Color palette for the spectrum analyzer.
///
/// Each palette maps a normalised frequency position (0.0 = lowest bar, 1.0 = highest)
/// to an RGB color. Beat reactivity and peak glow are applied on top by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualizerPalette {
    /// Classic LED meter: green → yellow → red based on bar height (ignores frequency).
    Mono,
    /// Frequency rainbow: warm bass (red/orange) → cyan mids → purple/magenta highs.
    Spectrum,
    /// Hot: deep red bass → orange → yellow → white highs.
    Fire,
    /// Synthwave: hot pink bass → electric blue mids → cyan highs.
    Neon,
}

impl VisualizerPalette {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "mono" => Self::Mono,
            "spectrum" => Self::Spectrum,
            "fire" => Self::Fire,
            "neon" => Self::Neon,
            _ => Self::Spectrum,
        }
    }

    /// Map a normalised frequency position (0.0..1.0) to an RGB color.
    /// For `Mono`, this is unused — the renderer uses height-based coloring instead.
    fn freq_color(self, t: f32) -> Color {
        match self {
            Self::Mono => Color::Green, // fallback; actual mono uses height-based
            Self::Spectrum => {
                // Bass (red/orange) → mids (cyan/blue) → highs (purple/magenta)
                if t < 0.33 {
                    let u = t / 0.33;
                    lerp_rgb((220, 50, 20), (230, 180, 30), u)
                } else if t < 0.66 {
                    let u = (t - 0.33) / 0.33;
                    lerp_rgb((230, 180, 30), (30, 180, 220), u)
                } else {
                    let u = (t - 0.66) / 0.34;
                    lerp_rgb((30, 180, 220), (180, 60, 220), u)
                }
            }
            Self::Fire => {
                // Deep red → orange → yellow → white
                if t < 0.33 {
                    let u = t / 0.33;
                    lerp_rgb((160, 20, 10), (230, 100, 10), u)
                } else if t < 0.66 {
                    let u = (t - 0.33) / 0.33;
                    lerp_rgb((230, 100, 10), (250, 220, 50), u)
                } else {
                    let u = (t - 0.66) / 0.34;
                    lerp_rgb((250, 220, 50), (255, 255, 200), u)
                }
            }
            Self::Neon => {
                // Hot pink → electric blue → cyan
                if t < 0.5 {
                    let u = t / 0.5;
                    lerp_rgb((255, 40, 130), (60, 80, 255), u)
                } else {
                    let u = (t - 0.5) / 0.5;
                    lerp_rgb((60, 80, 255), (40, 240, 255), u)
                }
            }
        }
    }
}

/// Linear RGB interpolation between two colors.
fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::Rgb(
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * t) as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * t) as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * t) as u8,
    )
}

/// Shift an RGB color toward white by a factor (0.0 = unchanged, 1.0 = pure white).
fn brighten(color: Color, amount: f32) -> Color {
    if let Color::Rgb(r, g, b) = color {
        let a = amount.clamp(0.0, 1.0);
        Color::Rgb(
            (r as f32 + (255.0 - r as f32) * a) as u8,
            (g as f32 + (255.0 - g as f32) * a) as u8,
            (b as f32 + (255.0 - b as f32) * a) as u8,
        )
    } else {
        color
    }
}

// ── VisualizerState ─────────────────────────────────────────────────────────

/// Processed visualizer data, ready for rendering.
///
/// All FFT/analysis work is done on a dedicated thread in koan-core (VizAnalyzer).
/// This struct handles only decay smoothing, peak hold, and rendering on the UI thread.
///
/// Lock discipline: `update_from_snapshot` acquires the VizSnapshot RwLock for <1us
/// (clone of ~200 bytes), then does all decay/smoothing on local copies with no locks held.
pub struct VisualizerState {
    /// Current spectrum bar values (0.0..1.0), one per bar.
    pub spectrum: [f32; NUM_BARS],
    /// Previous frame's spectrum for decay smoothing.
    prev_spectrum: [f32; NUM_BARS],
    /// Peak hold values (slowly decaying maxima).
    pub peaks: [f32; NUM_BARS],
    /// RMS levels for VU meters: [left, right].
    pub vu_levels: [f32; 2],
    /// Beat energy from the analyzer (0.0..1.0), used for color shifts.
    pub beat_energy: f32,
    /// Last update timestamp for time-based decay.
    pub(crate) last_update: Instant,
    /// Bar decay half-life in seconds (configurable).
    bar_half_life: f32,
    /// Peak decay half-life in seconds (configurable).
    peak_half_life: f32,
    /// Color palette for rendering.
    pub palette: VisualizerPalette,
}

impl VisualizerState {
    pub fn from_config(cfg: &koan_core::config::VisualizerConfig) -> Self {
        let bar_half_life = cfg.bar_decay_ms as f32 / 1000.0;
        let peak_half_life = cfg.peak_decay_ms as f32 / 1000.0;
        let palette = VisualizerPalette::parse(&cfg.palette);
        Self::with_config(bar_half_life, peak_half_life, palette)
    }

    pub fn with_config(
        bar_half_life: f32,
        peak_half_life: f32,
        palette: VisualizerPalette,
    ) -> Self {
        Self {
            spectrum: [0.0; NUM_BARS],
            prev_spectrum: [0.0; NUM_BARS],
            peaks: [0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            beat_energy: 0.0,
            last_update: Instant::now(),
            bar_half_life,
            peak_half_life,
            palette,
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

        // Beat energy: rise instantly from analyzer, decay locally for smooth falloff.
        self.beat_energy = frame.beat_energy.max(self.beat_energy * bar_decay);
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
        self.beat_energy *= bar_decay;
    }
}

/// 80s hi-fi LED-segment spectrum analyzer widget.
///
/// Supports multiple color palettes with frequency-mapped gradients,
/// beat-reactive brightness pulses, and glowing peak markers.
pub struct SpectrumWidget<'a> {
    state: &'a VisualizerState,
    theme: &'a Theme,
}

impl<'a> SpectrumWidget<'a> {
    pub fn new(state: &'a VisualizerState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }

    /// Compute the bar fill color for a given display bar.
    ///
    /// For `Mono` palette: height-based green/yellow/red (classic LED meter).
    /// For all other palettes: frequency-mapped gradient with beat-reactive brightening.
    fn bar_color(&self, freq_t: f32, height_ratio: f32) -> Style {
        let palette = self.state.palette;
        let beat = self.state.beat_energy;

        match palette {
            VisualizerPalette::Mono => {
                // Classic LED meter: color by vertical position.
                if height_ratio < 0.60 {
                    self.theme.spectrum_low
                } else if height_ratio < 0.85 {
                    self.theme.spectrum_mid
                } else {
                    self.theme.spectrum_high
                }
            }
            _ => {
                // Frequency-mapped color from the palette.
                let base_color = palette.freq_color(freq_t);
                // Beat-reactive: brighten toward white proportional to beat energy.
                let color = brighten(base_color, beat * 0.7);
                Style::new().fg(color)
            }
        }
    }

    /// Compute the peak marker color for a given display bar.
    ///
    /// For `Mono`: white (theme default).
    /// For other palettes: brightened version of the frequency color.
    fn peak_color(&self, freq_t: f32) -> Style {
        let palette = self.state.palette;

        match palette {
            VisualizerPalette::Mono => self.theme.spectrum_peak,
            _ => {
                let base = palette.freq_color(freq_t);
                // Peaks glow brighter than bars — 60% toward white.
                let color = brighten(base, 0.6);
                Style::new().fg(color)
            }
        }
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

            // Normalised frequency position for this display bar (0.0..1.0).
            let freq_t = if num_display_bars > 1 {
                bar_idx as f32 / (num_display_bars - 1) as f32
            } else {
                0.5
            };

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

            // Pre-compute peak style for this bar.
            let peak_style = self.peak_color(freq_t);

            // Render from bottom to top.
            for row in 0..area.height {
                let cell_from_bottom = (area.height - 1 - row) as usize;
                let y = area.y + row;

                // How many eighths fall within this cell?
                let cell_base = cell_from_bottom * 8;
                let fill = eighths.saturating_sub(cell_base).min(8);

                // Height ratio for mono palette's LED-meter coloring.
                let height_ratio = cell_from_bottom as f32 / height;
                let style = self.bar_color(freq_t, height_ratio);

                // Peak marker takes priority over bar fill — it renders on
                // top like a real LED meter's hold indicator.
                let peak_cell = peak_eighths / 8;
                let is_peak_cell =
                    peak_cell == cell_from_bottom && peak_eighths >= eighths && peak_eighths > 0;

                if is_peak_cell {
                    buf[(x, y)].set_char('▔').set_style(peak_style);
                } else if fill > 0 {
                    buf[(x, y)].set_char(EIGHTH_BLOCKS[fill]).set_style(style);
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
        let state = VisualizerState::with_config(0.045, 0.18, VisualizerPalette::Spectrum);
        assert_eq!(state.spectrum.len(), NUM_BARS);
        assert_eq!(state.peaks.len(), NUM_BARS);
        assert_eq!(state.vu_levels, [0.0, 0.0]);
        assert_eq!(state.beat_energy, 0.0);
    }

    #[test]
    fn update_from_snapshot_with_silence() {
        let mut state = VisualizerState::with_config(0.045, 0.18, VisualizerPalette::Spectrum);
        let snapshot = VizSnapshot::new();

        // Default snapshot has all zeros (silence).
        state.update_from_snapshot(&snapshot);

        for &bar in &state.spectrum {
            assert!(bar <= 0.01, "expected near-zero, got {}", bar);
        }
    }

    #[test]
    fn update_from_snapshot_with_signal() {
        let mut state = VisualizerState::with_config(0.045, 0.18, VisualizerPalette::Spectrum);
        let snapshot = VizSnapshot::new();

        // Write a frame with some energy.
        let mut spectrum = [0.0f32; NUM_BARS];
        spectrum[10] = 0.8;
        spectrum[20] = 0.5;
        snapshot.write(VizFrame {
            spectrum,
            vu_levels: [0.6, 0.6],
            beat_energy: 0.3,
            timestamp: Instant::now(),
        });

        state.update_from_snapshot(&snapshot);

        assert!(state.spectrum[10] > 0.5, "expected energy at bar 10");
        assert!(state.vu_levels[0] > 0.0, "expected non-zero VU");
        assert!(state.beat_energy > 0.0, "expected non-zero beat energy");
    }

    #[test]
    fn decay_to_zero_reduces_bars() {
        let mut state = VisualizerState::with_config(0.045, 0.18, VisualizerPalette::Spectrum);
        let snapshot = VizSnapshot::new();

        // Seed some energy.
        let spectrum = [1.0f32; NUM_BARS];
        snapshot.write(VizFrame {
            spectrum,
            vu_levels: [1.0, 1.0],
            beat_energy: 0.8,
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
        assert!(
            state.beat_energy < 0.01,
            "expected beat energy near-zero after decay, got {}",
            state.beat_energy
        );
    }

    #[test]
    fn peak_hold_rises_and_falls() {
        let mut state = VisualizerState::with_config(0.045, 0.18, VisualizerPalette::Spectrum);
        let snapshot = VizSnapshot::new();

        // Push a loud frame.
        let mut spectrum = [0.0f32; NUM_BARS];
        spectrum[5] = 0.9;
        snapshot.write(VizFrame {
            spectrum,
            vu_levels: [0.0; 2],
            beat_energy: 0.0,
            timestamp: Instant::now(),
        });
        state.update_from_snapshot(&snapshot);
        let peak_after_signal = state.peaks[5];
        assert!(peak_after_signal > 0.5, "peak should rise with signal");

        // Push silence — peak should hold or slowly decay, not jump to zero.
        snapshot.write(VizFrame {
            spectrum: [0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            beat_energy: 0.0,
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

    #[test]
    fn palette_parse_variants() {
        assert_eq!(VisualizerPalette::parse("mono"), VisualizerPalette::Mono);
        assert_eq!(
            VisualizerPalette::parse("spectrum"),
            VisualizerPalette::Spectrum
        );
        assert_eq!(VisualizerPalette::parse("fire"), VisualizerPalette::Fire);
        assert_eq!(VisualizerPalette::parse("neon"), VisualizerPalette::Neon);
        assert_eq!(VisualizerPalette::parse("FIRE"), VisualizerPalette::Fire);
        // Unknown falls back to spectrum.
        assert_eq!(
            VisualizerPalette::parse("garbage"),
            VisualizerPalette::Spectrum
        );
    }

    #[test]
    fn palette_freq_color_produces_distinct_colors() {
        // Spectrum palette should give different colors at different frequency positions.
        let low = VisualizerPalette::Spectrum.freq_color(0.0);
        let mid = VisualizerPalette::Spectrum.freq_color(0.5);
        let high = VisualizerPalette::Spectrum.freq_color(1.0);
        assert_ne!(low, mid, "low and mid should differ");
        assert_ne!(mid, high, "mid and high should differ");
    }

    #[test]
    fn brighten_produces_lighter_color() {
        let dark = Color::Rgb(100, 50, 20);
        let bright = brighten(dark, 0.5);
        if let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (dark, bright) {
            assert!(r2 > r1, "red should increase");
            assert!(g2 > g1, "green should increase");
            assert!(b2 > b1, "blue should increase");
        } else {
            panic!("expected Rgb colors");
        }
    }

    #[test]
    fn brighten_at_zero_is_identity() {
        let c = Color::Rgb(100, 150, 200);
        assert_eq!(brighten(c, 0.0), c);
    }

    #[test]
    fn brighten_at_one_is_white() {
        let c = Color::Rgb(100, 150, 200);
        assert_eq!(brighten(c, 1.0), Color::Rgb(255, 255, 255));
    }
}
