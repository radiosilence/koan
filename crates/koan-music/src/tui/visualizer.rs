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

// ── Visualizer Mode ────────────────────────────────────────────────────────

/// Active visualizer rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualizerMode {
    /// Classic LED-segment spectrum bars (default).
    Bars,
    /// Raw PCM waveform drawn as a continuous braille line.
    Oscilloscope,
    /// Spectrum bars mapped to polar coordinates — radial starburst.
    Radial,
    /// Frequency-driven particle system with physics sim.
    Particles,
    /// Stereo phase scope — L channel vs R channel as X/Y coordinates.
    Lissajous,
}

impl VisualizerMode {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bars" | "spectrum" => Self::Bars,
            "oscilloscope" | "scope" => Self::Oscilloscope,
            "radial" => Self::Radial,
            "particles" | "particle" => Self::Particles,
            "lissajous" | "phase" => Self::Lissajous,
            _ => Self::Bars,
        }
    }

    /// Cycle to the next mode.
    pub fn next(self) -> Self {
        match self {
            Self::Bars => Self::Oscilloscope,
            Self::Oscilloscope => Self::Radial,
            Self::Radial => Self::Particles,
            Self::Particles => Self::Lissajous,
            Self::Lissajous => Self::Bars,
        }
    }

    /// Config string for persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bars => "bars",
            Self::Oscilloscope => "oscilloscope",
            Self::Radial => "radial",
            Self::Particles => "particles",
            Self::Lissajous => "lissajous",
        }
    }

    /// Human-readable label for status messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bars => "bars",
            Self::Oscilloscope => "oscilloscope",
            Self::Radial => "radial",
            Self::Particles => "particles",
            Self::Lissajous => "lissajous",
        }
    }
}

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
    pub fn freq_color(self, t: f32) -> Color {
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

/// Dim an RGB color toward black by a factor (0.0 = unchanged, 1.0 = pure black).
fn dim(color: Color, amount: f32) -> Color {
    if let Color::Rgb(r, g, b) = color {
        let a = amount.clamp(0.0, 1.0);
        Color::Rgb(
            (r as f32 * (1.0 - a)) as u8,
            (g as f32 * (1.0 - a)) as u8,
            (b as f32 * (1.0 - a)) as u8,
        )
    } else {
        color
    }
}

// ── BrailleGrid ─────────────────────────────────────────────────────────────

/// Braille character subpixel grid.
///
/// Each terminal cell maps to one Unicode braille character (U+2800..U+28FF)
/// giving 2x4 subpixel resolution per cell. Color is per-cell (terminal limitation).
///
/// Braille dot layout per cell:
/// ```text
///   0 3
///   1 4
///   2 5
///   6 7
/// ```
/// Bit 0 = dot 1 (top-left), bit 3 = dot 4 (top-right), etc.
pub struct BrailleGrid {
    /// Terminal cell dimensions.
    width: usize,
    height: usize,
    /// 8 bits per cell — braille dot pattern.
    dots: Vec<u8>,
    /// One color per cell. Last write wins (per-cell limitation).
    colors: Vec<Color>,
}

impl BrailleGrid {
    /// Create a new grid sized for the given terminal area.
    pub fn new(width: usize, height: usize) -> Self {
        let cells = width * height;
        Self {
            width,
            height,
            dots: vec![0; cells],
            colors: vec![Color::Reset; cells],
        }
    }

    /// Pixel dimensions (subpixel resolution).
    pub fn px_width(&self) -> usize {
        self.width * 2
    }

    pub fn px_height(&self) -> usize {
        self.height * 4
    }

    /// Set a single subpixel dot at pixel coordinates (px, py).
    /// Returns false if out of bounds.
    pub fn set_dot(&mut self, px: usize, py: usize, color: Color) -> bool {
        if px >= self.px_width() || py >= self.px_height() {
            return false;
        }
        let cell_x = px / 2;
        let cell_y = py / 4;
        let sub_x = px % 2;
        let sub_y = py % 4;
        let bit = braille_bit(sub_x, sub_y);
        let idx = cell_y * self.width + cell_x;
        self.dots[idx] |= bit;
        self.colors[idx] = color;
        true
    }

    /// Draw a line between two subpixel points using Bresenham's algorithm.
    pub fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
        let mut x = x0;
        let mut y = y0;
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1.0 } else { -1.0 };
        let sy = if y0 < y1 { 1.0 } else { -1.0 };
        let steps = dx.max(dy).ceil() as usize;
        if steps == 0 {
            self.set_dot(x0 as usize, y0 as usize, color);
            return;
        }
        let step_x = (x1 - x0) / steps as f32;
        let step_y = (y1 - y0) / steps as f32;
        for _ in 0..=steps {
            let ix = x.round() as usize;
            let iy = y.round() as usize;
            self.set_dot(ix, iy, color);
            x += step_x;
            y += step_y;
        }
        // Ignore sx/sy warnings — they're used conceptually but the step-based
        // approach handles direction via step_x/step_y.
        let _ = (sx, sy);
    }

    /// Render the braille grid into a ratatui Buffer at the given area.
    pub fn render_to(&self, area: Rect, buf: &mut Buffer) {
        for cy in 0..self.height.min(area.height as usize) {
            for cx in 0..self.width.min(area.width as usize) {
                let idx = cy * self.width + cx;
                let pattern = self.dots[idx];
                if pattern == 0 {
                    continue;
                }
                let ch = char::from_u32(0x2800 + pattern as u32).unwrap_or(' ');
                let x = area.x + cx as u16;
                let y = area.y + cy as u16;
                buf[(x, y)]
                    .set_char(ch)
                    .set_style(Style::new().fg(self.colors[idx]));
            }
        }
    }
}

