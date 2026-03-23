# Project Rules

## What is koan

Bit-perfect music player (macOS + Linux). Pure Rust, Ratatui TUI. Two crates:

- **koan-core** — library crate. Audio engine, player, database, indexer, format strings, file organization, remote (Subsonic/Navidrome) client. No UI code, no terminal deps.
- **koan-music** — binary crate (`koan`). Ratatui TUI, CLI (clap), media keys. Depends on koan-core.

If you want a different UI, write a new crate against koan-core. The CLI owns zero business logic.

## Architecture overview

Read `ARCHITECTURE.md` for the full technical manual (threading model, data flow, sync primitives, module reference). This section is the quick-ref.

### Threading model (5 threads at steady state)

```
Main Thread (TUI, 60fps)   ──crossbeam channel──►  Player Thread ("koan-player")
                                                       │
                                                       ├──rtrb ring buffer──►  Decode Thread ("koan-decode")
                                                       │
                                                       └──controls──►  Audio RT Thread (CoreAudio/cpal, system-managed)

Analyzer Thread ("koan-analyzer") ◄──VizBuffer──  Decode Thread
                                  ──VizSnapshot──►  Main Thread (TUI)
```

**Golden rule: the audio render callback must NEVER allocate or lock.** It only touches atomics and the rtrb consumer.

### Sync primitives

| Data | Primitive | Why |
|------|-----------|-----|
| PCM samples (decode→audio output) | `rtrb` SPSC ring buffer | Lock-free, cache-friendly |
| Commands (TUI→Player) | `crossbeam-channel` bounded(16) | Backpressure, timeout recv |
| Atomics (position, state, samples_played) | `AtomicU8/U64/Bool` Relaxed | Hot path, no contention |
| Complex shared state (playlist, track info) | `parking_lot::RwLock` | Faster than std, no poisoning |
| Viz samples (decode→analyzer) | `VizBuffer` (`parking_lot::Mutex`) | Ring of f32 for FFT |
| Analysis output (analyzer→TUI) | `VizSnapshot` (`parking_lot::Mutex`) | Atomic snapshot |
| Parallel work (scan, remote sync) | `rayon` | Work-stealing thread pool |

### Key data flow

```
File → Symphonia → f32 → rtrb ring buffer → platform audio callback → DAC
```

No resampling. Device sample rate switched to match source (bit-perfect). Float32 all the way.

### Key design decisions

- **QueueItemId (UUIDv7)** — all queue ops use IDs, not indices. Survives reordering, handles duplicate tracks.
- **Status is derived** — `QueueEntryStatus` computed from cursor + load state, never stored.
- **Decode cursor ≠ UI cursor** — decode thread peeks ahead for gapless without moving the playlist cursor.
- **One `derive_visible_queue()` per frame** — cached snapshot, all render/mouse ops see consistent state.
- **Track dedup across sources** — local file + remote entry = one DB row. 3-strategy match: path → remote_id → content.
- **Two-layer config** — `config.toml` (base, committable) + `config.local.toml` (machine-specific override).

## Git

- **NEVER push tags.** Tags and releases are handled externally. Only push commits.
- Work in PRs, never push to main.
- Don't rebase on merge — we squash PRs.

## Build & check

```bash
just check    # cargo test + clippy -D warnings
just fmt      # cargo fmt
just cli      # cargo run --release -p koan-music -- <args>
just build    # cargo build --release
```

Pre-push hook (`.claude/settings.json`) runs `cargo fmt --all` + `cargo clippy --workspace -- -D warnings` before any `git push`. If clippy fails, fix before pushing.

**Zero warnings policy.** Fix all clippy/compiler/lint warnings immediately. Run fmt after every change.

## Where things live

### koan-core (`crates/koan-core/src/`)

