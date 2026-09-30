# Architecture Improvements Plan



## Linux: direct ALSA

The Linux backend is cpal (`CpalBackend`), chosen for ALSA, PipeWire and PulseAudio coverage. A direct ALSA `hw:` backend for bit-perfect output remains an option.

---

## 3. Gapless: Custom vs Symphonia

### What Symphonia Provides

Symphonia reports each track's encoder delay and padding (`Track::delay`, `Track::padding`): the leading and trailing frames the encoder inserted, which a player can skip.

### What kōan Does

kōan opens files with default `FormatOptions` and **does not use the trim info Symphonia provides**. The gapless implementation is entirely about ring buffer continuity:

1. Decode thread loops: decode track A → EOF → get next track → decode track B
2. Ring buffer producer stays alive across track boundaries
3. CoreAudio render callback never sees silence
4. `PlaybackTimeline::push_boundary()` marks each track's start sample offset
5. UI binary-searches boundaries with `samples_played` to detect track changes

### What Could Change

| Aspect | Current | Could Delegate to Symphonia |
|--------|---------|----------------------------|
| Codec delay trimming | Ignored (bit-perfect) | Yes — `Track::delay` for MP3/AAC pre-skip |
| Ring buffer continuity | Custom (must stay custom) | No — Symphonia is single-file |
| Track boundary tracking | Custom PlaybackTimeline | No — Symphonia doesn't know about playlists |
| Decode cursor lookahead | Custom (separate from UI cursor) | No — player architecture concern |

Most of kōan's gapless code is playlist orchestration that Symphonia can't handle. The one thing Symphonia could help with is trimming encoder delay/padding (relevant for MP3 where there's ~50ms silence between tracks without it). Whether to use it depends on philosophy: bit-perfect purists want all samples, but fb2k and most players do trim encoder artifacts.

### Recommendation

Use Symphonia's trim info for lossy codecs (MP3, AAC, Opus) where encoder delay is a format artifact, not musical content. Leave lossless (FLAC, ALAC, WAV) untouched. This matches foobar2000's behavior.

---


