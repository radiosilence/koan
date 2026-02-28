# Changelog

## Unreleased

### Added

- **Ratatui TUI** — full-screen terminal UI replaces custom ANSI rendering. Transport bar with click-to-seek gauge, album-grouped queue view, fuzzy picker as centered overlay, context-sensitive key hints, mouse support (click, drag reorder, scroll wheel). Uses crossterm alternate screen with panic-safe terminal restore.
- **Format string engine** — fb2k-compatible title formatting: `%field%` references, `[conditional]` blocks, `$function(args)` calls. 18 built-in functions (string, logic, numeric, path). Drives library views and file organization.
- **File organization** — `koan organize --pattern '...'` renames/moves library files using format strings. Dry-run preview by default, `--execute` to apply, `--undo` to revert. Moves ancillary files (cover.jpg, .cue, .log), cleans empty dirs. Move log stored in SQLite for undo.
- **Mouse support** — mouse editing (select, drag-reorder, shift-click range, option-click toggle, multi-drag) works in any mode — modality is keyboard-only. Double-click any track to play it (forward or backward). Click seek bar to jump, scroll wheel in queue. Click/double-click items in fuzzy picker. Click outside picker to dismiss.
- **Queue display** — full-screen playback UI with album-grouped headers, rich metadata (track number, artist, title, album, year, codec, duration), animated braille spinners for downloads, pending queue shown before downloads complete
- **Queue editing** — press `e` during playback to enter edit mode: navigate with arrows, `d` to delete, `j`/`k` to reorder. Finder-style multi-selection with shift+arrows, visual markers for cursor/selected/drag target
- **Playlist-style queue** — played tracks stay visible (dimmed) as the playing indicator moves down, like foobar2000. Click any track to skip to it — forward or backward. Played track history tracked server-side (in the player thread), so gapless transitions, skip-to, and prev-track all stay in sync.
- **Library browser** — press `l` during playback to open a split-pane tree view: artists → albums → tracks. Tab switches focus between library and queue. `a` enqueues all tracks under cursor. Press `f` or `/` to filter artists by name — type to narrow results, Enter to accept, Esc to clear. Launch directly with `koan play --library` or just `koan` (no subcommand).
- **Track info modal** — press `i` on any track to see all metadata (title, artist, album, year, track/disc number, codec, sample rate, bit depth, channels, duration, file path, status). Shows audio format details for the currently playing track.
- **Priority play for pending tracks** — double-click a downloading track to play it as soon as its download finishes, interrupting the current track without clearing the queue. Priority track shows `>` with spinner/progress in the playing color.
- **Download progress** — actively downloading tracks show percentage (`42%`) or bytes (`123K` if no Content-Length). Waiting tracks show braille spinner. Priority tracks show `>` prefix with progress.
- **Bare `koan` invocation** — running `koan` with no subcommand opens the TUI with the library browser.
- **Media keys** — macOS Control Center integration via souvlaki. Play/pause, next/prev, now playing info (title, artist, album, duration) pushed to the OS media widget.
- **Inline picker** — press `p`/`a`/`r` during playback to fuzzy-pick tracks/albums/artists and append to queue without interrupting playback
- **Parallel batched downloads** — first track plays immediately, remaining download in batches of 4 using `std::thread::scope`, queue order preserved
- **Built-in fuzzy picker** — nucleo-powered in-process picker replaces fzf dependency. `koan pick`, `--album`/`--artist` modes with drill-down
- **Colourised CLI** — owo-colors integration across all commands (artists cyan, albums green, IDs dimmed, tree glyphs)
- **Tree-structured output** — search results, albums, and errors displayed with `├──`/`└──` hierarchy
- **Dynamic shell completions** — `source <(COMPLETE=zsh koan)` enables tab-completion of artist/album IDs from the library DB
- **Structured cache paths** — remote downloads cached as `Album Artist/(Year) Album [Codec]/01. Artist - Title.flac`
- **Play by track ID** — `koan play --id 42 43 44` resolves tracks from DB, downloads remote tracks for playback
- **Play by album/artist** — `koan play --album 5`, `koan play --artist 3`
- **Browse commands** — `koan artists [query]`, `koan albums [query]` with IDs for playback
- **Previous track** — `<` goes back through play history
- **Parallel remote sync** — album detail fetches parallelized with rayon, batch DB writes
- **Config local overlay** — `config.local.toml` for machine-specific overrides, base `config.toml` committable to dotfiles
- **`koan init`** — scaffolds `~/.config/koan/` with default configs, database, and cache directory
- **`koan cache status/clear`** — view cache size, nuke all cached downloads
- **File logging** — all log output written to `~/.config/koan/koan.log` with timestamps
- **Password in config** — stored in `config.local.toml` instead of Keychain, with Keychain fallback
- **26 unit tests** — config, DB (CRUD, FTS5 search, dedup, playback resolution, scan cache, stats), metadata

### Fixed

