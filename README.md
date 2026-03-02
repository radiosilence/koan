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
cargo install --path crates/koan-music
```

Requires macOS (CoreAudio). Single binary, no runtime dependencies.

### Set up your music

```bash
# Create config directory with sensible defaults
koan init
```

This creates `~/.config/koan/` with two config files. kōan needs at least one music source — local files, a remote server, or both.

**Local files** — point kōan at your music directory:

```toml
# ~/.config/koan/config.local.toml
[library]
folders = ["/Volumes/Music/library"]
```

Then scan:

```bash
koan scan
```

Indexing runs in parallel — fast even for large collections.

**Remote server (Navidrome/Subsonic):**

If you run Navidrome, Subsonic, or anything with a Subsonic-compatible API:

```bash
koan remote login https://music.example.com admin
koan remote sync
```

Remote and local tracks merge seamlessly into one library — if the same track exists in both sources (matched by artist + album + title + track number), it becomes a single entry. Local files always take playback priority; remote is only used as a fallback if the local file is missing. Remote-only tracks download on first play and cache locally — subsequent plays are instant.

You can use both sources together. Run `koan remote sync` periodically (or after adding music to your server) to pull new tracks.

### Play something

```bash
# Open the TUI
koan

# Or play files/directories directly
koan play ~/Music/Aphex\ Twin/
koan play ~/Music/album/*.flac

# Interactive fuzzy pickers
koan pick              # search all tracks
koan pick --album      # browse albums
koan pick --artist     # browse artists
```

The TUI launches immediately — no waiting. If tracks need downloading (remote library), they appear in the queue with animated spinners while loading in the background.

### The TUI

kōan is built around a full-screen terminal interface. The transport bar shows what's playing with album art (halfblock rendering), the queue groups tracks by album, and a hint bar at the bottom shows available keys for the current mode.

**The basics:** `space` to pause, `<`/`>` to skip tracks, `,`/`.` or arrow keys to seek. `p` opens a fuzzy track picker, `a` for albums, `r` for artists. `l` opens the library browser for tree-style browsing. `i` shows track info with cover art. `q` to quit.

**Building a queue:** Use the pickers (`p`/`a`/`r`) or library browser (`l`) to find music. `Enter` appends to the queue, `Ctrl+Enter` appends and starts playing, `Ctrl+R` replaces the entire queue. You can also drag files from Finder straight into the terminal.

**Editing the queue:** Press `e` to enter edit mode. Select tracks with shift-arrows or ctrl-click, `d` to delete, `j`/`k` to reorder. `Ctrl+Z` undoes any queue change, `Ctrl+Y` or `Ctrl+Shift+Z` to redo. Everything is mouse-friendly too — click, drag, scroll wheel all work.

**Your DAC matters:** kōan sends bit-perfect audio to CoreAudio with automatic sample rate switching. No resampling, no mixing — the bits that left the encoder are the bits that hit your DAC. Run `koan devices` to see your audio outputs.

---

## What works

- **Bit-perfect playback** — CoreAudio AUHAL, no resampling, automatic device sample rate switching
- **Gapless** — decode thread keeps the ring buffer alive across track boundaries, AudioUnit never stops
- **Format support** — FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (via Symphonia)
- **Ratatui TUI** — full-screen terminal UI with transport bar, album-grouped queue, fuzzy picker overlay, library browser, track info modal with embedded album art (halfblock rendering), scrollbar, mouse support (click-to-seek, click-to-play, drag-to-reorder, scrollbar drag, scroll wheel)
- **Media keys** — macOS Control Center integration via souvlaki (play/pause, next/prev, seek, now playing info with album art)
- **Library indexing** — parallel metadata scanning with rayon, SQLite FTS5 full-text search
- **Subsonic/Navidrome** — parallel remote library sync, unified local+remote browsing, lazy parallel downloads
- **Format string engine** — fb2k-compatible `%field%`, `[conditionals]`, `$functions()` for library views and file organization
- **File organization** — in-TUI organize modal: select tracks → context menu → pick a named pattern → preview moves → execute. Playlist paths update live, playback continues uninterrupted
- **Queue management** — playlist-style display (played tracks stay visible dimmed), album-grouped headers, edit mode with Finder-style multi-selection (shift/option-click, shift-arrows), reorder/delete, multi-drag, undo/redo (Ctrl+Z/Y, 100-deep stack covering all playlist operations). Mouse editing (select, drag-reorder) works in any mode; double-click to skip to any track (forward or backward). Drag/drop files from Finder into the terminal to add them to the queue
- **Track deduplication** — 3-strategy match (path → remote ID → content) merges local and remote into one DB row. No duplicates in search or browse. Playback priority: local file → cached download → remote stream
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

## CLI reference

```bash
# setup
koan init                     # create config directory with defaults
koan scan                     # scan configured library folders

# play
koan                          # open TUI (use `l` to browse library)
koan play --library           # open TUI in library browse mode
koan play ~/Music/album/      # play a directory (recursive)
koan play ~/Music/*.flac      # play specific files
koan play --album 5           # play album by ID (use tab completion)
koan play --artist 3          # play artist by ID

# search & browse
koan pick                     # fuzzy search all tracks
koan pick --album             # fuzzy browse albums
koan pick --artist            # fuzzy browse artists
koan search "radiohead"       # text search (CLI output)
koan artists                  # list all artists
koan albums                   # list all albums
koan library                  # library statistics

# remote
koan remote login URL user    # authenticate with Subsonic/Navidrome server
koan remote sync              # sync remote library to local database
koan remote status            # show remote server info

# utilities
koan config                   # show resolved config from both layers
koan devices                  # list audio output devices
koan cache status             # show download cache size
koan cache clear              # clear cached remote downloads
koan probe track.flac         # show format/codec info for a file
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

**Drag/drop:** Drag files or folders from Finder into the terminal window to add them to the queue.

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

Rename and reorganize your music library using fb2k-compatible format strings, directly from the TUI.

Select tracks in edit mode (`e`) → `Space` to open the context menu → Organize → pick a named pattern from your config → preview the file moves → execute. Playlist paths update automatically, playback continues uninterrupted (Unix rename preserves open file descriptors). Ancillary files (cover.jpg, .cue, .log) move with the music. Empty directories are cleaned up.

Define organize patterns in your config — see [Configuration](#configuration) below and [docs/format-strings.md](docs/format-strings.md) for the full syntax reference.

## Configuration

Two-layer config at `~/.config/koan/`:

- **`config.toml`** — shareable defaults, safe to commit to dotfiles
- **`config.local.toml`** — machine-specific paths, credentials, overrides (gitignored)

Local values override base. Run `koan config` to see both layers and the resolved result.

### Playback

```toml
# config.toml
[playback]
software_volume = false   # volume control in software (vs hardware/DAC)
replaygain = "album"      # off | track | album
```

### Library

```toml
# config.local.toml
[library]
folders = ["/Volumes/Music/library"]
```

### Remote server

```toml
# config.local.toml
[remote]
enabled = true
url = "https://music.example.com"
username = "admin"
```

Password is prompted by `koan remote login` and saved to `config.local.toml` (gitignored).

### Organize patterns

The TUI organize modal picks from named patterns defined in your config. Format strings use fb2k syntax — `%field%` for metadata, `$function()` for transforms, `[conditionals]` to omit blocks when fields are missing. See [docs/format-strings.md](docs/format-strings.md) for the full reference.

```toml
# config.toml
[organize]
default = "standard"      # which pattern the modal selects by default

[organize.patterns]
standard = "%album artist%/(%date%) %album%/%tracknumber%. %title%"
va-aware = "%album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%"
flat = "%artist% - %title%"
```

The `va-aware` pattern handles compilations: if the album artist is "Various Artists", it includes the per-track artist in the filename and omits the redundant year prefix. The `$stricmp`, `$if`, `$left`, `$num` functions work the same as in foobar2000.

Database, download cache, and log file all live at `~/.config/koan/`.

## Dev

```bash
just check    # test + clippy
just fmt      # cargo fmt
just cli      # cargo run -p koan-music -- <args>
```

## License

MIT