/// Map subpixel position within a cell to the braille bit index.
/// Layout: col 0 = bits 0,1,2,6 (top to bottom), col 1 = bits 3,4,5,7.
fn braille_bit(sub_x: usize, sub_y: usize) -> u8 {
    match (sub_x, sub_y) {
        (0, 0) => 1 << 0,
        (0, 1) => 1 << 1,
        (0, 2) => 1 << 2,
        (0, 3) => 1 << 6,
        (1, 0) => 1 << 3,
        (1, 1) => 1 << 4,
        (1, 2) => 1 << 5,
        (1, 3) => 1 << 7,
        _ => 0,
    }
}

// ── Particle System ─────────────────────────────────────────────────────────

/// Maximum active particles at any time.
const MAX_PARTICLES: usize = 2000;

/// A single particle in the frequency-driven particle system.
#[derive(Clone)]
struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// Remaining lifetime (0.0..1.0). Particle dies at 0.
    life: f32,
    /// Normalized frequency of the source band (0.0..1.0) — for coloring.
    freq_t: f32,
}

/// Particle system state. Persists across frames.
pub struct ParticleSystem {
    particles: Vec<Particle>,
}

impl ParticleSystem {
    pub fn new() -> Self {
        Self {
            particles: Vec::with_capacity(MAX_PARTICLES),
        }
    }

    /// Emit new particles from spectrum bands and step physics.
    pub fn update(
        &mut self,
        spectrum: &[f32; NUM_BARS],
        beat_energy: f32,
        px_width: f32,
        px_height: f32,
        dt: f32,
    ) {
        // Physics step for existing particles.
        let gravity = px_height * 0.3; // Gentle downward pull.
        for p in self.particles.iter_mut() {
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.vy += gravity * dt;
            p.life -= dt * 1.5; // ~0.67s lifetime.
        }
        // Remove dead particles.
        self.particles.retain(|p| p.life > 0.0);

        // Emit new particles from high-energy bands.
        let emit_center_x = px_width / 2.0;
        let emit_y = px_height * 0.85; // Emit from bottom area.
        let beat_boost = 1.0 + beat_energy * 3.0;

        for (i, &energy) in spectrum.iter().enumerate() {
            if energy < 0.15 {
                continue;
            }
            let freq_t = i as f32 / (NUM_BARS - 1) as f32;
            // Higher energy = more particles per frame.
            let emit_count = ((energy * beat_boost * 2.0) as usize).min(3);
            for _ in 0..emit_count {
                if self.particles.len() >= MAX_PARTICLES {
                    break;
                }
                // Spread across X based on frequency position.
                let spread = (freq_t - 0.5) * px_width * 0.6;
                let angle_spread = (freq_t - 0.5) * 0.8;
                // Velocity: upward with some horizontal scatter.
                let speed = px_height * (0.4 + energy * 0.6) * beat_boost;
                let vx = speed * angle_spread + spread * 0.1;
                let vy = -speed * (0.6 + energy * 0.4);
                self.particles.push(Particle {
                    x: emit_center_x + spread,
                    y: emit_y,
                    vx,
                    vy,
                    life: 1.0,
                    freq_t,
                });
            }
        }
    }

