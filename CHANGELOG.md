# Changelog

## Unreleased

### Added

- **Colourised CLI** — full owo-colors integration, meaningful colour coding across all commands (artists cyan, albums green, IDs dimmed, codecs yellow, errors red, etc.)
- **Tree-structured output** — search results, albums, and errors displayed with `├──`/`└──` tree glyphs for visual hierarchy
- **Dynamic shell completions** — `source <(COMPLETE=zsh koan)` enables tab-completion of artist/album IDs from the library DB (clap_complete CompleteEnv)
- **Structured cache paths** — remote downloads cached as `Album Artist/(Year) Album [Codec]/01. Artist - Title.flac` instead of `track_40866.flac`
- **Play by track ID** — `koan play --id 42 43 44` resolves tracks from DB, downloads remote tracks to cache for playback
- **Browse commands** — `koan artists [query]`, `koan albums [query]` with IDs for playback
- **Play by album/artist** — `koan play --album 5`, `koan play --artist 3`
- **Parallel remote sync** — album detail fetches parallelized with rayon, batch DB writes per page
- **Config local overlay** — `config.local.toml` for machine-specific overrides (gitignored), base `config.toml` committable to dotfiles
- **`koan config`** — shows source files (config.toml, config.local.toml) and the resolved merged config
- **`koan pick`** — interactive fzf-powered library picker: fuzzy-find tracks, albums, or artists and play immediately. `--album`/`--artist` modes with drill-down flows
- **`koan cache status/clear`** — view cache size + file count, nuke all cached downloads (clears DB cached_path too)
- **MultiProgress playback UI** — parallel download spinners render cleanly alongside the playback progress bar, track changes don't stomp the display
- **Lazy parallel downloads** — first track plays immediately, remaining tracks download in parallel via rayon and enqueue as they complete
- **Password in config** — Navidrome password stored in `config.local.toml` instead of Keychain, with Keychain fallback for backwards compat
- **26 unit tests** — config, DB (CRUD, FTS5 search, dedup, playback resolution, scan cache, stats), metadata
- **Ctrl+C handling** — SIGINT resets to default so blocking operations die immediately
- **Track deduplication** — local+remote tracks merged into single rows via 3-level matching (path, remote_id, content match). Local path always wins for playback.

### Fixed

- **Seek past end of track** — skips to next track instead of crashing Symphonia with "seek timestamp out-of-range"
- **FTS5 deletes** — switched from contentless to content-managed FTS5 (contentless can't DELETE)
- **Search ordering** — results grouped by artist/album/disc/track instead of FTS rank
- **Cached path persisted** — `cached_path` column updated after download so subsequent plays skip re-download

### Changed

- Config/data paths: `~/.config/koan/` (was `~/Library/Application Support/koan/`)
- DB lives at `~/.config/koan/koan.db` (next to config)
- Search results show disc/track numbers, grouped by artist/album with album IDs
- Progress bar shows track name from file stem (works with structured cache names)

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
- **Justfile** — build, check, fmt, cli recipes
