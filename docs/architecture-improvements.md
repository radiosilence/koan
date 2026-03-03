# Architecture Improvements Plan

## 1. Audio Backend Decoupling

### Current State

Audio output is deeply coupled to CoreAudio across two files:

- **`audio/engine.rs`** (~270 lines) — AUHAL output unit, render callback, start/stop lifecycle
- **`audio/device.rs`** (~270 lines) — device enumeration, sample rate get/set, CFString conversion

The good news: **`audio/buffer.rs` is entirely platform-agnostic.** The decode thread, ring buffer producer, timeline tracking, gapless logic — none of it touches CoreAudio. The interface between platform-specific and generic code is already clean:

- Engine takes: `device_id`, `sample_rate`, `channels`, `rtrb::Consumer<f32>`, `Arc<AtomicU64>`
- Engine provides: `start()`, `stop()`, `is_running()`, `Drop`

Total replacement surface: ~540 lines.

### Proposed Trait

```rust
pub trait AudioOutput: Send {
    fn start(&mut self) -> Result<(), PlayerError>;
    fn stop(&mut self) -> Result<(), PlayerError>;
    fn is_running(&self) -> bool;
}

pub trait AudioDevices {
    fn default_output() -> Result<DeviceId, PlayerError>;
    fn list_outputs() -> Result<Vec<OutputDevice>, PlayerError>;
    fn get_sample_rate(device: &DeviceId) -> Result<f64, PlayerError>;
    fn set_sample_rate(device: &DeviceId, rate: f64) -> Result<(), PlayerError>;
    fn available_sample_rates(device: &DeviceId) -> Result<Vec<f64>, PlayerError>;
}
```

### File Layout

```
audio/
├── mod.rs              # re-exports, #[cfg] switching
├── buffer.rs           # unchanged — platform-agnostic
├── replaygain.rs       # unchanged — platform-agnostic
├── output.rs           # trait definitions
├── macos/
│   ├── engine.rs       # current engine.rs
│   └── device.rs       # current device.rs
└── linux/
    ├── engine.rs       # ALSA write thread
    └── device.rs       # ALSA card/device enumeration
```

### Approach

Conditional compilation via `#[cfg(target_os = "...")]` in `mod.rs`. Both backends expose the same trait. Feature flags optional but not required — platform detection at compile time is sufficient.

---

## 2. Linux Audio Backend

### Recommendation: ALSA Direct

ALSA `hw:` device is the only path to true bit-perfect on Linux. It bypasses PulseAudio/PipeWire entirely — no resampling, no mixing, no format conversion. This matches koan's CoreAudio philosophy exactly.

| Option | Bit-perfect | Sample Rate Switch | Crate Maturity | Verdict |
|--------|:-----------:|:------------------:|:--------------:|---------|
| **alsa** (direct hw:) | **Yes** | **Yes** | Stable (v0.9) | **Use this** |
| PipeWire | Partial (needs force-rate) | Via graph negotiation | OK (v0.9, docs spotty) | Future option |
| cpal | No (uses `default` device) | No (issue #788 open) | Popular but wrong tool | Skip |
| rodio | No (adds mixing layer) | No | N/A | Skip |

### How ALSA Fits

The render model flips from pull (CoreAudio calls you via callback) to push (you call `writei()`). This is actually **simpler** — no unsafe extern "C" callback, no raw pointer juggling:

```
Decode thread → rtrb::Producer<f32>
ALSA write thread → rtrb::Consumer<f32> → pcm.writei() → DAC
```

The write thread owns the consumer directly, blocks on ALSA when the hardware buffer is full, increments `samples_played` after each successful write.

### Gotchas

- Requires `libasound2-dev` at build time, `libasound2` at runtime
- `hw:` device is exclusive (one app at a time) — that's the point, but users need to know
- Some DACs don't accept f32 — may need s32le/s24le fallback with trivial conversion
- No desktop integration (volume knob in GNOME won't work) — ALSA bypasses the sound server

### Non-Audio Platform Concerns

| Component | macOS | Linux | Status |
|-----------|-------|-------|--------|
| **souvlaki** (media keys) | CFRunLoop | MPRIS/D-Bus | Already cross-platform, code has `#[cfg]` |
| **security-framework** (Keychain) | macOS only | N/A | Password already in config.local.toml, Keychain is legacy fallback |
| **core-foundation** (CFRunLoop) | macOS only | N/A | Already `#[cfg(target_os = "macos")]` gated |
| **build.rs** | Links CoreAudio framework | Needs `#[cfg]` guard | Trivial fix |
| **TUI** (ratatui + crossterm) | Works | Works | No changes needed |
| Everything else | Works | Works | Pure Rust / cross-platform |

---

## 3. Gapless: Custom vs Symphonia

### What Symphonia Provides

Symphonia 0.5.5 has gapless support:
- `FormatOptions::enable_gapless` — tells format reader to provide trim info
- `codec_params.delay` / `codec_params.padding` — encoder delay/trailing samples
- `SampleBuffer` — handles codec output format conversion

### What koan Does

koan sets `enable_gapless: true` but **doesn't use the trim info Symphonia provides**. The gapless implementation is entirely about ring buffer continuity:

1. Decode thread loops: decode track A → EOF → get next track → decode track B
2. Ring buffer producer stays alive across track boundaries
3. CoreAudio render callback never sees silence
4. `PlaybackTimeline::push_boundary()` marks each track's start sample offset
5. UI binary-searches boundaries with `samples_played` to detect track changes

### What Could Change

| Aspect | Current | Could Delegate to Symphonia |
|--------|---------|----------------------------|
| Codec delay trimming | Ignored (bit-perfect) | Yes — `codec_params.delay` for MP3/AAC pre-skip |
| Ring buffer continuity | Custom (must stay custom) | No — Symphonia is single-file |
| Track boundary tracking | Custom PlaybackTimeline | No — Symphonia doesn't know about playlists |
| Decode cursor lookahead | Custom (separate from UI cursor) | No — player architecture concern |
| Seek precision | `SeekMode::Coarse` | Could use `SeekMode::Accurate` |

**Bottom line:** Most of koan's gapless code is playlist orchestration that Symphonia can't handle. The one thing Symphonia could help with is trimming encoder delay/padding (relevant for MP3 where there's ~50ms silence between tracks without it). Whether to use it depends on philosophy: bit-perfect purists want all samples, but fb2k and most players do trim encoder artifacts.

### Recommendation

Use Symphonia's trim info for lossy codecs (MP3, AAC, Opus) where encoder delay is a format artifact, not musical content. Leave lossless (FLAC, ALAC, WAV) untouched. This matches foobar2000's behavior.

---

## 4. Dead Code Cleanup

Config fields that exist but aren't wired into anything:

| Field | Config Location | Library Code | Wired In? |
|-------|----------------|-------------|-----------|
| `playback.replaygain` | config.rs | replaygain.rs (full impl) | **No** — needs decode pipeline integration |
| `playback.software_volume` | config.rs | N/A | **No** — needs sample scaling in decode |
| `remote.transcode_quality` | config.rs | N/A | **No** — needs stream URL parameter |

These should be wired in or removed. ReplayGain wiring is in progress on the `rg-config-wiring` branch.
