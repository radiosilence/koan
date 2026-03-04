# Plan: ANSI Art Audio Visualizers

## Summary

Terminal-based audio visualization integrated into koan's TUI. The core challenge is tapping into the real-time audio stream without disrupting the lock-free CoreAudio path, then processing PCM data through FFT to produce frequency/time-domain visuals rendered via Ratatui widgets using Unicode block and braille characters.

This is fully feasible. The audio architecture already has a clean separation between decode, ring buffer, and render callback. The existing `cover_art.rs` halfblock rendering proves the terminal can handle pixel-level graphics at 20fps. The main architectural decision is _where_ to tap the audio data.

## What

There should be a old-school-stereo "vertical bar" style visualizer above the now playing display and to the right of the album art, this should be designed as if it's a multi-modal display (could display stuff like lyrics instead etc)

## Audio Data Tap Architecture

### The Problem

koan's audio path: `Decode Thread → rtrb Producer → [SPSC Ring Buffer] → rtrb Consumer → CoreAudio RT Callback → DAC`

rtrb is SPSC (Single-Producer, Single-Consumer). There is exactly one consumer, owned by the CoreAudio render callback. We cannot add a second consumer. The render callback is real-time — it MUST NOT allocate, lock, or do anything that could block.

### Option A: Copy in the Render Callback (REJECTED)

Add a secondary ring buffer write inside `render_callback()`. The callback already does `ptr::copy_nonoverlapping` from the rtrb consumer into CoreAudio's output buffer. We could simultaneously write those samples into a visualization buffer.

**Why not:** The render callback runs on CoreAudio's real-time thread. Adding _any_ extra work there is a red flag. Even a lock-free ring buffer write adds cache pressure and potential for the viz buffer to fill up (what happens when viz isn't consuming fast enough?). This path leads to audio glitches eventually. The current callback is tight and correct — don't touch it.

### Option B: Copy on the Decode Side (RECOMMENDED)

The decode thread (`decode_single()` in `buffer.rs`) already has access to every decoded sample before it pushes them into the ring buffer. At line 437:

```rust
let samples = sbuf.samples(); // interleaved f32 slice
```

Right here, we have the full decoded audio data. We can copy a rolling window of samples into a shared visualization buffer _before_ pushing to the ring buffer. The decode thread is NOT real-time — it's a normal thread that already does `thread::sleep` when the ring buffer is full. A memcpy of ~2048-4096 samples per FFT window is negligible here.

**Architecture:**

```
Decode Thread ──┬── rtrb Producer ──→ CoreAudio Consumer ──→ DAC
                │
                └── VizBuffer (lock-free circular write)
                        │
                        ▼
                    TUI Thread (reads on tick, runs FFT, renders)
```

**Implementation:**

1. Add a `VizBuffer` to `koan-core` — a fixed-size `Arc<VizRing>` where:
   - The decode thread overwrites a circular buffer of the last N samples (N = FFT window size, e.g. 4096)
   - Uses `AtomicU64` write cursor + raw `UnsafeCell<[f32; N]>` (or just a `parking_lot::Mutex` — the TUI only reads at 20fps, contention is near-zero)
   - Actually, even simpler: `Arc<Mutex<Vec<f32>>>` with parking_lot. The decode thread holds the lock for ~10us to memcpy 4096 floats. The TUI reads at 50ms intervals. Zero contention in practice.

