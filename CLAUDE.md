# Project Rules

## What is koan

Bit-perfect music player (macOS + Linux). Rust core, Ratatui TUI, plus a native SwiftUI app on macOS. Five crates:

- **koan-core** — library crate. Audio engine, player, database, indexer, format strings, file organization, remote (Subsonic/Navidrome) client, shared helpers. No UI code, no terminal deps.
- **koan-tui** — library crate. Ratatui TUI, visualizers, media keys, download queue. Exports `run_tui()`. Depends on koan-core.
- **koan-server** — library crate. GraphQL (async-graphql + axum), Subsonic REST API, MCP server. Depends on koan-core.
- **koan-ffi** — staticlib/cdylib crate. uniffi bindings exposing koan-core to Swift. Depends on koan-core only. Not published to crates.io.
- **koan-cli** — binary crate (`koan`). Thin entry point: clap CLI, logger, signal handling, command routing. Depends on koan-core + koan-tui + koan-server.

Plus **apps/macos** — SwiftUI app (SwiftPM, Swift 6, macOS 26+). Links koan-ffi.

Dependency rules (compiler-enforced): koan-tui, koan-server and koan-ffi cannot import each other; all three depend only on koan-core. Native clients import koan-core through koan-ffi.

**Local UI goes through FFI, not GraphQL.** The macOS app links the engine in-process — a daemon, a port and an auth surface buy nothing when the UI is sitting on top of the audio engine. GraphQL is the surface for clients that genuinely can't link the core: the web SPA, iOS, jukebox remotes. When adding a capability to one, consider whether the other needs it too — both are thin shims over the same koan-core helpers.

## Architecture overview

Read `ARCHITECTURE.md` for the full technical manual (threading model, data flow, sync primitives, module reference). This section is the quick-ref.

### Threading model (5 threads at steady state)

