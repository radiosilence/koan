use std::time::Instant;

use koan_core::audio::viz::VizSnapshot;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
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
    /// Time×frequency heatmap — scrolls vertically, newest at bottom.
    Spectrogram,
    /// L and R waveforms drawn separately, stacked top/bottom.
    StereoWaveform,
    /// Analog-style needle VU meters.
    VuMeter,
    /// Filled spectrum curve with stacked decay trails.
    Flame,
    /// Classic demoscene plasma — overlapping sine waves, audio-reactive.
    Plasma,
    /// Fly-through tunnel with spectrum-driven radius wobble.
    Tunnel,
    /// Rotating 3D wireframe mesh with spectrum-modulated vertices.
    Wireframe,
    /// Implicit surface blobs driven by frequency bands.
    Metaballs,
    /// 3D starfield with beat-driven acceleration.
    Starfield,
    /// 3D heightmap terrain from spectrum history, perspective projected.
    Terrain,
    /// Overlapping rotating line grids — interference moiré patterns.
    Moire,
    /// Waveform mirrored 6-8 ways around center — kaleidoscope.
    Kaleidoscope,
    /// Julia fractal with audio-driven complex constant.
    Julia,
    /// Archimedean spiral that breathes and rotates with music.
    Spiral,
    /// Concentric wave sources — ripple interference patterns.
    Interference,
    /// 3D wireframe tunnel fly-through with reactive stars.
    Wormhole,
    /// Matrix digital rain — audio-reactive falling characters.
    Matrix,
}

impl VisualizerMode {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bars" | "spectrum" => Self::Bars,
            "oscilloscope" | "scope" => Self::Oscilloscope,
            "radial" => Self::Radial,
            "particles" | "particle" => Self::Particles,
            "lissajous" | "phase" => Self::Lissajous,
            "spectrogram" | "waterfall" => Self::Spectrogram,
            "stereo" | "stereo_waveform" | "stereo-waveform" => Self::StereoWaveform,
            "vu" | "vu_meter" | "vu-meter" | "meter" => Self::VuMeter,
            "flame" | "mountain" => Self::Flame,
            "plasma" => Self::Plasma,
            "tunnel" => Self::Tunnel,
            "wireframe" | "wire" | "3d" => Self::Wireframe,
            "metaballs" | "blobs" => Self::Metaballs,
            "starfield" | "stars" => Self::Starfield,
            "terrain" | "landscape" | "pleasures" => Self::Terrain,
            "moire" | "moiré" => Self::Moire,
            "kaleidoscope" | "kaleid" | "kaleido" => Self::Kaleidoscope,
            "julia" | "fractal" => Self::Julia,
            "spiral" | "hypno" => Self::Spiral,
            "interference" | "ripple" | "ripples" => Self::Interference,
            "wormhole" | "wormholes" => Self::Wormhole,
            "matrix" | "rain" | "cmatrix" => Self::Matrix,
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
            Self::Lissajous => Self::Spectrogram,
            Self::Spectrogram => Self::StereoWaveform,
            Self::StereoWaveform => Self::VuMeter,
            Self::VuMeter => Self::Flame,
            Self::Flame => Self::Plasma,
            Self::Plasma => Self::Tunnel,
            Self::Tunnel => Self::Wireframe,
            Self::Wireframe => Self::Metaballs,
            Self::Metaballs => Self::Starfield,
            Self::Starfield => Self::Terrain,
            Self::Terrain => Self::Moire,
            Self::Moire => Self::Kaleidoscope,
            Self::Kaleidoscope => Self::Julia,
            Self::Julia => Self::Spiral,
            Self::Spiral => Self::Interference,
            Self::Interference => Self::Wormhole,
            Self::Wormhole => Self::Matrix,
            Self::Matrix => Self::Bars,
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
            Self::Spectrogram => "spectrogram",
            Self::StereoWaveform => "stereo_waveform",
            Self::VuMeter => "vu_meter",
            Self::Flame => "flame",
            Self::Plasma => "plasma",
            Self::Tunnel => "tunnel",
            Self::Wireframe => "wireframe",
            Self::Metaballs => "metaballs",
            Self::Starfield => "starfield",
            Self::Terrain => "terrain",
            Self::Moire => "moire",
            Self::Kaleidoscope => "kaleidoscope",
            Self::Julia => "julia",
            Self::Spiral => "spiral",
            Self::Interference => "interference",
            Self::Wormhole => "wormhole",
            Self::Matrix => "matrix",
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
            Self::Spectrogram => "spectrogram",
            Self::StereoWaveform => "stereo waveform",
            Self::VuMeter => "vu meter",
            Self::Flame => "flame",
            Self::Plasma => "plasma",
            Self::Tunnel => "tunnel",
            Self::Wireframe => "wireframe",
            Self::Metaballs => "metaballs",
            Self::Starfield => "starfield",
            Self::Terrain => "pleasures",
            Self::Moire => "moiré",
            Self::Kaleidoscope => "kaleidoscope",
            Self::Julia => "julia fractal",
            Self::Spiral => "spiral",
            Self::Interference => "interference",
            Self::Wormhole => "wormhole",
            Self::Matrix => "matrix",
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
    ///
    /// All braille cells are rendered bold with boosted brightness to compensate
    /// for the inherent sparsity of braille dots (each cell is mostly empty space).
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
                // Boost brightness: braille dots are sparse so colors look dim.
                let color = brighten(self.colors[idx], 0.25);
                buf[(x, y)]
                    .set_char(ch)
                    .set_style(Style::new().fg(color).add_modifier(Modifier::BOLD));
            }
        }
    }
}