    /// Render particles onto a braille grid.
    pub fn render(&self, grid: &mut BrailleGrid, palette: VisualizerPalette, beat: f32) {
        for p in &self.particles {
            let ix = p.x as usize;
            let iy = p.y as usize;
            if ix < grid.px_width() && iy < grid.px_height() {
                let base = palette.freq_color(p.freq_t);
                let color = dim(brighten(base, beat * 0.5), 1.0 - p.life);
                grid.set_dot(ix, iy, color);
            }
        }
    }
}

// ── Lissajous Trail ─────────────────────────────────────────────────────────

/// Number of trail frames for the afterglow effect.
const LISSAJOUS_TRAIL_FRAMES: usize = 4;

/// Stored trail of previous lissajous point sets for afterglow.
pub struct LissajousTrail {
    /// Ring buffer of past frames' point sets (newest last).
    frames: Vec<Vec<(usize, usize)>>,
    write_idx: usize,
}

impl LissajousTrail {
    pub fn new() -> Self {
        Self {
            frames: (0..LISSAJOUS_TRAIL_FRAMES).map(|_| Vec::new()).collect(),
            write_idx: 0,
        }
    }

    /// Push a new set of points. Old frames dim as afterglow.
    pub fn push(&mut self, points: Vec<(usize, usize)>) {
        self.frames[self.write_idx] = points;
        self.write_idx = (self.write_idx + 1) % LISSAJOUS_TRAIL_FRAMES;
    }

    /// Render all trail frames onto a braille grid with fading.
    pub fn render(&self, grid: &mut BrailleGrid, palette: VisualizerPalette, beat: f32) {
        for age in 0..LISSAJOUS_TRAIL_FRAMES {
            // Oldest frame = highest dim, newest = brightest.
            let frame_idx = (self.write_idx + age) % LISSAJOUS_TRAIL_FRAMES;
            let brightness = (age + 1) as f32 / LISSAJOUS_TRAIL_FRAMES as f32;
            let color_t = 0.3 + brightness * 0.7;
            let base = palette.freq_color(color_t);
            let color = dim(brighten(base, beat * 0.3), 1.0 - brightness);
            for &(px, py) in &self.frames[frame_idx] {
                grid.set_dot(px, py, color);
            }
        }
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
    /// Accumulated hue offset from beats (wraps 0.0..1.0). Jumps on beat, decays back.
    pub beat_hue_offset: f32,
    /// Creation time — used for the slow dreamy color drift.
    created_at: Instant,
    /// Last update timestamp for time-based decay.
    pub(crate) last_update: Instant,
    /// Bar decay half-life in seconds (configurable).
    bar_half_life: f32,
    /// Peak decay half-life in seconds (configurable).
    peak_half_life: f32,
    /// Color palette for rendering.
    pub palette: VisualizerPalette,
    /// Active visualizer mode.
    pub mode: VisualizerMode,
    /// Latest raw waveform samples (interleaved stereo) from VizFrame.
    pub waveform: Vec<f32>,
    /// Particle system state (persists across frames).
    pub particles: ParticleSystem,
    /// Lissajous afterglow trail.
    pub lissajous_trail: LissajousTrail,
    /// Radial rotation angle (radians), slowly drifts.
    pub radial_angle: f32,
}

impl VisualizerState {
    pub fn from_config(cfg: &koan_core::config::VisualizerConfig) -> Self {
        let bar_half_life = cfg.bar_decay_ms as f32 / 1000.0;
        let peak_half_life = cfg.peak_decay_ms as f32 / 1000.0;
        let palette = VisualizerPalette::parse(&cfg.palette);
        let mode = VisualizerMode::parse(&cfg.mode);
        Self::with_config(bar_half_life, peak_half_life, palette, mode)
    }

