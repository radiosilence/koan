# Changelog

## Unreleased

### Added

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
