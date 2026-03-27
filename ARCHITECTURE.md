# Architecture

Developer manual for working on koan. Read this before touching the code.

## Crate structure

```
crates/
├── koan-core/     Library crate. Audio engine, player, database, indexer,
│                  format strings, file organization, remote client.
│                  No UI code, no terminal deps.
│
└── koan-music/      Binary crate. The `koan` executable.
                   Ratatui TUI, CLI arg parsing, media keys.
                   Depends on koan-core.
```

Two crates, one workspace. `koan-core` is the engine, `koan-music` is the interface. If you wanted a different UI (GUI, web, whatever), you'd write a new crate that depends on `koan-core` — the CLI doesn't own any business logic.

## Threading model

Five threads at steady state during playback:

```
┌─────────────────────────────────────────────────┐
│ Main Thread (TUI)                               │
│ Event loop: configurable fps (default 60,       │
│ ~16.7ms per frame). Sends PlayerCommands,       │
│ reads SharedPlayerState.                        │
└───────────────┬─────────────────────────────────┘
                │ crossbeam channel (bounded 16)
                ▼
┌─────────────────────────────────────────────────┐
│ Player Thread ("koan-player")                   │
│ Command loop: recv_timeout(50ms)                │
│ Owns ActivePlayback (engine + decode handle)    │
│ Writes SharedPlayerState, syncs timeline        │
└───────┬───────────────────┬─────────────────────┘
        │ rtrb ring buffer  │ controls AudioEngine
        ▼                   ▼
┌─────────────────┐  ┌──────────────────────────┐
│ Decode Thread   │  │ Audio RT Thread          │
│ ("koan-decode") │  │ (system-managed)         │
│ Symphonia codec │  │ render_callback()        │
│ Writes producer │  │ Reads consumer           │
│ side of ring    │  │ Increments samples_played│
│ buffer          │  │ MUST NOT allocate/lock   │
└─────────────────┘  └──────────────────────────┘

┌─────────────────────────────────────────────────┐
│ Analyzer Thread ("koan-analyzer")               │
│ Always spawned. Reads VizBuffer, runs FFT,      │
│ writes VizSnapshot. Configurable fps (default   │
│ 60). Never blocks audio or UI.                  │
└─────────────────────────────────────────────────┘
```

**Sync primitives by thread:**

| Shared data | Written by | Read by | Primitive |
|---|---|---|---|
| PCM samples | Decode | Audio RT | `rtrb` SPSC ring buffer (lock-free) |
| `samples_played` | Audio RT | TUI, Player | `AtomicU64` (Relaxed) |
| `PlaybackState` | Player | TUI | `AtomicU8` (Relaxed) |
| `position_ms` | Player | TUI | `AtomicU64` (Relaxed) |
| `track_info` | Player | TUI | `parking_lot::RwLock` |
| `Playlist` | Player | TUI, Decode | `parking_lot::RwLock` |
| `playlist_version` | Player | TUI | `AtomicU64` (Relaxed) |
| Track boundaries | Decode | TUI, Player | `parking_lot::RwLock` |
| `running` flag | Player (start/stop) | Audio RT | `AtomicBool` (Relaxed) |
| Viz samples | Decode | Analyzer | `VizBuffer` (`parking_lot::Mutex` ring) |
| Analysis output | Analyzer | TUI | `VizSnapshot` (`parking_lot::Mutex`) |

**The golden rule: nothing on the audio render thread may allocate or lock.** It only touches atomics and the ring buffer consumer. This applies to both the CoreAudio callback (macOS) and the cpal callback (Linux).

## Audio data flow

```
File on disk
    │
    ▼
Symphonia (probe + decode) ─── Decode Thread
    │
    ▼
SampleBuffer<f32> (interleaved)
    │
    ▼
rtrb::Producer::write_chunk_uninit()
    │
    ▼
Ring Buffer (SPSC, 384k samples — 192k frames × 2 channels)
    │
    ▼
rtrb::Consumer::read_chunk() ─── Audio RT Thread (platform callback)
    │
    ▼
Platform output buffer (CoreAudio AudioBufferList / cpal &mut [f32])
    │
    ▼
DAC → Speakers
```

**No resampling.** On macOS, the device sample rate is switched to match the source file (bit-perfect). On Linux, the sample rate is set at stream creation. Float32 PCM from Symphonia all the way to the platform audio output.