    pub fn with_config(
        bar_half_life: f32,
        peak_half_life: f32,
        palette: VisualizerPalette,
        mode: VisualizerMode,
    ) -> Self {
        Self {
            spectrum: [0.0; NUM_BARS],
            prev_spectrum: [0.0; NUM_BARS],
            peaks: [0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            beat_energy: 0.0,
            beat_hue_offset: 0.0,
            created_at: Instant::now(),
            last_update: Instant::now(),
            bar_half_life,
            peak_half_life,
            palette,
            mode,
            waveform: Vec::new(),
            particles: ParticleSystem::new(),
            lissajous_trail: LissajousTrail::new(),
            radial_angle: 0.0,
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
    /// The snapshot read is <1us (RwLock clone of ~200 bytes + waveform vec).
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
        let prev_beat = self.beat_energy;
        self.beat_energy = frame.beat_energy.max(self.beat_energy * bar_decay);

        // Beat hue shift: on a fresh beat (energy rising), jump the hue offset forward.
        // Decays back toward 0 between beats for a "snap then relax" feel.
        if self.beat_energy > prev_beat + 0.05 {
            // Jump: 15-30% hue rotation proportional to beat strength.
            self.beat_hue_offset = (self.beat_hue_offset + self.beat_energy * 0.3) % 1.0;
        } else {
            // Decay back toward 0 — slower than bar decay for lingering color shift.
            self.beat_hue_offset *= 0.95;
        }

        // Stash waveform for oscilloscope/lissajous modes.
        self.waveform = frame.waveform;

        // Advance radial rotation — slow drift + beat burst.
        let dt = 1.0 / 60.0; // Approximate frame time.
        self.radial_angle += dt * 0.3 + self.beat_energy * 0.1;
        if self.radial_angle > std::f32::consts::TAU {
            self.radial_angle -= std::f32::consts::TAU;
        }
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
        self.beat_hue_offset *= 0.95;
    }
}

// ── VisualizerWidget (mode-dispatching wrapper) ─────────────────────────────

/// Top-level visualizer widget that dispatches to the active mode's renderer.
pub struct VisualizerWidget<'a> {
    state: &'a mut VisualizerState,
    theme: &'a Theme,
}

impl<'a> VisualizerWidget<'a> {
    pub fn new(state: &'a mut VisualizerState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }

    pub fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        match self.state.mode {
            VisualizerMode::Bars => {
                // Delegate to the existing spectrum bar renderer.
                let widget = SpectrumWidget::new(self.state, self.theme);
                Widget::render(widget, area, buf);
            }
            VisualizerMode::Oscilloscope => {
                render_oscilloscope(self.state, area, buf);
            }
            VisualizerMode::Radial => {
                render_radial(self.state, area, buf);
            }
            VisualizerMode::Particles => {
                render_particles(self.state, area, buf);
            }
            VisualizerMode::Lissajous => {
                render_lissajous(self.state, area, buf);
            }
        }
    }
}

// ── Oscilloscope Renderer ──────────────────────────────────────────────────

fn render_oscilloscope(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width();
    let px_h = grid.px_height();

    if state.waveform.is_empty() || px_w == 0 || px_h == 0 {
        return;
    }

    // Mix to mono from interleaved stereo.
    let channels = if state.waveform.len() > 2 { 2 } else { 1 };
    let num_frames = state.waveform.len() / channels;
    if num_frames < 2 {
        return;
    }

    let center_y = px_h as f32 / 2.0;
    let amplitude = px_h as f32 * 0.4; // ±40% of height.

    let mut prev_x = 0.0f32;
    let mut prev_y = center_y;

    for i in 0..px_w {
        // Map pixel X to waveform frame index.
        let frame_idx = i * num_frames / px_w;
        let sample = if channels == 2 {
            (state.waveform[frame_idx * 2] + state.waveform[frame_idx * 2 + 1]) * 0.5
        } else {
            state.waveform[frame_idx]
        };

        let x = i as f32;
        let y = center_y - sample * amplitude;
        let y = y.clamp(0.0, (px_h - 1) as f32);

        if i > 0 {
            // Color by amplitude — palette mapped.
            let amp_t = sample.abs().clamp(0.0, 1.0);
            let base = state.palette.freq_color(amp_t);
            let color = brighten(base, state.beat_energy * 0.5);
            grid.draw_line(prev_x, prev_y, x, y, color);
        }

        prev_x = x;
        prev_y = y;
    }

    grid.render_to(area, buf);
}

// ── Radial Spectrum Renderer ───────────────────────────────────────────────

fn render_radial(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width() as f32;
    let px_h = grid.px_height() as f32;

    if px_w < 4.0 || px_h < 4.0 {
        return;
    }

    let cx = px_w / 2.0;
    let cy = px_h / 2.0;
    let max_radius = cx.min(cy) * 0.9;
    let inner_radius = max_radius * 0.15;
    let rotation = state.radial_angle;
    let beat_pulse = 1.0 + state.beat_energy * 0.3;

    let elapsed = state.created_at.elapsed().as_secs_f32();
    let drift = (elapsed * std::f32::consts::TAU / 8.0).sin() * 0.15;

    for i in 0..NUM_BARS {
        let freq_t = i as f32 / (NUM_BARS - 1) as f32;
        let angle = freq_t * std::f32::consts::TAU + rotation;
        let magnitude = state.spectrum[i] * beat_pulse;
        let bar_len = magnitude * (max_radius - inner_radius);

        if bar_len < 0.5 {
            continue;
        }

        let cos_a = angle.cos();
        let sin_a = angle.sin();

        let x0 = cx + inner_radius * cos_a;
        let y0 = cy + inner_radius * sin_a;
        let x1 = cx + (inner_radius + bar_len) * cos_a;
        let y1 = cy + (inner_radius + bar_len) * sin_a;

        let warped = (freq_t + drift + state.beat_hue_offset).rem_euclid(1.0);
        let base = state.palette.freq_color(warped);
        let color = brighten(base, state.beat_energy * 0.5);

        grid.draw_line(x0, y0, x1, y1, color);
    }

    grid.render_to(area, buf);
}

// ── Particle Renderer ──────────────────────────────────────────────────────

fn render_particles(state: &mut VisualizerState, area: Rect, buf: &mut Buffer) {
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width() as f32;
    let px_h = grid.px_height() as f32;

    // Step physics and emit new particles.
    let dt = 1.0 / 60.0;
    state
        .particles
        .update(&state.spectrum, state.beat_energy, px_w, px_h, dt);

    // Render particles to the grid.
    state
        .particles
        .render(&mut grid, state.palette, state.beat_energy);

    grid.render_to(area, buf);
}

// ── Lissajous Renderer ─────────────────────────────────────────────────────

fn render_lissajous(state: &mut VisualizerState, area: Rect, buf: &mut Buffer) {
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width();
    let px_h = grid.px_height();

    if state.waveform.is_empty() || px_w == 0 || px_h == 0 {
        // Still render the trail for afterglow.
        state.lissajous_trail.push(Vec::new());
        state
            .lissajous_trail
            .render(&mut grid, state.palette, state.beat_energy);
        grid.render_to(area, buf);
        return;
    }

    let channels = if state.waveform.len() > 2 { 2 } else { 1 };
    let num_frames = state.waveform.len() / channels;

    let cx = px_w as f32 / 2.0;
    let cy = px_h as f32 / 2.0;
    let scale_x = cx * 0.85;
    let scale_y = cy * 0.85;

    let mut points = Vec::with_capacity(num_frames.min(1024));

    // Downsample to ~1024 points for performance.
    let step = (num_frames / 1024).max(1);
    for i in (0..num_frames).step_by(step) {
        let (left, right) = if channels == 2 {
            (state.waveform[i * 2], state.waveform[i * 2 + 1])
        } else {
            (state.waveform[i], state.waveform[i])
        };

        let px = (cx + left * scale_x).clamp(0.0, (px_w - 1) as f32) as usize;
        let py = (cy - right * scale_y).clamp(0.0, (px_h - 1) as f32) as usize;
        points.push((px, py));

        // Draw the current frame's points brightly.
        let amp = ((left * left + right * right) * 0.5).sqrt().clamp(0.0, 1.0);
        let base = state.palette.freq_color(amp);
        let color = brighten(base, state.beat_energy * 0.4);
        grid.set_dot(px, py, color);
    }

    // Push to trail and render afterglow.
    state.lissajous_trail.push(points);
    state
        .lissajous_trail
        .render(&mut grid, state.palette, state.beat_energy);

    grid.render_to(area, buf);
}

// ── SpectrumWidget (original bars mode) ─────────────────────────────────────

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
    /// Warp the frequency position with dreamy drift + beat hue shift.
    /// Returns a new freq_t in 0.0..1.0 with both effects applied.
    fn warped_freq_t(&self, freq_t: f32) -> f32 {
        // Dreamy drift: slow sine wave (~8 second period) that shifts the
        // color mapping ±15% back and forth across the spectrum.
        let elapsed = self.state.created_at.elapsed().as_secs_f32();
        let drift = (elapsed * std::f32::consts::TAU / 8.0).sin() * 0.15;

        // Beat hue shift: jarring jump on transients, decays back.
        let beat_offset = self.state.beat_hue_offset;

        // Combine and wrap to 0.0..1.0.
        (freq_t + drift + beat_offset).rem_euclid(1.0)
    }

