# kōan

A music player for people who give a shit about audio quality.

Pure Rust, Ratatui TUI. Bit-perfect playback, gapless transitions, fast library indexing, Subsonic/Navidrome integration, fb2k-style format strings. No Electron. No subscriptions. No bullshit.

## What works

- **Bit-perfect playback** — CoreAudio AUHAL, no resampling, automatic device sample rate switching
- **Gapless** — decode thread keeps the ring buffer alive across track boundaries, AudioUnit never stops
- **Format support** — FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (via Symphonia)
- **Ratatui TUI** — full-screen terminal UI with transport bar, album-grouped queue, fuzzy picker overlay, library browser, track info modal with embedded album art (halfblock rendering), mouse support (click-to-seek, click-to-play, drag-to-reorder, scroll wheel)
- **Media keys** — macOS Control Center integration via souvlaki (play/pause, next/prev, now playing info)
- **Library indexing** — parallel metadata scanning with rayon, SQLite FTS5 full-text search
- **File watching** — FSEvents via notify, debounced 500ms, auto-updates DB on changes
- **Subsonic/Navidrome** — parallel remote library sync, unified local+remote browsing, lazy parallel downloads
- **Format string engine** — fb2k-compatible `%field%`, `[conditionals]`, `$functions()` for library views and file organization
- **File organization** — `koan organize` renames/moves files using format strings, with dry-run preview and undo
- **Queue management** — playlist-style display (played tracks stay visible dimmed), album-grouped headers, edit mode with Finder-style multi-selection (shift/option-click, shift-arrows), reorder/delete, multi-drag. Mouse editing (select, drag-reorder) works in any mode; double-click to skip to any track (forward or backward)
- **Track deduplication** — local+remote tracks merged into single rows, local path always wins for playback
- **Proper artist handling** — track artist stored separately from album artist; compilations/VA albums display correctly

## Architecture

```
Pure Rust, top to bottom.

File → Symphonia → f32 samples → rtrb ring buffer → CoreAudio render callback → DAC

Lock-free audio thread. No FFI boundaries.
```

Two crates:

- `koan-core` — audio engine, player, database, indexer, format strings, file organization, remote client
- `koan-cli` — `koan` binary (Ratatui TUI)

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

# open TUI with library browser
koan                    # bare invocation opens library
koan play --library     # explicit flag also works

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

# organize files using format strings
koan organize --pattern '%album artist%/(%date%) %album%/%tracknumber%. %title%'
koan organize --pattern '...' --execute   # actually move (default is dry-run)
koan organize --undo                      # revert last organize

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

### Playback TUI

During playback, a full-screen Ratatui TUI shows the transport bar, queue, and key hints. The queue never goes blank during downloads — pending tracks appear immediately with animated spinners.

| Key     | Action                 |
| ------- | ---------------------- |
| `space` | pause / resume         |
| `< >`   | previous / next track  |
| `, .`   | seek ±10s              |
| `←` `→` | seek ±10s              |
| `p`     | pick tracks to enqueue |
| `a`     | pick album to enqueue  |
| `r`     | pick artist to enqueue |
| `i`     | track info             |
| `z`     | zoom album art         |
| `l`     | library browser        |
| `f`/`/` | filter library (in library mode) |
| `e`     | edit queue             |
| `q`     | quit                   |

**Mouse** (works in any mode — modality is keyboard-only): double-click a queue track to skip to it (forward or backward); double-click a downloading track to prioritize and play it as soon as it finishes. Click the seek bar to jump, scroll wheel in queue. Single-click selects, drag to reorder. Shift-click for range selection, Option-click to toggle individual tracks, drag selected group to reorder. In the fuzzy picker, click items to select, double-click to confirm, click outside to dismiss. In the library browser, click to select, double-click to expand/enter/enqueue; click queue pane to switch focus.

**Queue edit mode** (`e`):

| Key           | Action                   |
| ------------- | ------------------------ |
| `↑` `↓`       | navigate                 |
| `Shift+↑` `↓` | extend selection         |
| `d`           | remove selected track(s) |
| `j` / `k`     | move selected down/up    |
| `⌥-click`     | toggle select            |
| `Shift-click`  | range select             |
| `Esc`         | exit edit mode           |

### Queue display

Tracks are grouped by album with headers showing album artist, year, album title, and codec. Track artist is shown inline only when it differs from the album artist (compilations, VA albums). Downloading tracks show progress percentage, waiting tracks show braille spinners. Double-clicked priority tracks show `>` with progress.

```
 Limewax — (2007) Therapy Session 4 [FLAC]
   > 01 Agent Orange                              4:56
     02 Pigeons and Marshmellows feat. The Panacea 2:53
     03 SPL — Fade                                 1:52
     04 Icicle                                     2:27
```

### File organization

Rename and reorganize your music library using fb2k-compatible format strings. Default is dry-run (preview), add `--execute` to apply. Undo with `--undo`.

```bash
# preview
koan organize --pattern '%album artist%/(%date%) %album%/%tracknumber%. %title%'

# apply
koan organize --pattern '...' --execute

# revert
koan organize --undo
```

Ancillary files (cover.jpg, .cue, .log) move with the music. Empty directories are cleaned up.

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
```

## License

MIT
