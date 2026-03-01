# Architecture

Developer manual for working on koan. Read this before touching the code.

## Crate structure

```
crates/
├── koan-core/     Library crate. Audio engine, player, database, indexer,
│                  format strings, file organization, remote client.
│                  No UI code, no terminal deps.
│
└── koan-cli/      Binary crate. The `koan` executable.
                   Ratatui TUI, CLI arg parsing, media keys.
                   Depends on koan-core.
```

Two crates, one workspace. `koan-core` is the engine, `koan-cli` is the interface. If you wanted a different UI (GUI, web, whatever), you'd write a new crate that depends on `koan-core` — the CLI doesn't own any business logic.

## Threading model

Four threads at steady state during playback:

```
┌─────────────────────────────────────────────────┐
│ Main Thread (TUI)                               │
│ Event loop: poll keyboard/mouse/tick at 50ms    │
│ Sends PlayerCommands, reads SharedPlayerState    │
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
│ Decode Thread   │  │ CoreAudio RT Thread      │
│ ("koan-decode") │  │ (system-managed)         │
│ Symphonia codec │  │ render_callback()        │
│ Writes producer │  │ Reads consumer           │
│ side of ring    │  │ Increments samples_played│
│ buffer          │  │ MUST NOT allocate/lock   │
└─────────────────┘  └──────────────────────────┘
```

**Sync primitives by thread:**

| Shared data | Written by | Read by | Primitive |
|---|---|---|---|
| PCM samples | Decode | CoreAudio RT | `rtrb` SPSC ring buffer (lock-free) |
| `samples_played` | CoreAudio RT | TUI, Player | `AtomicU64` (Relaxed) |
| `PlaybackState` | Player | TUI | `AtomicU8` (Relaxed) |
| `position_ms` | Player | TUI | `AtomicU64` (Relaxed) |
| `track_info` | Player | TUI | `parking_lot::RwLock` |
| `Playlist` | Player | TUI, Decode | `parking_lot::RwLock` |
| `playlist_version` | Player | TUI | `AtomicU64` (Relaxed) |
| Track boundaries | Decode | TUI, Player | `parking_lot::RwLock` |
| `running` flag | Player (start/stop) | CoreAudio RT | `AtomicBool` (Relaxed) |

**The golden rule: nothing on the CoreAudio render thread may allocate or lock.** It only touches atomics and the ring buffer consumer.

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
Ring Buffer (SPSC, ~192k samples)
    │
    ▼
rtrb::Consumer::read_chunk() ─── CoreAudio RT Thread
    │
    ▼
AudioBufferList (CoreAudio AUHAL output)
    │
    ▼
