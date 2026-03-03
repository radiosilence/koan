# Architecture Improvement Plans

Feasibility research and implementation plans for koan's next major features.

## Plans

| # | Plan | Effort | Key Decision | Status |
|---|------|--------|-------------|--------|
| [01](01-linux-and-audio-backends.md) | Linux + Audio Backends | ~7-9 days | Custom gapless is correct — just abstract the output. `AudioBackend` trait wrapping CoreAudio/ALSA directly. | Research |
| [02](02-dsp-and-profiles.md) | DSP + Headphone Profiles | ~5-7 days | Insert between decode and ring buffer. `biquad` for parametric EQ. AutoEQ profiles trivially parseable. Fixes ReplayGain never being applied. | Research |
| [03](03-ansi-visualizers.md) | ANSI Art Visualizers | ~4-6 days | Tap audio on decode thread via mutex buffer. `realfft` for FFT. Spectrogram waterfall reuses cover art halfblock technique. | Research |
| [04](04-tagging.md) | Tag Editing | ~8-12 days | lofty 0.23 writes fine. vimv-style (TSV + $EDITOR) first, TUI inline editor second. Terminal suspend/resume is standard ratatui pattern. | Research |
| [05](05-mouse-support.md) | GUI-Grade Mouse Support | ~5-8 days | Foundation already solid. Main gaps: hover (Moved events ignored), right-click menus, drag threshold, seek scrubbing. | Research |
| [06](06-decoupled-backends.md) | Decoupled Backends | ~6-10 days | Trait-based subsystems. `keyring` for credentials (trivial). Don't abstract SQLite. Custom `AudioBackend` over cpal for bit-perfect. | Research |
| [07](07-non-tag-metadata.md) | Non-Tag Metadata | ~7-10 days | Last.fm + LRCLIB + Cover Art Archive — all free/open. Radio mode = queue feature. Lyrics is highest ROI. | Research |

## Dependencies Between Plans

```
06 Decoupled Backends ──► 01 Linux + Audio Backends
                     └──► 02 DSP Pipeline (audio trait must exist first)
                     └──► 07 Non-Tag Metadata (remote trait enables more sources)

02 DSP Pipeline ────────► 03 Visualizers (shared audio tap infrastructure)
```

## Open Questions

- **cpal vs raw ALSA**: Plans 01 and 06 disagree. cpal may not support sample rate switching for bit-perfect playback. Needs hands-on testing before committing.
- **MusicBrainz/AcoustID** (Plan 04): Requires chromaprint C FFI, breaking the pure-Rust philosophy. Marked as optional stretch goal.

## Suggested Implementation Order

1. **06 Decoupled Backends** — foundational, unblocks everything
2. **01 Linux + Audio Backends** — biggest reach expansion
3. **05 Mouse Support** — incremental, low risk, high polish
4. **04 Tagging** — vimv phase is self-contained
5. **02 DSP + Profiles** — builds on audio backend trait
6. **07 Non-Tag Metadata** — lyrics first, radio mode later
7. **03 Visualizers** — fun but lowest priority; shares DSP audio tap
