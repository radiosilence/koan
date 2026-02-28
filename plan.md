# kōan — Implementation Plan

## Context

Every music player on macOS is either abandonware, subscription garbage, or Electron trash. kōan is a pure Rust music player with a Ratatui TUI that does bit-perfect audio playback, fast library indexing, and fb2k-style customization. The name is a zen kōan — a question with no logical answer, like "why can't anyone make a decent music player?"

This plan gets us from working CLI to a fully-featured TUI music player. Each phase is independently demoable.

---

## Architecture Summary

```
Pure Rust, top to bottom. No FFI. No Swift.

koan-cli (Ratatui TUI)
    ├── Library browser (tree views, faceted, format-string-driven)
    ├── Queue / playlist view
    ├── Now playing + audio chain status
    ├── Fuzzy search (nucleo)
    └── Media keys (souvlaki → MPRemoteCommandCenter)
            │
            ▼
    koan-core (library)
    ├── audio/    CoreAudio HAL wrapper, ring buffer, gapless engine
    ├── db/       SQLite + FTS5, WAL mode
    ├── index/    Symphonia metadata, lofty tags, FSEvents watcher
    ├── format/   fb2k-style format string engine
    └── player/   Playback state machine, queue management
```

**Key insight:** PCM audio data flows entirely within Rust: file → Symphonia → rtrb ring buffer → CoreAudio render callback → DAC. No FFI boundaries, no bridging overhead.

### Crate Stack

| Need | Crate | Notes |
|------|-------|-------|
| Audio decoding | `symphonia` | FLAC/ALAC/MP3/Vorbis/WAV/AAC-LC native |
| CoreAudio HAL | `coreaudio-sys` | Raw bindings, safe wrappers on top |
| SQLite + FTS5 | `rusqlite` | `bundled-full` feature, WAL mode |
| File watching | `notify` | FSEvents backend on macOS |
| Metadata tags | `lofty` | Read/write all tag formats, ReplayGain, MBID |
| Ring buffer | `rtrb` | Wait-free SPSC, audio-thread safe |
| ReplayGain scan | `ebur128` | EBU R128 compliant loudness measurement |
| TUI framework | `ratatui` | Terminal UI with widgets, layouts, events |
| TUI events | `crossterm` | Terminal event handling, raw mode |
| Fuzzy search | `nucleo` | Already used for picker |
| Media keys | `souvlaki` | macOS MPRemoteCommandCenter, Now Playing |

### Project Layout

```
koan/
├── Cargo.toml                      # workspace root
├── crates/
│   ├── koan-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── audio/
│   │       │   ├── engine.rs       # AudioEngine: owns AUHAL unit, render callback
│   │       │   ├── device.rs       # Device enumeration, selection, hog mode
│   │       │   ├── format.rs       # Sample rate switching, integer mode
│   │       │   ├── buffer.rs       # rtrb ring buffer, decode pipeline, gapless
│   │       │   └── replaygain.rs   # ebur128 integration, gain application
│   │       ├── db/
│   │       │   ├── schema.rs       # Table definitions, migrations
│   │       │   ├── queries.rs      # Library queries, search, stats
│   │       │   └── connection.rs   # WAL setup, reader pool
│   │       ├── index/
│   │       │   ├── scanner.rs      # Directory walker, metadata extraction
│   │       │   ├── watcher.rs      # FSEvents via notify, debounced updates
│   │       │   └── metadata.rs     # lofty integration, tag reading/writing
│   │       ├── format/
│   │       │   ├── parser.rs       # Format string tokenizer
│   │       │   ├── eval.rs         # Format string evaluator
│   │       │   └── functions.rs    # $if, $left, $right, $pad, etc.
│   │       ├── player/
│   │       │   ├── state.rs        # PlaybackState enum, shared state
│   │       │   ├── queue.rs        # Play queue, gapless transitions
│   │       │   └── commands.rs     # Lock-free command channel
│   │       ├── remote/             # Subsonic/Navidrome client (done)
│   │       └── config.rs           # TOML config (done)
│   │
│   ├── koan-cli/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs             # Entry point, clap CLI
│   │       ├── app.rs              # Ratatui App struct, event loop
│   │       ├── ui/
│   │       │   ├── mod.rs
│   │       │   ├── library.rs      # Library browser (tree/faceted)
│   │       │   ├── queue.rs        # Queue/playlist view
│   │       │   ├── now_playing.rs  # Transport bar + audio chain
│   │       │   ├── search.rs       # Fuzzy search overlay
│   │       │   ├── picker.rs       # Fuzzy picker (nucleo)
│   │       │   └── theme.rs        # Colours, styles
│   │       └── media_keys.rs       # souvlaki integration
│   │
│   └── koan-ffi/                   # Keep for potential future GUI, low priority
│       └── ...
│
└── justfile                        # Build commands
```

---

## Current State (What's Done)

