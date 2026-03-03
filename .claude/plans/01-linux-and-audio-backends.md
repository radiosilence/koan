# Plan: Linux Compatibility & Audio Backend Strategy

## Summary

koan's macOS-only surface area is smaller than it looks. The actual platform-locked code is concentrated in four files totalling ~400 lines. The **right move** is to abstract the audio output backend behind a trait, implement a cpal backend for Linux (and potentially replace the CoreAudio backend too), swap `security-framework` for the `keyring` crate, and leave the gapless decode architecture exactly as-is. The custom gapless approach (Symphonia decode thread feeding a continuous rtrb ring buffer) is already the correct architecture and is better than anything the higher-level libraries offer.

---

## 1. Current Architecture Analysis

### Platform-locked code

| File | Dependency | What it does | Lines |
|------|-----------|--------------|-------|
| `koan-core/src/audio/engine.rs` | `coreaudio-sys` | AUHAL AudioUnit setup, render callback draining rtrb consumer | ~270 |
| `koan-core/src/audio/device.rs` | `coreaudio-sys` | Device enumeration, name lookup, sample rate get/set | ~270 |
| `koan-core/src/credentials.rs` | `security-framework` | Keychain store/get/delete password | ~37 |
| `koan-music/src/media_keys.rs` | `core-foundation` | `pump_run_loop()` for CFRunLoop on macOS (already has `#[cfg(not(target_os = "macos"))]` no-op) | ~5 lines macOS-specific |
| `koan-music/src/media_keys.rs` | `souvlaki` | Media key handling | Already cross-platform (MPRIS on Linux) |

### Platform-portable code (no changes needed)

| Module | Why it's fine |
|--------|--------------|
| `audio/buffer.rs` | Pure Rust: Symphonia decode + rtrb producer. Zero platform deps. |
| `audio/replaygain.rs` | Pure Rust: lofty tags + ebur128 analysis. |
| `player/mod.rs` | Orchestration only. Calls engine/device through their current API. |
| All of `koan-music/` TUI | crossterm + ratatui. Already cross-platform. |
| Database, config, indexing | rusqlite, serde, walkdir. All portable. |

### The gapless playback architecture

The decode thread (`decode_queue_loop` in `buffer.rs`) keeps the rtrb producer alive across track boundaries. The CoreAudio render callback just drains whatever's in the ring buffer. Track boundaries are tracked via `PlaybackTimeline` using cumulative sample offsets. The UI derives "what's playing" from `samples_played` vs boundary offsets.

This is **textbook lock-free audio architecture**. The ring buffer is the abstraction boundary between decode and output. The output backend doesn't know or care about track boundaries -- it just consumes f32 samples. This means swapping the output backend requires zero changes to the gapless logic.

---

## 2. Evaluation of Rust Audio Output Libraries

### cpal -- The Viable Option

**What it is**: Low-level cross-platform audio I/O. Callback-based, same model as CoreAudio.

**Backends**: ALSA (default on Linux), PulseAudio (merged Feb 2026), JACK (optional), CoreAudio (macOS), WASAPI (Windows), AAudio (Android).

**Callback model**: `build_output_stream()` takes a closure `FnMut(&mut [f32], &OutputCallbackInfo)` called on a high-priority audio thread. This maps almost 1:1 to koan's current CoreAudio render callback -- drain from rtrb consumer, fill output buffer, zero-pad on underrun.

**Gapless**: cpal doesn't "do" gapless -- it's a raw PCM output pipe. But that's exactly what we need. Our gapless is handled by the decode thread feeding continuous PCM into the ring buffer. cpal just needs to drain it. Perfect fit.

**Sample rate control**: This is the **critical limitation**. cpal has NO API to change a device's sample rate. You can:
- Query supported sample rates via `supported_output_configs()`
- Specify a sample rate when building a stream via `StreamConfig`
- But you **cannot** dynamically switch the hardware device's nominal sample rate

On macOS, koan currently calls `AudioObjectSetPropertyData` to set the device's nominal sample rate before creating the AudioUnit. This is essential for bit-perfect playback (playing 96kHz files at 96kHz on the DAC).

On Linux/ALSA, the situation is actually simpler: when you open an ALSA `hw:` device, you specify the sample rate in the hardware params and ALSA switches the device directly. cpal's ALSA backend does this when you `build_output_stream` with a specific `SampleRate` in the config. So for Linux, cpal's "specify at stream creation" model actually works for bit-perfect -- you just need to tear down and recreate the stream when the sample rate changes (which koan already does on every track change via `start_playback`).