| Module | What |
|--------|------|
| `audio/backend.rs` | `AudioBackend` + `AudioEngineHandle` traits — platform-agnostic audio output |
| `audio/coreaudio_backend.rs` | macOS `CoreAudioBackend` impl (wraps engine.rs + device.rs) |
| `audio/cpal_backend.rs` | Linux `CpalBackend` impl (ALSA/PipeWire/PulseAudio via cpal) |
| `audio/engine.rs` | CoreAudio AUHAL setup, render callback (macOS only) |
| `audio/buffer.rs` | `PlaybackTimeline`, track boundaries, decode thread entry points (`start_decode`, `decode_queue_loop`, `decode_single`) |
| `audio/device.rs` | CoreAudio device enumeration, sample rate get/set (macOS only) |
| `audio/replaygain.rs` | EBU R128 loudness scanning, gain application via lofty |
| `audio/viz.rs` | `VizBuffer` (ring of f32 samples for analyzer), `VizSnapshot` (atomic snapshot for UI) |
| `audio/analyzer.rs` | FFT analysis thread — 48-band spectrum, VU meters, peak hold. Runs at configurable FPS |
| `audio/streaming.rs` | Progressive download with `Condvar`-based ready signaling |
| `player/mod.rs` | `Player` struct, command loop (`run()`), `start_playback()`, `update_playback_state()` |
| `player/commands.rs` | `PlayerCommand` enum, `CommandChannel` (bounded crossbeam) |
| `player/state.rs` | `SharedPlayerState`, `Playlist`, `PlaylistItem`, `QueueItemId`, `LoadState`, `PlaybackState`, `derive_visible_queue()` |
| `player/undo.rs` | Undo/redo stack for playlist operations (100-deep) |
| `db/schema.rs` | DDL: artists, albums, tracks, scan_cache, remote_servers, organize_log, tracks_fts (FTS5) |
| `db/connection.rs` | `Database::open()`, WAL mode, pragmas |
| `db/queries/` | Row types, upsert (3-strategy dedup), FTS5 search, scan cache, stats, snapshots |
| `index/scanner.rs` | Parallel library scan: walkdir → rayon → sequential DB upsert |
| `index/metadata.rs` | Tag reading via lofty (ID3, Vorbis, MP4, APE), codec detection |
| `format/` | fb2k-compatible template engine: parser (recursive descent), evaluator, 55 built-in functions |
| `remote/client.rs` | Subsonic/Navidrome HTTP client (reqwest blocking, MD5+salt auth) |
| `remote/sync.rs` | Parallel library sync: paginate → rayon fetch → batch DB write |
| `config.rs` | Two-layer TOML config loader |
| `credentials.rs` | Cross-platform credential store via keyring (macOS Keychain, Linux secret-service) |
| `organize.rs` | File rename using format strings. Preview/execute/undo. Moves ancillary files |
| `lyrics.rs` | LRCLIB lyrics fetching and parsing (synced LRC + plain) |

### koan-music (`crates/koan-music/src/`)

| Module | What |
|--------|------|
| `main.rs` | CLI entry point (clap), logger (file + buffer), signal handling |
| `commands/play.rs` | `cmd_play`, `run_tui` (event loop, picker loading, enqueue routing) |
| `commands/enqueue.rs` | `enqueue_playlist` (append/play/replace), download coordination |
| `commands/scan.rs` | `cmd_scan` |
| `commands/search.rs` | `cmd_search` (FTS5 with tree output) |
| `commands/remote.rs` | Remote login/sync/status |
| `commands/graphql/` | GraphQL schema (async-graphql), resolvers, axum HTTP server, in-process execution for MCP. Snapshot/radio/favourite mutations with remote sync |
| `commands/mod.rs` | Shared helpers: `open_db`, formatters, cache paths, playlist item builders |
| `tui/app.rs` | `App` state machine, `Mode` enum, event handlers per mode |
| `tui/ui.rs` | Render pipeline: layout → transport → content → overlays → hints |
| `tui/transport.rs` | Transport bar widget: seek bar, track info, click-to-seek |
| `tui/queue.rs` | Album-grouped queue with status icons, selection, drag targets |
| `tui/library.rs` | Flattened tree (artist→album→track), expand/collapse, substring filter |
| `tui/picker.rs` | Nucleo fuzzy search, multi-select, colored matches |
| `tui/cover_art.rs` | Halfblock rendering (2px per terminal cell, Lanczos3 resize) |
| `tui/visualizer.rs` | Spectrum analyzer widget (reads `VizSnapshot`) |
| `tui/lyrics.rs` | Lyrics side panel — synced line highlighting, scroll |
| `tui/organize.rs` | Organize modal: pattern picker → preview table → background execute |
| `media_keys.rs` | macOS Control Center via souvlaki, manual CFRunLoop pump |

