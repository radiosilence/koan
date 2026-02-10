# kōan

A music player for people who give a shit about audio quality.

macOS-native (SwiftUI shell, Rust core). Bit-perfect playback, gapless transitions, fast library indexing, Subsonic/Navidrome integration. No Electron. No subscriptions. No bullshit.

## What works

- **Bit-perfect playback** — CoreAudio AUHAL, no resampling, automatic device sample rate switching
- **Gapless** — decode thread keeps the ring buffer alive across track boundaries, AudioUnit never stops
- **Format support** — FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (via Symphonia)
- **Library indexing** — parallel metadata scanning with rayon, SQLite FTS5 full-text search
- **File watching** — FSEvents via notify, debounced 500ms, auto-updates DB on changes
- **Subsonic/Navidrome** — parallel remote library sync, unified local+remote browsing, lazy parallel downloads
- **CLI** — colourised output with tree-structured display, dynamic shell completions from library DB, built-in fuzzy picker (nucleo)
- **Queue management** — grouped album headers, edit mode with reorder/delete, animated download spinners, pending queue shown before downloads complete
- **Track deduplication** — local+remote tracks merged into single rows, local path always wins for playback
- **Proper artist handling** — track artist stored separately from album artist; compilations/VA albums display correctly

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
cargo build --release
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
# initialise config directory with defaults
koan init

# scan library (path or configured folders)
koan scan /path/to/music
koan scan

# play files
koan play ~/Music/album/*.flac

# play by track/album/artist ID
koan play --id 42 43 44
koan play --album 5
koan play --artist 3

# interactive fuzzy picker
koan pick               # search all tracks
koan pick --album       # browse albums
koan pick --artist      # browse artists → drill into albums
koan pick "aphex"       # pre-filter

# browse library
koan search "radiohead"
koan artists
koan artists "aphex"
koan albums
koan albums "boards"
koan library

# remote server
koan remote login https://music.example.com admin
koan remote sync
koan remote status

# cache management
koan cache status
koan cache clear

# utilities
koan config
koan devices
koan probe track.flac
```

### Playback controls

During playback the queue is displayed with album-grouped headers, track metadata, and download status (animated spinners for in-progress downloads). Press `e` to enter edit mode.

| Key     | Action                 |
| ------- | ---------------------- |
| `space` | pause / resume         |
| `< >`   | previous / next track  |
| `, .`   | seek ±10s              |
| `←` `→` | seek ±10s              |
| `p`     | pick tracks to enqueue |
| `a`     | pick album to enqueue  |
| `r`     | pick artist to enqueue |
| `e`     | edit queue             |
| `q`     | quit                   |

**Queue edit mode** (`e`):

| Key       | Action             |
| --------- | ------------------ |
| `↑` `↓`   | navigate           |
| `d`       | remove track       |
| `j` / `k` | move track down/up |
| `Esc`     | exit edit mode     |

### Queue display

Tracks are grouped by album with headers showing album artist, year, album title, and codec. Track artist is shown inline only when it differs from the album artist (compilations, VA albums). Downloads show animated braille spinners.

```
 Limewax — (2007) Therapy Session 4 [FLAC]
   > 01 Agent Orange                              4:56
     02 Pigeons and Marshmellows feat. The Panacea 2:53
     03 SPL — Fade                                 1:52
     04 Icicle                                     2:27
```

### Remote (Subsonic/Navidrome)

```bash
koan remote login https://music.example.com admin
koan remote sync
koan remote status
```

Remote and local tracks appear in the same library. Local files take playback priority. Remote tracks are downloaded to a structured cache (`Album Artist/(Year) Album [Codec]/01. Artist - Title.ext`) on first play, then cached for subsequent plays.

## Configuration

Two-layer config — `config.toml` for defaults you can commit to dotfiles, `config.local.toml` for machine-specific overrides (gitignored).

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
password = ""
```

Local values override base. `koan config` shows both sources and the resolved result. Database, cache, and log live at `~/.config/koan/`.

## Dev

```bash
just check    # test + clippy
just fmt      # cargo fmt
just cli      # cargo run -p koan-cli -- <args>
just build    # full build (rust + bindings + xcframework)
```

## License

MIT