2. Pass `Arc<VizBuffer>` through `PlaybackTimeline` (it's already `Arc`-shared between decode thread and UI) or as a new field alongside it.

3. In `decode_single()`, after `sbuf.copy_interleaved_ref(decoded)`, copy the latest samples to the viz buffer before the ring buffer push loop.

**Latency consideration:** The viz buffer will be slightly ahead of what's playing (the ring buffer holds ~1s of audio). For visualization this doesn't matter — human perception of audio-visual sync tolerance is ~50ms, and the viz data is at most one ring buffer's worth ahead. In practice the decode thread is usually only slightly ahead of playback.

### Option C: Read from PlaybackTimeline counters + separate buffer (ALTERNATIVE)

Use the `samples_played` atomic counter (already shared) to know _where_ in the stream we are, combined with Option B's buffer, to display the _currently playing_ samples rather than the _currently decoded_ samples.

This would give better sync but adds complexity. For a first implementation, Option B's slight lookahead is fine.

### Visualization Buffer Design

```rust
/// Shared visualization sample buffer.
/// Written by decode thread, read by TUI at ~20fps.
pub struct VizBuffer {
    /// Rolling buffer of recent interleaved samples.
    samples: parking_lot::Mutex<VizSamples>,
}

struct VizSamples {
    /// Circular buffer, always power-of-2 size.
    buf: Vec<f32>,
    /// Write position (wraps).
    write_pos: usize,
    /// Channel count for de-interleaving.
    channels: u16,
    /// Sample rate for frequency calculations.
    sample_rate: u32,
}
```

The TUI reads a snapshot (memcpy out of the mutex) once per tick, then does all FFT work on its own copy. Total lock hold time: <50us on either side.

## FFT / Spectrum Analysis

### Recommended Crate: `realfft`

- Wraps `rustfft` for real-valued input (which is what we have — f32 PCM samples)
- Avoids unnecessary complex→complex transforms
- ~2x faster than `rustfft` alone for real data
- Pure Rust, no system dependencies

`spectrum-analyzer` is a higher-level wrapper but pulls in `microfft` and adds abstractions we don't need. Better to use `realfft` directly — it's only a few lines of code.

### FFT Parameters

| Parameter       | Value             | Rationale                                                                                                   |
| --------------- | ----------------- | ----------------------------------------------------------------------------------------------------------- |
| Window size     | 2048 samples      | Good balance: ~46ms at 44.1kHz. 1024 is too coarse for bass, 4096 adds latency                              |
| Window function | Hann              | Standard for music visualization. Smooth, low sidelobe leakage                                              |
| Overlap         | 0% (for now)      | At 20fps with 2048 samples, we get a new window every ~46ms anyway. Overlap only helps at higher framerates |
| Frequency bins  | 1024 (N/2)        | Half the window size for real FFT                                                                           |
| Frequency range | 20Hz — 20kHz      | Full audible range, mapped logarithmically to bars                                                          |
| Smoothing       | Exponential decay | `bar[i] = max(new_value, bar[i] * 0.85)`. Gives that satisfying "gravity" fall-off                          |

### Frequency-to-Bar Mapping

Logarithmic mapping (like cava) so bass frequencies get more visual space:

```rust
fn freq_to_bar(freq: f32, num_bars: usize, min_freq: f32, max_freq: f32) -> usize {
    let log_min = min_freq.ln();
    let log_max = max_freq.ln();
    let log_freq = freq.ln().clamp(log_min, log_max);
    let normalized = (log_freq - log_min) / (log_max - log_min);
    (normalized * num_bars as f32) as usize
}
```

Multiple FFT bins map to each visual bar → take the max (or weighted average) of all bins in that bar's frequency range.

### Magnitude Scaling

Convert complex FFT output to dB:

```rust
let magnitude = (re * re + im * im).sqrt();
let db = 20.0 * (magnitude / reference).log10();
// Clamp to display range, e.g. -60dB to 0dB
let normalized = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
```

## Rendering Techniques Comparison

### 1. Block Characters (Spectrum Bars) — PRIMARY

Characters: `▁▂▃▄▅▆▇█` (U+2581–U+2588)

Each character represents 1/8th of a cell height. With a 20-row visualization area, that's 160 vertical levels of resolution. Combined with color gradients, this is what cava uses and it looks great.

**Pros:** Universally supported, fast rendering, familiar look, great color gradient support.
**Cons:** Only vertical bars. Limited to 1 column per bar.

Implementation: Direct buffer cell writes (no Ratatui widget needed — same approach as `cover_art.rs` but simpler).

### 2. Halfblock Characters (High-Res Waveform) — SECONDARY

Character: `▀` (U+2580) — top half block. Each cell encodes 2 vertical pixels via fg/bg colors.

Already proven in koan's `cover_art.rs`. For waveforms: the y-axis maps amplitude, each column is a time sample, fg/bg encode whether the top or bottom pixel of each cell is "on".

**Pros:** 2x vertical resolution, proven in codebase, smooth curves possible.
**Cons:** Only 2 colors per cell (fg+bg), harder to do multi-color gradients.

### 3. Braille Characters (Oscilloscope / Lissajous) — TERTIARY

Characters: `⠀` through `⣿` (U+2800–U+28FF). Each braille cell is a 2x4 pixel grid.

**Resolution:** Each terminal cell = 2 columns x 4 rows of dots = 8 sub-pixels.
A 80x24 area becomes 160x96 pseudo-pixels.

**Pros:** Highest resolution, perfect for continuous waveforms, oscilloscope, Lissajous figures.
**Cons:** Monochrome per cell (fg color only, bg is transparent), requires more complex coordinate mapping.

Ratatui's `Canvas` widget with `Marker::Braille` handles coordinate mapping, but for raw performance we'd write directly to the buffer.

### 4. Color Gradients

All approaches benefit from ANSI 24-bit color (truecolor). Terminal support is universal on modern macOS (Terminal.app, iTerm2, Alacritty, Kitty, WezTerm).

Gradient ideas:

- **Spectrum bars:** Green (low) → Yellow (mid) → Red (hot) — classic VU meter look
- **Frequency gradient:** Blue (bass) → Cyan (mids) → Magenta (highs) — maps color to frequency
- **Intensity heat:** Dark blue → Cyan → White — for spectrograms
- **Theme-matched:** Use the existing cyan/green/yellow from `theme.rs`

## Visualizer Types: Cool Factor Assessment

### Tier 1: Must Have

**Spectrum Analyzer (Frequency Bars)**

- The classic. Bars bounce to the music. Logarithmic frequency mapping so bass hits hard.
- Render: block characters + color gradient
- CPU cost: One FFT per frame (~0.1ms for 2048-point on M1)
- Cool factor: 9/10 — this is what everyone pictures

**VU Meters (Stereo)**

- L/R level meters with peak hold indicators
- No FFT needed — just RMS of the sample window per channel
- Render: horizontal block bars, tiny footprint (2 rows)
- Cool factor: 7/10 — subtle but useful. Can embed in transport bar.

### Tier 2: Looks Sick

**Waveform (Oscilloscope)**

- Time-domain display of the raw waveform
- No FFT needed — directly plot the sample buffer
- Render: braille characters for smooth curves, or halfblock for chunkier look
- Cool factor: 8/10 — mesmerizing on clean signals, chaotic on compressed music

**Spectrogram (Waterfall)**

- 2D frequency×time display scrolling vertically. Each row is one FFT frame, color = magnitude.
- Render: halfblock characters with RGB color per pixel (same technique as cover art)
- Cool factor: 10/10 — genuinely looks amazing in a terminal. The "whoa" feature.
- CPU cost: Same FFT as spectrum analyzer, just keeping a history buffer

### Tier 3: Extra Credit

**Lissajous / Vectorscope**

- Plot L channel vs R channel as X/Y coordinates
- Mono = diagonal line, stereo = spread ellipse/circle
- Render: braille characters on Canvas
- Cool factor: 8/10 among audio nerds, 5/10 for everyone else

**Particle Effects**

- Particles emitted from bar peaks, subject to gravity
- Render: braille dots or sparse Unicode characters
- Cool factor: 7/10 — fun but gimmicky. Save for later.

### Recommended Priority

1. Spectrum analyzer (bars) — it's the main event
2. VU meters — cheap to add, useful in transport
3. Spectrogram (waterfall) — the visual showstopper
4. Waveform — simple, effective
5. Lissajous — niche but cool

## Widget Integration Plan

### Layout Options

**Option A: Dedicated Visualizer Pane (Recommended)**

Replace the content area split. Currently:

```
┌─ Transport ──────────────────────┐
│ [Art] Now Playing / Seek Bar     │
├──────────────────────────────────┤
│ Queue (100% or Library|Queue)    │
├──────────────────────────────────┤
│ Hints                            │
└──────────────────────────────────┘
```

With visualizer:

```
┌─ Transport ──────────────────────┐
│ [Art] Now Playing / Seek Bar     │
├──────────────────────────────────┤
│ ▁▂▃▅▇█▇▅▃▂▁▁▃▅▇█▇▅▃ Visualizer │  ← configurable height
├──────────────────────────────────┤
│ Queue                            │
├──────────────────────────────────┤
│ Hints                            │
└──────────────────────────────────┘
```

Toggle with a hotkey (e.g. `v`). Configurable height in config.toml (default: 8 rows).

**Option B: Overlay Mode**

Full-screen visualizer overlay (like CoverArtZoom) toggled with a key. Queue disappears, visualizer takes over. Good for "sit back and watch" mode.

**Option C: Embedded in Transport**

Small VU meter bars next to the seek bar. Always visible, minimal footprint. This is a nice complement to Option A.

**Recommendation:** Implement A + C. A gives the full experience, C gives subtle always-on feedback. B is a natural extension of A (just make the viz area fill the content region).

### New Mode / State

```rust
// In App struct:
pub visualizer: Option<VisualizerState>,
pub viz_enabled: bool,  // persisted in config
pub viz_height: u16,    // configurable, default 8

// New struct:
pub struct VisualizerState {
    /// Which visualizer is active.
    pub kind: VizKind,
    /// Processed spectrum data ready for rendering (0.0..1.0 per bar).
    pub spectrum: Vec<f32>,
    /// Previous frame's spectrum for smoothing/decay.
    pub prev_spectrum: Vec<f32>,
    /// Peak hold values (for peak indicators that slowly fall).
    pub peaks: Vec<f32>,
    /// Spectrogram history (ring buffer of spectrum frames).
    pub spectrogram_history: VecDeque<Vec<f32>>,
    /// RMS levels for VU meters [left, right].
    pub vu_levels: [f32; 2],
    /// Waveform samples for oscilloscope.
    pub waveform: Vec<f32>,
}

pub enum VizKind {
    Spectrum,
    Spectrogram,
    Waveform,
    VuMeter,
    Oscilloscope,
}
```

### Widget Implementation

Custom Ratatui widget, same pattern as `CoverArt`, `QueueView`, `TransportBar`:

```rust
pub struct SpectrumWidget<'a> {
    bars: &'a [f32],     // 0.0..1.0 normalized magnitudes
    peaks: &'a [f32],    // peak hold values
    theme: &'a Theme,
}

impl Widget for SpectrumWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let bar_count = area.width as usize;
        for x in 0..bar_count.min(self.bars.len()) {
            let height = (self.bars[x] * area.height as f32 * 8.0) as u16;
            // full_cells: complete █ characters
            // partial: one of ▁▂▃▄▅▆▇
            render_bar(buf, area.x + x as u16, area.y, area.height, height, ...);
        }
    }
}
```

The spectrogram widget would reuse the halfblock rendering from `cover_art.rs` — each cell's fg/bg colors represent magnitude at two time points.

### Tick Integration

In `handle_tick()`:

```rust
// After existing tick work:
if self.viz_enabled {
    if let Some(ref viz_buffer) = self.viz_buffer {
        let samples = viz_buffer.snapshot();  // lock + memcpy, <50us
        self.update_visualizer(&samples);     // FFT + smoothing
    }
}
```

The FFT runs on the main/TUI thread during the tick. At 2048 points, `realfft` completes in ~0.05ms on M1. Totally negligible alongside the ~2ms terminal render.

## Performance Considerations

### CPU Budget Per Frame (50ms tick @ 20fps)

| Operation                   | Time (M1)  | Notes                     |
| --------------------------- | ---------- | ------------------------- |
| VizBuffer lock + copy       | ~0.02ms    | 4096 f32s = 16KB memcpy   |
| Hann window application     | ~0.005ms   | Multiply 2048 floats      |
| FFT (2048-point real)       | ~0.05ms    | realfft on M1             |
| Magnitude + log scale       | ~0.01ms    | 1024 bins                 |
| Frequency bin → bar mapping | ~0.01ms    | Logarithmic mapping       |
| Smoothing / peak decay      | ~0.005ms   | Per-bar exponential decay |
| Widget render               | ~0.1ms     | Direct buffer cell writes |
| **Total**                   | **~0.2ms** | **<0.5% of 50ms budget**  |

This is nothing. The terminal render itself (~2-5ms) dominates. Even a spectrogram with history buffer stays well under budget.

### Memory

- VizBuffer: 4096 \* 4 bytes = 16KB (shared)
- Spectrum bars: ~200 \* 4 = 800 bytes
- Spectrogram history (128 frames): 128 _ 200 _ 4 = ~100KB
- FFT scratch: ~16KB

Total: <200KB. Negligible.

### Thread Safety

- `VizBuffer` uses `parking_lot::Mutex` — no poisoning, fast uncontended lock
- Decode thread writes once per packet (~1000x/sec for CD audio)
- TUI reads once per tick (20x/sec)
- Contention probability: near zero (lock held <50us, 50ms between reads)

### Impact on Audio Path

**Zero.** The decode thread already does `sbuf.copy_interleaved_ref(decoded)` which creates the f32 slice. We're adding one `memcpy` of those samples to the viz buffer before pushing to the ring buffer. This happens on the decode thread, not the RT audio thread. The CoreAudio render callback is untouched.

## Dependencies to Add

### koan-core/Cargo.toml

```toml
# None — VizBuffer only needs parking_lot (already a dependency)
```

### koan-music/Cargo.toml

```toml
realfft = "3"  # Real-valued FFT wrapper around rustfft
```

That's it. One new dependency. `realfft` pulls in `rustfft` which is pure Rust.

## Config Integration

```toml
[visualizer]
# Enable/disable visualizer
enabled = true
# Which visualizer to show: "spectrum", "spectrogram", "waveform", "vu"
kind = "spectrum"
# Height in terminal rows (spectrum/spectrogram/waveform)
height = 8
# Color scheme: "classic" (green→red), "cool" (blue→cyan→white), "theme" (match player theme)
colors = "classic"
```

Hotkeys:

- `v` — toggle visualizer on/off
- `V` (shift+v) — cycle through visualizer types

## Implementation Phases

### Phase 1: Audio Tap (koan-core)

1. Add `VizBuffer` struct to `koan-core::audio`
2. Thread it through `start_decode()` / `decode_single()` as `Option<Arc<VizBuffer>>`
3. Write samples in `decode_single()` after `sbuf.copy_interleaved_ref(decoded)`
4. Expose from `Player::spawn()` alongside state and timeline

### Phase 2: FFT Pipeline (koan-music)

1. Add `realfft` dependency
2. Create `tui/visualizer.rs` module with:
   - `VisualizerState` struct
   - `update_spectrum()` — snapshot → window → FFT → magnitude → bars
   - `update_waveform()` — snapshot → downsample to terminal width
   - `update_vu()` — snapshot → RMS per channel
3. Wire into `App` struct and `handle_tick()`

### Phase 3: Spectrum Widget (the main event)

1. `SpectrumWidget` — block character bars with gradient colors
2. Layout integration in `ui.rs` — add viz pane between transport and queue
3. Toggle hotkey and config
4. Smoothing / peak decay for satisfying visual response

### Phase 4: Additional Visualizers

1. Spectrogram (waterfall) — halfblock rendering, history ring buffer
2. VU meters — compact horizontal bars, embed in transport
3. Waveform — braille character oscilloscope
4. Lissajous — braille canvas, L vs R plot

## Open Questions

1. **Should the viz buffer live in `PlaybackTimeline` or be a separate `Arc`?** Separate is cleaner — timeline is about position tracking, viz is about sample data. Pass both from `Player::spawn()`.

2. **FFT on main thread or separate thread?** Main thread. At 0.05ms per FFT, spawning a thread adds more overhead than it saves. Keep it simple.

3. **Adjustable FFT size?** Start fixed at 2048. Could expose in config later for users who want smoother (4096) or more responsive (1024) spectrum.

4. **Gravity / fall speed config?** The decay constant (0.85) determines how fast bars fall. Could be configurable. Start with a hardcoded value that looks good.
