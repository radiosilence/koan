# Changelog

## Unreleased

### Fixed

- **`koan play /dir` with large libraries** — for >1000 files, uses a single `all_tracks_by_path` DB query instead of hundreds of batched `WHERE IN` queries. Directory walk + metadata resolution now runs on a background thread so the TUI starts immediately
- **Organize preview takes minutes on large libraries** — `preview_for_paths` now loads metadata from the DB (single query) instead of re-reading every file's tags from disk. Falls back to parallel disk reads (rayon) for files not in the DB. 48k-track library: ~5 minutes → ~3 seconds

## v0.18.5 (2026-04-05)

### Fixed

- **Hi-res audio playing at wrong speed** — CoreAudio sample rate switches are asynchronous, but the player read back the device rate immediately after requesting the change and got the *old* rate. A fallback (`unwrap_or(source_rate)`) then masked the mismatch by lying to the ASBD. Result: 96kHz files played at quarter speed (device still clocked at the old rate, draining the ring buffer too slowly). `set_device_sample_rate` now polls until CoreAudio confirms the switch (10ms intervals, 2s timeout) and returns the verified rate. Both file and streaming playback paths fixed. ([#124](https://github.com/radiosilence/koan/pull/124))

## v0.18.4 (2026-04-01)

### Fixed

- **Audio seize-up at album transitions** — when the gapless decode loop exhausted the playlist, the Player had no way to know playback finished. Audio engine kept running, outputting silence. Now the decode thread signals `DecodeFinished` and the Player auto-advances or stops cleanly ([#122](https://github.com/radiosilence/koan/pull/122))
- **Double engine restart at session restore** — startup sent Play+Pause+Seek, causing three engine teardown/rebuild cycles. Now sets cursor without playback; the deferred seek is the single start point ([#122](https://github.com/radiosilence/koan/pull/122))
- **Key repeat rapid-skipping** — terminal key repeat on `>`/`<` could fire dozens of NextTrack/PrevTrack commands. Added 150ms debounce in the Player command loop ([#122](https://github.com/radiosilence/koan/pull/122))

### Changed

- **Now-playing queue indicator** — playing track now shows ▶ instead of `>`, with bold title text for visibility ([#122](https://github.com/radiosilence/koan/pull/122))

## v0.18.3 (2026-04-01)

### Changed

- **CLI: `koan init` → `koan config init`** — config initialization is now a subcommand of `koan config`. `koan config` with no subcommand still shows resolved config ([#120](https://github.com/radiosilence/koan/pull/120))
- **Config init generates commented template** — `config.toml` now contains all defaults as commented lines for reference. Uncomment what you want to customize. No more silent duplication of values across config files ([#120](https://github.com/radiosilence/koan/pull/120))
- **`[library]` and `[remote]` excluded from `config.toml`** — machine-specific paths and credentials belong in `config.local.toml` only. Prevents accidental credential leaks into dotfile repos ([#120](https://github.com/radiosilence/koan/pull/120))
- **ReplayGain default changed to `off`** — was `album`. Users who want loudness normalization can opt in via `replaygain = "album"` or `"track"` ([#120](https://github.com/radiosilence/koan/pull/120))

### Fixed

- **`koan remote login` no longer bloats `config.local.toml`** — previously wrote all default config sections; now patches only the `[remote]` section, preserving the rest of the file as-is ([#120](https://github.com/radiosilence/koan/pull/120))
- **Removed `Config::save()` footgun** — method could leak secrets from merged config into `config.toml`. Replaced with `Config::patch_local(section, values)` for targeted local config updates ([#120](https://github.com/radiosilence/koan/pull/120))
- **`.gitignore` now covers `*.db-wal` and `*.db-shm`** — SQLite WAL files were previously not gitignored ([#120](https://github.com/radiosilence/koan/pull/120))

## v0.18.2 (2026-03-29)

### Changed

- **CLI: `koan play` subcommand** — play-related args (`paths`, `--album`, `--artist`, `--id`, `--library`, `--clear`, `--server`, `--jukebox`) moved from the root command to `koan play`. Running bare `koan` still launches the TUI. This fixes zsh tab completions which broke when positional paths were on the root struct alongside subcommands ([#116](https://github.com/radiosilence/koan/issues/116))

### Fixed

- **Tab completions** — zsh/bash/fish completions now correctly suggest subcommands instead of filesystem paths ([#116](https://github.com/radiosilence/koan/issues/116))
- **Docs: `koan mcp`** — corrected all references from `--mcp` flag to `mcp` subcommand
- **Docs: removed fabricated Docker content** — no Docker image exists
- **Docs: GraphQL operations table** — fixed naming convention, added missing operations
- **Docs: radio mode scoring** — corrected signal descriptions to match actual implementation
- **Docs: added missing CLI commands** — `koan analyze`, `koan completions`, `scan --force`
- **Docs: added missing `V` keybinding** for visualizer toggle

- **Documentation rewrite** — slimmed README from 740 lines to a focused hook + install + quickstart + feature list + doc links. All detailed content moved to dedicated guides and references under `docs/`:
  - `docs/getting-started.md` — progressive first-time setup tutorial
  - `docs/guide/` — radio mode, remote servers, file organization, GraphQL API, MCP integration, headless server
  - `docs/reference/` — configuration (all fields including previously undocumented `ticker_fps`, `target_fps`, `show_fps`, `art_size`, `output_device`), keybindings (every key in every mode), CLI reference
  - `docs/recipes/` — troubleshooting, cache management

## v0.18.1 (2026-03-28)

### Changed

- **Config loading uses figment** — replaced hand-rolled TOML deep-merge with [figment](https://docs.rs/figment) for layered config: defaults → `config.toml` → `config.local.toml` → `KOAN_*` env vars. Any config field is now overridable via environment variables using `KOAN_SECTION__FIELD` naming (e.g. `KOAN_REMOTE__PASSWORD`, `KOAN_GRAPHQL__PORT`, `KOAN_PLAYBACK__TARGET_FPS`)

### Fixed

- **Secret round-trip leak in config save** — `save()` on a merged Config would serialize secrets from `config.local.toml` and env vars back into `config.toml`. Callers now use `Config::update_base()` which reads only `config.toml`, applies the mutation, and writes back without leaking sensitive fields

- **Path traversal in organize** — `sanitize_relative_path` now strips `..` and `.` components, and `plan_single_move` validates the destination stays under the base directory. Prevents malicious metadata from writing files outside the library ([#99](https://github.com/radiosilence/koan/issues/99))
- **RT safety in CPAL audio callback** — changed `Mutex::lock()` to `try_lock()` in the audio render callback so the real-time thread never blocks; outputs silence on contention instead ([#99](https://github.com/radiosilence/koan/issues/99))
- **O(N) LRU cache query** — replaced correlated `SELECT MAX(played_at)` subquery per track with a single `LEFT JOIN` on pre-aggregated play_history ([#99](https://github.com/radiosilence/koan/issues/99))
- **Sequential scan_cache lookups** — scanner now batch-loads the entire scan cache into a HashMap instead of issuing one DB query per file, dramatically faster for large libraries ([#99](https://github.com/radiosilence/koan/issues/99))
- **Memory usage on playlist build** — `playlist_items_from_paths` now uses `tracks_by_paths()` (batched IN-query) instead of loading every track in the library into a HashMap ([#99](https://github.com/radiosilence/koan/issues/99))

### Added

- **API concurrency limit** — GraphQL server now applies a tower `ConcurrencyLimitLayer` (max 10 concurrent requests) to prevent mutation spam / DoS ([#99](https://github.com/radiosilence/koan/issues/99))
- **Composite index** on `tracks(album_id, disc, track_number)` for faster album-ordered queries ([#99](https://github.com/radiosilence/koan/issues/99))
- **CoreAudio crash during sample rate switch** — `stop_engine()` was dropping the `AudioEngine` on a background cleanup thread while the player thread immediately changed the device sample rate. The engine is now dropped synchronously before any sample rate changes; only the decode handle cleanup runs in the background ([#89](https://github.com/radiosilence/koan/issues/89))
- **Render callback drain on AudioEngine drop** — `AudioOutputUnitStop` can return before the render callback finishes during sample rate switches. Added `in_callback` atomic flag and spin-wait in `Drop` to ensure the callback has fully exited before tearing down buffers ([#89](https://github.com/radiosilence/koan/issues/89))
- **Pending items never downloaded on session restore** — the cache verify fix correctly marked missing files as `Pending`, but never actually triggered downloads. Introduced a persistent `DownloadQueue` that lives for the app's lifetime: session restore feeds pending items into it, and double-clicking a pending track triggers a priority download with stream-when-ready playback. The same queue replaces the one-shot scoped thread pool previously used by `enqueue_playlist` ([#94](https://github.com/radiosilence/koan/issues/94))
- **GraphQL/Subsonic port bind panic** — `run_api_blocking` called `.expect()` on port bind, crashing the entire app on `AddrInUse`. Now logs a warning and gracefully disables the API server ([#95](https://github.com/radiosilence/koan/issues/95))
- **TUI layout jump when album art loads** — the transport bar now always reserves a 24×12 cell placeholder for album art, preventing layout reflow when art loads or when switching between tracks with/without embedded art ([#96](https://github.com/radiosilence/koan/issues/96))

### Added

- **Track `db_id` in playlist items** — `PlaylistItem` and `PersistedQueueItem` now carry `db_id: Option<i64>`, enabling re-download of remote tracks after session restore. Backwards-compatible: old persisted state without `db_id` deserializes cleanly via `#[serde(default)]` ([#94](https://github.com/radiosilence/koan/issues/94))
- **Cache management with LRU eviction** — cached remote downloads are now tracked in the DB (path, size, download date). Set `cache_limit` in `[remote]` config (e.g. `"50GB"`) to enable automatic LRU eviction on startup. Evicts whole albums, oldest last-played first. Favourited tracks are never evicted. New `koan cache evict` subcommand for manual eviction ([#88](https://github.com/radiosilence/koan/issues/88))

## v0.17.1 (2026-03-27)

### Fixed

- **GraphQL/Subsonic servers now bind to 127.0.0.1 by default** — previously bound to `0.0.0.0` with no authentication, exposing library enumeration, file moves, and queue clearing to anyone on the network. Added `bind` field to `[graphql]` config and `--bind` CLI flag ([#85](https://github.com/radiosilence/koan/issues/85))

### Added

- **Album-aware download priority** — when a track starts playing, remaining tracks from the same album are bumped to the front of the download queue, ensuring gapless album playback ([#87](https://github.com/radiosilence/koan/issues/87))
- **CONTRIBUTING.md** — contribution guidelines ([#82](https://github.com/radiosilence/koan/issues/82))

## v0.17.0 (2026-03-26)

### Fixed

#### Remote

- **Replace hand-rolled ISO 8601 parser with chrono** — the manual RFC 3339 parser in `remote/sync.rs` (~70 lines) could panic on malformed input from Subsonic servers. Replaced with `chrono::DateTime::parse_from_rfc3339()` + fallback patterns for common server quirks (missing timezone, fractional seconds, space separators). Added 11 unit tests ([#74](https://github.com/radiosilence/koan/pull/74))

#### Audio

- **Atomic ordering hardened** — `samples_played` uses `AcqRel` on fetch_add, `Acquire` on loads. `running` flag uses `Acquire`/`Release`. No more `Relaxed` for cross-thread state ([#76](https://github.com/radiosilence/koan/pull/76))
- **Timeline lock ordering** — `PlaybackTimeline::current_playback()` now acquires the boundaries read lock first, then reads `samples_played` inside that scope. Dead standalone `channels`/`sample_rate` atomics removed ([#76](https://github.com/radiosilence/koan/pull/76))
- **Alignment check in CoreAudio callback** — replaced `debug_assert!` with a runtime check that fills silence on misalignment instead of UB ([#76](https://github.com/radiosilence/koan/pull/76))
- **Buffer bounds validation** — `ptr::copy_nonoverlapping` in `engine.rs` and `cpal_backend.rs` now clamps to available space and fills remainder with silence ([#76](https://github.com/radiosilence/koan/pull/76))
- **VizBuffer allocation reuse** — added `VizBuffer::snapshot_into(&self, out: &mut Vec<f32>)` to reuse caller's buffer instead of allocating per frame ([#76](https://github.com/radiosilence/koan/pull/76))

#### Player

- **Atomic ordering across player state** — all cross-thread atomics (`playback_state`, `position_ms`, `playback_generation`, `playlist_version`, `bytes_written`, `quit_requested`, `metadata_refresh_pending`, `radio_mode`, `pump_written`) upgraded from `Relaxed` to `Acquire`/`Release`/`AcqRel` ([#78](https://github.com/radiosilence/koan/pull/78))
- **Undo stack O(1) eviction** — `Vec` replaced with `VecDeque` for O(1) `pop_front()` instead of O(n) `remove(0)`. Batch depth capped at 500 to prevent unbounded nesting ([#78](https://github.com/radiosilence/koan/pull/78))
- **Seek underflow on short tracks** — guard `max_ms > 5_000` before subtracting safety margin, preventing short/partially-downloaded tracks from clamping seek to 0 ([#78](https://github.com/radiosilence/koan/pull/78))
- **ClearPlaylist snapshot race** — `stop_playback_and_clear_state()` (engine teardown only) now runs before snapshotting the playlist for undo, then clears. Previously `stop()` cleared the playlist before the undo snapshot was captured ([#78](https://github.com/radiosilence/koan/pull/78))

#### TUI

- **Terminal restoration on any thread panic** — removed main-thread-only guard from panic hook. Terminal is restored (raw mode, alternate screen, mouse capture, bracketed paste, cursor) regardless of which thread panics ([#77](https://github.com/radiosilence/koan/pull/77))
- **Cursor clamping on queue mutation** — `clamp_queue_cursor()` called after `delete_selected()` and on every playlist version change. Render-time clamp kept as safety net ([#77](https://github.com/radiosilence/koan/pull/77))
- **Cover art cache bounded** — `CoverArt::clear()` frees the `DynamicImage` when nothing is playing ([#77](https://github.com/radiosilence/koan/pull/77))
- **Double-click timeout clearing** — stale `last_click_time`/`last_click_idx` cleared after 1 second, preventing misinterpreted double-clicks ([#77](https://github.com/radiosilence/koan/pull/77))
- **Picker cursor safety** — render loop clamps cursor to `matched_count` range before computing scroll offset, preventing out-of-bounds when results shrink between ticks ([#77](https://github.com/radiosilence/koan/pull/77))

#### Database

- **Transaction boundaries** — `upsert_track()` wraps the entire artist/album/track/FTS5 operation in a savepoint. Scanner and analyzer use proper `unchecked_transaction()` with error propagation instead of silent `let _ =` drops ([#79](https://github.com/radiosilence/koan/pull/79))
- **Source column validation** — `CHECK (source IN ('local', 'remote', 'cached'))` constraint on tracks table ([#79](https://github.com/radiosilence/koan/pull/79))
- **WAL checkpoint on connect** — `PRAGMA wal_checkpoint(PASSIVE)` at connection open prevents unbounded WAL growth across sessions ([#79](https://github.com/radiosilence/koan/pull/79))
- **LIKE wildcard escaping** — `remove_stale_tracks` now escapes `%`, `_`, `\` in path prefixes via `escape_like()` ([#79](https://github.com/radiosilence/koan/pull/79))
- **Missing index** — added `idx_library_folders_path` on `library_folders(path)` ([#79](https://github.com/radiosilence/koan/pull/79))

#### Misc

- **Batch ID collision** — `chrono_batch_id()` uses `as_nanos()` instead of `as_millis()` to prevent millisecond-resolution collisions ([#75](https://github.com/radiosilence/koan/pull/75))
- **Unicode-aware string comparison** — `stricmp` format function uses `.to_lowercase()` instead of ASCII-only `eq_ignore_ascii_case()` ([#75](https://github.com/radiosilence/koan/pull/75))
- **Ancillary file move errors logged** — `execute_single_move_no_db` now logs warnings via `log::warn!` instead of silently swallowing with `.ok()` ([#75](https://github.com/radiosilence/koan/pull/75))
- **Tokio features scoped** — `"full"` replaced with `["rt-multi-thread", "net", "macros", "signal"]` in koan-music ([#75](https://github.com/radiosilence/koan/pull/75))

### Changed

- **Unified daemon mode** — `koan` now runs TUI + GraphQL API in one process by default. No more separate `koan serve`. All interfaces share one player, one state ([#70](https://github.com/radiosilence/koan/issues/70))
  - `koan --headless` replaces `koan serve` (GraphQL API only, no TUI)
  - `koan -d` / `koan --daemonize` forks a headless background daemon
  - `koan --mcp` replaces `koan mcp` (MCP server on stdio)
  - `koan --no-api` opts out of the API server (TUI-only, old behaviour)
  - `koan --port`, `--subsonic`, `--playground` configure the API from top-level
  - Play args (`--album`, `--artist`, `--id`, `--library`, `--clear`, `--server`, `--jukebox`) moved to top-level — `koan play` removed
  - `koan scan --analyze` combines scan + acoustic analysis in one pass

### Removed

- **`koan play`** — `koan` IS play. All args moved to top-level
- **`koan serve`** — replaced by `koan --headless`
- **`koan graphql`** — dead alias, removed
- **`koan mcp`** — replaced by `koan --mcp` flag
- **`koan pick`** — standalone picker removed, TUI has built-in pickers (`p`/`a`/`r`)
- **`koan artists`**, **`koan albums`** — use `koan search` or GraphQL queries

## 0.16.0

### Changed

- **GraphiQL v2** — replaced deprecated GraphQL Playground with the official GraphQL Foundation IDE. Actively maintained, subscription-ready, better UX. `koan serve --playground` now serves GraphiQL ([#71](https://github.com/radiosilence/koan/issues/71))
- **Clean schema type names** — stripped `Gql` prefix from all GraphQL types. `GqlArtist` → `Artist`, `GqlTrack` → `Track`, `GqlNowPlaying` → `NowPlaying`, etc. The public schema now has clean, idiomatic names

## 0.15.0

### Added

- **Linux audio support** — `AudioBackend` trait abstraction with `CpalBackend` (ALSA/PipeWire/PulseAudio via cpal) for Linux and `CoreAudioBackend` for macOS. Bit-perfect gapless on both platforms. Decode pipeline untouched — backends are dumb ring buffer consumers ([#58](https://github.com/radiosilence/koan/pull/58))
- **`koan serve`** — unified server command. GraphQL API (always on) + optional Subsonic REST (`--subsonic <port>`). Replaces `koan graphql`. One process, one player, two interfaces ([#55](https://github.com/radiosilence/koan/pull/55))
- **Subsonic REST API** — 22 endpoints for third-party clients (play:Sub, Amperfy). Browsing, search, streaming with Range + proxy, cover art, star/unstar, scrobble, playlists (mapped to snapshots), genres. XML + JSON, MD5+salt auth ([#55](https://github.com/radiosilence/koan/pull/55))
- **`koan play --server`** — TUI client mode via GQL. Streams audio locally from a remote `koan serve` instance ([#55](https://github.com/radiosilence/koan/pull/55))
- **`--jukebox` mode** — server plays audio, client is remote control only ([#55](https://github.com/radiosilence/koan/pull/55))
- **Acoustic similarity** — `koan analyze` generates 23-dim bliss-audio fingerprints. Radio mode gains `SimilarityAxis::Acoustic`. `similarTracks(trackId, limit)` GQL query ([#68](https://github.com/radiosilence/koan/pull/68))
- **GraphQL API** — full Relay-style cursor pagination, rich metadata filters (year, codec, genre, sample rate, bit depth, duration), fuzzy search, lyrics, cover art, organize, scan, sync, share mutations ([#36](https://github.com/radiosilence/koan/pull/36))
- **MCP server: GraphQL-first** — 2 tools: `schema_sdl` + `graphql`. Claude reads the schema, drives everything through one tool
- **Named queue snapshots** — save/restore/list/delete via GQL + MCP. Bank curated mixes and switch between them
- **Radio mode via API** — `enableRadio`/`disableRadio` mutations. SharedPlayerState atomic keeps TUI and API in sync
- **Favourites filter + remote sync** — `favouritesOnly` on all queries, `isFavourite` on tracks. Star/unstar auto-syncs to Subsonic/Navidrome
- **`[discovery]` config** — `analysis_on_scan`, `acoustic_weight` for acoustic similarity tuning
- **Neural discovery (feature-gated)** — DCLAP ONNX embeddings behind `neural-discovery` cargo feature. `textSearch` GQL query, `koan analyze --neural`. Opt-in, graceful degradation ([#69](https://github.com/radiosilence/koan/pull/69))
- **Cross-platform credentials** — `keyring` crate replaces `security-framework` (macOS Keychain + Linux secret-service)
- **CI for Linux** — clippy, test, build on macOS + Ubuntu. Release binaries: macOS arm64/x86_64 + Linux x86_64/arm64 (native runners)

### Fixed

- **Remote tracks silently skipped** — GQL mutations now trigger background downloads. Correct cache paths via `resolve_item_path()` (single code path with TUI)
- **`restoreSnapshot` downloads** — snapshot restore now runs the download pipeline like `addToQueue`
- **N+1 query elimination** — genre/favourite filtering uses batch SQL instead of per-item calls ([#64](https://github.com/radiosilence/koan/pull/64))
- **GraphQL injection** — query building converted from `format!()` to proper variables ([#63](https://github.com/radiosilence/koan/pull/63))
- **Remote bridge hardening** — exhaustive `PlayerCommand` match, incomplete downloads marked Failed, 30s HTTP timeouts ([#60](https://github.com/radiosilence/koan/pull/60))
- **Linux: ALSA/JACK stderr spam** — cpal backend probe output suppressed via fd redirect during all operations
- **Linux: Ctrl+C terminal restore** — second Ctrl+C force-restores raw mode and exits immediately
- **Scanner: empty files** — 0-byte files get clear error instead of confusing Symphonia probe messages
- **`--playground` flag** — changed from `Option<bool>` to proper flag
- **`insert_in_queue`** — was silently appending, now uses `InsertInPlaylist`
- **Ctrl+C on GQL server** — graceful shutdown via `tokio::signal::ctrl_c`

### Changed

- **graphql.rs split** — 2400-line file decomposed into `graphql/{mod,types,queries,mutations,helpers,server}.rs` ([#67](https://github.com/radiosilence/koan/pull/67))
- **`Player` holds `Box<dyn AudioBackend>`** — all device/engine calls go through trait
- **SubsonicClient factory** — `subsonic_client()` helper replaces 9 manual creation sites ([#65](https://github.com/radiosilence/koan/pull/65))
- **Player device restart dedup** — `restart_on_current_track()` + `Config::load_or_default()` ([#62](https://github.com/radiosilence/koan/pull/62))
- **serve.rs route dedup** — `register_subsonic_routes()` shared between prod and test ([#61](https://github.com/radiosilence/koan/pull/61))
- **Platform-gated deps** — `coreaudio-sys`/`core-foundation` macOS-only, `cpal` Linux-only

## 0.14.0

### Added

- **Acoustic similarity** — `koan analyze` generates 23-dim acoustic fingerprints (tempo, timbre, chroma, spectral features) via bliss-audio. Stored in SQLite, brute-force KNN is sub-millisecond. Radio mode gains `SimilarityAxis::Acoustic` — finds tracks that *sound* similar regardless of metadata. `similarTracks(trackId, limit)` GraphQL query for "more like this"
- **`[discovery]` config section** — `analysis_on_scan` (run analysis during library scan, default false) and `acoustic_weight` (scoring weight for acoustic signal)
- **Empty file handling** — scanner skips 0-byte files with a clear error instead of confusing "probe reach EOF" messages

## 0.13.1

### Fixed

- **N+1 query elimination** — genre and favourite filtering now use batch SQL queries instead of per-item DB calls. O(1) instead of O(n*m) on large libraries
- **Remote bridge hardening** — exhaustive PlayerCommand match (compiler catches new variants), incomplete downloads marked as Failed instead of Ready, 30s HTTP timeouts
- **GraphQL client injection fix** — all query building converted from format!() string interpolation to proper GraphQL variables
- **Player device restart dedup** — extracted shared restart logic, config load errors now logged instead of silently swallowed (`Config::load_or_default()`)
- **SubsonicClient factory** — single `subsonic_client()` helper replaces 9 manual construction sites, 30s timeout on all HTTP clients
- **serve.rs route dedup** — extracted `register_subsonic_routes()`, test router no longer duplicates prod routes
- **CI reliability** — arm64 cross-compile no longer silently fails, tags not force-pushed, doc tests added

### Changed

- **graphql.rs split** — 2400-line god file decomposed into `graphql/{mod,types,queries,mutations,helpers,server}.rs`

## 0.13.0

### Added

- **Linux audio support** — `AudioBackend` trait abstraction with `CpalBackend` (ALSA/PipeWire/PulseAudio via cpal) for Linux and `CoreAudioBackend` wrapper for macOS. Bit-perfect gapless playback on both platforms. The decode pipeline and ring buffer are untouched — backends are dumb consumers
- **`koan serve`** — unified server command. GraphQL API (always on) + optional Subsonic REST (`--subsonic <port>`). Replaces `koan graphql` (kept as hidden alias). One process, one player, two interfaces
- **Subsonic REST API** — 22 endpoints for third-party client compatibility (play:Sub, Amperfy). Browsing, search, streaming with Range support, cover art, star/unstar, scrobble, playlists (mapped to snapshots), genres. XML + JSON, MD5+salt auth. Proxy streaming for remote tracks
- **`koan play --server`** — TUI client mode. Connects to a remote `koan serve` via GQL. Client streams audio locally from the server
- **`--jukebox` mode** — server plays audio, client is remote control only
- **GQL client library** in koan-core — typed helpers for all queries and mutations
- **Cross-platform credentials** — `keyring` crate replaces `security-framework`. macOS Keychain + Linux secret-service
- **CI builds for Linux** — clippy, test, build on both macOS and Ubuntu. Release binaries for macOS arm64/x86_64 + Linux x86_64/arm64

### Changed

- `Player` holds `Box<dyn AudioBackend>` instead of direct CoreAudio FFI
- Platform-gated deps: `coreaudio-sys`/`core-foundation` macOS-only, `cpal` Linux-only

## 0.12.5

### Fixed

- **Remote tracks now play when queued via GQL/MCP** — two-part fix:
  1. GQL mutations now trigger background downloads for remote tracks (0.12.4)
  2. Remote tracks now get the correct cache path via `resolve_item_path()` — same code path as the TUI

## 0.12.4

### Fixed

- **Remote track download pipeline wired to GQL mutations** — `addToQueue` and `replaceQueue` now spawn background downloads for remote tracks using the same pipeline as the TUI

## 0.12.3

### Added

- **`lyrics(trackId)` query** — fetch synced LRC or plain text lyrics for any track. Checks embedded tags, sidecar `.lrc` files, and LRCLIB. Cached in DB
- **`coverArt(trackId)` query** — extract embedded cover art as base64 with MIME type. Supports JPEG and PNG
- **`organizePreview` / `organizeExecute` mutations** — preview and execute file renames using fb2k-compatible format strings. Supports per-track or whole-library operations
- **`organizeUndo` mutation** — undo the last organize batch
- **`triggerScan` mutation** — trigger a library rescan from the API. Returns added/updated/unchanged counts
- **`triggerRemoteSync` mutation** — trigger Subsonic/Navidrome library sync from the API
- **`createShare(trackIds, description)` mutation** — create Subsonic sharing links for tracks. Returns the public URL. Claude can now share what it's playing

## 0.12.2

### Added

- **`fuzzySearch` GraphQL query** — nucleo-powered typo-tolerant fuzzy matching for tracks, albums, and artists. Same engine as the TUI picker (and Helix editor). Returns ranked results. `{ fuzzySearch(query: "aphx twn", kind: TRACK, limit: 10) { id name rank kind } }`

## 0.12.1

### Changed

- **MCP server: GraphQL-first interface** — stripped 40+ individual tools down to just 2: `schema_sdl` (introspect the full schema) and `graphql` (execute any query or mutation). All operations go through GraphQL now. Claude calls `schema_sdl` first to learn the API, then uses `graphql` for everything. Cleaner, less context overhead, same capabilities

## 0.12.0

### Added

- **GraphQL API** — `koan graphql` starts a headless player with an HTTP GraphQL server (default port 4000). Full Relay-style cursor pagination on artists, albums, and tracks. One nested query replaces multiple MCP tool calls: `{ artists(first: 100) { edges { node { id, name } } } }`. Mutations for all playback control, queue management, favourites, and device switching. Optional GraphQL Playground UI at `GET /graphql` with `--playground` flag or `playground = true` in `[graphql]` config
- **MCP `graphql` tool** — single tool on the MCP server that executes GraphQL queries in-process (no HTTP). Claude Desktop can now fetch artists, albums, and tracks with nested queries in one round-trip instead of fanning out across individual tools
- **`[graphql]` config section** — `port` (default 4000) and `playground` (default false) in config.toml
- **Named queue snapshots** — save/restore/list/delete named queue states via GQL mutations (`saveSnapshot`, `restoreSnapshot`, `deleteSnapshot`) and MCP tools (`save_snapshot`, `restore_snapshot`, `list_snapshots`, `delete_snapshot`). Bank the techno, switch to hardcore, jump back. Stored in the DB (`queue_snapshots` table) with queue JSON, cursor path, and playback position
- **Radio mode via API** — `enableRadio`/`disableRadio` GQL mutations and `enable_radio`/`disable_radio`/`radio_status` MCP tools. Radio mode was previously TUI-only (Shift+R). Uses SharedPlayerState atomic so TUI and API stay in sync
- **Favourites filter** — `favouritesOnly: true` parameter on `artists`, `albums`, and `tracks` queries. Dedicated `favourites` query with cursor pagination. `isFavourite` field on track type
- **Favourite → remote sync** — favouriting/unfavouriting via GQL or MCP automatically syncs to Subsonic/Navidrome (`star`/`unstar` API) on a background thread. Fire-and-forget, best-effort
- **`clear_device` MCP tool** — reset audio output to system default (was GQL-only)
- **Daemon mode** — `koan graphql -d` forks the server into background, writes PID to `~/.config/koan/graphql.pid`. Claude Code can start it and query via HTTP
- **`schema_sdl` MCP tool** — dumps the full GraphQL schema in SDL format so Claude can introspect all available queries, mutations, types, and filter params on first connect
- **`similarArtists` query** — returns scored similar artists (from ListenBrainz, MusicBrainz, Subsonic) with source and relationship type
- **`playHistory` query** — recent play history with track info, paginated
- **Comprehensive MCP instructions** — rewritten server instructions guide Claude through discovery, the graphql power tool, all filter params, snapshots, radio, favourites, and device control
- **Rich metadata filters on all queries** — albums: `title`, `yearStart`/`yearEnd`, `codec`, `label`, `genre`. Tracks: `title`, `artistName`, `albumTitle`, `genre`, `codec`, `yearStart`/`yearEnd`, `minSampleRate`, `minBitDepth`, `channels`, `minDurationMs`/`maxDurationMs`. Artists: `genre`. All string filters case-insensitive substring

### Fixed

- **`insert_in_queue` MCP tool** — was silently appending instead of inserting after the specified `after_queue_item_id`. Now uses `InsertInPlaylist` command directly
- **`--playground` CLI flag** — was `Option<bool>` requiring `--playground true`. Now a proper flag
- **Ctrl+C on GraphQL server** — `axum::serve` was blocking forever. Added `with_graceful_shutdown` using `tokio::signal::ctrl_c`

## 0.11.1

### Fixed

- **MCP server crash on startup** — all tool return types now use object schemas (MCP 2025-11-25 spec requires `outputSchema` root type to be `object`). Bare string and array returns replaced with `StatusResponse`, `QueueResponse`, `TrackListResponse`, `ArtistListResponse`, `AlbumListResponse`, `DeviceListResponse` wrapper types

### Tests

- **32 MCP server tests** — coverage for all playback commands, queue management (add/remove/clear/replace/reorder), library discovery (search, list_artists, list_albums, list_tracks, get_track, library_stats), state queries (now_playing, list_devices, set_device), UUID parsing, track resolution, and error paths

## 0.11.0

### Added

- **MCP server** — `koan mcp` runs koan as a headless MCP (Model Context Protocol) server on stdio, controllable by Claude Desktop or any MCP client. Exposes 21 tools: playback control (play/pause/resume/stop/next/previous/seek), queue management (add/insert/remove/clear/replace/reorder/get), library discovery (search/list_artists/list_albums/list_tracks/get_track/library_stats), state queries (now_playing/list_devices/set_device), and favourites (favourite/unfavourite/list_favourites). The LLM provides the taste and reasoning — koan just exposes the controls. Configure in Claude Desktop with `{"command": "koan", "args": ["mcp"]}`
- **Visualiser toggle** — press `V` (Shift-V) to enable/disable the spectrum visualiser at runtime. Persists to config.toml. Visible in `?` help menu under Toggles
- **Multi-signal radio mode** — radio now uses ListenBrainz ML similarity, MusicBrainz relationship graph (collaborators, band members, associated acts), Subsonic, genre/era matching, and play history to pick tracks across multiple axes instead of just one source. Drifting seed window follows your recent plays instead of anchoring to a single track. Recency scoring surfaces buried gems (never-played and long-forgotten tracks get a discovery bonus). New config options: `history_window` (don't repeat last N, default 200), `seed_window` (last N plays as seed, default 5), `discovery_weight` (0.0-1.0, default 0.3)
- **Play history tracking** — koan now records track completions in a `play_history` table, used for recency scoring in radio mode and future scrobbling

## 0.10.0

### Added

- **Radio mode** — press `R` to toggle infinite play. When the queue runs low, koan automatically picks similar tracks using Subsonic `getSimilarSongs2` (when a remote server is configured), cached similar-artist relationships, and genre/artist matching from the local library. A magenta `RADIO` badge appears in the hint bar when active. Configurable via `[radio]` in config.toml (lookahead, batch_size, use_subsonic)

## 0.9.2

### Fixed

- **UI freeze on track change** — `stop_engine()` no longer blocks the player command loop waiting for the decode thread to join. Engine teardown (thread join + AudioUnit dispose) is moved to a background cleanup thread, so the player stays responsive even when CoreAudio or I/O is slow to shut down
- **Escape sequence dump on crash** — the panic hook was calling `disable_raw_mode()` and `LeaveAlternateScreen` from whichever thread panicked, corrupting the terminal when a background thread (decode, download) hit an error. The hook now captures the main thread ID at install time and only restores terminal state from the TUI thread
- **Decode thread panic on missing file** — `SourceEntry::from_file` used `panic!` when a file couldn't be opened (e.g. deleted during gapless lookahead). The `make_mss` closure is now fallible (`-> io::Result`), and decode errors are logged gracefully instead of crashing
- **AudioEngine drop race** — removed unreliable `thread::yield_now()` before `AudioUnitUninitialize`. `AudioOutputUnitStop` is synchronous (callback guaranteed finished on return), so no extra wait is needed. The callback is also explicitly removed as a safety net for the rare case where stop fails
- **Silent decode thread panics** — `DecodeHandle::stop()` now logs the panic message instead of silently swallowing `handle.join()` errors

## 0.9.1

### Added

- **Queue persistence** — queue and playback position are automatically saved every second and restored on next launch. Ctrl+C and `q` both trigger a clean save. Use `--clear` to start fresh instead of restoring
- **Graceful Ctrl+C** — replaced raw `SIG_DFL` with a safe signal handler so Ctrl+C performs a clean shutdown (saving state, restoring terminal) instead of killing the process

### Fixed

- **Quit race condition** — quit handlers were sending `PlayerCommand::Stop` (which clears the playlist) before saving state, so persisted queue was always empty. Stop is now sent after saving

## 0.9.0

### Added

- **Navidrome share links** — right-click a song or album header in the queue and select "Share link" to create a public sharing URL via the Subsonic API. The link is copied to clipboard and shown in the hint bar. Prefers album-level shares when all selected tracks are from the same album. Requires `[remote]` to be configured with sharing enabled on the server
- **Double-click album headers** — double-clicking an album header in the queue now starts playback from the first track of that album

## 0.8.2

### Added

- **Homebrew tap** — `brew install radiosilence/koan/koan`. Formula auto-updates on each release via CI

## 0.8.1

### Added

- **Help modal** — press `?` to open a two-column keybindings reference showing all modes (playback, navigation, queue edit, picker, library). Status bar now shows only high-priority hints; full reference lives in the modal

## 0.8.0

### Added

- **Output device selector** — press `Shift+D` to open a modal listing all available CoreAudio output devices. Current device is marked with a green bullet. Selecting a device switches playback immediately (preserving position and pause state). Choice is persisted to `[playback] output_device` in config.toml and restored on startup, with automatic fallback to system default if the device is unavailable

### Fixed

- **Stale album codec after format upgrade** — upgrading files from MP3→FLAC (or any format change) now correctly updates the album's codec in the picker. Previously `get_or_create_album()` only set codec on first insert, so the album row kept the old format even after all tracks were re-scanned
- **Streaming tracks skip mid-playback** — two issues causing premature track advancement during streaming: (1) the pump thread treated `read() → Ok(0)` as EOF even when the download reported more bytes available (OS buffer flush lag), now retries instead of breaking; (2) `refresh_track_metadata` (called when download completes mid-stream) didn't update `TrackInfo.duration_ms`, leaving the UI with an underestimated duration from the initial 256KB partial probe — now re-probes the complete file

## 0.7.2

### Fixed

- **`koan init` leaks home directory into config.toml** — `library.folders` (containing the resolved `~/Music` path) was written to the shareable `config.toml` instead of `config.local.toml`. Now `config.toml` omits `library.folders` entirely, and `config.local.toml` gets the detected music directory as a starting point

## 0.7.1

### Fixed

- **Resilient tag parsing** — files with corrupted tags (e.g. malformed UTF-16 ID3 frames) no longer fail the entire scan. When lofty errors, falls back to Symphonia for duration/codec/properties and indexes the file with whatever metadata is available
- **Suppressed library log spam** — noisy warn-level messages from lofty/symphonia internals are filtered from stderr (still written to log file). Fallback warnings from koan include the file path for diagnostics

## 0.7.0

### Added

- **DB cache for playlist loading** — when adding files that are already in the library database, metadata is pulled from SQLite instead of re-reading from disk, making re-adds near-instant
- **Scan progress bar** — `koan scan` now shows a clean inline progress indicator with track count and rate (e.g. `• 1234 scanned (567/s)`) instead of per-track log spam
- **Library source indicator** — tracks in the TUI library browser show a colored icon indicating whether they are local (green HDD) or remote (cyan cloud)

## 0.6.3

### Added

- **Sub-pixel scrollbar** — scrollbar thumb renders at 1/8th-cell resolution using Unicode block elements for smooth visual movement
- **Parallel disk scanning** — adding files from disk now uses rayon for parallel metadata reads, significantly faster for large collections

### Fixed

- **Scrollbar tracking with album headers** — scrollbar now accounts for album header lines in its position/size calculations, fixing drift when dragging and inability to scroll to the end
- **Mouse wheel scroll bounds** — wheel scrolling now correctly bounds against the display line count (including album headers) instead of just the entry count

## 0.6.2

### Added

- **WebP cover art support** — cover art in WebP format (embedded or external) is now decoded and displayed

## 0.6.1

### Added

- **Organize file path diff** — organize modal now shows a before/after visual diff of file paths, highlighting changed path segments in green
- **Ctrl-A select all** — select entire playlist from normal mode (enters edit mode) or edit mode
- **Album header context menu** — right-click on album headers to apply actions (organize, remove, favourite, etc.) to the whole album group

### Fixed

- **ALAC codec detection** — MP4 files containing ALAC audio are now correctly identified as ALAC instead of AAC, using lofty's `Mp4File` codec probe
- **Unicode string slicing panics** — fixed two panics in organize path diff caused by byte-slicing fullwidth/CJK characters; all path helpers now use char-based operations
- **Modal mode restoration** — context menu and organize modal now use a mode stack (push/pop) instead of hardcoding return to edit mode; closing a modal returns to whatever mode was active before opening it

### Tests

- **Unicode torture tests** — comprehensive coverage for fullwidth Japanese, CJK, emoji (ZWJ, flags, skin tones), Arabic bismillah, Zalgo/combining diacritics, and extreme combining mark sequences
- **ALAC codec tests** — fallback tests for `mp4_codec()` plus integration test against real ALAC files
- **Organize path diff tests** — coverage for `common_path_prefix`, `shared_prefix_len`, `truncate_path` helpers

## 0.6.0

Full codebase audit of v0.5.2 covering security, performance, architecture, dependencies, and test coverage. Every change was reviewed individually and as a combined integration.

### Fixed

- **Security hardening** — credentials removed from stored remote URLs (reconstructed from config at playback time), config and DB files restricted to 0o600 on Unix, FTS5 and LIKE query inputs sanitized, HTTPS warning for non-localhost remotes, secure random salt via `getrandom`, PID-namespaced cover art temp files
- **Streaming duration display** — seek bar metrics now use the DB-sourced track duration instead of the probed partial-file duration, so elapsed/total and click-to-seek are correct during streaming playback

### Performance

- **Render loop allocations eliminated** — playlist version gate skips redundant O(n) visible queue rebuild when queue is idle; borrowed string keys in display line builder remove 2 allocations per entry per call; spectrum data changed from heap Vec to stack arrays ([f32; 48]) eliminating allocation on every frame clone at 60fps

### Changed

- **Symphonia codec features scoped** — replaced blanket `features = ["all"]` with only the codecs koan actually uses (FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV, AIFF), reducing compile time
- **`row_to_track_row` helper** — deduplicated 4 identical 22-line row-mapping closures in tracks.rs into a single shared function
- **`plan_single_move` helper** — extracted shared move-planning logic (path formatting, sanitization, extension preservation, ancillary file handling) from two `plan_moves` variants in organize.rs
- **rusqlite removed from koan-music** — 3 raw SQL calls replaced with koan-core query functions (`album_date`, `clear_cached_paths`). Binary crate no longer links rusqlite directly
- **rusqlite features scoped** — `bundled-full` → `bundled`, removing unused extensions (load_extension, backup, blob, hooks, session)
- **Workspace dependencies** — added `[workspace.dependencies]` for rusqlite and walkdir, centralizing version management

### Removed

- **Dead code cleanup** — removed 6 unused functions/fields: `LyricsState::clear`, `CoverArt::centered`, `scrollbar_hover` theme field, `event.rs` module, `VisualizerState::num_bars`, 3 unused `HoverZone` variants

### Tests

- **Test coverage expanded** — 332 → 371 tests. Added coverage for PlaybackTimeline (6), SharedPlayerState (12), favourites (8), Subsonic client (5), metadata probe (5). Removed 4 AI-generated duplicate streaming tests

## 0.5.2

### Fixed

- **Drag reorder selects wrong track** — dragging a single track up/down no longer switches selection to the displaced track. The dragged track's ID is now captured before the move instead of reading from the stale visible queue cache

## 0.5.1

### Added

- **ReplayGain playback support** — track and album ReplayGain tags are read via lofty at decode time and gain is applied with peak limiting. Configure via `[playback] replaygain` (`track`, `album`, or `off`) and `pre_amp_db` in config.toml. Zero overhead when disabled
- **Streaming seek bar** — during streaming playback the seek bar dims the not-yet-downloaded portion. Downloaded section renders as a solid line that grows as the download progresses. Seeking past the downloaded point is prevented (click, keyboard, and core seek all clamped)
- **Accurate duration for streaming tracks** — transport bar now prefers the database-sourced track duration over the probed partial-file duration, so elapsed/total always shows the real track length

### Fixed

- **TIFF cover art rejected** — embedded TIFF artwork is now skipped during extraction, falling back to the next JPEG/PNG picture. Fixes `CGImageDestinationFinalize failed` errors on macOS Now Playing
- **Spectrum peak markers hidden by bars** — peak hold markers now render on top of bar fill instead of being overwritten

## 0.5.0

### Added

- **Spectrum analyser** — 80s hi-fi LED-segment style spectrum visualiser renders above the transport bar when album art is present. 48-band FFT with configurable frequency scale (Bark/Mel/Log/Linear), eighth-block sub-cell resolution, green/yellow/red gradient, peak hold markers, and time-based exponential decay
- **Dedicated analysis thread** — FFT runs on a background thread (`VizAnalyzer`) decoupled from both the decode and UI threads. The UI reads a pre-computed `VizSnapshot` every frame with sub-microsecond lock hold times, ensuring buttery-smooth 60fps rendering
- **VizBuffer audio tap** — circular sample buffer shared between decode thread and analysis thread via `parking_lot::Mutex`
- **FFT pipeline** — 2048-point real FFT via `realfft` crate. Hann window, dB magnitude scaling, Bark/Mel/Log/Linear frequency scales
- **A-weighted amplitude scaling** — bars reflect perceived loudness using IEC 61672 A-weighting, matching human hearing sensitivity (Fletcher-Munson curves). Configurable via `amplitude_scale`: `aweight` (default), `perceptual` (A-weight + gamma), `sqrt`, `linear`
- **Signal-level coloring** — spectrum bars are colored by amplitude, not position. Green at safe headroom, yellow when hot, red only near clipping (0dBFS)
- **Visualiser config** — `[visualizer]` section with `enabled`, `fps` (default: 60), `scale`, `amplitude_scale`, `bar_decay_ms` (default: 50), `peak_decay_ms` (default: 180). Also accepts `[visualiser]` spelling
- **Spectrum theme colours** — `spectrum_low` (green), `spectrum_mid` (yellow), `spectrum_high` (red), `spectrum_peak` (white) in theme config
- **FPS overlay** — `[playback] show_fps = true` displays an FPS counter in the top-right corner

## 0.4.0

### Added

- **Streaming playback for remote tracks** — playback starts after 256 KB is buffered instead of waiting for the full download. A `StreamingSource` backed by a shared in-memory buffer feeds Symphonia while the download continues in the background. When the download finishes, full lofty metadata and cover art are re-read and media key info (souvlaki) is updated progressively
- **Vim-style navigation everywhere** — pickers, library browser, and queue all support Ctrl+U/Ctrl+D (half-page), PageUp/PageDown, Home/End. Library also accepts j/k/h/l, g/G
- **Wrap-around cursor** — pressing Up on the first item wraps to the last, and Down on the last wraps to the first (queue, library, picker)
- **Lyrics panel** — press `L` to toggle a lyrics panel (60/40 split with queue). Fetches synced and plain lyrics from LRCLIB (zero-config, no API key). Synced lyrics highlight the current line and auto-scroll with playback
- **Lyrics DB caching** — fetched lyrics are cached in SQLite so subsequent views are instant
- **LRCLIB search fallback** — when exact match (`/api/get`) returns 404, falls back to fuzzy search (`/api/search`) by artist + title
- **Incremental remote sync** — `koan remote sync` now only fetches albums newer than the last sync timestamp, dramatically reducing sync time. Use `--full` to force a complete re-sync
- **Resilient stale track removal** — when local files are removed, remote-backed tracks are demoted to remote-only (preserving streaming fallback) instead of being deleted entirely

### Changed

- **Fixed-timestep render loop** — replaced tick-on-timeout event loop with a game-engine-style frame-deadline loop. Animations (ticker, spinner) no longer stall during mouse interaction or key holds
- **Configurable frame rate** — `[playback] target_fps` (default: 60) controls TUI redraw rate. Accepts 30, 60, or 120
- **Transport icons** — play/pause/stop status icons use Unicode symbols instead of ASCII

### Fixed

- **Standalone picker mouse support** — `koan pick --artist`/`--album` now enables mouse capture. Click to select, double-click to confirm, scroll wheel to navigate
- **Lyrics fetch on toggle** — pressing `L` mid-track now fetches lyrics immediately. Previously, lyrics only loaded on track change
- **Lyrics error logging** — fetch errors are now logged to stderr instead of being silently swallowed
- **Favourites import for remote tracks** — starred tracks from Navidrome now correctly import as local favourites. Previously, remote-only tracks (with no local path) were silently skipped during import
- **Favourites sync error logging** — errors from `getStarred2` and `import_remote_favourites` are now surfaced instead of silently returning 0
- **Event drain starvation** — opening album art (or any slow render) no longer freezes the UI. The event loop now always polls for input even when behind on frame budget
- **Cover art zoom performance** — full-screen album art view no longer runs Lanczos3 resize every frame. Rendered output is cached and reused until terminal size changes
- **Ticker double-speed after merge** — duplicate ticker animation block from merge caused scrolling text to advance twice per frame
- **Anchored drag reorder** — dragging selected tracks now moves them anchored to the mousedown position instead of snapping to the top of the selection
- **Album header drag** — clicking and dragging an album header reorders the entire album group as a unit
- **Play/pause click** — clicking the status icon (play/pause indicator) next to the seek bar now toggles playback
- **Download progress on all tracks** — tracks before the playing position now correctly show download progress and status instead of being unconditionally marked as played

### Removed

- **Event::Tick** — tick variant removed from event enum. Ticking is now unconditional every frame

## 0.3.0

### Added

- **Ticker-style transport bar** — when the artist/title text overflows the available width, it scrolls horizontally like a ticker banner. Album, year, and codec info stay fixed. Scroll speed is configurable via `playback.ticker_fps` in config (default: 8)
- **Favourites** — press `f` to favourite/unfavourite tracks. A yellow star (★) appears in the queue gutter. Persisted to SQLite. Available in the context menu too
- **Favourite sync** — favouriting a remote track stars it on Navidrome. `koan remote sync` now pushes local favourites and pulls remote starred songs
- **Subsonic star/unstar/getStarred2 API** — new SubsonicClient methods for managing server-side favourites
- **Rich context menu** — right-click (or `Space` in edit mode) opens a positioned context menu with Play, Favourite, Track info, Remove, and Organize actions. Hotkey shortcuts work within the menu
- **Mouse hover highlighting** — queue and library items show underline on hover
- **Event drain loop** — mouse move events are coalesced so the UI always renders the latest cursor position
- **HoverZone tracking** — typed enum tracks which UI element (queue item, library item, seek bar, etc.) is under the mouse

### Changed

- **Scroll step reduced** — mouse scroll wheel moves 1 line instead of 3
- **Queue jump scroll** — `/` search now scrolls the matched track to near the top of the visible area (with album header) instead of keeping current scroll position

### Fixed

- **Scrollbar drag jump** — clicking the scrollbar thumb no longer jumps to a wrong position. The grab offset within the thumb is tracked so dragging feels natural. Clicking the track area still jumps as expected
- **Multi-select drag reorder** — dragging multiple selected tracks no longer causes chaotic oscillation. Moves only trigger when the target is outside the current selection range
- **Drag undo batching** — one drag operation (single or multi-track) is now a single undo step instead of one per row crossed

## 0.2.3

### Added

- **Cover art in Now Playing** — macOS Control Center shows embedded album art (extracted to temp file, passed as file:// URL to souvlaki)
- **Seek from Control Center** — absolute position, relative with duration, and direction-only (10s steps)
- **Quit from Control Center** — clean shutdown via atomic flag on SharedPlayerState

### Fixed

- **mise binary name** — release tarballs now contain `koan` instead of `koan-macos-arm64`, fixing mise installs

### Removed

- **Dead file watcher** — notify/FSEvents module was implemented but never wired in. Removed watcher.rs, notify deps, `config.watch` field

## 0.2.0

First public release. Full TUI rewrite, undo/redo, file organization, CI/CD pipeline.

### Added

- **Ratatui TUI** — full-screen terminal UI with transport bar (click-to-seek), album-grouped queue, fuzzy picker overlay, library browser, track info modal with embedded album art (halfblock rendering), scrollbar, mouse support throughout
- **Undo/redo** — 100-deep undo stack covering all playlist operations (add, remove, move, clear). `Ctrl+Z` to undo, `Ctrl+Y` or `Ctrl+Shift+Z` to redo. Batch operations (multi-delete, multi-move) undo as a single step
- **File organization** — in-TUI organize modal: select tracks in edit mode → `Space` → Organize → pick a named pattern → preview → execute. Playlist paths update live, playback continues uninterrupted. Ancillary files move with the music
- **Format string engine** — fb2k-compatible title formatting: `%field%` references, `[conditional]` blocks, `$function(args)` calls. 30+ built-in functions (string, logic, numeric, path). 234 tests
- **Named organize patterns** — define reusable patterns in config (`[organize.patterns]`), set a default, pick from them in the TUI modal
- **Context menu** — `Space` in edit mode opens action overlay (currently: Organize)
- **Drag/drop** — drag files or folders from Finder into the terminal to add to the queue (bracketed paste)
- **Queue editing** — edit mode (`e`) with Finder-style multi-selection (shift-arrows, option-click toggle, ctrl-click range), reorder (`j`/`k`), delete (`d`), multi-drag
- **Library browser** — split-pane tree view (artists → albums → tracks), substring filter (`f`/`/`), click/double-click support
- **Picker actions** — `Enter` appends, `Ctrl+Enter` appends and plays, `Ctrl+R` replaces entire queue
- **Mouse support** — double-click to play, click-to-seek, drag-to-reorder, scrollbar drag, scroll wheel, picker click/dismiss — works in every mode
- **Priority play** — double-click a downloading track to play it as soon as it finishes
- **Media keys** — macOS Control Center integration via souvlaki (play/pause, next/prev, now playing info)
- **Track info modal** — `i` shows full metadata + audio format details + embedded album art
- **Cover art zoom** — `z` for full-screen album art (halfblock rendering)
- **Dynamic shell completions** — `source <(COMPLETE=zsh koan)` for artist/album ID tab-completion from the DB
- **Parallel remote sync** — album detail fetches parallelized with rayon, batch DB writes per page
- **`koan init`** — scaffolds `~/.config/koan/` with config templates (organize patterns, playback defaults, library paths), database, cache dir, and `.gitignore` for dotfile repos
- **`koan pick`** — in-process fuzzy picker powered by nucleo (replaces fzf dependency). `--album`/`--artist` modes with drill-down
- **CI/CD pipeline** — test + clippy + fmt check, cross-compiled binaries (arm64 + x86_64), GitHub releases with auto-tagging, crates.io publishing (`koan-core` then `koan-music`)
- **MIT LICENSE** file

### Fixed

- **Album picker adds wrong tracks** — was passing album IDs as track IDs, now correctly expands via DB query
- **Track artist vs album artist** — stored separately in DB, compilations display correctly
- **Seek past end of track** — skips to next instead of crashing
- **Scroll past end** — queue scroll clamps correctly
- **Scroll in modals** — routes to active modal instead of always scrolling queue
- **Library shows album artists only** — no spurious entries from featured artists on compilations
- **Crash on pick subcommand** — fixed usize underflow race with `saturating_sub`, added panic hook for terminal restore
- **Queue metadata for local tracks** — was blank, now populated correctly
- **Album header dimming** — only dims when ALL tracks in group are played

### Changed

- **Crate renamed** — `koan-cli` → `koan-music` (binary stays `koan`), directory `crates/koan-music/`
- **Config path** — `~/.config/koan/` (was `~/Library/Application Support/koan/`)
- **Two-layer config** — `config.toml` (committable) + `config.local.toml` (gitignored)
- **Password storage** — stored in `config.local.toml` via `koan remote login`, not macOS Keychain

### Removed

- **`koan organize` CLI subcommand** — file organization is now TUI-only (context menu → organize modal)
- **FFI/Swift layer** — removed entirely, pure Rust
- **fzf dependency** — replaced with built-in nucleo fuzzy picker

## 0.1.0

Initial release.

- Bit-perfect CoreAudio playback (AUHAL, automatic sample rate switching)
- Gapless transitions
- Format support: FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (Symphonia)
- Library indexing with rayon, SQLite FTS5 search
- Subsonic/Navidrome remote sync
- Track deduplication (path → remote_id → content match)
- CLI: play, scan, search, library, config, probe, devices, remote login/sync/status
