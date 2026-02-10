# Changelog

## Unreleased

### Added

- **Queue display** — full-screen playback UI with album-grouped headers, rich metadata (track number, artist, title, album, year, codec, duration), animated braille spinners for downloads, pending queue shown before downloads complete
- **Queue editing** — press `e` during playback to enter edit mode: navigate with arrows, `d` to delete, `j`/`k` to reorder
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
