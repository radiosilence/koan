# Project Rules

## What is koan

Bit-perfect music player (macOS + Linux). Pure Rust, Ratatui TUI. Four crates:

- **koan-core** — library crate. Audio engine, player, database, indexer, format strings, file organization, remote (Subsonic/Navidrome) client, shared helpers. No UI code, no terminal deps.
- **koan-tui** — library crate. Ratatui TUI, visualizers, media keys, download queue. Exports `run_tui()`. Depends on koan-core.
- **koan-server** — library crate. GraphQL (async-graphql + axum), Subsonic REST API, MCP server. Depends on koan-core.
- **koan-cli** — binary crate (`koan`). Thin entry point: clap CLI, logger, signal handling, command routing. Depends on koan-core + koan-tui + koan-server.

Dependency rules (compiler-enforced): koan-tui and koan-server cannot import each other. Future iOS app imports only koan-core.

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
- **Figment-layered config** — defaults → `config.toml` → `config.local.toml` → `KOAN_*` env vars. Use `Config::update_base()` for base config writes, `patch_local(section, values)` for machine-specific updates to `config.local.toml`.

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
| `config.rs` | Figment-based layered config: defaults → config.toml → config.local.toml → KOAN_* env vars |
| `credentials.rs` | Cross-platform credential store via keyring (macOS Keychain, Linux secret-service) |
| `organize.rs` | File rename using format strings. Preview/execute/undo. Moves ancillary files |
| `lyrics.rs` | LRCLIB lyrics fetching and parsing (synced LRC + plain) |

### koan-tui (`crates/koan-tui/src/`)

| Module | What |
|--------|------|
| `play.rs` | `run_tui()` — TUI event loop entry point, frame timing, input handling |
| `app.rs` | `App` state machine, `Mode` enum, event handlers per mode |
| `ui.rs` | Render pipeline: layout → transport → content → overlays → hints |
| `transport.rs` | Transport bar widget: seek bar, track info, click-to-seek |
| `queue.rs` | Album-grouped queue with status icons, selection, drag targets |
| `library.rs` | Flattened tree (artist→album→track), expand/collapse, substring filter |
| `picker.rs` | Nucleo fuzzy search, multi-select, colored matches |
| `cover_art.rs` | Halfblock rendering (2px per terminal cell, Lanczos3 resize) |
| `visualizer.rs` | Spectrum analyzer widget (reads `VizSnapshot`) |
| `lyrics.rs` | Lyrics side panel — synced line highlighting, scroll |
| `organize.rs` | Organize modal: pattern picker → preview table → background execute |
| `media_keys.rs` | macOS Control Center via souvlaki, manual CFRunLoop pump |
| `download_queue.rs` | Persistent download queue with priority/cursor-aware reordering |
| `enqueue.rs` | `enqueue_playlist()` — build PlaylistItems from track IDs, submit downloads |
| `remote_bridge.rs` | Remote bridge: connects TUI to a remote koan server via GraphQL |

### koan-server (`crates/koan-server/src/`)

| Module | What |
|--------|------|
| `graphql/mod.rs` | GraphQL schema builder, `KoanSchema` type, DB handle wrapper |
| `graphql/queries.rs` | GraphQL query resolvers (artists, albums, tracks, nowPlaying, etc.) |
| `graphql/mutations.rs` | GraphQL mutations (playback, queue, favourites, snapshots, organize) |
| `graphql/types.rs` | GraphQL type definitions (GqlArtist, GqlTrack, GqlNowPlaying, etc.) |
| `graphql/server.rs` | HTTP server (axum), `cmd_serve`, `start_api_background`, daemon mode |
| `subsonic.rs` | Subsonic-compatible REST API (XML/JSON, auth, streaming, cover art) |
| `mcp.rs` | MCP server for Claude Desktop (schema_sdl + graphql tools) |

### koan-cli (`crates/koan-cli/src/`)

| Module | What |
|--------|------|
| `main.rs` | CLI entry point (clap), logger (file + buffer), signal handling |
| `commands/play.rs` | `cmd_play` — orchestrates player spawn, queue restore, calls `run_tui()` |
| `commands/scan.rs` | `cmd_scan` |
| `commands/search.rs` | `cmd_search` (FTS5 with tree output) |
| `commands/remote.rs` | Remote login/sync/status |
| `commands/mod.rs` | Shared CLI helpers: `open_db`, formatters, path parsing, playlist builders |

## How to read the code

1. **Start:** `koan-core/src/player/state.rs` — the data model
2. **Then:** `koan-core/src/player/mod.rs` — the command loop
3. **Audio:** `audio/buffer.rs` (decode pipeline) → `audio/engine.rs` (CoreAudio setup)
4. **TUI:** `koan-tui/src/app.rs` (state machine) → `ui.rs` (render)
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