**Backpressure:** If the ring buffer is full, decode sleeps 500µs and retries. If the ring buffer is empty (underrun), the render callback zeros the output (silence beats glitches).

## Gapless playback

The decode thread doesn't stop between tracks. When Symphonia hits EOF:

1. Decode calls `next_track()` closure → peeks the playlist for the next Ready item
2. If found, `decode_single()` starts on the next file immediately
3. The ring buffer producer stays alive — no gap in the PCM stream
4. A new `TrackBoundary` is pushed to the timeline with `sample_offset` = cumulative samples so far
5. The audio backend keeps draining. When `samples_played` crosses the boundary, the UI sees the track change
6. Player's `update_playback_state()` (every 50ms tick) notices the boundary crossing and syncs the cursor

The decode thread has its own cursor (`decode_cursor`) separate from the UI playlist cursor. Decode only *peeks* ahead — it never moves the real cursor. The player thread moves the cursor when the timeline confirms the transition.

## Player state machine

```
                    ┌───────────┐
                    │  STOPPED  │ ← default
                    └─────┬─────┘
                          │ Play(id) if Ready
                          ▼
        ┌─────────── PLAYING ◄──────────┐
        │           (engine on)         │
        │ Pause()        │              │ Resume()
        ▼                │              │
     PAUSED ─────────────┘──────────────┘
     (engine off, decode continues)
```

**Seek** = restart playback. New decode thread, new timeline, ring buffer flushed. `position_ms` is set immediately (before decode starts) to prevent UI flicker.

**Pause** stops the engine but decode continues filling the ring buffer. Resume is instant — no re-buffering.

## Queue & playlist

All queue entries get a `QueueItemId` (UUIDv7, time-ordered). Every operation uses IDs, not indices — handles duplicate tracks correctly and survives reordering.

```rust
struct Playlist {
    items: Vec<PlaylistItem>,
    cursor: Option<QueueItemId>,  // "what's playing"
}
```

**Status is derived, not stored.** Each item's display status (Playing, Queued, Played, Downloading, Failed) is computed from its position relative to the cursor and its `LoadState`. This happens once per frame in `derive_visible_queue()`.

**Advance vs peek:** `advance_cursor()` moves the cursor (called by explicit NextTrack). `peek_next_ready_after()` reads without moving (called by decode thread for gapless lookahead).

## koan-core modules

### `audio/`

| File | Purpose |
|---|---|
| `backend.rs` | `AudioBackend` + `AudioEngineHandle` traits — platform-agnostic audio output abstraction |
| `coreaudio_backend.rs` | macOS: `CoreAudioBackend` impl wrapping `engine.rs` + `device.rs` (`#[cfg(target_os = "macos")]`) |
| `cpal_backend.rs` | Linux: `CpalBackend` impl using cpal (ALSA/PipeWire/PulseAudio) (`#[cfg(target_os = "linux")]`) |
| `engine.rs` | CoreAudio AUHAL setup, render callback (unsafe extern "C"), AudioEngine lifecycle (macOS only) |
| `device.rs` | CoreAudio device enumeration, sample rate get/set (macOS only) |
| `buffer.rs` | `PlaybackTimeline` — track boundaries, `current_playback()` position query (binary search), decode thread entry points (`start_decode`, `decode_single`, `decode_queue_loop`) |
| `replaygain.rs` | EBU R128 loudness scanning, gain application, tag read/write via lofty |
| `viz.rs` | `VizBuffer` (lock-protected ring of f32 samples for analyzer), `VizSnapshot` (atomic snapshot for UI thread) |
| `analyzer.rs` | FFT analysis thread — 48-band spectrum, VU meters, peak hold. Configurable fps. Writes to `VizSnapshot`. |
| `streaming.rs` | Progressive download with `Condvar`-based ready signaling for streaming playback |

### `player/`

| File | Purpose |
|---|---|
| `mod.rs` | `Player` struct, command loop (`run()`), `start_playback()`, `update_playback_state()` |
| `commands.rs` | `PlayerCommand` enum (includes `UpdatePaths`, `InsertInPlaylist`), `CommandChannel` (bounded crossbeam) |
| `state.rs` | `SharedPlayerState`, `PlaylistItem`, `Playlist`, `QueueItemId`, `LoadState`, `PlaybackState`, `derive_visible_queue()`, `insert_items_after()`, `update_paths()` |
| `undo.rs` | Undo/redo stack for playlist operations (100-deep). Batching support for multi-step ops (e.g. drag). |