    /// Compute the bar fill color for a given display bar.
    ///
    /// For `Mono` palette: height-based green/yellow/red (classic LED meter).
    /// For all other palettes: frequency-mapped gradient with dreamy drift,
    /// beat-reactive hue shifts, and brightness pulses.
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
                let warped = self.warped_freq_t(freq_t);
                let base_color = palette.freq_color(warped);
                // Beat-reactive brightness pulse on top of the hue shift.
                let color = brighten(base_color, beat * 0.7);
                Style::new().fg(color)
            }
        }
    }

    /// Compute the peak marker color for a given display bar.
    ///
    /// For `Mono`: white (theme default).
    /// For other palettes: brightened version of the warped frequency color.
    fn peak_color(&self, freq_t: f32) -> Style {
        let palette = self.state.palette;

        match palette {
            VisualizerPalette::Mono => self.theme.spectrum_peak,
            _ => {
                let warped = self.warped_freq_t(freq_t);
                let base = palette.freq_color(warped);
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
        let state = VisualizerState::with_config(
            0.045,
            0.18,
            VisualizerPalette::Spectrum,
            VisualizerMode::Bars,
        );
        assert_eq!(state.spectrum.len(), NUM_BARS);
        assert_eq!(state.peaks.len(), NUM_BARS);
        assert_eq!(state.vu_levels, [0.0, 0.0]);
        assert_eq!(state.beat_energy, 0.0);
    }

    #[test]
    fn update_from_snapshot_with_silence() {
        let mut state = VisualizerState::with_config(
            0.045,
            0.18,
            VisualizerPalette::Spectrum,
            VisualizerMode::Bars,
        );
        let snapshot = VizSnapshot::new();

        // Default snapshot has all zeros (silence).
        state.update_from_snapshot(&snapshot);

        for &bar in &state.spectrum {
            assert!(bar <= 0.01, "expected near-zero, got {}", bar);
        }
    }

    #[test]
    fn update_from_snapshot_with_signal() {
        let mut state = VisualizerState::with_config(
            0.045,
            0.18,
            VisualizerPalette::Spectrum,
            VisualizerMode::Bars,
        );
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
            waveform: Vec::new(),
        });

        state.update_from_snapshot(&snapshot);

        assert!(state.spectrum[10] > 0.5, "expected energy at bar 10");
        assert!(state.vu_levels[0] > 0.0, "expected non-zero VU");
        assert!(state.beat_energy > 0.0, "expected non-zero beat energy");
    }

    #[test]
    fn decay_to_zero_reduces_bars() {
        let mut state = VisualizerState::with_config(
            0.045,
            0.18,
            VisualizerPalette::Spectrum,
            VisualizerMode::Bars,
        );
        let snapshot = VizSnapshot::new();

        // Seed some energy.
        let spectrum = [1.0f32; NUM_BARS];
        snapshot.write(VizFrame {
            spectrum,
            vu_levels: [1.0, 1.0],
            beat_energy: 0.8,
            timestamp: Instant::now(),
            waveform: Vec::new(),
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
        let mut state = VisualizerState::with_config(
            0.045,
            0.18,
            VisualizerPalette::Spectrum,
            VisualizerMode::Bars,
        );
        let snapshot = VizSnapshot::new();

        // Push a loud frame.
        let mut spectrum = [0.0f32; NUM_BARS];
        spectrum[5] = 0.9;
        snapshot.write(VizFrame {
            spectrum,
            vu_levels: [0.0; 2],
            beat_energy: 0.0,
            timestamp: Instant::now(),
            waveform: Vec::new(),
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
            waveform: Vec::new(),
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

    #[test]
    fn mode_parse_variants() {
        assert_eq!(VisualizerMode::parse("bars"), VisualizerMode::Bars);
        assert_eq!(VisualizerMode::parse("spectrum"), VisualizerMode::Bars);
        assert_eq!(
            VisualizerMode::parse("oscilloscope"),
            VisualizerMode::Oscilloscope
        );
        assert_eq!(VisualizerMode::parse("scope"), VisualizerMode::Oscilloscope);
        assert_eq!(VisualizerMode::parse("radial"), VisualizerMode::Radial);
        assert_eq!(
            VisualizerMode::parse("particles"),
            VisualizerMode::Particles
        );
        assert_eq!(
            VisualizerMode::parse("lissajous"),
            VisualizerMode::Lissajous
        );
        assert_eq!(VisualizerMode::parse("phase"), VisualizerMode::Lissajous);
        assert_eq!(VisualizerMode::parse("garbage"), VisualizerMode::Bars);
    }

    #[test]
    fn mode_cycles_through_all() {
        let mode = VisualizerMode::Bars;
        let mode = mode.next();
        assert_eq!(mode, VisualizerMode::Oscilloscope);
        let mode = mode.next();
        assert_eq!(mode, VisualizerMode::Radial);
        let mode = mode.next();
        assert_eq!(mode, VisualizerMode::Particles);
        let mode = mode.next();
        assert_eq!(mode, VisualizerMode::Lissajous);
        let mode = mode.next();
        assert_eq!(mode, VisualizerMode::Bars);
    }

    #[test]
    fn braille_grid_basic() {
        let mut grid = BrailleGrid::new(10, 5);
        assert_eq!(grid.px_width(), 20);
        assert_eq!(grid.px_height(), 20);

        // Set a dot and verify it sticks.
        assert!(grid.set_dot(0, 0, Color::White));
        assert_eq!(grid.dots[0], 1 << 0); // top-left dot of cell (0,0).

        // Out of bounds returns false.
        assert!(!grid.set_dot(20, 0, Color::White));
        assert!(!grid.set_dot(0, 20, Color::White));
    }

    #[test]
    fn braille_grid_line_drawing() {
        let mut grid = BrailleGrid::new(10, 5);
        grid.draw_line(0.0, 0.0, 19.0, 19.0, Color::Cyan);

        // At least some dots should be set along the diagonal.
        let any_set = grid.dots.iter().any(|&d| d != 0);
        assert!(any_set, "diagonal line should set some dots");
    }

    #[test]
    fn particle_system_emits_and_decays() {
        let mut ps = ParticleSystem::new();
        let mut spectrum = [0.0f32; NUM_BARS];
        spectrum[10] = 0.8;
        spectrum[20] = 0.6;

        // Emit.
        ps.update(&spectrum, 0.5, 100.0, 100.0, 1.0 / 60.0);
        assert!(!ps.particles.is_empty(), "should have emitted particles");

        // Decay to death with large dt.
        for _ in 0..100 {
            let silence = [0.0f32; NUM_BARS];
            ps.update(&silence, 0.0, 100.0, 100.0, 0.1);
        }
        assert!(
            ps.particles.is_empty(),
            "particles should die after enough time"
        );
    }

    #[test]
    fn dim_produces_darker_color() {
        let bright = Color::Rgb(200, 150, 100);
        let dark = dim(bright, 0.5);
        if let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (bright, dark) {
            assert!(r2 < r1, "red should decrease");
            assert!(g2 < g1, "green should decrease");
            assert!(b2 < b1, "blue should decrease");
        } else {
            panic!("expected Rgb colors");
        }
    }
}
