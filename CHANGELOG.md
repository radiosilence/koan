# Changelog

## Unreleased

### Added

- **Play by track ID** — `koan play --id 42 43 44` resolves tracks from DB, downloads remote tracks to cache for playback
- **Parallel remote sync** — album detail fetches parallelized with rayon, batch DB writes per page
- **Config local overlay** — `config.local.toml` for machine-specific overrides (gitignored), base `config.toml` committable to dotfiles
- **`koan config`** — shows source files (config.toml, config.local.toml) and the resolved merged config
- **24 unit tests** — config (load, merge, overlay), DB (CRUD, FTS5 search, upsert, playback resolution, scan cache, stats), metadata (audio detection, codec mapping)
- **Ctrl+C handling** — SIGINT resets to default so blocking operations die immediately
- **FTS5 fix** — switched from contentless to content-managed FTS5 table (deletes actually work now)

### Changed

- Config/data paths: `~/.config/koan/` (was `~/Library/Application Support/koan/`)
- DB lives at `~/.config/koan/koan.db` (next to config)
- Search results now show track IDs: `[42] Artist - Album - Title`

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