/// Fill an area with a beat-reactive pulsating background color.
/// Intensity is low (subtle wash), color shifts with beat_hue_offset.
/// Fill an area with a beat-reactive background color.
/// Cycles through bold colors on beat hits, fades to black between.
fn fill_reactive_bg(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let r = state.reactivity;
    let intensity = (state.beat_energy * r).clamp(0.0, 1.0);
    if intensity < 0.02 {
        return;
    }

    // Pick a background color from the palette, cycling with beat hue offset.
    let hue_t = state.beat_hue_offset.rem_euclid(1.0);
    let base = state.palette.freq_color(hue_t);
    // Scale brightness by beat intensity — strong beats get vivid bg, quiet = black.
    let bg = dim(base, 1.0 - intensity * 0.35);
    let style = Style::new().bg(bg);

    for row in 0..area.height {
        for col in 0..area.width {
            buf[(area.x + col, area.y + row)].set_style(style);
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

// ── Spectrum History ────────────────────────────────────────────────────────

/// Maximum spectrum history frames (enough for tall terminals).
const SPECTRUM_HISTORY_CAP: usize = 512;

/// Ring buffer of past spectrum frames for spectrogram and flame modes.
pub struct SpectrumHistory {
    frames: Vec<[f32; NUM_BARS]>,
    write_idx: usize,
    len: usize,
}

impl SpectrumHistory {
    pub fn new() -> Self {
        Self {
            frames: vec![[0.0; NUM_BARS]; SPECTRUM_HISTORY_CAP],
            write_idx: 0,
            len: 0,
        }
    }

    /// Push the current spectrum into the history ring.
    pub fn push(&mut self, spectrum: &[f32; NUM_BARS]) {
        self.frames[self.write_idx] = *spectrum;
        self.write_idx = (self.write_idx + 1) % SPECTRUM_HISTORY_CAP;
        if self.len < SPECTRUM_HISTORY_CAP {
            self.len += 1;
        }
    }

    /// Iterate frames from newest to oldest (up to `count`).
    pub fn iter_newest_first(&self, count: usize) -> impl Iterator<Item = &[f32; NUM_BARS]> {
        let count = count.min(self.len);
        (0..count).map(move |age| {
            let idx = (self.write_idx + SPECTRUM_HISTORY_CAP - 1 - age) % SPECTRUM_HISTORY_CAP;
            &self.frames[idx]
        })
    }
}

// ── Matrix Column State ────────────────────────────────────────────────────

/// A single column of falling matrix characters.
pub struct MatrixColumn {
    /// Current head position (fractional row).
    pub head_y: f32,
    /// Fall speed (rows per frame).
    pub speed: f32,
    /// Trail length in rows.
    pub trail_len: f32,
    /// Character cycle seed — determines which chars appear.
    pub char_seed: u32,
}

// ── VisualizerState ─────────────────────────────────────────────────────────

/// Processed visualizer data, ready for rendering.
///
/// All FFT/analysis work is done on a dedicated thread in koan-core (VizAnalyzer).
/// Spectrum, peaks, and VU levels are passed through directly from the analyzer
/// (single layer of smoothing). Only beat energy has local decay for the hue-shift effect.
/// `decay_to_zero` provides graceful falloff when paused/stopped.
///
/// Lock discipline: `update_from_snapshot` acquires the VizSnapshot RwLock for <1us
/// (clone of ~200 bytes). No further computation on spectrum/peaks.
pub struct VisualizerState {
    /// Current spectrum bar values (0.0..1.0), one per bar.
    pub spectrum: [f32; NUM_BARS],
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
    /// Now-playing artist name (for pleasures mode overlay).
    pub now_artist: String,
    /// Now-playing album name (for pleasures mode overlay).
    pub now_album: String,
    /// Particle system state (persists across frames).
    pub particles: ParticleSystem,
    /// Lissajous afterglow trail.
    pub lissajous_trail: LissajousTrail,
    /// Radial rotation angle (radians), slowly drifts.
    pub radial_angle: f32,
    /// Spectrum history for spectrogram and flame modes.
    pub spectrum_history: SpectrumHistory,
    /// VU needle angle (smoothed), [left, right] in radians.
    pub vu_needle_angle: [f32; 2],
    /// Starfield: persistent star positions [(x, y, z)].
    pub stars: Vec<(f32, f32, f32)>,
    /// Wireframe rotation angles [x, y, z] in radians.
    pub wire_rotation: [f32; 3],
    /// Tunnel depth offset — advances with time/beat.
    pub tunnel_z: f32,
    /// Elapsed time accumulator for plasma phase.
    pub plasma_time: f32,
    /// Reactivity multiplier — scales all beat/spectrum-driven coefficients.
    /// 0.0 = static, 1.0 = normal, 2.0 = hypersensitive.
    pub reactivity: f32,
    /// Matrix rain: per-column state [(head_y, speed, trail_len)].
    pub matrix_cols: Vec<MatrixColumn>,
    /// Camera shake offset in subpixels [x, y]. Spikes on bass, decays fast.
    pub shake: [f32; 2],
    /// Scale pulse factor (1.0 = normal). Spikes >1.0 on bass hits.
    pub scale_pulse: f32,
    /// Whether bass shake is enabled.
    pub bass_shake: bool,
    /// Matrix overlay: replace all rendered chars with matrix glyphs in green.
    pub matrix_overlay: bool,
}

impl VisualizerState {
    pub fn from_config(cfg: &koan_core::config::VisualizerConfig) -> Self {
        let bar_half_life = cfg.bar_decay_ms as f32 / 1000.0;
        let peak_half_life = cfg.peak_decay_ms as f32 / 1000.0;
        let palette = VisualizerPalette::parse(&cfg.palette);
        let mode = VisualizerMode::parse(&cfg.mode);
        let reactivity = cfg.reactivity.clamp(0.0, 2.0);
        let bass_shake = cfg.bass_shake;
        let matrix_overlay = cfg.matrix_overlay;
        Self::with_config(
            bar_half_life,
            peak_half_life,
            palette,
            mode,
            reactivity,
            bass_shake,
            matrix_overlay,
        )
    }

    pub fn with_config(
        bar_half_life: f32,
        peak_half_life: f32,
        palette: VisualizerPalette,
        mode: VisualizerMode,
        reactivity: f32,
        bass_shake: bool,
        matrix_overlay: bool,
    ) -> Self {
        Self {
            spectrum: [0.0; NUM_BARS],
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
            now_artist: String::new(),
            now_album: String::new(),
            particles: ParticleSystem::new(),
            lissajous_trail: LissajousTrail::new(),
            radial_angle: 0.0,
            spectrum_history: SpectrumHistory::new(),
            vu_needle_angle: [0.0; 2],
            stars: (0..1500)
                .map(|i| {
                    // Deterministic pseudo-random spread.
                    let hash = ((i * 2654435761u64) >> 16) as f32;
                    let x = (hash % 200.0) - 100.0;
                    let y = ((hash * 1.7) % 200.0) - 100.0;
                    let z = (hash * 0.3) % 100.0 + 1.0;
                    (x, y, z)
                })
                .collect(),
            wire_rotation: [0.0; 3],
            tunnel_z: 0.0,
            plasma_time: 0.0,
            matrix_cols: Vec::new(), // Initialized lazily on first render.
            reactivity,
            shake: [0.0; 2],
            scale_pulse: 1.0,
            bass_shake,
            matrix_overlay,
        }
    }

    /// Apply camera shake + scale pulse to a subpixel coordinate.
    /// No-op when bass_shake is disabled. `cx`, `cy` = center of the grid.
    #[inline]
    pub fn shaken(&self, x: f32, y: f32, cx: f32, cy: f32) -> (f32, f32) {
        if !self.bass_shake {
            return (x, y);
        }
        let dx = (x - cx) * self.scale_pulse + cx + self.shake[0];
        let dy = (y - cy) * self.scale_pulse + cy + self.shake[1];
        (dx, dy)
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

    /// Read the latest analysis frame from VizSnapshot.
    ///
    /// Spectrum, peaks, and VU levels come directly from the analyzer thread
    /// (which already applies decay smoothing and peak hold). No double-smoothing.
    /// Only beat energy gets local decay for the hue-shift effect.
    ///
    /// The snapshot read is <1us (RwLock clone of ~200 bytes + waveform vec).
    /// Called once per frame (~60fps) from `handle_tick()`.
    pub fn update_from_snapshot(&mut self, snapshot: &VizSnapshot) {
        let frame = snapshot.read();

        // Spectrum + peaks: pass through directly from analyzer (already smoothed).
        self.spectrum = frame.spectrum;
        self.peaks = frame.peaks;
        self.vu_levels = frame.vu_levels;

        // Beat energy: rise instantly from analyzer, decay locally for smooth falloff.
        // Local decay gives the hue-shift a longer tail than the analyzer's own decay.
        let (bar_decay, _) = self.decay_factors();
        let prev_beat = self.beat_energy;
        self.beat_energy = frame.beat_energy.max(self.beat_energy * bar_decay);

        // Beat hue shift: on a fresh beat (energy rising), jump the hue offset forward.
        // Reactivity scales the hue jump size.
        let r = self.reactivity;
        if self.beat_energy > prev_beat + 0.05 {
            self.beat_hue_offset = (self.beat_hue_offset + self.beat_energy * 0.3 * r) % 1.0;
        } else {
            self.beat_hue_offset *= 0.95;
        }

        // Stash waveform for oscilloscope/lissajous modes.
        self.waveform = frame.waveform;

        // Advance radial rotation — slow drift + beat burst.
        let dt = 1.0 / 60.0;
        self.radial_angle += dt * 0.3 + self.beat_energy * 0.1;
        if self.radial_angle > std::f32::consts::TAU {
            self.radial_angle -= std::f32::consts::TAU;
        }

        // Push spectrum snapshot for spectrogram/flame history.
        self.spectrum_history.push(&self.spectrum);

        // Smooth VU needle angles (ballistic: fast rise, slow fall).
        for ch in 0..2 {
            let target = self.vu_levels[ch];
            if target > self.vu_needle_angle[ch] {
                self.vu_needle_angle[ch] += (target - self.vu_needle_angle[ch]) * 0.5;
            } else {
                self.vu_needle_angle[ch] *= bar_decay;
            }
        }

        // Camera shake + scale pulse (gated by config).
        let r = self.reactivity;
        if self.bass_shake {
            let bass_now = self.spectrum[..6].iter().sum::<f32>() / 6.0;
            let shake_intensity = (self.beat_energy * bass_now * 8.0 * r).min(6.0);
            let shake_angle = self.plasma_time * 137.5;
            self.shake[0] = shake_angle.cos() * shake_intensity;
            self.shake[1] = shake_angle.sin() * shake_intensity;
            let pulse_target = 1.0 + self.beat_energy * 0.15 * r;
            self.scale_pulse = pulse_target.max(self.scale_pulse * 0.92);
        } else {
            self.shake = [0.0; 2];
            self.scale_pulse = 1.0;
        }

        // Advance demoscene state.
        self.plasma_time += dt * (1.0 + self.beat_energy * 2.0 * r);
        self.tunnel_z += dt * (2.0 + self.beat_energy * 8.0 * r);

        // Wireframe rotation: bass drives X, mids drive Y, treble drives Z.
        let bass = self.spectrum[..8].iter().sum::<f32>() / 8.0;
        let mids = self.spectrum[16..32].iter().sum::<f32>() / 16.0;
        let treble = self.spectrum[32..].iter().sum::<f32>() / 16.0;
        let beat_mult = 1.0 + self.beat_energy * 4.0 * r;
        self.wire_rotation[0] += dt * (0.8 + bass * 12.0 * r) * beat_mult;
        self.wire_rotation[1] += dt * (0.5 + mids * 8.0 * r) * beat_mult;
        self.wire_rotation[2] += dt * (0.3 + treble * 6.0 * r) * beat_mult;

        // Starfield: beat slams the throttle.
        let star_speed = 30.0 + bass * 200.0 * r + self.beat_energy * 300.0 * r;
        for star in &mut self.stars {
            star.2 -= dt * star_speed;
            // Lateral drift: stars wobble on X/Y based on spectrum, not just fly forward.
            let lateral = self.beat_energy * 20.0 * r;
            star.0 += (star.2 * 0.1 + self.plasma_time).sin() * lateral * dt;
            star.1 += (star.2 * 0.13 + self.plasma_time * 1.3).cos() * lateral * dt;
            if star.2 <= 0.5 {
                let hash = ((star.0.to_bits() ^ star.1.to_bits()) as f32).abs();
                star.2 = 60.0 + (hash % 40.0);
                star.0 = (hash * 1.3 % 300.0) - 150.0;
                star.1 = (hash * 0.7 % 300.0) - 150.0;
            }
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
            VisualizerMode::Spectrogram => {
                render_spectrogram(self.state, area, buf);
            }
            VisualizerMode::StereoWaveform => {
                render_stereo_waveform(self.state, area, buf);
            }
            VisualizerMode::VuMeter => {
                render_vu_meter(self.state, area, buf);
            }
            VisualizerMode::Flame => {
                render_flame(self.state, area, buf);
            }
            VisualizerMode::Plasma => {
                render_plasma(self.state, area, buf);
            }
            VisualizerMode::Tunnel => {
                render_tunnel(self.state, area, buf);
            }
            VisualizerMode::Wireframe => {
                render_wireframe(self.state, area, buf);
            }
            VisualizerMode::Metaballs => {
                render_metaballs(self.state, area, buf);
            }
            VisualizerMode::Starfield => {
                render_starfield(self.state, area, buf);
            }
            VisualizerMode::Terrain => {
                render_terrain(self.state, area, buf);
            }
            VisualizerMode::Moire => {
                render_moire(self.state, area, buf);
            }
            VisualizerMode::Kaleidoscope => {
                render_kaleidoscope(self.state, area, buf);
            }
            VisualizerMode::Julia => {
                render_julia(self.state, area, buf);
            }
            VisualizerMode::Spiral => {
                render_spiral(self.state, area, buf);
            }
            VisualizerMode::Interference => {
                render_interference(self.state, area, buf);
            }
            VisualizerMode::Wormhole => {
                render_wormhole(self.state, area, buf);
            }
            VisualizerMode::Matrix => {
                render_matrix(self.state, area, buf);
            }
        }

        // Matrix overlay post-process: replace all non-empty cells with
        // random matrix characters in green. Applies to any mode.
        if self.state.matrix_overlay {
            apply_matrix_overlay(self.state, area, buf);
        }
    }
}

/// Post-processing pass: replace every non-empty cell with a random matrix
/// character in green, preserving the spatial structure of the underlying
/// visualizer. Brightness derived from the original cell's foreground color.
fn apply_matrix_overlay(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let t = state.plasma_time;
    for row in 0..area.height {
        for col in 0..area.width {
            let x = area.x + col;
            let y = area.y + row;
            let cell = &buf[(x, y)];

            // Skip empty/space cells — preserve the void.
            let sym = cell.symbol().chars().next().unwrap_or(' ');
            if sym == ' ' {
                continue;
            }

            // Extract brightness from original foreground color.
            let brightness = match cell.fg {
                Color::Rgb(r, g, b) => {
                    (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) / 255.0
                }
                _ => 0.5,
            };

            // Pick a matrix character — changes slowly per position.
            let hash = (col as u32)
                .wrapping_mul(31)
                .wrapping_add(row as u32 * 137)
                .wrapping_add((t * 2.0) as u32);
            let ch = MATRIX_CHARS[hash as usize % MATRIX_CHARS.len()];

            // Green palette: brightness maps to green intensity.
            let g = (80.0 + brightness * 175.0) as u8;
            let rb = (brightness * 40.0) as u8;
            let fg = Color::Rgb(rb, g, rb);

            let mut style = Style::new().fg(fg);
            if brightness > 0.6 {
                style = style.add_modifier(Modifier::BOLD);
            }
            buf[(x, y)].set_char(ch).set_style(style);
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

    let cx = px_w as f32 / 2.0;
    let cy = px_h as f32 / 2.0;
    let amplitude = px_h as f32 * 0.4;

    let mut prev_x = 0.0f32;
    let mut prev_y = cy;

    for i in 0..px_w {
        let frame_idx = i * num_frames / px_w;
        let sample = if channels == 2 {
            (state.waveform[frame_idx * 2] + state.waveform[frame_idx * 2 + 1]) * 0.5
        } else {
            state.waveform[frame_idx]
        };

        let raw_x = i as f32;
        let raw_y = (cy - sample * amplitude).clamp(0.0, (px_h - 1) as f32);
        let (x, y) = state.shaken(raw_x, raw_y, cx, cy);

        if i > 0 {
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
    let beat_pulse = 1.0 + state.beat_energy * 0.3 * state.reactivity;

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

        let (x0, y0) = state.shaken(cx + inner_radius * cos_a, cy + inner_radius * sin_a, cx, cy);
        let (x1, y1) = state.shaken(
            cx + (inner_radius + bar_len) * cos_a,
            cy + (inner_radius + bar_len) * sin_a,
            cx,
            cy,
        );

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
    fill_reactive_bg(state, area, buf);
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
    let scale_x = cx * 0.95;
    let scale_y = cy * 0.95;

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

// ── Spectrogram Renderer ──────────────────────────────────────────────────

fn render_spectrogram(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    // Each terminal row = one spectrum frame. Full block chars with intensity coloring.
    // Heat map: black → dark blue → blue → cyan → white for maximum contrast.
    for (row_age, frame) in state.spectrum_history.iter_newest_first(h).enumerate() {
        let y = area.y + (h - 1 - row_age) as u16;

        for col in 0..w {
            // Interpolate between spectrum bars.
            let bar_f = col as f32 * (NUM_BARS - 1) as f32 / (w - 1).max(1) as f32;
            let bar_lo = (bar_f as usize).min(NUM_BARS - 1);
            let bar_hi = (bar_lo + 1).min(NUM_BARS - 1);
            let frac = bar_f - bar_lo as f32;
            let raw_energy = frame[bar_lo] * (1.0 - frac) + frame[bar_hi] * frac;

            if raw_energy < 0.01 {
                continue;
            }

            // Non-linear mapping: sqrt pushes more values into the visible range.
            // Raw spectrum rarely exceeds 0.5 even on loud music, so sqrt(0.5)=0.7
            // puts loud content solidly into the red zone.
            let energy = raw_energy.sqrt();

            // Heat map: blue → yellow → red → white.
            let color = if energy > 0.85 {
                // White-hot.
                let t = (energy - 0.85) / 0.15;
                lerp_rgb((255, 40, 0), (255, 255, 255), t)
            } else if energy > 0.6 {
                // Red.
                let t = (energy - 0.6) / 0.25;
                lerp_rgb((255, 200, 0), (255, 40, 0), t)
            } else if energy > 0.35 {
                // Yellow.
                let t = (energy - 0.35) / 0.25;
                lerp_rgb((0, 80, 255), (255, 200, 0), t)
            } else if energy > 0.12 {
                // Blue.
                let t = (energy - 0.12) / 0.23;
                lerp_rgb((0, 0, 60), (0, 80, 255), t)
            } else {
                // Near-black blue.
                let t = energy / 0.12;
                lerp_rgb((0, 0, 0), (0, 0, 60), t)
            };

            let x = area.x + col as u16;
            buf[(x, y)].set_char('█').set_style(Style::new().fg(color));
        }
    }
}

// ── Stereo Waveform Renderer ──────────────────────────────────────────────

fn render_stereo_waveform(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h < 2 || state.waveform.is_empty() {
        return;
    }

    let channels = if state.waveform.len() > 2 { 2 } else { 1 };
    let num_frames = state.waveform.len() / channels;
    if num_frames < 2 {
        return;
    }

    // Split area: top half = left channel, bottom half = right channel.
    let half_h = h / 2;
    let top_area = Rect::new(area.x, area.y, area.width, half_h as u16);
    let bottom_area = Rect::new(
        area.x,
        area.y + half_h as u16,
        area.width,
        (h - half_h) as u16,
    );

    // Render each channel.
    for (ch, ch_area) in [(0, top_area), (1, bottom_area)] {
        let mut grid = BrailleGrid::new(ch_area.width as usize, ch_area.height as usize);
        let px_w = grid.px_width();
        let px_h = grid.px_height();
        if px_w == 0 || px_h == 0 {
            continue;
        }

        let center_y = px_h as f32 / 2.0;
        let amplitude = px_h as f32 * 0.4;

        let mut prev_x = 0.0f32;
        let mut prev_y = center_y;

        for i in 0..px_w {
            let frame_idx = i * num_frames / px_w;
            let sample = if channels == 2 {
                state.waveform[frame_idx * 2 + ch]
            } else {
                state.waveform[frame_idx]
            };

            let x = i as f32;
            let y = (center_y - sample * amplitude).clamp(0.0, (px_h - 1) as f32);

            if i > 0 {
                // Left = warm colors, right = cool colors.
                let color_t = if ch == 0 { 0.15 } else { 0.75 };
                let base = state.palette.freq_color(color_t);
                let color = brighten(base, state.beat_energy * 0.4);
                grid.draw_line(prev_x, prev_y, x, y, color);
            }

            prev_x = x;
            prev_y = y;
        }

        grid.render_to(ch_area, buf);
    }

    // Draw separator line between channels.
    let sep_y = area.y + half_h as u16;
    if sep_y < area.y + area.height {
        let sep_color = dim(state.palette.freq_color(0.5), 0.6);
        for x in 0..area.width {
            buf[(area.x + x, sep_y)]
                .set_char('─')
                .set_style(Style::new().fg(sep_color));
        }
    }
}

// ── VU Meter Renderer ─────────────────────────────────────────────────────

fn render_vu_meter(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w < 8 || h < 4 {
        return;
    }

    // Split into left and right meter areas.
    let meter_w = w / 2;
    let left_area = Rect::new(area.x, area.y, meter_w as u16, area.height);
    let right_area = Rect::new(
        area.x + meter_w as u16,
        area.y,
        (w - meter_w) as u16,
        area.height,
    );

    for (ch, meter_area) in [(0, left_area), (1, right_area)] {
        let mut grid = BrailleGrid::new(meter_area.width as usize, meter_area.height as usize);
        let px_w = grid.px_width();
        let px_h = grid.px_height();
        if px_w < 4 || px_h < 4 {
            continue;
        }

        let cx = px_w as f32 / 2.0;
        let cy = px_h as f32 * 0.95; // Pivot near bottom.
        let radius = (px_w as f32 * 0.45).min(px_h as f32 * 0.85);

        // Arc from -135° to -45° (sweep of 90°, opening upward).
        let arc_start = std::f32::consts::PI * 0.75; // 135° from positive x
        let arc_end = std::f32::consts::PI * 0.25; // 45° from positive x
        let arc_sweep = arc_start - arc_end;

        // Draw the arc scale (tick marks).
        let num_ticks = 21;
        for tick in 0..num_ticks {
            let t = tick as f32 / (num_ticks - 1) as f32;
            let angle = arc_start - t * arc_sweep;
            let cos_a = angle.cos();
            let sin_a = angle.sin();

            // Tick mark: short line at the outer edge.
            let major = tick % 5 == 0;
            let tick_inner = if major { radius * 0.85 } else { radius * 0.92 };
            let tick_outer = radius;

            let x0 = cx + tick_inner * cos_a;
            let y0 = cy - tick_inner * sin_a;
            let x1 = cx + tick_outer * cos_a;
            let y1 = cy - tick_outer * sin_a;

            // Color: green for low, yellow for mid, red for high (>80%).
            let color = if t < 0.6 {
                state.palette.freq_color(0.3)
            } else if t < 0.8 {
                state.palette.freq_color(0.6)
            } else {
                state.palette.freq_color(0.9)
            };
            let color = dim(color, 0.3);
            grid.draw_line(x0, y0, x1, y1, color);
        }

        // Draw the needle.
        let level = state.vu_needle_angle[ch].clamp(0.0, 1.0);
        let needle_angle = arc_start - level * arc_sweep;
        let needle_len = radius * 0.82;

        let nx = cx + needle_len * needle_angle.cos();
        let ny = cy - needle_len * needle_angle.sin();

        let needle_color = if level > 0.8 {
            brighten(state.palette.freq_color(0.95), state.beat_energy * 0.5)
        } else {
            brighten(state.palette.freq_color(0.4), state.beat_energy * 0.3)
        };
        grid.draw_line(cx, cy, nx, ny, needle_color);

        // Draw pivot dot.
        grid.set_dot(cx as usize, cy as usize, needle_color);

        grid.render_to(meter_area, buf);
    }
}

// ── Flame Renderer ────────────────────────────────────────────────────────

fn render_flame(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width();
    let px_h = grid.px_height();
    if px_w == 0 || px_h == 0 {
        return;
    }

    let elapsed = state.created_at.elapsed().as_secs_f32();
    let drift = (elapsed * std::f32::consts::TAU / 8.0).sin() * 0.15;

    // Draw stacked layers: oldest (back) first, newest (front) last.
    // Each layer is a filled area under the spectrum curve, dimmer with age.
    let num_layers = 8.min(state.spectrum_history.len);
    let layer_offset_px = (px_h as f32 * 0.06).max(1.0); // Vertical shift per layer.

    for (age, frame) in state
        .spectrum_history
        .iter_newest_first(num_layers)
        .enumerate()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let brightness = 1.0 - (age as f32 / num_layers as f32) * 0.7;
        let y_offset = age as f32 * layer_offset_px;

        for px_x in 0..px_w {
            // Map pixel X to spectrum bar with interpolation.
            let bar_f = px_x as f32 * (NUM_BARS - 1) as f32 / (px_w - 1).max(1) as f32;
            let bar_lo = (bar_f as usize).min(NUM_BARS - 1);
            let bar_hi = (bar_lo + 1).min(NUM_BARS - 1);
            let frac = bar_f - bar_lo as f32;
            let energy = frame[bar_lo] * (1.0 - frac) + frame[bar_hi] * frac;

            if energy < 0.02 {
                continue;
            }

            // Fill from bottom up to the energy height.
            let peak_y = px_h as f32 * (1.0 - energy * 0.9) + y_offset;
            let bottom_y = px_h as f32;

            let freq_t = bar_f / (NUM_BARS - 1) as f32;
            let warped = (freq_t + drift + state.beat_hue_offset).rem_euclid(1.0);
            let base = state.palette.freq_color(warped);
            let color = dim(brighten(base, state.beat_energy * 0.3), 1.0 - brightness);

            let y_start = (peak_y as usize).max(0);
            let y_end = (bottom_y as usize).min(px_h);
            for py in y_start..y_end {
                grid.set_dot(px_x, py, color);
            }
        }
    }

    grid.render_to(area, buf);
}

// ── Plasma Renderer ───────────────────────────────────────────────────────

fn render_plasma(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let t = state.plasma_time;
    let r = state.reactivity;
    // Pull audio-reactive parameters from spectrum bands.
    let bass = state.spectrum[..6].iter().sum::<f32>() / 6.0 * r;
    let mids = state.spectrum[16..32].iter().sum::<f32>() / 16.0 * r;
    let treble = state.spectrum[36..].iter().sum::<f32>() / 12.0 * r;

    for row in 0..h {
        let y = area.y + row as u16;
        let ny = row as f32 / h as f32;
        for col in 0..w {
            let x = area.x + col as u16;
            let nx = col as f32 / w as f32;

            // Classic plasma: sum of sine waves at different frequencies/phases.
            let v1 = (nx * 6.0 + t * 1.3 + bass * 4.0).sin();
            let v2 = (ny * 8.0 - t * 0.9 + mids * 3.0).sin();
            let v3 = ((nx * 4.0 + ny * 4.0 + t * 0.7).sin() + treble * 2.0).sin();
            let v4 = ((nx * nx + ny * ny).sqrt() * 8.0 - t * 1.5 + bass * 5.0).sin();

            let v = (v1 + v2 + v3 + v4) / 4.0; // -1..1
            let n = (v + 1.0) * 0.5; // 0..1

            let warped = (n + state.beat_hue_offset).rem_euclid(1.0);
            let color = brighten(state.palette.freq_color(warped), state.beat_energy * 0.3);

            // Block character by intensity.
            let intensity = (n * 1.2).clamp(0.0, 1.0);
            let ch = if intensity > 0.8 {
                '█'
            } else if intensity > 0.6 {
                '▓'
            } else if intensity > 0.35 {
                '▒'
            } else {
                '░'
            };

            buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
        }
    }
}

// ── Tunnel Renderer ───────────────────────────────────────────────────────

fn render_tunnel(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let z_offset = state.tunnel_z;

    // Audio-driven tunnel wobble from low bands.
    let r = state.reactivity;
    let bass = state.spectrum[..6].iter().sum::<f32>() / 6.0 * r;
    let mids = state.spectrum[16..32].iter().sum::<f32>() / 16.0 * r;

    for row in 0..h {
        let y = area.y + row as u16;
        let dy = row as f32 - cy;
        for col in 0..w {
            let x = area.x + col as u16;
            // Correct for terminal aspect ratio (~2:1 char width:height).
            let dx = (col as f32 - cx) * 0.5;

            let dist = (dx * dx + dy * dy).sqrt().max(0.1);
            let angle = dy.atan2(dx);

            // Tunnel mapping: distance from center → depth, angle → texture U.
            let depth = 40.0 / dist + z_offset;
            let tex_u = angle / std::f32::consts::TAU + 0.5; // 0..1

            // Wobble the tunnel walls with bass.
            let wobble = (angle * 3.0 + z_offset * 0.5).sin() * bass * 0.3;
            let adjusted_depth = depth + wobble;

            // Ring pattern from depth.
            let ring = ((adjusted_depth * 0.8).fract() * 2.0 - 1.0).abs();
            // Stripe pattern from angle.
            let stripe = ((tex_u * 8.0 + mids * 2.0).fract() * 2.0 - 1.0).abs();

            let pattern = (ring * 0.6 + stripe * 0.4).clamp(0.0, 1.0);

            // Depth fog: farther = dimmer.
            let fog = (1.0 - dist / cx.max(cy)).clamp(0.0, 1.0);
            let brightness = pattern * fog;

            if brightness < 0.05 {
                continue;
            }

            let color_t = (tex_u + state.beat_hue_offset).rem_euclid(1.0);
            let base = state.palette.freq_color(color_t);
            let color = dim(brighten(base, state.beat_energy * 0.4), 1.0 - brightness);

            let ch = if brightness > 0.7 {
                '█'
            } else if brightness > 0.45 {
                '▓'
            } else if brightness > 0.25 {
                '▒'
            } else {
                '░'
            };

            buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
        }
    }
}

// ── Wireframe Renderer ────────────────────────────────────────────────────

fn render_wireframe(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    fill_reactive_bg(state, area, buf);
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width() as f32;
    let px_h = grid.px_height() as f32;
    if px_w < 4.0 || px_h < 4.0 {
        return;
    }

    let cx = px_w / 2.0;
    let cy = px_h / 2.0;
    let scale = cx.min(cy) * 0.6;
    let [rx, ry, rz] = state.wire_rotation;

    // Build a torus mesh: major radius R, minor radius r.
    // Beat smashes the whole shape outward.
    let r = state.reactivity;
    let r_major = 1.0 + state.beat_energy * 0.4 * r;
    let r_minor = 0.35 + state.beat_energy * 0.5 * r;
    let segments_major = 24;
    let segments_minor = 12;

    // Audio-modulate the minor radius per segment — aggressive deformation.
    let mut verts: Vec<(f32, f32, f32)> = Vec::with_capacity(segments_major * segments_minor);
    for i in 0..segments_major {
        let theta = (i as f32 / segments_major as f32) * std::f32::consts::TAU;
        let bar_idx = (i * NUM_BARS / segments_major).min(NUM_BARS - 1);
        let modulation = 1.0 + state.spectrum[bar_idx] * 2.5 * r;

        for j in 0..segments_minor {
            let phi = (j as f32 / segments_minor as f32) * std::f32::consts::TAU;
            let r = r_minor * modulation;
            let x = (r_major + r * phi.cos()) * theta.cos();
            let y = (r_major + r * phi.cos()) * theta.sin();
            let z = r * phi.sin();
            verts.push((x, y, z));
        }
    }

    // 3D rotation (Euler angles).
    let rotate = |x: f32, y: f32, z: f32| -> (f32, f32, f32) {
        // Rotate X.
        let (y1, z1) = (y * rx.cos() - z * rx.sin(), y * rx.sin() + z * rx.cos());
        // Rotate Y.
        let (x2, z2) = (x * ry.cos() + z1 * ry.sin(), -x * ry.sin() + z1 * ry.cos());
        // Rotate Z.
        let (x3, y3) = (x2 * rz.cos() - y1 * rz.sin(), x2 * rz.sin() + y1 * rz.cos());
        (x3, y3, z2)
    };

    // Perspective projection with shake + scale pulse.
    let fov = 3.0;
    let project = |x: f32, y: f32, z: f32| -> Option<(f32, f32)> {
        let depth = z + fov;
        if depth < 0.3 {
            return None;
        }
        let px = cx + x * scale * fov / depth;
        let py = cy - y * scale * fov / depth;
        Some(state.shaken(px, py, cx, cy))
    };

    // Draw edges.
    let elapsed = state.created_at.elapsed().as_secs_f32();
    let drift = (elapsed * std::f32::consts::TAU / 8.0).sin() * 0.15;

    for i in 0..segments_major {
        for j in 0..segments_minor {
            let idx = i * segments_minor + j;
            let (x, y, z) = verts[idx];
            let (rx0, ry0, rz0) = rotate(x, y, z);

            // Edge along minor circle.
            let next_j = (j + 1) % segments_minor;
            let idx2 = i * segments_minor + next_j;
            let (x2, y2, z2) = verts[idx2];
            let (rx1, ry1, rz1) = rotate(x2, y2, z2);

            if let (Some((px0, py0)), Some((px1, py1))) =
                (project(rx0, ry0, rz0), project(rx1, ry1, rz1))
            {
                let freq_t = i as f32 / segments_major as f32;
                let warped = (freq_t + drift + state.beat_hue_offset).rem_euclid(1.0);
                let color = brighten(state.palette.freq_color(warped), state.beat_energy * 0.4);
                grid.draw_line(px0, py0, px1, py1, color);
            }

            // Edge along major circle.
            let next_i = (i + 1) % segments_major;
            let idx3 = next_i * segments_minor + j;
            let (x3, y3, z3) = verts[idx3];
            let (rx2, ry2, rz2) = rotate(x3, y3, z3);

            if let (Some((px0, py0)), Some((px1, py1))) =
                (project(rx0, ry0, rz0), project(rx2, ry2, rz2))
            {
                let freq_t = i as f32 / segments_major as f32;
                let warped = (freq_t + drift + state.beat_hue_offset).rem_euclid(1.0);
                let color = brighten(state.palette.freq_color(warped), state.beat_energy * 0.3);
                grid.draw_line(px0, py0, px1, py1, color);
            }
        }
    }

    grid.render_to(area, buf);
}

// ── Metaballs Renderer ────────────────────────────────────────────────────

fn render_metaballs(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let t = state.plasma_time;
    let num_balls = 6;

    let r = state.reactivity;

    // Position each metaball driven by a spectrum band + time.
    let balls: Vec<(f32, f32, f32)> = (0..num_balls)
        .map(|i| {
            let phase = i as f32 * std::f32::consts::TAU / num_balls as f32;
            let bar_idx = (i * 8).min(NUM_BARS - 1);
            let energy = state.spectrum[bar_idx];

            let x = 0.5 + (t * 0.7 + phase).sin() * (0.35 + state.beat_energy * 0.15 * r);
            let y = 0.5 + (t * 0.5 + phase * 1.3).cos() * (0.35 + state.beat_energy * 0.15 * r);
            let radius = 0.08 + energy * 0.2 * r + state.beat_energy * 0.1 * r;
            (x, y, radius)
        })
        .collect();

    for row in 0..h {
        let y = area.y + row as u16;
        let ny = row as f32 / h as f32;
        for col in 0..w {
            let x = area.x + col as u16;
            // Correct for terminal aspect ratio.
            let nx = col as f32 / w as f32;

            // Sum metaball field: f(p) = Σ r² / |p - c|²
            let mut field = 0.0f32;
            let mut dominant_ball = 0usize;
            let mut max_contrib = 0.0f32;
            for (i, &(bx, by, br)) in balls.iter().enumerate() {
                let dx = (nx - bx) * 2.0; // Aspect correction.
                let dy = ny - by;
                let dist_sq = dx * dx + dy * dy;
                let contrib = br * br / (dist_sq + 0.001);
                field += contrib;
                if contrib > max_contrib {
                    max_contrib = contrib;
                    dominant_ball = i;
                }
            }

            // Threshold: inside the surface.
            if field < 1.0 {
                continue;
            }

            // Color by dominant ball's spectrum position.
            let freq_t = dominant_ball as f32 / (num_balls - 1) as f32;
            let warped = (freq_t + state.beat_hue_offset).rem_euclid(1.0);
            let base = state.palette.freq_color(warped);

            // Brightness by how far above threshold.
            let edge = ((field - 1.0) * 3.0).clamp(0.0, 1.0);
            let color = brighten(dim(base, 1.0 - edge), state.beat_energy * 0.3);

            let ch = if edge > 0.7 {
                '█'
            } else if edge > 0.4 {
                '▓'
            } else if edge > 0.15 {
                '▒'
            } else {
                '░'
            };

            buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
        }
    }
}

// ── Starfield Renderer ────────────────────────────────────────────────────

fn render_starfield(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    fill_reactive_bg(state, area, buf);
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width() as f32;
    let px_h = grid.px_height() as f32;
    if px_w < 4.0 || px_h < 4.0 {
        return;
    }

    let cx = px_w / 2.0;
    let cy = px_h / 2.0;

    // Beat-reactive FOV zoom — pulls stars outward on hits.
    let r = state.reactivity;
    let fov_mult = 50.0 + state.beat_energy * 40.0 * r;

    for &(sx, sy, sz) in &state.stars {
        // Perspective projection with shake + scale pulse.
        let raw_x = cx + sx * fov_mult / sz;
        let raw_y = cy + sy * fov_mult / sz;
        let (proj_x, proj_y) = state.shaken(raw_x, raw_y, cx, cy);

        if proj_x < 0.0 || proj_x >= px_w || proj_y < 0.0 || proj_y >= px_h {
            continue;
        }

        // Brightness by proximity (closer = brighter). Beat pushes all stars brighter.
        let depth_t = (1.0 - sz / 100.0).clamp(0.0, 1.0);
        let warped = (depth_t + state.beat_hue_offset).rem_euclid(1.0);
        let color = brighten(
            state.palette.freq_color(warped),
            state.beat_energy * 0.6 + depth_t * 0.4,
        );

        // Draw the star dot.
        grid.set_dot(proj_x as usize, proj_y as usize, color);

        // Motion trails — most stars streak, length scales with proximity and beat.
        let trail_threshold = 70.0 + (1.0 - state.beat_energy) * 20.0;
        if sz < trail_threshold {
            let base_len = (trail_threshold - sz) * 0.4;
            let trail_len = (base_len + state.beat_energy * 20.0 * r).min(40.0);
            let dx = proj_x - cx;
            let dy = proj_y - cy;
            let dist = (dx * dx + dy * dy).sqrt().max(1.0);
            let tx = proj_x - dx / dist * trail_len;
            let ty = proj_y - dy / dist * trail_len;
            let trail_color = dim(color, 0.4);
            grid.draw_line(proj_x, proj_y, tx, ty, trail_color);
        }
    }

    grid.render_to(area, buf);
}

// ── Terrain Renderer ──────────────────────────────────────────────────────

fn render_terrain(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w < 10 || h < 6 {
        return;
    }

    // ── Unknown Pleasures layout ────────────────────────────────────────────
    // Artist name above, album below, waveform box centered between.
    // Pure white on black, no palette colors.
    let white = Color::Rgb(255, 255, 255);
    let white_style = Style::new().fg(white).add_modifier(Modifier::BOLD);

    // Artist label: centered, 2 rows from top of area.
    let artist = state.now_artist.to_uppercase();
    let album = state.now_album.to_uppercase();

    // Layout: blank | artist | blank | viz... | blank | album | blank
    let artist_y = area.y + 1;
    let album_y = area.y + area.height.saturating_sub(2);

    // Center-render text helper.
    let render_centered = |text: &str, y: u16, buf: &mut Buffer| {
        if text.is_empty() || y >= area.y + area.height {
            return;
        }
        let text_w = text.len().min(w);
        let x_start = area.x + (area.width.saturating_sub(text_w as u16)) / 2;
        for (i, ch) in text.chars().take(w).enumerate() {
            buf[(x_start + i as u16, y)]
                .set_char(ch)
                .set_style(white_style);
        }
    };

    render_centered(&artist, artist_y, buf);
    render_centered(&album, album_y, buf);

    // ── Waveform box: centered square-ish region ────────────────────────────
    // Leave space for labels + padding.
    let box_top = (artist_y + 2).min(area.y + area.height); // blank line after artist
    let box_bottom = album_y.saturating_sub(1); // blank line before album
    if box_bottom <= box_top {
        return;
    }
    let box_h = (box_bottom - box_top) as usize;
    // Square aspect: width = height * 2 (terminal chars ~2:1).
    let box_w_chars = (box_h * 2).min(w);
    let box_x = area.x + (area.width.saturating_sub(box_w_chars as u16)) / 2;
    let box_area = Rect::new(box_x, box_top, box_w_chars as u16, box_h as u16);

    // Render waveform ridgelines into the box.
    let mut grid = BrailleGrid::new(box_area.width as usize, box_area.height as usize);
    let px_w = grid.px_width();
    let px_h = grid.px_height();
    if px_w < 4 || px_h < 4 {
        return;
    }

    let num_rows = 40.min(state.spectrum_history.len);
    if num_rows < 2 {
        return;
    }

    let frames: Vec<&[f32; NUM_BARS]> =
        state.spectrum_history.iter_newest_first(num_rows).collect();

    // Draw oldest first (top) → newest last (bottom), so newest ridgelines
    // overlay older ones — matches the Unknown Pleasures stacking.
    for (depth_idx, frame) in frames.iter().enumerate().rev() {
        let t = depth_idx as f32 / num_rows as f32;
        // Newest (idx 0) at bottom, oldest (idx N) at top.
        // Top margin for breathing room, zero bottom margin.
        let row_y = px_h as f32 * (0.08 + (1.0 - t) * 0.92);
        // Height scale: capped so a 100% peak exactly touches the top of the box.
        // This prevents clipping on the upper ridgelines.
        let height_scale = (px_h as f32 * 0.25).min(row_y);

        let mut prev_x = None;
        let mut prev_y = None;

        for px_x in 0..px_w {
            let bar_f = px_x as f32 * (NUM_BARS - 1) as f32 / (px_w - 1).max(1) as f32;
            let bar_lo = (bar_f as usize).min(NUM_BARS - 1);
            let bar_hi = (bar_lo + 1).min(NUM_BARS - 1);
            let frac = bar_f - bar_lo as f32;
            let raw_energy = frame[bar_lo] * (1.0 - frac) + frame[bar_hi] * frac;

            // Horizontal window: raised cosine tapers to zero at edges,
            // giving the "hills rising from flat baseline" look.
            let nx = px_x as f32 / (px_w - 1).max(1) as f32; // 0..1
            let window = (nx * std::f32::consts::PI).sin().powi(2);
            let energy = raw_energy * window;

            let y = (row_y - energy * height_scale).clamp(0.0, (px_h - 1) as f32);

            if let (Some(px), Some(py)) = (prev_x, prev_y) {
                grid.draw_line(px, py, px_x as f32, y, white);
            }

            prev_x = Some(px_x as f32);
            prev_y = Some(y);
        }
    }

    // Override the braille render — we want pure white, no brightness boost
    // (it's already white). Render directly instead of through render_to.
    for cy in 0..grid.height.min(box_area.height as usize) {
        for cx in 0..grid.width.min(box_area.width as usize) {
            let idx = cy * grid.width + cx;
            let pattern = grid.dots[idx];
            if pattern == 0 {
                continue;
            }
            let ch = char::from_u32(0x2800 + pattern as u32).unwrap_or(' ');
            let x = box_area.x + cx as u16;
            let y = box_area.y + cy as u16;
            buf[(x, y)].set_char(ch).set_style(white_style);
        }
    }
}

// ── Moiré Renderer ────────────────────────────────────────────────────────

fn render_moire(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let t = state.plasma_time;
    let r = state.reactivity;
    let bass = state.spectrum[..6].iter().sum::<f32>() / 6.0;
    let mids = state.spectrum[16..32].iter().sum::<f32>() / 16.0;

    // Three rotating line grids at different angles/spacings.
    // Slow rotation — moiré is already visually intense, keep it dreamy.
    let angle1 = t * (0.08 + bass * 0.3 * r);
    let angle2 = t * (0.06 + mids * 0.2 * r) + std::f32::consts::TAU / 3.0;
    let angle3 = t * (0.04 + state.beat_energy * 0.5 * r) + std::f32::consts::TAU * 2.0 / 3.0;

    // Tighter spacing range — wide spacings cause seizure-inducing flicker.
    let spacing1 = 6.0 + bass * 2.0 * r;
    let spacing2 = 7.0 + mids * 1.5 * r;
    let spacing3 = 8.0 + state.beat_energy * 2.0 * r;

    for row in 0..h {
        let y = area.y + row as u16;
        let ny = (row as f32 / h as f32 - 0.5) * 2.0;
        for col in 0..w {
            let x = area.x + col as u16;
            let nx = (col as f32 / w as f32 - 0.5) * 2.0 * 0.5; // Aspect correct.

            // Project each pixel onto each grid's perpendicular axis.
            let d1 = (nx * angle1.cos() + ny * angle1.sin()) * spacing1;
            let d2 = (nx * angle2.cos() + ny * angle2.sin()) * spacing2;
            let d3 = (nx * angle3.cos() + ny * angle3.sin()) * spacing3;

            // Line pattern: sin² gives smooth stripes.
            let v1 = d1.sin().powi(2);
            let v2 = d2.sin().powi(2);
            let v3 = d3.sin().powi(2);

            // Interference: multiply the patterns.
            let v = (v1 * v2 * v3).sqrt();

            if v < 0.05 {
                continue;
            }

            let color_t = (v + state.beat_hue_offset).rem_euclid(1.0);
            let base = state.palette.freq_color(color_t);
            let color = brighten(base, state.beat_energy * 0.4 * r);

            let ch = if v > 0.7 {
                '█'
            } else if v > 0.45 {
                '▓'
            } else if v > 0.2 {
                '▒'
            } else {
                '░'
            };

            buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
        }
    }
}

// ── Kaleidoscope Renderer ─────────────────────────────────────────────────

fn render_kaleidoscope(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    fill_reactive_bg(state, area, buf);
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width() as f32;
    let px_h = grid.px_height() as f32;
    if px_w < 4.0 || px_h < 4.0 {
        return;
    }

    let cx = px_w / 2.0;
    let cy = px_h / 2.0;
    let r = state.reactivity;
    let t = state.plasma_time;
    let segments = 8; // 8-fold symmetry.
    let segment_angle = std::f32::consts::TAU / segments as f32;
    let rotation = t * (0.3 + state.beat_energy * 1.5 * r);

    // Source pattern: use spectrum as a radial function + waveform for texture.
    let max_radius = cx.min(cy) * 0.95;

    for py in 0..px_h as usize {
        for px in 0..px_w as usize {
            let dx = px as f32 - cx;
            let dy = py as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > max_radius || dist < 1.0 {
                continue;
            }

            // Convert to polar, fold into first segment.
            let angle = (dy.atan2(dx) + rotation).rem_euclid(std::f32::consts::TAU);
            let mut seg_angle = angle % segment_angle;
            // Mirror alternate segments for kaleidoscope symmetry.
            let seg_idx = (angle / segment_angle) as usize;
            if seg_idx % 2 == 1 {
                seg_angle = segment_angle - seg_angle;
            }

            // Map the folded polar coords to a pattern.
            let norm_r = dist / max_radius;
            let bar_idx =
                ((seg_angle / segment_angle * NUM_BARS as f32) as usize).min(NUM_BARS - 1);
            let energy = state.spectrum[bar_idx];

            // Radial rings modulated by spectrum.
            let ring = (norm_r * 12.0 + t * 2.0 + energy * 6.0 * r).sin() * 0.5 + 0.5;
            let angular = (seg_angle * 20.0 + t * 3.0).cos() * 0.3 + 0.7;
            let intensity = (ring * angular * (0.5 + energy * r)).clamp(0.0, 1.0);

            if intensity < 0.1 {
                continue;
            }

            let freq_t = (norm_r + state.beat_hue_offset).rem_euclid(1.0);
            let base = state.palette.freq_color(freq_t);
            let color = brighten(base, state.beat_energy * 0.4 * r + intensity * 0.2);

            let (sx, sy) = state.shaken(px as f32, py as f32, cx, cy);
            grid.set_dot(sx as usize, sy as usize, color);
        }
    }

    grid.render_to(area, buf);
}

// ── Julia Fractal Renderer ────────────────────────────────────────────────

fn render_julia(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let r = state.reactivity;
    let t = state.plasma_time;
    let bass = state.spectrum[..6].iter().sum::<f32>() / 6.0;
    let mids = state.spectrum[16..32].iter().sum::<f32>() / 16.0;

    // Julia constant: orbits slowly, bass/mids perturb it.
    let c_re = (t * 0.15).sin() * 0.7885 + bass * 0.3 * r;
    let c_im = (t * 0.13).cos() * 0.7885 + mids * 0.2 * r;

    // Zoom drifts in slowly, beat punches in.
    let zoom = 1.5 + (t * 0.05).sin() * 0.3 - state.beat_energy * 0.4 * r;
    let max_iter = 40;

    for row in 0..h {
        let y = area.y + row as u16;
        for col in 0..w {
            let x = area.x + col as u16;

            // Map pixel to complex plane. Aspect correct.
            let mut zr = (col as f32 / w as f32 - 0.5) * 2.0 * zoom * 0.5; // Aspect.
            let mut zi = (row as f32 / h as f32 - 0.5) * 2.0 * zoom;

            let mut iter = 0u32;
            while iter < max_iter && zr * zr + zi * zi < 4.0 {
                let tmp = zr * zr - zi * zi + c_re;
                zi = 2.0 * zr * zi + c_im;
                zr = tmp;
                iter += 1;
            }

            if iter == max_iter {
                continue; // Inside the set — black.
            }

            // Smooth coloring: fractional escape count.
            let smooth = iter as f32 + 1.0 - (zr * zr + zi * zi).ln().ln() / 2.0f32.ln();
            let t_color = (smooth / max_iter as f32 + state.beat_hue_offset).rem_euclid(1.0);
            let base = state.palette.freq_color(t_color);
            let brightness = (smooth / max_iter as f32).clamp(0.0, 1.0);
            let color = brighten(base, state.beat_energy * 0.3 * r + brightness * 0.2);

            let ch = if brightness > 0.6 {
                '█'
            } else if brightness > 0.35 {
                '▓'
            } else if brightness > 0.15 {
                '▒'
            } else {
                '░'
            };

            buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
        }
    }
}

// ── Spiral Renderer ───────────────────────────────────────────────────────

fn render_spiral(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    fill_reactive_bg(state, area, buf);
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width() as f32;
    let px_h = grid.px_height() as f32;
    if px_w < 4.0 || px_h < 4.0 {
        return;
    }

    let cx = px_w / 2.0;
    let cy = px_h / 2.0;
    let r = state.reactivity;
    let t = state.plasma_time;
    let max_radius = cx.min(cy) * 0.95;

    // Draw multiple interleaved spirals.
    let num_arms = 3 + (state.beat_energy * 4.0 * r) as usize; // 3-7 arms.
    let rotation = t * (0.8 + state.beat_energy * 3.0 * r);
    let bass = state.spectrum[..6].iter().sum::<f32>() / 6.0;

    for arm in 0..num_arms {
        let arm_offset = arm as f32 * std::f32::consts::TAU / num_arms as f32;

        // Archimedean spiral: r = a + b*θ. Draw ~500 points along it.
        let num_points = 600;
        let max_theta = std::f32::consts::TAU * 4.0; // 4 full turns.

        let mut prev_x = None;
        let mut prev_y = None;

        for i in 0..num_points {
            let frac = i as f32 / num_points as f32;
            let theta = frac * max_theta + rotation + arm_offset;

            // Radius: linear growth + spectrum modulation.
            let bar_idx = ((frac * (NUM_BARS - 1) as f32) as usize).min(NUM_BARS - 1);
            let energy = state.spectrum[bar_idx];
            let base_r = frac * max_radius;
            let modulated_r = base_r * (1.0 + energy * 0.5 * r + bass * 0.3 * r);

            // Breathe: scale pulse.
            let breath = 1.0 + (t * 1.5).sin() * 0.1 * r + state.beat_energy * 0.15 * r;
            let final_r = modulated_r * breath;

            let px = cx + final_r * theta.cos();
            let py = cy + final_r * theta.sin();

            let (sx, sy) = state.shaken(px, py, cx, cy);

            if let (Some(prev_px), Some(prev_py)) = (prev_x, prev_y) {
                let freq_t = (frac + state.beat_hue_offset + arm as f32 * 0.15).rem_euclid(1.0);
                let base = state.palette.freq_color(freq_t);
                let color = brighten(base, state.beat_energy * 0.4 * r + energy * 0.3);
                grid.draw_line(prev_px, prev_py, sx, sy, color);
            }

            prev_x = Some(sx);
            prev_y = Some(sy);
        }
    }

    grid.render_to(area, buf);
}

// ── Interference Renderer ─────────────────────────────────────────────────

fn render_interference(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let t = state.plasma_time;
    let r = state.reactivity;
    let num_sources = 5;

    // Position each wave source driven by spectrum bands.
    let sources: Vec<(f32, f32, f32)> = (0..num_sources)
        .map(|i| {
            let phase = i as f32 * std::f32::consts::TAU / num_sources as f32;
            let bar_idx = (i * 10).min(NUM_BARS - 1);
            let energy = state.spectrum[bar_idx];

            let x = 0.5 + (t * 0.5 + phase).sin() * (0.3 + state.beat_energy * 0.15 * r);
            let y = 0.5 + (t * 0.4 + phase * 1.3).cos() * (0.3 + state.beat_energy * 0.15 * r);
            // Wavelength: shorter when band is energetic.
            let wavelength = 8.0 + (1.0 - energy) * 12.0;
            (x, y, wavelength)
        })
        .collect();

    for row in 0..h {
        let y_pos = area.y + row as u16;
        let ny = row as f32 / h as f32;
        for col in 0..w {
            let x_pos = area.x + col as u16;
            let nx = col as f32 / w as f32;

            // Sum wave contributions from all sources.
            let mut wave_sum = 0.0f32;
            for &(sx, sy, wl) in &sources {
                let dx = (nx - sx) * 2.0; // Aspect correct.
                let dy = ny - sy;
                let dist = (dx * dx + dy * dy).sqrt();
                // Concentric waves: sin(dist * freq - time).
                let freq = std::f32::consts::TAU / wl;
                wave_sum += (dist * freq * 20.0 - t * 3.0).sin();
            }

            // Normalize to 0..1.
            let v = (wave_sum / num_sources as f32 + 1.0) * 0.5;

            if !(0.05..=0.95).contains(&v) {
                continue;
            }

            let color_t = (v + state.beat_hue_offset).rem_euclid(1.0);
            let base = state.palette.freq_color(color_t);
            let color = brighten(base, state.beat_energy * 0.4 * r);

            let ch = if !(0.3..=0.7).contains(&v) {
                '█'
            } else if !(0.4..=0.6).contains(&v) {
                '▓'
            } else {
                '▒'
            };

            buf[(x_pos, y_pos)]
                .set_char(ch)
                .set_style(Style::new().fg(color));
        }
    }
}

// ── Wormhole Renderer ─────────────────────────────────────────────────────

/// Deterministic pseudo-random from an integer seed. Returns 0.0..1.0.
#[inline]
fn hash_f32(seed: u32) -> f32 {
    // Fast integer hash (Wang hash variant).
    let mut x = seed;
    x = x.wrapping_mul(0x85ebca6b);
    x ^= x >> 13;
    x = x.wrapping_mul(0xc2b2ae35);
    x ^= x >> 16;
    (x & 0xFFFF) as f32 / 65535.0
}

fn render_wormhole(state: &VisualizerState, area: Rect, buf: &mut Buffer) {
    fill_reactive_bg(state, area, buf);
    let mut grid = BrailleGrid::new(area.width as usize, area.height as usize);
    let px_w = grid.px_width() as f32;
    let px_h = grid.px_height() as f32;
    if px_w < 4.0 || px_h < 4.0 {
        return;
    }

    let cx = px_w / 2.0;
    let cy = px_h / 2.0;
    let r = state.reactivity;
    let t = state.plasma_time;
    let cam_z = state.tunnel_z;
    let bass = state.spectrum[..6].iter().sum::<f32>() / 6.0;

    // ── Background stars ────────────────────────────────────────────────────
    // Reuse the persistent star array but project relative to camera Z.
    let star_fov = 60.0 + state.beat_energy * 30.0 * r;
    for &(sx, sy, sz) in &state.stars {
        // Stars at fixed world positions — camera moves through them.
        let rel_z = ((sz * 3.0 - cam_z * 5.0) % 300.0 + 300.0) % 300.0;
        if rel_z < 1.0 {
            continue;
        }
        let proj_x = cx + sx * star_fov / rel_z;
        let proj_y = cy + sy * star_fov / rel_z;
        if proj_x < 0.0 || proj_x >= px_w || proj_y < 0.0 || proj_y >= px_h {
            continue;
        }
        let depth_t = (1.0 - rel_z / 300.0).clamp(0.0, 1.0);
        let color = dim(
            state
                .palette
                .freq_color((depth_t + state.beat_hue_offset).rem_euclid(1.0)),
            0.4 + depth_t * 0.3,
        );
        grid.set_dot(proj_x as usize, proj_y as usize, color);
    }

    // ── Wireframe tunnel rings ──────────────────────────────────────────────
    let num_rings = 60;
    let ring_spacing = 2.0;
    let ring_verts = 24; // Vertices per ring.
    let fov = 80.0 + state.beat_energy * 40.0 * r;

    // Camera Z determines which ring indices are visible.
    // Ring i is at world Z = i * ring_spacing.
    let first_ring = (cam_z / ring_spacing) as i32;

    for ring_offset in 0..num_rings {
        let ring_idx = first_ring + ring_offset;
        let ring_z_world = ring_idx as f32 * ring_spacing;
        let rel_z = ring_z_world - cam_z;

        // Only render rings ahead of the camera.
        if rel_z < 0.5 || rel_z > ring_spacing * num_rings as f32 {
            continue;
        }

        let depth = rel_z;
        let seed = ring_idx as u32;

        // Each ring has a procedural center offset that snakes around.
        // Low-frequency wander + spectrum modulation.
        let wander_x = (hash_f32(seed) - 0.5) * 2.0
            + (ring_z_world * 0.07 + t * 0.3).sin() * 1.2
            + bass * (hash_f32(seed.wrapping_add(100)) - 0.5) * 3.0 * r;
        let wander_y = (hash_f32(seed.wrapping_add(1)) - 0.5) * 2.0
            + (ring_z_world * 0.09 + t * 0.2).cos() * 1.2
            + bass * (hash_f32(seed.wrapping_add(200)) - 0.5) * 3.0 * r;

        // Ring radius: base + spectrum modulation + beat pulse.
        let bar_idx = (ring_offset as usize * NUM_BARS / num_rings as usize).min(NUM_BARS - 1);
        let energy = state.spectrum[bar_idx];
        let base_radius = 2.0 + hash_f32(seed.wrapping_add(2)) * 1.5;
        let radius = base_radius + energy * 2.0 * r + state.beat_energy * 1.0 * r;

        // Project ring vertices.
        let mut projected: Vec<Option<(f32, f32)>> = Vec::with_capacity(ring_verts);
        for v in 0..ring_verts {
            let angle = v as f32 / ring_verts as f32 * std::f32::consts::TAU;
            // Wobble each vertex for organic feel.
            let wobble = 1.0
                + (angle * 3.0 + ring_z_world * 0.5 + t).sin() * 0.15 * r
                + energy * (angle * 5.0 + t * 2.0).sin().abs() * 0.3 * r;
            let vx = wander_x + radius * wobble * angle.cos();
            let vy = wander_y + radius * wobble * angle.sin();

            let px = cx + vx * fov / depth;
            let py = cy + vy * fov / depth;
            let (sx, sy) = state.shaken(px, py, cx, cy);

            if sx >= 0.0 && sx < px_w && sy >= 0.0 && sy < px_h {
                projected.push(Some((sx, sy)));
            } else {
                projected.push(None);
            }
        }

        // Draw wireframe edges around the ring.
        let depth_t = (1.0 - rel_z / (ring_spacing * num_rings as f32)).clamp(0.0, 1.0);
        let freq_t = (depth_t + state.beat_hue_offset).rem_euclid(1.0);
        let base_color = state.palette.freq_color(freq_t);
        let color = brighten(base_color, state.beat_energy * 0.4 * r + depth_t * 0.3);
        let color = dim(color, 1.0 - depth_t * 0.7); // Far rings dimmer.

        for v in 0..ring_verts {
            let next_v = (v + 1) % ring_verts;
            if let (Some((x0, y0)), Some((x1, y1))) = (projected[v], projected[next_v]) {
                grid.draw_line(x0, y0, x1, y1, color);
            }
        }

        // Connect to next ring with longitudinal struts (every 4th vertex).
        if ring_offset + 1 < num_rings {
            let next_ring_idx = ring_idx + 1;
            let next_rel_z = (next_ring_idx as f32 * ring_spacing) - cam_z;
            if next_rel_z > 0.5 {
                let next_seed = next_ring_idx as u32;
                let next_wander_x = (hash_f32(next_seed) - 0.5) * 2.0
                    + (next_ring_idx as f32 * ring_spacing * 0.07 + t * 0.3).sin() * 1.2
                    + bass * (hash_f32(next_seed.wrapping_add(100)) - 0.5) * 3.0 * r;
                let next_wander_y = (hash_f32(next_seed.wrapping_add(1)) - 0.5) * 2.0
                    + (next_ring_idx as f32 * ring_spacing * 0.09 + t * 0.2).cos() * 1.2
                    + bass * (hash_f32(next_seed.wrapping_add(200)) - 0.5) * 3.0 * r;
                let next_bar_idx =
                    ((ring_offset as usize + 1) * NUM_BARS / num_rings as usize).min(NUM_BARS - 1);
                let next_energy = state.spectrum[next_bar_idx];
                let next_base_r = 2.0 + hash_f32(next_seed.wrapping_add(2)) * 1.5;
                let next_radius = next_base_r + next_energy * 2.0 * r + state.beat_energy * 1.0 * r;

                for v in (0..ring_verts).step_by(3) {
                    let angle = v as f32 / ring_verts as f32 * std::f32::consts::TAU;
                    let nvx = next_wander_x + next_radius * angle.cos();
                    let nvy = next_wander_y + next_radius * angle.sin();
                    let npx = cx + nvx * fov / next_rel_z;
                    let npy = cy + nvy * fov / next_rel_z;
                    let (nsx, nsy) = state.shaken(npx, npy, cx, cy);

                    if let Some((x0, y0)) = projected[v]
                        && nsx >= 0.0
                        && nsx < px_w
                        && nsy >= 0.0
                        && nsy < px_h
                    {
                        let strut_color = dim(color, 0.3);
                        grid.draw_line(x0, y0, nsx, nsy, strut_color);
                    }
                }
            }
        }
    }

    grid.render_to(area, buf);
}

// ── Matrix Rain Renderer ──────────────────────────────────────────────────

/// Half-width katakana + digits + symbols for that authentic matrix look.
const MATRIX_CHARS: &[char] = &[
    'ﾊ', 'ﾐ', 'ﾋ', 'ｰ', 'ｳ', 'ｼ', 'ﾅ', 'ﾓ', 'ﾆ', 'ｻ', 'ﾜ', 'ﾂ', 'ｵ', 'ﾘ', 'ｱ', 'ﾎ', 'ﾃ', 'ﾏ', 'ｹ',
    'ﾒ', 'ｴ', 'ｶ', 'ｷ', 'ﾑ', 'ﾕ', 'ﾗ', 'ｾ', 'ﾈ', 'ｽ', 'ﾀ', 'ﾇ', 'ﾍ', '0', '1', '2', '3', '4', '5',
    '7', '8', '9', 'Z', ':', '.', '=', '*', '+', '-', '<', '>', '¦', '|',
];

fn render_matrix(state: &mut VisualizerState, area: Rect, buf: &mut Buffer) {
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let r = state.reactivity;
    let bass = state.spectrum[..6].iter().sum::<f32>() / 6.0;

    // Lazy init: create columns on first render or if terminal resized.
    if state.matrix_cols.len() != w {
        state.matrix_cols.clear();
        for col in 0..w {
            let seed = col as u32 * 2654435761;
            state.matrix_cols.push(MatrixColumn {
                head_y: -(hash_f32(seed) * h as f32 * 2.0), // Stagger widely.
                speed: 0.15 + hash_f32(seed.wrapping_add(1)) * 0.25,
                trail_len: 8.0 + hash_f32(seed.wrapping_add(2)) * 12.0,
                char_seed: seed,
            });
        }
    }

    // Beat cluster spawn: on a beat, pick a cluster of ~8-15 adjacent columns
    // and reset them to the top together — creates a visible group.
    if state.beat_energy > 0.4 && bass > 0.2 {
        let cluster_seed = (state.plasma_time * 100.0) as u32;
        let center = (hash_f32(cluster_seed) * w as f32) as usize;
        let width = 6 + (hash_f32(cluster_seed.wrapping_add(1)) * 10.0) as usize;
        let start = center.saturating_sub(width / 2);
        let end = (center + width / 2).min(w);
        for col_idx in start..end {
            let col = &mut state.matrix_cols[col_idx];
            // Only respawn if this column has already fallen past the top.
            if col.head_y > 0.0 {
                col.head_y = -(hash_f32(col.char_seed.wrapping_add(cluster_seed)) * 3.0);
                col.speed = 0.15 + state.beat_energy * 0.2 * r;
                col.trail_len = 10.0 + bass * 10.0 * r;
            }
        }
    }

    // Overall amplitude for density control.
    let overall_energy: f32 = state.spectrum.iter().sum::<f32>() / NUM_BARS as f32;

    // Update and render each column.
    for (col_idx, column) in state.matrix_cols.iter_mut().enumerate() {
        // Map column to a spectrum band for per-column reactivity.
        let bar_idx = (col_idx * NUM_BARS / w).min(NUM_BARS - 1);
        let band_energy = state.spectrum[bar_idx];

        // Density: quiet music = fewer active columns.
        // Each column has a threshold from its hash — only renders if energy exceeds it.
        let density_threshold = hash_f32(column.char_seed.wrapping_add(99)) * 0.6;
        let is_active = overall_energy > density_threshold || state.beat_energy > 0.3;

        // Speed: driven by peak mid-low energy (drums ~80-500Hz).
        // Exponential curve: quiet barely moves, only loud drums slam it.
        let drums = state.spectrum[4..16].iter().cloned().fold(0.0f32, f32::max);
        let speed_mult =
            0.05 + (drums * drums) * 20.0 * r + (state.beat_energy * state.beat_energy) * 8.0 * r;
        column.head_y += column.speed * speed_mult;

        // Respawn when fully off-screen.
        if column.head_y > (h as f32 + column.trail_len + 5.0) {
            column.head_y = -(hash_f32(column.char_seed.wrapping_add(state.plasma_time as u32))
                * h as f32
                * 0.5);
            column.speed = 0.15 + hash_f32(column.char_seed.wrapping_mul(7)) * 0.25;
            column.trail_len = 8.0 + band_energy * 6.0 * r + hash_f32(column.char_seed) * 8.0;
        }

        // Skip rendering inactive columns (still advance position above).
        if !is_active {
            continue;
        }

        // Trail length: gentle pulse with energy.
        let effective_trail = column.trail_len + band_energy * 3.0 * r;

        // Render the trail.
        let head = column.head_y as i32;
        let x = area.x + col_idx as u16;

        for row in 0..h {
            let y = area.y + row as u16;
            let dist_from_head = head - row as i32;

            // Only render if within the trail.
            if dist_from_head < 0 || dist_from_head as f32 > effective_trail {
                continue;
            }

            let trail_frac = dist_from_head as f32 / effective_trail;

            // Character: pseudo-random per position, flickers at staggered intervals.
            // Each row has its own phase so they don't all flip simultaneously.
            let row_phase = column.char_seed.wrapping_add(row as u32 * 137);
            let flicker_rate = 2.0; // Changes per second.
            let flicker_tick = ((state.plasma_time * flicker_rate) as u32).wrapping_add(row_phase);
            let char_hash = row_phase.wrapping_mul(31).wrapping_add(flicker_tick);
            let ch = MATRIX_CHARS[char_hash as usize % MATRIX_CHARS.len()];

            // Color: head is bright white/green, trail fades to dark green.
            let (fg, bold) = if dist_from_head == 0 {
                // Head: bright white, pulsing with beat.
                let brightness = 0.8 + state.beat_energy * 0.2 * r;
                (
                    Color::Rgb((200.0 * brightness) as u8, 255, (200.0 * brightness) as u8),
                    true,
                )
            } else if dist_from_head <= 2 {
                // Near head: bright green.
                (Color::Rgb(50, (220.0 + band_energy * 35.0) as u8, 50), true)
            } else {
                // Trail: fade from green to dark, intensity from band energy.
                let fade = (1.0 - trail_frac).max(0.0);
                let g = (180.0 * fade + band_energy * 60.0 * r) as u8;
                let rb = (30.0 * fade) as u8;
                (Color::Rgb(rb, g.max(rb), rb), false)
            };

            let mut style = Style::new().fg(fg);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            buf[(x, y)].set_char(ch).set_style(style);
        }
    }
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
            1.0,
            true,
            false,
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
            1.0,
            true,
            false,
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
            1.0,
            true,
            false,
        );
        let snapshot = VizSnapshot::new();

        // Write a frame with some energy.
        let mut spectrum = [0.0f32; NUM_BARS];
        spectrum[10] = 0.8;
        spectrum[20] = 0.5;
        snapshot.write(VizFrame {
            spectrum,
            peaks: [0.0; NUM_BARS],
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
            1.0,
            true,
            false,
        );
        let snapshot = VizSnapshot::new();

        // Seed some energy.
        let spectrum = [1.0f32; NUM_BARS];
        snapshot.write(VizFrame {
            spectrum,
            peaks: [1.0; NUM_BARS],
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
            1.0,
            true,
            false,
        );
        let snapshot = VizSnapshot::new();

        // Push a loud frame — peaks come from the analyzer via VizFrame.
        let mut spectrum = [0.0f32; NUM_BARS];
        spectrum[5] = 0.9;
        let mut peaks = [0.0f32; NUM_BARS];
        peaks[5] = 0.9;
        snapshot.write(VizFrame {
            spectrum,
            peaks,
            vu_levels: [0.0; 2],
            beat_energy: 0.0,
            timestamp: Instant::now(),
            waveform: Vec::new(),
        });
        state.update_from_snapshot(&snapshot);
        assert!(state.peaks[5] > 0.5, "peak should match analyzer value");

        // Push silence with decayed peak — simulates analyzer's own peak decay.
        let mut decayed_peaks = [0.0f32; NUM_BARS];
        decayed_peaks[5] = 0.7; // Analyzer decayed it a bit.
        snapshot.write(VizFrame {
            spectrum: [0.0; NUM_BARS],
            peaks: decayed_peaks,
            vu_levels: [0.0; 2],
            beat_energy: 0.0,
            timestamp: Instant::now(),
            waveform: Vec::new(),
        });
        state.update_from_snapshot(&snapshot);
        assert!(
            state.peaks[5] > 0.0,
            "peak should not instantly zero — analyzer provides gradual decay"
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
        assert_eq!(
            VisualizerMode::parse("spectrogram"),
            VisualizerMode::Spectrogram
        );
        assert_eq!(
            VisualizerMode::parse("waterfall"),
            VisualizerMode::Spectrogram
        );
        assert_eq!(
            VisualizerMode::parse("stereo"),
            VisualizerMode::StereoWaveform
        );
        assert_eq!(VisualizerMode::parse("vu"), VisualizerMode::VuMeter);
        assert_eq!(VisualizerMode::parse("meter"), VisualizerMode::VuMeter);
        assert_eq!(VisualizerMode::parse("flame"), VisualizerMode::Flame);
        assert_eq!(VisualizerMode::parse("mountain"), VisualizerMode::Flame);
        assert_eq!(VisualizerMode::parse("plasma"), VisualizerMode::Plasma);
        assert_eq!(VisualizerMode::parse("tunnel"), VisualizerMode::Tunnel);
        assert_eq!(
            VisualizerMode::parse("wireframe"),
            VisualizerMode::Wireframe
        );
        assert_eq!(VisualizerMode::parse("wire"), VisualizerMode::Wireframe);
        assert_eq!(VisualizerMode::parse("3d"), VisualizerMode::Wireframe);
        assert_eq!(
            VisualizerMode::parse("metaballs"),
            VisualizerMode::Metaballs
        );
        assert_eq!(VisualizerMode::parse("blobs"), VisualizerMode::Metaballs);
        assert_eq!(
            VisualizerMode::parse("starfield"),
            VisualizerMode::Starfield
        );
        assert_eq!(VisualizerMode::parse("stars"), VisualizerMode::Starfield);
        assert_eq!(VisualizerMode::parse("terrain"), VisualizerMode::Terrain);
        assert_eq!(VisualizerMode::parse("landscape"), VisualizerMode::Terrain);
        assert_eq!(VisualizerMode::parse("moire"), VisualizerMode::Moire);
        assert_eq!(
            VisualizerMode::parse("kaleidoscope"),
            VisualizerMode::Kaleidoscope
        );
        assert_eq!(VisualizerMode::parse("julia"), VisualizerMode::Julia);
        assert_eq!(VisualizerMode::parse("fractal"), VisualizerMode::Julia);
        assert_eq!(VisualizerMode::parse("spiral"), VisualizerMode::Spiral);
        assert_eq!(
            VisualizerMode::parse("interference"),
            VisualizerMode::Interference
        );
        assert_eq!(
            VisualizerMode::parse("ripples"),
            VisualizerMode::Interference
        );
        assert_eq!(VisualizerMode::parse("wormhole"), VisualizerMode::Wormhole);
        assert_eq!(VisualizerMode::parse("matrix"), VisualizerMode::Matrix);
        assert_eq!(VisualizerMode::parse("cmatrix"), VisualizerMode::Matrix);
        assert_eq!(VisualizerMode::parse("pleasures"), VisualizerMode::Terrain);
        assert_eq!(VisualizerMode::parse("garbage"), VisualizerMode::Bars);
    }

    #[test]
    fn mode_cycles_through_all() {
        let mut mode = VisualizerMode::Bars;
        let expected = [
            VisualizerMode::Oscilloscope,
            VisualizerMode::Radial,
            VisualizerMode::Particles,
            VisualizerMode::Lissajous,
            VisualizerMode::Spectrogram,
            VisualizerMode::StereoWaveform,
            VisualizerMode::VuMeter,
            VisualizerMode::Flame,
            VisualizerMode::Plasma,
            VisualizerMode::Tunnel,
            VisualizerMode::Wireframe,
            VisualizerMode::Metaballs,
            VisualizerMode::Starfield,
            VisualizerMode::Terrain,
            VisualizerMode::Moire,
            VisualizerMode::Kaleidoscope,
            VisualizerMode::Julia,
            VisualizerMode::Spiral,
            VisualizerMode::Interference,
            VisualizerMode::Wormhole,
            VisualizerMode::Matrix,
            VisualizerMode::Bars,
        ];
        for exp in expected {
            mode = mode.next();
            assert_eq!(mode, exp);
        }
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