## How to read the code

1. **Start:** `koan-core/src/player/state.rs` — the data model
2. **Then:** `koan-core/src/player/mod.rs` — the command loop
3. **Audio:** `audio/buffer.rs` (decode pipeline) → `audio/engine.rs` (CoreAudio setup)
4. **TUI:** `koan-music/src/tui/app.rs` (state machine) → `tui/ui.rs` (render)
5. **Database:** `db/schema.rs` (tables) → `db/queries/tracks.rs` (dedup logic)

## Concurrency patterns to follow

- **TUI→Player communication:** always via `PlayerCommand` through the crossbeam channel. Never reach into player internals from the TUI thread.
- **Player→TUI communication:** via `SharedPlayerState` (atomics + RwLock). TUI polls on tick (50ms).
- **Audio thread (CoreAudio/cpal):** atomics and rtrb only. No allocations, no locks, no channels.
- **Decode thread:** owns the Symphonia decoder. Communicates via rtrb producer + `PlaybackTimeline` (RwLock for boundaries, atomics for counters).
- **Background work** (downloads, lyrics fetch, organize): spawn named threads, communicate results via crossbeam one-shot channels or `Arc<Mutex<Option<T>>>` polling.
- **Parallel iteration** (scan, remote sync): rayon. Don't hand-roll thread pools.

## Dependencies (key choices)

| Dep | Why chosen |
|-----|-----------|
| `symphonia` | Rust-native decoder, all codecs, gapless support |
| `rtrb` | Lock-free SPSC ring buffer for audio — the only bridge between decode and audio output |
| `coreaudio-sys` | Raw CoreAudio AUHAL bindings for bit-perfect output (macOS) |
| `cpal` | Cross-platform audio I/O — ALSA/PipeWire/PulseAudio (Linux) |
| `keyring` | Cross-platform credential storage (macOS Keychain, Linux secret-service) |
| `crossbeam-channel` | Bounded MPSC with timeout recv — command channel + one-shots |
| `parking_lot` | Faster RwLock/Mutex, no poisoning |
| `rusqlite` (bundled) | SQLite with FTS5 for full-text search |
| `lofty` | Tag read/write across ID3, Vorbis, MP4, APE |
| `ratatui` + `crossterm` | TUI framework + terminal backend |
| `nucleo` | Fuzzy matching (same engine as Helix editor) |
| `souvlaki` | Media key / MPRIS / Now Playing |
| `reqwest` (blocking, rustls) | HTTP client for Subsonic API |
| `rayon` | Data parallelism for scan + sync |
| `ebur128` | EBU R128 loudness measurement for ReplayGain |
| `realfft` | FFT for spectrum analyzer |
| `async-graphql` | GraphQL schema derivation, execution engine |
| `axum` | HTTP server for GraphQL/Subsonic API |

## Roadmap

Active plans live in `.claude/plans/`. Key upcoming work:

1. **Decoupled backends** (plan 06) — trait-based audio/credentials abstraction. Foundational, unblocks everything.
2. **Linux support** (plan 01) — ALSA/PipeWire backends via `AudioBackend` trait.
3. **Tag editing** (plan 04) — vimv-style (TSV + $EDITOR) first, TUI inline editor second.
4. **DSP pipeline** (plan 02) — EQ, headphone profiles, crossfeed. Inserts between decode and ring buffer.
5. **Artist metadata** (plan 09) — bios, images, similar artists from MusicBrainz/Last.fm.

See `.claude/plans/README.md` for dependency graph and status.
