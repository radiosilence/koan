# Architecture Improvement Plans

Feasibility research and implementation plans for koan's next major features.

## Active

| # | Plan | Effort | Key Decision |
|---|------|--------|-------------|
| [02](02-dsp-and-profiles.md) | DSP + Headphone Profiles | ~5-7 days | Insert between decode and ring buffer. `biquad` for parametric EQ. AutoEQ profiles trivially parseable. |
| [04](04-tagging.md) | Tag Editing | ~8-12 days | vimv-style (TSV + $EDITOR) first, TUI inline editor second. Terminal suspend/resume is a standard ratatui pattern. |
| [09](09-artist-metadata.md) | Artist Metadata | — | Bios, images and similar artists from MusicBrainz/Last.fm. |
| [10](10-ios.md) | koan on iOS | ~2-4 weeks | The core already builds and links for both iOS targets; 51 of 60 app files typecheck. The work is the build system and a phone shell. |

## Shipped

| # | Plan | Landed as |
|---|------|-----------|
| 01 | Linux + Audio Backends | `audio/cpal_backend.rs`; CI builds and lints on ubuntu |
| 03 | ANSI Art Visualizers | `koan-tui/src/visualizer.rs` |
| 06 | Decoupled Backends | `audio/backend.rs` trait with CoreAudio and cpal implementors; `credentials.rs` over `keyring` |
| 07 | Non-Tag Metadata | `remote/lrclib.rs` + the `lyrics_cache` table |
| 08 | ReplayGain Wiring | applied in `decode_single()` before the ring-buffer push |

Plans 03, 07 and 08 live in `archive/`. Plans 01, 06 and 09 were written before the crate split —
`koan-music/` paths in them map to `koan-tui/` (TUI) and `koan-cli/` (binary).

## Dependencies Between Plans

```
02 DSP Pipeline ────────► shares the audio tap with the visualizers
```

## Open Questions

- **MusicBrainz/AcoustID** (Plan 04): requires chromaprint C FFI, breaking the pure-Rust philosophy.
  Optional stretch goal.

The cpal-vs-raw-ALSA question is settled: cpal, for compatibility. Direct ALSA remains a future option
if bit-perfect output on Linux needs it — see `docs/architecture-improvements.md`.

## Suggested Implementation Order

1. **04 Tagging** — self-contained, vimv phase first
2. **02 DSP + Profiles** — builds on the audio backend trait
3. **09 Artist Metadata**