```
Main Thread (TUI, 60fps)   ──crossbeam channel──►  Player Thread ("koan-player")
                                                       │
                                                       ├──rtrb ring buffer──►  Decode Thread ("koan-decode")
                                                       │
                                                       └──controls──►  Audio RT Thread (CoreAudio/cpal, system-managed)

Analyzer Thread ("viz-analyzer") ◄──VizBuffer──  Decode Thread
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
- **Figment-layered config** — defaults → `config.toml` → `config.local.toml` → `KOAN_*` env vars. All writes go through `Config::persist()`, which diffs the mutation and routes each changed key by `config::layer_of` — secrets, this machine's paths/hardware/account and volatile UI state to `config.local.toml`, taste to `config.toml`. Comments survive; untouched keys are never rewritten.

## Git

- **NEVER push tags.** Tags and releases are handled externally. Only push commits.
- Work in PRs, never push to main.
- Don't rebase on merge — we squash PRs.

## Build & check

```bash
just check      # cargo test + clippy -D warnings
just fmt        # cargo fmt
just cli        # cargo run --release -p koan-cli -- <args>
just build      # cargo build --release
just macos-run  # build + launch the macOS app
just macos-dmg  # package the app for release
```

The macOS app needs `just macos-ffi` to have run at least once — it generates the Swift bindings that `swift build` compiles against. `macos-build` does this for you.

Development builds are signed with a self-signed certificate — `just macos-signing-cert` creates it, once. Without it the app is ad-hoc signed, which derives its identity from the binary's own hash: every rebuild is a different application to macOS, so TCC permissions are forgotten each time. It buys nothing against Gatekeeper, which wants Developer ID and notarisation.

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
| `audio/viz.rs` | `VizBuffer` (ring of f32 samples for analyzer), `VizSnapshot` (atomic snapshot for UI), `VizLevels` (the spectrum as three bands, for callers that poll often and draw little) |
| `audio/analyzer.rs` | FFT analysis thread — 48-band spectrum, VU meters, peak hold. Runs at configurable FPS |
| `audio/streaming.rs` | Progressive download with `Condvar`-based ready signaling |
| `player/mod.rs` | `Player` struct, command loop (`run()`), `start_playback()`, `update_playback_state()` |
| `player/commands.rs` | `PlayerCommand` enum, `CommandChannel` (bounded crossbeam) |
| `player/state.rs` | `SharedPlayerState`, `Playlist`, `PlaylistItem`, `QueueItemId`, `LoadState`, `PlaybackState`, `derive_visible_queue()` |
| `player/undo.rs` | Undo/redo stack for playlist operations (100-deep) |
| `player/history.rs` | Play history recording — writes an entry when a track starts, fills in listening time when it ends. Owns the `koan-history` writer thread |
| `db/schema.rs` | DDL: artists, albums, tracks, scan_cache, remote_servers, organize_log, tracks_fts (FTS5) |
| `db/connection.rs` | `Database::open()`, WAL mode, pragmas |
| `db/queries/` | Row types, upsert (3-strategy dedup), FTS5 search, scan cache, stats, playlists, `batch` (SQL-side track filtering, batched parent→child reads) |
| `index/scanner.rs` | Streaming library scan: walkdir → rayon tag reads → bounded channel → batched DB transactions. `ScanOptions` carries a cancel flag and an optional progress sink. `import_paths` indexes named files where they lie (Finder drops), removing nothing |
| `index/metadata.rs` | Tag reading via lofty (ID3, Vorbis, MP4, APE), codec detection |
| `index/id3v2_pictures.rs` | MP3 tag reads with the embedded art held back — walks the ID3v2 frame headers and serves lofty zeros over the picture frames it would only discard |
| `format/` | fb2k-compatible template engine: parser (recursive descent), evaluator, 59 built-in functions |
| `remote/client.rs` | Subsonic/Navidrome HTTP client (reqwest blocking, MD5+salt auth) |
| `remote/download.rs` | Streaming downloads: `.part` → verify → atomic rename, progress, retries. All disk-bound remote bytes go through here |
| `remote/sync.rs` | Parallel library sync: paginate → rayon fetch → batch DB write |
| `config.rs` | Figment-based layered config: defaults → config.toml → config.local.toml → KOAN_* env vars |
| `helpers.rs` | Shared by every front end: sign-in, favourite reconciliation, sharing, auto-sync and folder watching, forget-folder/forget-remote, cache and index maintenance |
| `playlists.rs` | Playlists beyond the database: two-way Subsonic reconciliation, background pushes, M3U8 export |
| `organize.rs` | File rename using format strings. Preview/execute/undo — one `PlanEntry` per file carrying its destination and outcome. Moves ancillary files |
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

### koan-ffi (`crates/koan-ffi/src/`)

| Module | What |
|--------|------|
| `lib.rs` | `KoanEngine` — the whole facade. Transport, queue ops, library queries, favourites, playlists, devices, scan. Every call that can block is `async`; only single-atomic reads stay sync |
| `offload.rs` | Where blocking work goes — a growing thread pool for reads, and one ordered lane for anything that ends in a `PlayerCommand` |
| `types.rs` | uniffi records mirroring koan-core types (`Track`, `Album`, `NowPlaying`, `QueueItem`, …) and the conversions |

Swift bindings are generated, not checked in — `just macos-ffi` builds the lib and regenerates them.

### apps/macos (`apps/macos/Sources/Koan/`)

| Module | What |
|--------|------|
| `KoanApp.swift` | `@main`, `AppState`, menu commands, keyboard shortcuts |
| `Support/ActivityModel.swift` | The one place that knows what koan is busy with. Library tasks are exclusive — they queue behind SQLite's single writer — and each is cancellable |
| `Support/SettingsModel.swift` | Settings state over `config.toml`. Commits on edit, re-reads on focus |
| `Support/PlayerModel.swift` | Polls `now_playing()` at 10 Hz; refetches the queue only when `playlistVersion` moves |
| `Support/Navigator.swift` | Where the app is: one page, the linear history of pages visited, and a cursor. No `NavigationStack` — koan navigates like a browser, any page from any page |
| `Support/LibraryModel.swift` | Browse state. Holds what the section on screen is showing and nothing else — narrowing and sorting happen in SQL, listings arrive whole. Follows the navigator; never moves it |
| `Support/CoverArtCache.swift` | Album-keyed art cache: bytes once per record on disk, bitmaps per record and draw size in a bounded `NSCache`. Each miss is an HTTP round trip on remote libraries |
| `Support/PlayingLevels.swift` | One analyser poller for every playing indicator on screen. Runs only while something is playing and something is watching |
| `Views/QueueView.swift` | The main stage — album-grouped queue, drag reorder, multi-select. Never torn down: `StageView` keeps it mounted behind other pages, because a macOS `List` cannot be scrolled back to where it was |
| `Views/PickerSheet.swift` | ⇧⌘K picker: multi-select, add / add-and-play / replace queue |
| `Views/TransportBar.swift` | Transport, seek, format badge, output device |
| `Views/LyricsPanel.swift` | Synced lyrics highlighted against position |
| `Views/SettingsView.swift` | Library / Server / Playback / Radio — everything needed to set koan up without a terminal |
| `Views/ActivityIndicator.swift` | The running-task rows at the foot of the sidebar |
| `Views/FavouriteButton.swift` | The heart, wherever something can be favourited |
| `Views/HistoryView.swift` | Play history, grouped by day — read-only, select and ⌫ to forget |
| `Views/PlaylistView.swift` | A playlist, laid out like the queue — grouped or flat, drag reorder, drop to add |
| `Support/PlaylistsModel.swift` | The playlists and everything done to them. Rows held whole; contents one at a time |
| `Views/OrganizeSheet.swift` | Organize: pattern + destination pickers, preview table with conflicts flagged per row |
| `Support/OrganizeModel.swift` | Organize sheet state — debounced preview, generation-guarded so a slow plan can't land on a newer one |

### koan-server (`crates/koan-server/src/`)

| Module | What |
|--------|------|
| `graphql/mod.rs` | GraphQL schema builder, `KoanSchema` type, SQLite connection pool, `with_db`/`blocking` offload helpers |
| `graphql/loaders.rs` | Dataloaders for artist→albums, album→tracks, counts, favourites |
| `graphql/jobs.rs` | Job registry for `triggerScan`/`triggerRemoteSync` — detached threads, polled via `job(id:)` |
| `graphql/queries.rs` | GraphQL query resolvers (artists, albums, tracks, nowPlaying, etc.) |
| `graphql/mutations.rs` | GraphQL mutations (playback, queue, favourites, playlists, organize) |
| `graphql/types.rs` | GraphQL type definitions (GqlArtist, GqlTrack, GqlNowPlaying, etc.) |
| `graphql/server.rs` | HTTP server (axum), `cmd_serve`, `start_api_background`, daemon mode, timeout/load-shed/panic-catch layers |
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

1. **Tag editing** (plan 04) — vimv-style (TSV + $EDITOR) first, TUI inline editor second.
2. **DSP pipeline** (plan 02) — EQ, headphone profiles, crossfeed. Inserts between decode and ring buffer.
3. **Artist metadata** (plan 09) — bios, images, similar artists from MusicBrainz/Last.fm.

See `.claude/plans/README.md` for dependency graph and status.