- **Album picker adds wrong tracks** — album picker was passing album IDs directly to enqueue, which treated them as track IDs. Now correctly expands album IDs to track IDs via DB query.
- **Picker highlighting** — fuzzy picker cursor now uses reversed style for a visible highlight bar instead of subtle text color.
- **Mouse in normal mode** — clicking on any queue track during playback skips to it (forward or backward). Previously mouse clicks only worked in edit mode.
- **Played track history** — server-side `finished_paths` in `SharedPlayerState` replaces broken UI-side tracking. Gapless auto-advance, skip-to, and prev-track all correctly maintain the played history. Queue snapshot rebuilt on gapless transition so the display never goes stale.
- **Pending/downloading track editing** — loading tracks can now be deleted, reordered, and selected like normal queued tracks. QueueSegment categorisation maps visible indices to the correct backing store (queued vs pending).
- **Delete playing track** — pressing `d` on the currently playing track skips to the next track.
- **Scroll past end** — queue scroll now clamps to prevent content from disappearing.
- **Scroll in modals** — scroll wheel events route to the active modal (picker or library) instead of always scrolling the queue.
- **Picker mouse support** — click to select items, double-click to confirm. Click outside picker to dismiss.
- **Track change flicker** — `start_playback` now sets `track_info` immediately after probing (before starting the decode thread) and `stop_engine` no longer clears display state. `on_track_change` callback only pushes to `finished_paths` when the track actually changes (path comparison). `visible_queue()` defensively filters the current track from finished to handle any remaining race. Prev-track and skip-back do a single `rebuild_queue_snapshot` after all mutations (removed intermediate rebuilds that caused transient shorter list for one frame). Skip-to and next-track commit finished paths and rebuild the queue snapshot BEFORE popping the target track. Playback generation counter (`AtomicU64`) prevents stale decode thread callbacks from corrupting state during rapid track changes. Auto-scroll derives playing track from the atomic visible queue snapshot instead of `track_info` directly, eliminating TOCTOU jank.
- **Library shows album artists only** — library tree now shows only artists that own at least one album (album artists), not track-only featured artists. Compilations with different per-track artists no longer create spurious top-level entries.
- **Removed auto-quit** — app no longer exits when the queue is empty. Stays open for library browsing and further interaction.
- **Crash on pick subcommand** — `koan pick --artist` could SIGTRAP when `segment_for_index` hit usize underflow from a TOCTOU race between independent RwLock reads. Fixed with `saturating_sub`. Added panic hook to `cmd_pick` so terminal is restored on crash instead of dumping raw escape codes.
- **Library cursor highlight** — library browser cursor now fills the full row width (matching picker overlay style) instead of only highlighting the text portion.
- **Library mouse support** — click items to select, double-click to expand/enter/enqueue. Click queue pane to switch focus.
- **Loading overlay** — "loading..." overlay shown while album IDs are resolved and pending queue is built. Album expansion moved off the main thread to prevent UI blocking.
- **Album header dimming** — album headers are now dimmed only when ALL tracks in the group are played, not just the first track. Prevents headers for currently-playing albums from appearing dimmed.
- **Artist "all tracks" in picker** — selecting "all tracks" from the artist drill-down album picker now correctly enqueues all tracks for that artist. Previously the -1 sentinel was silently dropped.
- **Track artist vs album artist** — track artist now stored separately from album artist in DB. Compilations/VA albums display per-track artists correctly. Album grouping uses album artist. FTS indexes both. Requires DB nuke + re-scan.
- **Queue metadata for local/cached tracks** — metadata wasn't registered for local and cached playback sources, causing blank queue display with no colours or headers
- **Seek past end of track** — skips to next track instead of crashing Symphonia
- **Exit on prev-track** — transient stop state during prev-track no longer triggers premature exit
- **FTS5 deletes** — switched from contentless to content-managed FTS5
- **Search ordering** — results grouped by artist/album/disc/track instead of FTS rank
- **Cached path persisted** — `cached_path` column updated after download

### Changed

- Config/data paths: `~/.config/koan/` (was `~/Library/Application Support/koan/`)
- Queue display replaces indicatif progress bars — custom ANSI renderer with save/restore cursor
- Downloads use `std::thread::scope` batches instead of rayon (ordering control)
- Search results show disc/track numbers, grouped by artist/album with album IDs

## 0.1.0

### Added

- **Bit-perfect playback** — CoreAudio AUHAL, automatic device sample rate switching, no resampling
- **Gapless transitions** — decode thread keeps ring buffer alive across track boundaries, Symphonia `enable_gapless`
- **Format support** — FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF via Symphonia
- **Library indexing** — parallel metadata scanning (rayon + walkdir), lofty tag reading, SQLite FTS5 search
- **File watching** — FSEvents via notify-debouncer-full, 500ms debounce, auto DB updates
- **Subsonic/Navidrome** — remote library sync, unified local+remote schema, Keychain credential storage, rustls TLS
- **Unified library** — every track has source (local/remote/cached), `resolve_playback_path` prefers local > cached > remote
- **CLI commands** — play, scan, search, library, config, probe, devices, remote login/sync/status, completions
- **Config** — TOML at `~/.config/koan/config.toml`, library folders, playback settings, remote server config
- **Track deduplication** — local+remote tracks merged via 3-level matching (path, remote_id, content match)