**Verdict**: cpal works well for Linux. For macOS, it *could* work but you'd lose the ability to explicitly control the device sample rate independent of stream creation. Since koan already recreates the engine per-track, this might be acceptable -- but it's a regression in control vs. raw CoreAudio.

### rodio -- Not Suitable

**What it is**: Higher-level library built on cpal. Has a `Sink` with `append()`.

**Gapless**: rodio's `Sink::append()` is designed for sequential playback, but it resamples everything to the sink's configured sample rate. It doesn't support per-track sample rate switching. This is a non-starter for bit-perfect.

**Control**: No access to buffer sizes, callback timing, device selection, or sample rate switching. Too high-level.

**Verdict**: Hard no. Would require rewriting the entire audio pipeline and losing bit-perfect.

### kira -- Not Suitable

**What it is**: Game audio engine. Tweens, mixers, spatial audio, clocks.

**Gapless**: Has "seamless playback" in the game-audio sense (crossfades, etc.), not the music-player sense (sample-accurate track boundary).

**Control**: Designed for game sound effects, not music playback. No sample rate switching, no device selection, no bit-perfect anything.

**Verdict**: Wrong tool entirely. This is for games, not audiophile music players.

### Symphonia -- Decode Only

Symphonia is purely a demuxer/decoder. It has `enable_gapless: true` in `FormatOptions` which handles encoder padding removal (critical for MP3/AAC gapless). koan already uses this. Symphonia has no audio output infrastructure.

### Direct ALSA bindings (alsa-rs)

**What it is**: Thin Rust wrapper around libasound.

**Pros**: Full control over ALSA hw params, sample rate, buffer size, period size. Bit-perfect via `hw:` device. Same level of control as CoreAudio.