DAC → Speakers
```

**No resampling.** The device sample rate is switched to match the source file (bit-perfect). Float32 PCM from Symphonia all the way to CoreAudio.

**Backpressure:** If the ring buffer is full, decode sleeps 500µs and retries. If the ring buffer is empty (underrun), the render callback zeros the output (silence beats glitches).

## Gapless playback

The decode thread doesn't stop between tracks. When Symphonia hits EOF:

1. Decode calls `next_track()` closure → peeks the playlist for the next Ready item
2. If found, `decode_single()` starts on the next file immediately
3. The ring buffer producer stays alive — no gap in the PCM stream
4. A new `TrackBoundary` is pushed to the timeline with `sample_offset` = cumulative samples so far
5. CoreAudio keeps draining. When `samples_played` crosses the boundary, the UI sees the track change
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
| `engine.rs` | CoreAudio AUHAL setup, render callback (unsafe extern "C"), AudioEngine lifecycle |
| `buffer.rs` | `PlaybackTimeline` — track boundaries, `current_playback()` position query (binary search), decode thread entry points (`start_decode`, `decode_single`, `decode_queue_loop`) |
| `device.rs` | CoreAudio device enumeration, sample rate get/set, hog mode |
| `replaygain.rs` | EBU R128 loudness scanning, gain application, tag read/write via lofty |

### `player/`

| File | Purpose |
|---|---|
| `mod.rs` | `Player` struct, command loop (`run()`), `start_playback()`, `update_playback_state()` |
| `commands.rs` | `PlayerCommand` enum, `CommandChannel` (bounded crossbeam) |
| `state.rs` | `SharedPlayerState`, `PlaylistItem`, `Playlist`, `QueueItemId`, `LoadState`, `PlaybackState`, `derive_visible_queue()` |

### `db/`

| File | Purpose |
|---|---|
| `schema.rs` | DDL: artists, albums, tracks, scan_cache, remote_servers, organize_log, tracks_fts (FTS5) |
| `connection.rs` | `Database::open()`, WAL mode, pragmas |
| `queries/mod.rs` | Row types (`ArtistRow`, `AlbumRow`, `TrackRow`, `PlaybackSource`, `LibraryStats`, `TrackMeta`), re-exports |
| `queries/artists.rs` | Artist upsert/query |
| `queries/albums.rs` | Album upsert/query |
| `queries/tracks.rs` | Track upsert (3-strategy dedup: path → remote_id → content match), removal, playback source resolution |
| `queries/search.rs` | FTS5 full-text search |
| `queries/scan_cache.rs` | Mtime+size change detection to skip unchanged files |
| `queries/stats.rs` | Library statistics |

**Track dedup:** `upsert_track` tries three match strategies in order: (1) exact path match, (2) remote_id match, (3) content match (artist + album + title + track#). First match wins — the row is updated rather than duplicated. This merges local files with remote library entries into single rows.

### `index/`

| File | Purpose |
|---|---|
| `scanner.rs` | Parallel library scan: walkdir → rayon metadata extraction → sequential DB upsert in one transaction |
| `metadata.rs` | Tag reading via lofty (ID3, Vorbis, MP4, etc.), codec detection from extension |
| `watcher.rs` | FSEvents file watcher via notify, 500ms debounce, auto-indexes new/changed/deleted files |

### `format/`

fb2k-compatible template engine.

| File | Purpose |
|---|---|
| `parser.rs` | Recursive descent tokenizer: `%field%`, `[conditional]`, `$function(args)`, `'quoted'` |
| `eval.rs` | Evaluates token tree against a `MetadataProvider` trait. Conditionals omit block if any field missing. |
| `functions.rs` | 23 built-in functions: string ops (`left`, `right`, `pad`, `replace`, `trim`, `caps`), logic (`if`, `if2`, `if3`), numeric (`num`, `div`, `mod`), path (`directory`, `ext`, `filename`) |

### `remote/`

| File | Purpose |
|---|---|
| `client.rs` | Subsonic/Navidrome HTTP client. Token auth (MD5+salt). Endpoints: ping, getArtists, getAlbumList2, getAlbum, search3, scrobble, download |
| `sync.rs` | Parallel library sync: paginate albums (500/page) → rayon fetch full details → batch DB write per page |

### Other

| File | Purpose |
|---|---|
| `config.rs` | Two-layer TOML config: `config.toml` (base) + `config.local.toml` (override). Playback, library, remote settings. |
| `credentials.rs` | macOS Keychain integration via security-framework |
| `organize.rs` | File renaming using format strings. Preview/execute/undo. Moves ancillary files (cover art, cue sheets). Logs moves for undo. |

## koan-cli modules

### `main.rs`

CLI entry point (clap). Struct definitions, match dispatch, logger. Delegates everything to `commands/` modules.

### `commands/`

Subcommand handlers split into focused modules:

| File | Functions |
|---|---|
| `mod.rs` | Shared helpers: `open_db`, `format_time`, `format_bytes`, `install_terminal_panic_hook`, `get_remote_password`, `sanitise_filename`, `cache_path_for_track`, `playlist_item_from_track` |
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
| `app.rs` | `App` struct (state machine), `Mode` enum, `PickerAction` enum, event handlers (key/mouse per mode). Sub-state structs: `QueueState`, `LayoutRects`, `ArtState`. |
| `ui.rs` | Render pipeline: layout computation → transport bar → content area (queue ± library) → overlays (picker, track info, cover art zoom) → hint bar |
| `transport.rs` | `TransportBar` widget: seek bar (━─), current track info, click-to-seek |
| `queue.rs` | `QueueView` widget: album-grouped display with headers, status icons, selection markers, drag target line |
| `library.rs` | `LibraryState` + `LibraryView`: flattened tree (artist→album→track), expand/collapse, substring filter with cached artist list |
| `picker.rs` | `PickerState`: Nucleo fuzzy search engine, multi-select, colored result parts. Sentinel helpers for artist drill-down. |
| `cover_art.rs` | Halfblock rendering: extract from tags → resize with Lanczos3 → 2 pixels per terminal cell (upper half block char with FG/BG colors). Forces even pixel height to prevent black bar artifacts. |
| `track_info.rs` | `TrackInfoOverlay`: modal with full metadata fields + embedded album art |
| `theme.rs` | Color palette. Cyan for active/cursor, green for albums, DarkGray for hints. |
| `event.rs` | Event enum wrapper |
| `keys.rs` | `HintBar` widget: mode-specific key binding hints |

### `media_keys.rs`

macOS Control Center integration via souvlaki. Pumps CFRunLoop manually (terminal apps don't have a Cocoa event loop). Maps hardware play/pause/next/prev to PlayerCommands.

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
  │                                               ▲
  ├── 'p'/'a'/'r'/'/' ──► Picker ── Esc/Enter ───┤
  ├── 'l' ──────────────► LibraryBrowse ── Esc ───┤
  ├── 'i' ──────────────► TrackInfo ── Esc ────────┤
  └── 'z' ──────────────► CoverArtZoom ── Esc ─────┘
```