### `db/`

| File | Purpose |
|---|---|
| `schema.rs` | DDL: artists, albums, tracks, scan_cache, remote_servers, organize_log, tracks_fts (FTS5) |
| `connection.rs` | `Database::open()`, WAL mode, pragmas |
| `queries/mod.rs` | Row types (`ArtistRow`, `AlbumRow`, `TrackRow`, `PlaybackSource`, `LibraryStats`, `TrackMeta`), re-exports |
| `queries/artists.rs` | Artist upsert/query |
| `queries/albums.rs` | Album upsert/query |
| `queries/tracks.rs` | Track upsert (3-strategy dedup: path → remote_id → content match), removal, playback source resolution, `track_id_by_path()` |
| `queries/search.rs` | FTS5 full-text search |
| `queries/scan_cache.rs` | Mtime+size change detection to skip unchanged files |
| `queries/stats.rs` | Library statistics |
| `queries/lyrics.rs` | Lyrics caching (synced + plain, per-track) |
| `queries/favourites.rs` | Favourite/star status (syncs with Navidrome) |
| `queries/playback_state.rs` | Queue and playback position persistence across sessions |

**Track dedup:** `upsert_track` tries three match strategies in order: (1) exact path match, (2) remote_id match, (3) content match (artist + album + title + track#). First match wins — the row is updated rather than duplicated. This merges local files with remote library entries into single rows.

### `index/`

| File | Purpose |
|---|---|
| `scanner.rs` | Parallel library scan: walkdir → rayon metadata extraction → sequential DB upsert in one transaction |
| `metadata.rs` | Tag reading via lofty (ID3, Vorbis, MP4, etc.), codec detection from extension |

### `format/`

fb2k-compatible template engine.

| File | Purpose |
|---|---|
| `parser.rs` | Recursive descent tokenizer: `%field%`, `[conditional]`, `$function(args)`, `'quoted'` |
| `eval.rs` | Evaluates token tree against a `MetadataProvider` trait. Conditionals omit block if any field missing. |
| `functions.rs` | 55 built-in functions: string ops (`left`, `right`, `pad`, `replace`, `trim`, `caps`, `abbr`, `substr`, `insert`, `repeat`, `rot13`, etc.), logic (`if`, `if2`, `if3`, `ifequal`, `ifgreater`, `iflonger`, `select`, `not`, `and`, `or`, `xor`), numeric (`num`, `add`, `sub`, `mul`, `div`, `mod`, `max`, `min`, `hex`), path (`directory`, `directory_path`, `ext`, `filename`), info (`len`, `info`), special (`tab`, `crlf`, `char`) |

### `remote/`

| File | Purpose |
|---|---|
| `client.rs` | Subsonic/Navidrome HTTP client. Token auth (MD5+salt). Endpoints: ping, getArtists, getAlbumList2, getAlbum, search3, scrobble, download |
| `sync.rs` | Parallel library sync: paginate albums (500/page) → rayon fetch full details → batch DB write per page |
| `lrclib.rs` | LRCLIB API client for lyrics fetching (synced LRC + plain text) |

### Other

| File | Purpose |
|---|---|
| `config.rs` | Figment-based layered config: defaults → `config.toml` → `config.local.toml` → `KOAN_*` env vars. Playback, library, remote, graphql, radio, visualizer, organize, discovery settings. See `Config::update_base()` for safe writes. |
| `credentials.rs` | Cross-platform credential store via keyring (macOS Keychain, Linux secret-service) |
| `organize.rs` | File renaming using format strings. Preview/execute/undo. Scoped operations via `preview_for_tracks()`/`execute_for_tracks()` (used by TUI modal). Moves ancillary files (cover art, cue sheets). Logs moves for undo. |
| `lyrics.rs` | LRCLIB lyrics fetching and parsing (synced LRC + plain text). Cached per-track in SQLite. |

## koan-music modules

### `main.rs`

CLI entry point (clap). Struct definitions, match dispatch, logger. Delegates everything to `commands/` modules.

### `commands/`

Subcommand handlers split into focused modules:

| File | Functions |
|---|---|
| `mod.rs` | Shared helpers: `open_db`, `format_time`, `format_bytes`, `install_terminal_panic_hook`, `get_remote_password`, `sanitise_filename`, `cache_path_for_track`, `playlist_item_from_track`, `playlist_items_from_paths` |
| `play.rs` | `cmd_play` (path resolution, player spawn), `run_tui` (event loop, picker loading, enqueue routing) |
| `probe.rs` | `cmd_probe`, `cmd_devices` |
| `scan.rs` | `cmd_scan` |
| `search.rs` | `cmd_search` (FTS5 search with tree-grouped output) |
| `library.rs` | `cmd_artists`, `cmd_albums`, `cmd_library` |
| `config.rs` | `cmd_config`, `cmd_init` |
| `remote.rs` | `cmd_remote_login`, `cmd_remote_sync`, `cmd_remote_status` |
| `cache.rs` | `cmd_cache_status`, `cmd_cache_clear` |
| `organize.rs` | `cmd_organize` |
| `pick.rs` | `cmd_pick` (standalone fuzzy picker TUI) |
| `enqueue.rs` | `enqueue_playlist` (action-aware: append/play/replace), `resolve_item_path`, `download_single_track` |
| `picker_items.rs` | `load_picker_items`, `make_track_picker_items`, `make_album_picker_items`, `make_artist_picker_items` |

### `tui/`

| File | Purpose |
|---|---|
| `app.rs` | `App` struct (state machine), `Mode` enum, `PickerAction` enum, `ContextAction` enum, event handlers (key/mouse per mode). Sub-state structs: `QueueState`, `LayoutRects`, `ArtState`. |
| `ui.rs` | Render pipeline: layout computation → transport bar → content area (queue ± library) → overlays (picker, context menu, organize, track info, cover art zoom) → hint bar |
| `transport.rs` | `TransportBar` widget: seek bar (━─), current track info, click-to-seek |
| `queue.rs` | `QueueView` widget: album-grouped display with headers, status icons, selection markers, drag target line |
| `library.rs` | `LibraryState` + `LibraryView`: flattened tree (artist→album→track), expand/collapse, substring filter with cached artist list |
| `picker.rs` | `PickerState`: Nucleo fuzzy search engine, multi-select, colored result parts. Sentinel helpers for artist drill-down. |
| `cover_art.rs` | Halfblock rendering: extract from tags → resize with Lanczos3 → 2 pixels per terminal cell (upper half block char with FG/BG colors). Forces even pixel height to prevent black bar artifacts. |
| `track_info.rs` | `TrackInfoOverlay`: modal with full metadata fields + embedded album art |
| `theme.rs` | Color palette. Cyan for active/cursor, green for albums, DarkGray for hints. |
| `context_menu.rs` | `ContextMenuOverlay` widget: action list popup (currently: Organize) |
| `organize.rs` | `OrganizeModalState` + `OrganizeOverlay`: pattern picker, scoped preview table, background execute with path update propagation to player |
| `visualizer.rs` | Spectrum analyzer widget: reads `VizSnapshot`, renders 48-band bars with Unicode block chars, peak markers, amplitude coloring |
| `lyrics.rs` | Lyrics side panel: synced (LRC) line highlighting with auto-scroll, plain text fallback |
| `device_selector.rs` | Audio output device selection modal |
| `help_modal.rs` | Help overlay with key binding reference |
| `keys.rs` | `HintBar` widget: mode-specific key binding hints |

### `media_keys.rs`

macOS Control Center integration via souvlaki. Pumps CFRunLoop manually (terminal apps don't have a Cocoa event loop). Maps media key events to PlayerCommands: play/pause/stop, next/prev, seek (absolute + relative), quit. Sends track metadata including album art (extracted to temp file, passed as file:// URL).

## Picker actions

The picker (track/album/artist search) has three confirm actions:

| Key | Action | Behaviour |
|---|---|---|
| `Enter` | Append | Add to end of queue, don't play |
| `Ctrl+Enter` | Append & Play | Add to end, start playing first added track |
| `Ctrl+R` | Replace | Clear entire queue, add tracks, play from top |

Library browser and artist drill-down default to Append & Play. The `PickerAction` enum flows through `picker_result` → `enqueue_playlist()` → `PlayerCommand` sequence.

## TUI modes

```
Normal ──── 'e' ────► QueueEdit ──── Esc ────► Normal
  │                       │                       ▲
  │                       └── Space ──► ContextMenu ── Esc ──┤
  │                                        │                 │
  │                                        └── Enter ──► Organize ── Esc ──┤
  ├── 'p'/'a'/'r'/'/' ──► Picker ── Esc/Enter ───┤
  ├── 'l' ──────────────► LibraryBrowse ── Esc ───┤
  ├── 'i' ──────────────► TrackInfo ── Esc ────────┤
  └── 'z' ──────────────► CoverArtZoom ── Esc ─────┘
```

**Organize modal flow:** QueueEdit → select tracks → Space (context menu) → Organize → pick named pattern from config → scrollable preview of file moves → Enter to execute → paths updated in playlist, playback uninterrupted (Unix rename preserves open FDs).

Mouse works in every mode — modality is keyboard-only. Double-click a queue track to play it, click seek bar to jump, drag to reorder, scroll wheel navigates.

## How to read the code

**Start here:** `koan-core/src/player/state.rs` — this is the data model. `SharedPlayerState`, `Playlist`, `PlaylistItem`, `QueueItemId`, `LoadState`, `derive_visible_queue()`. Everything else revolves around this.

**Then:** `koan-core/src/player/mod.rs` — the command loop. See how `process_command()` handles each `PlayerCommand` variant and how `start_playback()` wires decode → ring buffer → engine.

**Audio path:** `audio/buffer.rs` has `start_decode` → `decode_queue_loop` → `decode_single` (the actual Symphonia decode loop). `audio/backend.rs` defines the `AudioBackend` trait; platform implementations live in `coreaudio_backend.rs` (macOS) and `cpal_backend.rs` (Linux).

**TUI:** `koan-music/src/tui/app.rs` is the state machine. Follow `handle_normal_key()` for the main mode, `handle_tick()` for the per-frame update cycle. `ui.rs` is the render pipeline.

**Database:** Start at `db/schema.rs` for the table definitions, then `db/queries/tracks.rs` for the dedup logic in `upsert_track`.

## Key design decisions

**QueueItemId (UUIDv7):** Every queue entry gets a unique, time-ordered ID at creation. Queue commands use IDs, not indices. Handles duplicate tracks, survives reordering.

**Status is derived:** `QueueEntryStatus` (Playing/Queued/Played/Downloading/Failed) is computed from cursor position + load state, not stored. Single source of truth.

**Decode cursor ≠ UI cursor:** The decode thread peeks ahead for gapless without moving the playlist cursor. The player thread syncs them on boundary crossing.

**Atomic visible queue snapshot:** One `derive_visible_queue()` call per frame, cached in `vq_cache`. All render/mouse operations see consistent state within a frame.

**Figment-layered config:** Four layers (defaults → `config.toml` → `config.local.toml` → `KOAN_*` env vars) merged by [figment](https://docs.rs/figment). Env vars use `KOAN_SECTION__FIELD` naming (double underscore splits into nested keys). `Config::load()` returns the fully merged result. **`Config::update_base()`** is the safe way to programmatically modify `config.toml` — it reads only the base file, applies a mutation closure, and writes back. Never call `save()` on a `load()`-ed Config — it would serialize secrets from `config.local.toml` and env vars into the base file. `save_local()` writes to `config.local.toml` with `0o600` permissions for sensitive values.

**Track dedup across sources:** Local file + Subsonic remote entry for the same song = one DB row. Local path always wins for playback.

## Dependencies

All deps are current as of March 2026. Key choices:

| Dep | Why |
|---|---|
| `symphonia` | Rust-native audio decoder. All codecs via `features = ["all"]`. Gapless support built in. |
| `rtrb` | Lock-free SPSC ring buffer. The only thing connecting decode → audio output. |
| `coreaudio-sys` | Raw CoreAudio bindings for AUHAL output unit (macOS only). |
| `cpal` | Cross-platform audio I/O — ALSA/PipeWire/PulseAudio backend (Linux only). |
| `keyring` | Cross-platform credential storage (macOS Keychain, Linux secret-service). |
| `rusqlite` | SQLite with `bundled-full` (portable, includes FTS5). |
| `lofty` | Tag reading/writing across ID3, Vorbis, MP4, APE. |
| `ratatui` + `crossterm` | TUI framework + terminal backend. |
| `nucleo` | Fuzzy matching engine (same as used by Helix editor). |
| `souvlaki` | Media key / MPRIS / Now Playing integration. |
| `reqwest` | HTTP client for Subsonic API (blocking mode, rustls TLS). |
| `rayon` | Data parallelism for library scanning and remote sync. |
| `ebur128` | EBU R128 loudness measurement for ReplayGain. |
| `parking_lot` | Faster RwLock/Mutex than std (no poisoning). |
