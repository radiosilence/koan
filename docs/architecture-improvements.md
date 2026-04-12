# Architecture Improvements Plan

> **Status:** Sections 1 and 2 are DONE as of v0.21.0. The `AudioBackend` trait exists in `audio/backend.rs` with `CoreAudioBackend` (macOS) and `CpalBackend` (Linux via cpal) implementations. Platform switching is via `#[cfg(target_os)]`.

## 1. Audio Backend Decoupling -- DONE

Completed in v0.21.0. The `AudioBackend` and `AudioEngineHandle` traits live in `audio/backend.rs`. Platform implementations:

- `audio/coreaudio_backend.rs` -- macOS (AUHAL, bit-perfect)
- `audio/cpal_backend.rs` -- Linux (ALSA/PipeWire/PulseAudio via cpal)

Conditional compilation via `#[cfg(target_os = "...")]` in the backend module.

## 2. Linux Audio Backend -- DONE

Implemented via cpal (`CpalBackend`). Supports ALSA, PipeWire, and PulseAudio. Not direct ALSA `hw:` as originally planned -- cpal was chosen for broader compatibility. A direct ALSA backend for bit-perfect output remains a future option.

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
| `playback.replaygain` | config.rs | replaygain.rs (full impl) | **Yes** -- wired into decode pipeline |
| `playback.software_volume` | config.rs | N/A | **No** -- needs sample scaling in decode |
| `remote.transcode_quality` | config.rs | N/A | **No** -- needs stream URL parameter |
