# kōan

A music player for people who give a shit about audio quality.

macOS-native (SwiftUI shell, Rust core). Bit-perfect playback, gapless transitions, fast library indexing, Subsonic/Navidrome integration. No Electron. No subscriptions. No bullshit.

## What works

- **Bit-perfect playback** — CoreAudio AUHAL, no resampling, automatic device sample rate switching
- **Gapless** — decode thread keeps the ring buffer alive across track boundaries, AudioUnit never stops
- **Format support** — FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (via Symphonia)
- **Library indexing** — parallel metadata scanning with rayon, SQLite FTS5 full-text search
- **File watching** — FSEvents via notify, debounced 500ms, auto-updates DB on changes
- **Subsonic/Navidrome** — parallel remote library sync (rayon), unified local+remote browsing, lazy parallel downloads
- **CLI** — colourised output with tree-structured display, dynamic shell completions from library DB, built-in fuzzy picker (nucleo)
- **Track deduplication** — local+remote tracks merged into single rows, local path always wins for playback

## Architecture

```
PCM never crosses FFI.

File → Symphonia → f32 samples → rtrb ring buffer → CoreAudio render callback → DAC

All in Rust. Lock-free audio thread.
```

Three crates:

- `koan-core` — audio engine, player, database, indexer, remote client
- `koan-ffi` — UniFFI (control plane) + C FFI via cbindgen (audio data plane)
- `koan-cli` — `koan` binary

## Install

```bash
# build
cargo build --release

# or install to PATH
cargo install --path crates/koan-cli
```

Requires macOS (CoreAudio).

## Shell completions

Dynamic completions that know your library — artist/album IDs tab-complete from the DB.

```bash
# zsh (add to .zshrc)
source <(COMPLETE=zsh koan)

# bash
source <(COMPLETE=bash koan)

# fish
COMPLETE=fish koan | source
```

Then `koan play --album <TAB>` shows your actual albums with artist names.

## Usage

```bash
# play files (gapless across tracks)
koan play ~/Music/album/*.flac

# play by track/album/artist ID (from search/browse results)
koan play --id 42 43 44
koan play --album 5
koan play --artist 3

# browse library
koan artists              # list all artists with IDs
koan artists "aphex"      # filter artists
koan albums               # list all albums grouped by artist
koan albums "boards"      # albums for matching artists

# scan library (path or configured folders)
koan scan /path/to/music
koan scan              # uses folders from config

# search (FTS5, prefix matching, tree-grouped output)
koan search "radiohead"

# interactive fuzzy picker (built-in, no external deps)
koan pick               # search all tracks
koan pick --album       # browse albums
koan pick --artist      # browse artists → drill into albums
koan pick "aphex"       # pre-filter

# library stats
koan library

# cache management
koan cache status       # show cache size + location
koan cache clear        # nuke all cached downloads

# show config sources + resolved values
koan config

# list audio devices
koan devices

# probe file format
koan probe track.flac
```

### Playback controls

During playback, the full queue is visible with download status. Press `e` to enter edit mode for reordering/removing tracks.

| Key     | Action                 |
| ------- | ---------------------- |
| `space` | pause / resume         |
| `< >`   | previous / next track  |
| `, .`   | seek ±10s              |
| `p`     | pick tracks to enqueue |
| `a`     | pick album to enqueue  |
| `r`     | pick artist to enqueue |
| `e`     | edit queue             |
| `n`     | next track             |
| arrows  | seek ±10s              |
| `q`     | quit                   |

**Queue edit mode** (`e`):

| Key       | Action             |
| --------- | ------------------ |
| `up/down` | navigate           |
| `d`       | remove track       |
| `J` / `K` | move track down/up |
| `Esc`     | exit edit mode     |

### Remote (Subsonic/Navidrome)

```bash
# authenticate (password saved to config.local.toml)
koan remote login https://music.example.com admin

# sync remote library into local DB
koan remote sync

# check connection
koan remote status
```

Remote and local tracks appear in the same library. Local files take playback priority when the same track exists in both.

## Configuration

Config uses a two-layer system — `config.toml` for defaults you can commit to dotfiles, `config.local.toml` for machine-specific overrides (gitignored).

`~/.config/koan/config.toml`

```toml
[library]
watch = true

[playback]
exclusive_mode = false
software_volume = false
replaygain = "album"  # off | track | album
```

`~/.config/koan/config.local.toml` (gitignored)

```toml
[library]
folders = ["/Volumes/Turtlehead/music"]

[remote]
enabled = true
url = "https://music.example.com"
username = "admin"
transcode_quality = "original"  # original | opus-128 | mp3-320
```

Local values override base values. `koan config` shows both sources and the resolved result.

Database and cache live alongside the config at `~/.config/koan/`.

## Dev

```bash
just check    # test + clippy
just fmt      # cargo fmt
just build    # full build (rust + bindings + xcframework)
just cli      # cargo run -p koan-cli
```

## License

MIT