### Phase 0-1: Core Audio ✅
- CoreAudio AUHAL engine with render callback
- Symphonia decode → rtrb ring buffer → DAC
- Device enumeration, sample rate switching, hog mode
- Gapless playback (decode thread keeps buffer alive across track boundaries)
- Format support: FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF

### Phase 2: Library + Scanner ✅
- SQLite FTS5 with WAL mode
- Parallel metadata scanning (rayon + walkdir + lofty)
- File watching (FSEvents via notify, 500ms debounce)
- Full schema: artists, albums, tracks, genres, play_stats, scan_cache

### Phase 3: Queue + Gapless ✅
- Track queue with gapless transitions
- Queue editing (reorder, delete)
- Next/previous track
- Inline picker during playback (p/a/r hotkeys)

### Phase 8: Subsonic/Navidrome ✅
- Remote library sync (parallel album detail fetches)
- Unified local+remote library
- Lazy parallel downloads with structured cache paths
- Track deduplication (local wins)

### CLI (Current) ✅
- Colourised output, tree glyphs
- Built-in nucleo fuzzy picker
- Queue display with album-grouped headers, braille download spinners
- Queue edit mode
- Dynamic shell completions from DB
- 26 unit tests

---

## Phase 4: Format String Engine

**Goal:** fb2k-compatible format strings for views, columns, and file operations. This is foundational — the TUI library browser, playlist columns, and file operations all depend on it.

**Status:** Files exist as empty stubs. Pure Rust, no new dependencies.

### `parser.rs` — Tokenizer
```rust
pub enum Token {
    Literal(String),          // plain text
    Field(String),            // %field%
    Conditional(Vec<Token>),  // [...]
    Function {                // $func(args)
        name: String,
        args: Vec<Vec<Token>>,
    },
}
pub fn parse(input: &str) -> Result<Vec<Token>, FormatError>;
```

### `eval.rs` — Evaluator
```rust
pub trait MetadataProvider {
    fn get_field(&self, name: &str) -> Option<String>;
}
pub fn evaluate(tokens: &[Token], provider: &dyn MetadataProvider) -> String;
```
- `%field%` → lookup from provider, empty string if missing
- `[...]` → only output if ALL field lookups inside succeeded
- `'...'` → literal, no expansion

### `functions.rs` — Built-in functions
- String: `$left`, `$right`, `$pad`, `$pad_right`, `$replace`, `$trim`, `$lower`, `$upper`, `$caps`
- Logic: `$if`, `$if2`, `$if3`, `$stricmp`
- Numeric: `$num`, `$div`, `$mod`
- Path: `$directory`, `$directory_path`, `$ext`, `$filename`
- Meta: `$info`, `$date`

### Test cases from real use
```
%album artist% → "Radiohead"
[%album artist% - ]%album% → "Radiohead - OK Computer" (or just "OK Computer" if no artist)
['('$left(%date%,4)')' ]%album% → "(1997) OK Computer"
$if(%genre%,%genre%,Unknown) → "Alternative" or "Unknown"
```

### Deliverable
Fully tested format string engine. All library view format strings from spec.md parse and evaluate correctly.

---

## Phase 5: Ratatui TUI

**Goal:** Replace the current custom ANSI rendering with a proper Ratatui-based TUI. Full-featured music player interface.

**Dependencies:** Format string engine (Phase 4) for library views and columns.

### Layout
```
┌──────────────────────────────────────────────┐
│ ◀ ▶ ■  ━━━━━━━━━●━━━━━━━  2:34/5:12  🔊    │  ← transport bar
├────────────┬─────────────────────────────────┤
│            │                                 │
│  Library   │   Track List / Queue            │
│  Browser   │                                 │
│  (tree or  │   columns, sortable, format-    │
│   facets)  │   string-driven                 │
│            │                                 │
│            │                                 │
│            │                                 │
├────────────┤                                 │
│            │                                 │
│  Album Art │                                 │
│  (sixel/   │                                 │
│   kitty)   │                                 │
├────────────┼─────────────────────────────────┤
│  Now Playing: Artist - Title | Album | FLAC  │
│  44.1kHz/16-bit → Built-in Output (matched)  │  ← audio chain
└──────────────────────────────────────────────┘
```

### Components
1. **Transport bar** — play/pause, prev/next, seek bar (gauge widget), time, volume
2. **Library browser** — tree view (artists → albums → tracks) driven by format strings
3. **Track list** — table widget with configurable columns, format-string-driven
4. **Queue view** — upcoming tracks, drag-reorder, status indicators
5. **Search overlay** — fuzzy search (nucleo) as a popup layer
6. **Now playing bar** — current track info + full audio chain status
7. **Album art** — sixel/kitty protocol for inline terminal images (ratatui-image)

