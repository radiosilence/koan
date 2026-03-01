# kōan

A music player for people who give a shit about audio quality.

Pure Rust, Ratatui TUI. Bit-perfect playback, gapless transitions, fast library indexing, Subsonic/Navidrome integration, fb2k-style format strings. No Electron. No subscriptions. No bullshit.

## Getting started

### Install

```bash
# pre-built binary via mise (recommended)
mise use -g github:radiosilence/koan@latest

# or via cargo
cargo install koan-music

# or build from source
git clone https://github.com/radiosilence/koan.git && cd koan
cargo install --path crates/koan-cli
```

Requires macOS (CoreAudio). Single binary, no runtime dependencies.

### First run

```bash
# Create config directory with sensible defaults
koan init

# Point it at your music
koan scan /path/to/music
```

kōan indexes your library in parallel — metadata extraction is fast even for large collections. The database lives at `~/.config/koan/` and auto-updates when files change (FSEvents file watcher, debounced).

### Play something

```bash
# Just open the TUI — browse your library, pick tracks, build a queue
koan

# Or play files/directories directly
koan play ~/Music/Aphex\ Twin/
koan play ~/Music/album/*.flac

# Fuzzy search your library
koan pick "boards of canada"
koan pick --album "geogaddi"
koan pick --artist "autechre"
```

The TUI launches immediately — no waiting. If tracks need downloading (remote library) or scanning, they appear in the queue with animated spinners while loading in the background.

### The TUI

kōan is built around a full-screen terminal interface. The transport bar shows what's playing with album art (halfblock rendering), the queue groups tracks by album, and a hint bar at the bottom shows available keys for the current mode.

**The basics:** `space` to pause, `<`/`>` to skip tracks, `,`/`.` or arrow keys to seek. `p` opens a fuzzy track picker, `a` for albums, `r` for artists. `l` opens the library browser for tree-style browsing. `i` shows track info with cover art. `q` to quit.

**Building a queue:** Use the pickers (`p`/`a`/`r`) or library browser (`l`) to find music. `Enter` appends to the queue, `Ctrl+Enter` appends and starts playing, `Ctrl+R` replaces the entire queue. You can also drag files from Finder straight into the terminal.

**Editing the queue:** Press `e` to enter edit mode. Select tracks with shift-arrows or ctrl-click, `d` to delete, `j`/`k` to reorder. `Ctrl+Z` undoes any queue change, `Ctrl+Y` or `Ctrl+Shift+Z` to redo. Everything is mouse-friendly too — click, drag, scroll wheel all work.

**Your DAC matters:** kōan sends bit-perfect audio to CoreAudio with automatic sample rate switching. No resampling, no mixing — the bits that left the encoder are the bits that hit your DAC. Run `koan devices` to see your audio outputs.

### Remote music (Navidrome/Subsonic)

If you run a Navidrome or Subsonic server, kōan can sync and stream from it:

```bash
koan remote login https://music.example.com admin
koan remote sync
```

Remote and local tracks merge into one library. Local files always win for playback. Remote tracks download on first play and cache locally in a structured directory.

### Configuration

```bash
koan config  # show resolved config from both layers
```

Two-layer config at `~/.config/koan/` — `config.toml` for defaults you'd commit to dotfiles, `config.local.toml` for machine-specific paths and credentials (gitignored). See the [Configuration](#configuration-1) section below for details.

---

## What works

- **Bit-perfect playback** — CoreAudio AUHAL, no resampling, automatic device sample rate switching
- **Gapless** — decode thread keeps the ring buffer alive across track boundaries, AudioUnit never stops
- **Format support** — FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (via Symphonia)
- **Ratatui TUI** — full-screen terminal UI with transport bar, album-grouped queue, fuzzy picker overlay, library browser, track info modal with embedded album art (halfblock rendering), scrollbar, mouse support (click-to-seek, click-to-play, drag-to-reorder, scrollbar drag, scroll wheel)
- **Media keys** — macOS Control Center integration via souvlaki (play/pause, next/prev, now playing info)
- **Library indexing** — parallel metadata scanning with rayon, SQLite FTS5 full-text search
- **File watching** — FSEvents via notify, debounced 500ms, auto-updates DB on changes
- **Subsonic/Navidrome** — parallel remote library sync, unified local+remote browsing, lazy parallel downloads
- **Format string engine** — fb2k-compatible `%field%`, `[conditionals]`, `$functions()` for library views and file organization
- **File organization** — `koan organize` CLI or in-TUI organize modal (select tracks → context menu → pattern picker → preview → execute). Format strings, dry-run preview, undo
- **Queue management** — playlist-style display (played tracks stay visible dimmed), album-grouped headers, edit mode with Finder-style multi-selection (shift/option-click, shift-arrows), reorder/delete, multi-drag, undo/redo (Ctrl+Z/Y, 100-deep stack covering all playlist operations). Mouse editing (select, drag-reorder) works in any mode; double-click to skip to any track (forward or backward). Drag/drop files from Finder into the terminal to add them to the queue
- **Track deduplication** — local+remote tracks merged into single rows, local path always wins for playback
- **Proper artist handling** — track artist stored separately from album artist; compilations/VA albums display correctly