Mouse works in every mode — modality is keyboard-only. Double-click a queue track to play it, click seek bar to jump, drag to reorder, scroll wheel navigates.

## How to read the code

**Start here:** `koan-core/src/player/state.rs` — this is the data model. `SharedPlayerState`, `Playlist`, `PlaylistItem`, `QueueItemId`, `LoadState`, `derive_visible_queue()`. Everything else revolves around this.

**Then:** `koan-core/src/player/mod.rs` — the command loop. See how `process_command()` handles each `PlayerCommand` variant and how `start_playback()` wires decode → ring buffer → engine.

**Audio path:** `audio/buffer.rs` has `start_decode` → `decode_queue_loop` → `decode_single` (the actual Symphonia decode loop). `audio/engine.rs` has the CoreAudio setup and render callback.

**TUI:** `koan-cli/src/tui/app.rs` is the state machine. Follow `handle_normal_key()` for the main mode, `handle_tick()` for the per-frame update cycle. `ui.rs` is the render pipeline.

**Database:** Start at `db/schema.rs` for the table definitions, then `db/queries/tracks.rs` for the dedup logic in `upsert_track`.

## Key design decisions

**QueueItemId (UUIDv7):** Every queue entry gets a unique, time-ordered ID at creation. Queue commands use IDs, not indices. Handles duplicate tracks, survives reordering.

**Status is derived:** `QueueEntryStatus` (Playing/Queued/Played/Downloading/Failed) is computed from cursor position + load state, not stored. Single source of truth.

**Decode cursor ≠ UI cursor:** The decode thread peeks ahead for gapless without moving the playlist cursor. The player thread syncs them on boundary crossing.

**Atomic visible queue snapshot:** One `derive_visible_queue()` call per frame, cached in `vq_cache`. All render/mouse operations see consistent state within a frame.

**Two-layer config:** `config.toml` for defaults (committable to dotfiles), `config.local.toml` for machine-specific overrides (passwords, paths).

**Track dedup across sources:** Local file + Subsonic remote entry for the same song = one DB row. Local path always wins for playback.

## Dependencies

All deps are current as of March 2026. Key choices:

| Dep | Why |
|---|---|
| `symphonia` | Rust-native audio decoder. All codecs via `features = ["all"]`. Gapless support built in. |
| `rtrb` | Lock-free SPSC ring buffer. The only thing connecting decode → CoreAudio. |
| `coreaudio-sys` | Raw CoreAudio bindings for AUHAL output unit. |
| `rusqlite` | SQLite with `bundled-full` (portable, includes FTS5). |
| `lofty` | Tag reading/writing across ID3, Vorbis, MP4, APE. |
| `ratatui` + `crossterm` | TUI framework + terminal backend. |
| `nucleo` | Fuzzy matching engine (same as used by Helix editor). |
| `souvlaki` | Media key / MPRIS / Now Playing integration. |
| `reqwest` | HTTP client for Subsonic API (blocking mode, rustls TLS). |
| `rayon` | Data parallelism for library scanning and remote sync. |
| `notify` | Cross-platform file watching (FSEvents on macOS). |
| `ebur128` | EBU R128 loudness measurement for ReplayGain. |
| `parking_lot` | Faster RwLock/Mutex than std (no poisoning). |