### Mouse Support
Full mouse interaction via crossterm — this is a TUI that doesn't feel like one:
- **Click** to select tracks, switch panes, press transport buttons
- **Drag** to reorder tracks in queue/playlist (grab + drop with visual feedback)
- **Scroll wheel** in any list/pane
- **Click seek bar** to jump to position
- **Double-click** track to play
- **Right-click** for context menu (queue, play next, remove, etc.)
- Drag column headers to resize/reorder

### Keyboard Navigation
| Key | Action |
|-----|--------|
| `j`/`k` | Navigate lists |
| `Space` | Play/pause |
| `Enter` | Play selected |
| `/` | Search focus |
| `g`/`G` | Top/bottom |
| `Tab` | Cycle panes |
| `1`-`4` | Jump to pane |
| `q` | Quit |
| `< >` | Prev/next track |
| `, .` / `← →` | Seek ±10s |
| `p` | Pick tracks |
| `a` | Pick albums |
| `r` | Pick artists |
| `e` | Edit queue |
| `:` | Command mode |

### Media Keys (souvlaki)
- `souvlaki::MediaControls` for macOS MPRemoteCommandCenter
- Play/pause, next, prev, seek mapped to PlayerCommand
- MPNowPlayingInfoCenter: title, artist, album, duration, position, art
- Requires spawning a headless NSApplication (souvlaki handles this)

### Album Art in Terminal
- `ratatui-image` crate for sixel/kitty/iterm2 protocol support
- Extract art via lofty (embedded) or cover.jpg fallback
- Resize to fit pane, cache rendered output
- Graceful fallback: no art = no art pane, don't crash

### Deliverable
Full TUI music player. Browse library, manage queue, see audio chain, media keys work.

---

## Phase 6: Advanced Audio

**Goal:** ReplayGain, exclusive mode, integer mode. Audiophile-grade output.

**Status:** replaygain.rs and format.rs are empty stubs. Device management is implemented.

### ReplayGain (`audio/replaygain.rs`)
- Read existing RG tags from lofty (track gain, album gain, peak)
- Album mode by default (preserve dynamics within albums)
- Apply gain in f64 in the decode pipeline (after Symphonia decode, before rtrb push)
- Simple lookahead limiter to prevent clipping
- `koan scan --replaygain` command: decode → ebur128 → compute gain → write tags via lofty
- R128/EBU loudness normalization as alternative mode

### Exclusive/Integer Mode (`audio/format.rs`)
- When enabled: hog device, set physical format to match source
- Integer mode: set 24-bit integer physical format for compatible DACs
- Automatic sample rate switching already works (device.rs)
- Show full signal path in audio chain status bar

### Software Volume
- Off by default (pure digital passthrough)
- When enabled: f64 volume scaling in decode pipeline
- Logarithmic volume curve (perceptual)

### Deliverable
Bit-perfect chain visible in TUI. ReplayGain scan + playback. Exclusive mode toggle.

---

## Phase 7: File Operations

**Goal:** Rename/move files using format strings. Preview, batch, undo.

**Dependencies:** Format string engine (Phase 4).

### Implementation
- Format string → path generation using track metadata
- `koan organize` command with preview mode (dry run default)
- Preview: old path → new path for all affected files
- Move ancillary files (cover.jpg, .cue, .log) with music
- Remove empty directories after move
- Move log stored in SQLite for undo (`koan organize --undo`)
- Named presets in config

### Deliverable
`koan organize --pattern '%album artist%/(%date%) %album%/%tracknumber%. %title%'` — with full preview and undo.

---

## Deferred to v2+

- DSP chain (EQ, room correction, crossfeed)
- Spectral analysis / fake lossless detection
- MusicBrainz auto-tagging / acoustid
- Smart playlists (query-based)
- Waveform display / spectrum analyzer
- Multiple output devices
- Last.fm / Listenbrainz scrobbling
- CUE sheet support
- DSD native playback (DoP passthrough)
- Native GUI (if ever — TUI is the primary interface)

---

## Execution Order

### Now (parallel)
1. **Format string engine** (Phase 4) — pure Rust, no deps, self-contained
2. **ReplayGain** (Phase 6 partial) — ebur128 integration, tag read/write/apply

### Next
3. **Ratatui TUI** (Phase 5) — depends on format strings for library views
4. **Audio format switching** (Phase 6 partial) — integer/exclusive mode

### Then
5. **File operations** (Phase 7) — depends on format strings
6. **TUI polish** — album art, command palette, themes

---

## Build & Test

```bash
just check    # cargo test + clippy -D warnings
just fmt      # cargo fmt
just cli      # cargo run -p koan-cli -- <args>
just build    # cargo build --workspace --release
```

### Rules
- `edition = "2024"` (Rust 2024 edition)
- Zero clippy warnings: `cargo clippy --workspace -- -D warnings`
- Always run `cargo fmt` after changes
- Always run `cargo test --workspace` after changes
- Shell commands via `zsh -i -c '...'` for mise tools

---

_Because life's too short for audio players that can't even do gapless right._