**Cons**: Linux-only (obviously). More code to write. No PipeWire/PulseAudio integration (though PipeWire's ALSA compatibility layer handles most cases).

**Verdict**: Viable fallback if cpal's ALSA support proves insufficient for bit-perfect needs.

---

## 3. Recommended Approach

### Phase 1: Audio Backend Trait Abstraction (Medium effort)

Define a trait that both the existing CoreAudio code and a new cpal backend can implement:

```rust
pub trait AudioBackend: Send {
    /// List available output devices.
    fn list_devices(&self) -> Result<Vec<DeviceInfo>>;

    /// Get the default output device.
    fn default_device(&self) -> Result<DeviceInfo>;

    /// Query supported sample rates for a device.
    fn supported_sample_rates(&self, device: &DeviceInfo) -> Result<Vec<u32>>;

    /// Create an output engine targeting a device at a specific format.
    /// Takes ownership of the rtrb consumer.
    fn create_engine(
        &self,
        device: &DeviceInfo,
        sample_rate: u32,
        channels: u16,
        consumer: rtrb::Consumer<f32>,
        samples_played: Arc<AtomicU64>,
    ) -> Result<Box<dyn AudioEngine>>;
}

pub trait AudioEngine: Send {
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn is_running(&self) -> bool;
}
```

The key insight: **the ring buffer stays**. Both backends just need to drain the consumer. The decode thread, gapless logic, and timeline tracking are completely untouched.

**Changes to `player/mod.rs`**: Replace direct `engine::AudioEngine::new()` and `device::*` calls with trait method calls. The `Player` holds a `Box<dyn AudioBackend>` injected at construction.

**Estimated effort**: 2-3 days. Mostly mechanical refactoring.

### Phase 2: cpal Backend for Linux (Medium effort)

Implement `AudioBackend` using cpal:

```rust
pub struct CpalBackend {
    host: cpal::Host,
}

impl AudioBackend for CpalBackend {
    fn create_engine(&self, ...) -> Result<Box<dyn AudioEngine>> {
        let device = /* resolve from DeviceInfo */;
        let config = cpal::StreamConfig {
            channels: channels as u16,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _info| {
                // Drain from rtrb consumer, same logic as render_callback
                let available = consumer.slots();
                let to_read = available.min(data.len());
                if to_read > 0 {
                    if let Ok(chunk) = consumer.read_chunk(to_read) {
                        // copy to output buffer
                        chunk.commit_all();
                        samples_played.fetch_add(to_read as u64, Ordering::Relaxed);
                    }
                }
                // zero-pad remainder
                data[to_read..].fill(0.0);
            },
            |err| log::error!("audio stream error: {}", err),
            None, // timeout
        )?;

        Ok(Box::new(CpalEngine { stream }))
    }
}
```

**Sample rate switching on Linux**: When `start_playback` detects a sample rate mismatch, it tears down the old engine and creates a new one with the correct `StreamConfig`. The cpal ALSA backend will configure the hardware accordingly. For PulseAudio/PipeWire, the server handles resampling transparently (not bit-perfect, but functional).

For true bit-perfect on Linux, users would configure ALSA to use a `hw:` device directly (standard audiophile Linux setup). cpal's ALSA backend will pass the sample rate to the hardware params.

**PipeWire note**: Most modern Linux distros use PipeWire with ALSA and PulseAudio compatibility layers. cpal's ALSA backend works through PipeWire's ALSA plugin. The new PulseAudio backend (merged Feb 2026) also works with PipeWire's PulseAudio compatibility. Either path works.

**Estimated effort**: 3-4 days including testing on actual Linux hardware.

### Phase 3: Credential Store Abstraction (Easy)

Replace `security-framework` with the `keyring` crate (v3.6+):

```toml
# Remove:
security-framework = "3.5"
# Add:
keyring = { version = "3", features = ["apple-native", "linux-native-sync-persistent"] }
```

The `keyring` crate provides:
- **macOS**: Native Keychain (same as current)
- **Linux**: `secret-service` (GNOME Keyring / KDE Wallet) + `keyutils` (kernel keyring)
- **Windows**: Windows Credential Manager (bonus)

The API is nearly identical to what we have. `credentials.rs` is 37 lines and the rewrite would be similar size.

**Headless Linux caveat**: `secret-service` requires D-Bus and a running keyring daemon. On headless servers, this may not work. Should fall back gracefully (prompt for password, store in config file, or error clearly).

**Estimated effort**: Half a day.

### Phase 4: Platform Gating in Cargo.toml (Easy)

```toml
[target.'cfg(target_os = "macos")'.dependencies]
coreaudio-sys = "0.2"
core-foundation = "0.9"

[target.'cfg(target_os = "linux")'.dependencies]
cpal = { version = "0.17", features = ["pulseaudio"] }
# or keep cpal as a universal dep and cfg-gate the backend selection at runtime
```

Use `#[cfg(target_os = "...")]` to select the backend at compile time, or use runtime detection with a config option.

**Recommended**: Compile-time cfg with feature flags. Keeps binary size down, no runtime cost, clear what each build supports.

**Estimated effort**: 1 day including CI setup for Linux builds.

### Optional Phase 5: Replace CoreAudio with cpal on macOS Too

This is **optional and lower priority**. The current CoreAudio code works perfectly and gives maximum control. Replacing it with cpal would:

- **Pro**: Single audio backend codebase for all platforms
- **Pro**: Reduced maintenance burden
- **Con**: Loss of direct device sample rate control (minor -- koan recreates engine per track anyway)
- **Con**: cpal's CoreAudio backend may have slightly different buffer size / timing behavior
- **Con**: One more dependency layer

**Recommendation**: Don't do this initially. Keep the native CoreAudio backend for macOS, use cpal for Linux. If maintaining two backends becomes painful, consolidate to cpal later.

---

## 4. What NOT to Change

### The gapless architecture stays as-is

The current approach is already the right one. Here's why:

1. **Symphonia decode + rtrb ring buffer + platform output** is the exact same architecture used by serious audio players (foobar2000, DeaDBeeF, etc.). The decode thread feeds continuous PCM, the output drains it.

2. **No library offers better gapless** for this use case. rodio resamples everything to one rate. kira is for games. cpal is output-only (no gapless concept). The only thing that "does gapless" at the decode level is Symphonia's `enable_gapless` flag for encoder delay/padding removal -- which koan already uses.

3. **Track boundary tracking via sample offsets** (`PlaybackTimeline`) is clean, correct, and decoupled from the output backend. Replacing it would add complexity for zero benefit.

4. The ring buffer IS the abstraction boundary. The output backend is a dumb consumer. This is exactly right.

### The rtrb ring buffer stays

rtrb is the best lock-free SPSC ring buffer in the Rust ecosystem. It's used by the same `RustAudio` org that maintains cpal. No reason to replace it.

### The decode thread architecture stays

`decode_queue_loop` with its `next_track` closure for lookahead is clean, testable, and correct. The decode cursor being separate from the UI cursor is a smart design that shouldn't change.

---

## 5. Effort Estimates

| Phase | Effort | Priority | Blocks |
|-------|--------|----------|--------|
| 1. AudioBackend trait | 2-3 days | High | Phases 2, 5 |
| 2. cpal Linux backend | 3-4 days | High | Phase 4 |
| 3. Credential store (`keyring`) | 0.5 days | Medium | None |
| 4. Platform gating (Cargo/CI) | 1 day | High | Phase 2 |
| 5. Replace CoreAudio with cpal (optional) | 2-3 days | Low | Phase 1 |

**Total for Linux support**: ~7-9 days of focused work.

---

## 6. Risks and Unknowns

### cpal ALSA bit-perfect uncertainty
cpal's ALSA backend opens devices via `snd_pcm_open` with `SND_PCM_STREAM_PLAYBACK`. It's unclear if it uses `hw:` directly or goes through `default`/`plughw:`. If it goes through `plughw:`, ALSA's software mixer may resample, breaking bit-perfect. **Mitigation**: Test on actual hardware. If cpal doesn't support `hw:` devices, consider direct `alsa-rs` as fallback, or contribute a fix upstream.

### PipeWire sample rate passthrough
PipeWire can be configured for sample rate passthrough, but the default is to resample everything to one rate. Bit-perfect on PipeWire requires specific user configuration (`pw-metadata` or config files). This is a user documentation issue, not a code issue.

### cpal callback thread priority
CoreAudio's render thread is real-time priority (THREAD_TIME_CONSTRAINT_POLICY). cpal's ALSA thread priority depends on the system. May need `SCHED_FIFO` or `SCHED_RR` for glitch-free playback. cpal may or may not set this. **Mitigation**: Test under load. If needed, set thread priority manually after stream creation.

### Secret service availability on headless Linux
`keyring` crate's Linux backend needs D-Bus + a keyring daemon. Headless servers, Docker containers, and minimal installs won't have this. **Mitigation**: Make credential storage optional. Allow storing Navidrome password in config file as fallback (with a security warning).

### Cross-compilation CI
Current CI cross-compiles x86_64 from arm64 macOS. Adding Linux targets means either:
- Linux runners in CI (GitHub Actions has Ubuntu runners)
- Cross-compilation from macOS to Linux (harder, needs linker + sysroot)

**Recommendation**: Add a separate Linux CI job on `ubuntu-latest`. Don't try to cross-compile from macOS.

### Different sample rate per gapless transition
Currently, koan creates a new engine when sample rate changes. In a gapless scenario, if track N is 44.1kHz and track N+1 is 96kHz, the decode thread will have already started pushing 96kHz samples into the ring buffer before the UI detects the transition. **This is an existing bug** on macOS too, but works because the ring buffer is small enough that the rate mismatch only lasts ~1s. On Linux/ALSA, changing sample rate means closing and reopening the device, which would cause an audible gap. **Mitigation**: Either:
1. Resample to the current engine's rate for cross-rate gapless (compromise)
2. Accept a gap when sample rate changes (pragmatic -- most albums are same rate)
3. Pre-scan the next track's sample rate and tear down/rebuild before the boundary (complex)

This is worth noting but is an existing limitation, not a new one introduced by Linux support.

---

## 7. Dependencies on Other Changes

- **None blocking**. This work is independent of any other proposed changes.
- The trait abstraction (Phase 1) would benefit from being done before any other audio refactoring.
- CI changes (Phase 4) should coordinate with the existing release workflow.

---

## Sources

- [cpal GitHub repository](https://github.com/RustAudio/cpal)
- [cpal docs.rs](https://docs.rs/cpal)
- [cpal PulseAudio PR (merged Feb 2026)](https://github.com/RustAudio/cpal/pull/957)
- [cpal sample rate issue #788](https://github.com/RustAudio/cpal/issues/788)
- [cpal WASAPI sample rate issue #593](https://github.com/RustAudio/cpal/issues/593)
- [cpal DeepWiki architecture overview](https://deepwiki.com/RustAudio/cpal)
- [rodio GitHub](https://github.com/RustAudio/rodio)
- [kira docs.rs](https://docs.rs/kira/latest/kira/)
- [Symphonia GitHub](https://github.com/pdeljanov/Symphonia)
- [keyring crate](https://crates.io/crates/keyring)
- [CamillaDSP (Rust ALSA bit-perfect reference)](https://github.com/HEnquist/camilladsp)
- [termusic (Rust TUI music player, multi-backend)](https://github.com/tramhao/termusic)
- [Bit-perfect audio on Linux with ALSA](https://kevinboone.me/alsa_bitperfect.html)