## Architecture

```
Pure Rust.

File → Symphonia → f32 samples → rtrb ring buffer → CoreAudio render callback → DAC

Lock-free audio thread. See ARCHITECTURE.md for the full technical manual.
```

Two crates:

- `koan-core` — audio engine, player, database, indexer, format strings, file organization, remote client
- `koan-music` — `koan` binary (Ratatui TUI)

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

# play files or directories (dirs are walked recursively for audio files)
koan play ~/Music/album/*.flac
koan play ~/Music/100\ gecs/

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
koan organize --pattern standard          # use named pattern from config
koan organize                             # use default pattern from config
koan organize --pattern '...' --execute   # actually move (default is dry-run)
koan organize --undo                      # revert last organize
koan organize --list                      # show configured patterns

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
| `/`     | search queue (jump to track) |
| `p`     | pick tracks            |
| `a`     | pick album             |
| `r`     | pick artist            |
| `i`     | track info             |
| `z`     | zoom album art         |
| `Ctrl+Z` | undo last queue change |
| `l`     | library browser        |
| `f`     | filter library (in library mode) |
| `e`     | edit queue             |
| `g`     | jump to start          |
| `G`     | jump to end            |
| `PgUp` / `Ctrl+U` | page up     |
| `PgDn` / `Ctrl+D` | page down   |
| `q`     | quit                   |

**Drag/drop:** Drag files or folders from Finder into the terminal window to add them to the queue at the current cursor position. A progress bar shows tag scanning progress for large imports.

**Picker confirm actions** (track/album/artist picker):

| Key          | Action                                 |
| ------------ | -------------------------------------- |
| `Enter`      | Append to queue (don't start playing)  |
| `Ctrl+Enter` | Append and play first added track      |
| `Ctrl+R`     | Replace entire queue and play          |

**Mouse** (works in any mode — modality is keyboard-only): double-click a queue track to skip to it (forward or backward); double-click a downloading track to prioritize and play it as soon as it finishes. Click the seek bar to jump, scroll wheel in queue. Single-click selects, drag to reorder. Ctrl-click for range selection, Option-click to toggle individual tracks, drag selected group to move all together. Scrollbar is clickable and draggable. In the fuzzy picker, click items to select, double-click to confirm, click outside to dismiss. In the library browser, click to select, double-click to expand/enter/enqueue; click queue pane to switch focus.

**Queue edit mode** (`e`):

| Key           | Action                   |
| ------------- | ------------------------ |
| `↑` `↓`       | navigate                 |
| `Shift+↑` `↓` | extend selection         |
| `d`           | remove selected track(s) |
| `j` / `k`     | move selected down/up    |
| `Ctrl+Z` / `Ctrl+Y` | undo / redo      |
| `Space`        | context menu (organize)  |
| `g`           | jump to start            |
| `G`           | jump to end (shift-extends) |
| `PgUp` / `PgDn` | page up/down           |
| `⌥-click`     | toggle select            |
| `Ctrl-click`  | range select             |
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

Rename and reorganize your music library using fb2k-compatible format strings. Two workflows:

- **TUI:** Select tracks in edit mode → `Space` → Organize → pick pattern from config → preview → run. Playlist paths update automatically, playback continues uninterrupted.
- **CLI:** `koan organize` for batch operations. Default is dry-run (preview), add `--execute` to apply. Undo with `--undo`.

See [docs/format-strings.md](docs/format-strings.md) for the full syntax reference, all available fields/functions, and examples.

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

[organize]
default = "standard"

[organize.patterns]
standard = "%album artist%/(%date%) %album%/%tracknumber%. %title%"
va-aware = "%album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%"
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
just cli      # cargo run -p koan-music -- <args>
```

## License

MIT
