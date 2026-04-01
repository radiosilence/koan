# koan

A music player for people who give a shit about audio quality.

<img width="874" height="942" alt="Screenshot 2026-03-04 at 18 30 07" src="https://github.com/user-attachments/assets/99782de3-5683-4dd9-97b6-10782e8e4099" />

Pure Rust, Ratatui TUI. Bit-perfect playback, gapless transitions, fast library indexing, Subsonic/Navidrome integration, fb2k-style format strings. No Electron. No subscriptions. No bullshit.

<img width="406" height="182" alt="Screenshot 2026-03-04 at 18 30 32" src="https://github.com/user-attachments/assets/d4fff1f7-7c1f-4aaa-87aa-41bd2b9c22f7" />

## Install

```bash
# homebrew (recommended)
brew install radiosilence/koan/koan

# pre-built binary via mise
mise use -g github:radiosilence/koan@latest

# or via cargo
cargo install koan-music

# or build from source
git clone https://github.com/radiosilence/koan.git && cd koan
cargo install --path crates/koan-music
```

Single binary. macOS works out of the box (CoreAudio). Linux needs ALSA dev headers:

```bash
# Debian/Ubuntu
sudo apt install libasound2-dev libdbus-1-dev

# Fedora
sudo dnf install alsa-lib-devel dbus-devel

# Arch
sudo pacman -S alsa-lib dbus
```

## 30-second quickstart

```bash
koan config init                            # create config dir + commented template
# edit ~/.config/koan/config.local.toml:
#   [library]
#   folders = ["/path/to/your/music"]
koan scan                                   # index your library
koan                                        # launch the TUI
```

`space` to pause, `<`/`>` to skip, `p` to pick tracks, `a` for albums, `q` to quit. That's it.

**Remote server?** If you run Navidrome or Subsonic:

```bash
koan remote login https://music.example.com admin
koan remote sync
koan
```

Local and remote tracks merge into one library. Local files take playback priority; remote tracks stream with progressive download.

## What it does

- **Bit-perfect playback** -- CoreAudio AUHAL / ALSA via cpal, automatic sample rate switching, no resampling
- **Gapless transitions** -- decode thread keeps the ring buffer alive across track boundaries
- **Format support** -- FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (via Symphonia)
- **Full-screen TUI** -- transport bar with album art, album-grouped queue, fuzzy picker, library browser, track info modal, spectrum analyzer, lyrics panel, mouse support
- **Subsonic/Navidrome** -- incremental sync, unified local+remote browsing, streaming playback, favourite sync
- **Radio mode** -- infinite play using Subsonic similarity, cached artist relationships, and genre matching
- **ReplayGain** -- track and album modes with peak limiting and configurable pre-amp
- **Format strings** -- fb2k-compatible `%field%`, `[conditionals]`, `$functions()` for display and file organization
- **File organization** -- rename/reorganize your library from inside the TUI using format string patterns
- **GraphQL API** -- full programmatic control alongside the TUI, or headless. Relay pagination, rich filters, mutations for everything
- **MCP server** -- `koan mcp` exposes the player to Claude Desktop via Model Context Protocol
- **Queue management** -- undo/redo (100-deep), multi-select, drag-reorder, Finder drag & drop, session persistence
- **SQLite FTS5 search** -- full-text search across your entire library
- **Media keys** -- macOS Control Center integration (play/pause, next/prev, now playing info)
- **Lyrics** -- synced (LRC) and plain lyrics from LRCLIB, current line highlighting
- **Spectrum analyzer** -- 48-band FFT on a dedicated thread, configurable frequency/amplitude scales

<img width="815" height="598" alt="Screenshot 2026-03-04 at 18 30 43" src="https://github.com/user-attachments/assets/9dab1d13-5d48-4e60-8625-7d72dd2e7957" />

## How it compares

No TUI player combines bit-perfect audio, Subsonic streaming, album art, fb2k-style format strings, and file organization in one binary. Most either need a daemon, lack remote support, or skip the audiophile bits.

### TUI / terminal players

| | koan | ncmpcpp | cmus | musikcube | termusic | rmpc | stmp |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| **Language** | Rust | C++ | C | C++ | Rust | Rust | Go |
| **Standalone** | **Yes** | No (MPD) | Yes | Yes | Yes | No (MPD) | No (Subsonic) |
| **Bit-perfect** | **Yes** | Via MPD | Via ALSA | No | No | Via MPD | No |
| **Gapless** | **Yes** | Yes | Yes | Yes | Yes | Yes | No |
| **Subsonic/Navidrome** | **Yes** | No | No | No | No | No | **Yes** |
| **Local library** | **Yes** | Via MPD | Yes | Yes | Yes | Via MPD | No |
| **Local + remote unified** | **Yes** | -- | -- | -- | -- | -- | -- |
| **Album art** | **Halfblock** | Kitty | No | No | Kitty/Sixel | Kitty/Sixel | No |
| **ReplayGain** | **Yes** | Via MPD | Yes | Yes | No | Via MPD | No |
| **fb2k format strings** | **55+ functions** | Column fmt | Basic | No | No | Basic | No |
| **File organization** | **Yes** | No | No | No | No | No | No |
| **FTS search** | **SQLite FTS5** | MPD search | Filter | Text | Filter | MPD search | Basic |
| **Queue undo/redo** | **100-deep** | No | No | No | No | No | No |
| **Mouse support** | **Full** | Yes | Yes | Basic | Yes | Yes | No |
| **Media keys** | **macOS CC** | Via MPRIS | Via MPRIS | -- | Via MPRIS | Via MPRIS | -- |
| **Drag & drop** | **Finder -> TUI** | No | No | No | No | No | No |
| **Lyrics** | **Synced + plain** | Via MPD | No | Plugin | No | Via MPD | No |
| **Spectrum analyzer** | **48-band FFT** | No | No | No | No | No | No |
| **Favourites** | **Yes (syncs)** | Via MPD | No | Yes | No | Via MPD | **Yes** |
| **Streaming playback** | **Yes (256KB)** | Via MPD | No | No | No | Via MPD | **Yes** |
| **API / MCP** | **GraphQL + MCP** | MPD protocol | No | No | No | MPD protocol | No |
| **Tag editing** | Soon | Via MPD | No | Yes | Yes | Via MPD | No |
| **DSP / EQ** | Soon | Via MPD | Yes | Yes | No | Via MPD | No |
| **Platforms** | macOS | Linux/macOS | Linux/macOS/BSD | Linux/macOS/Win | Linux/macOS/Win | Linux/macOS | Linux/macOS |
| **Maintained** | Yes | Yes | Yes (2.12.0) | Slowing | Yes | Very active | Stale |

### Desktop players (GUI)

| | koan | foobar2000 | Strawberry | DeaDBeeF |
|---|:---:|:---:|:---:|:---:|
| **Type** | TUI | GUI | GUI (Qt) | GUI (GTK) |
| **Bit-perfect** | **Yes** | Yes (WASAPI/ASIO) | Yes (Linux) | Yes (ALSA) |
| **Gapless** | **Yes** | Yes | Yes | Yes |
| **Subsonic** | **Built-in** | Plugin | **Built-in** | No |
| **ReplayGain** | **Track + album** | Scan + apply | Yes | Scan + apply |
| **Format strings** | **fb2k-compat** | **The original** | Organizer only | fb2k-like |
| **File organization** | **Yes** | Yes (component) | **Yes** | No |
| **Queue undo/redo** | **100-deep** | Partial | No | Yes |
| **Lyrics** | **Synced + plain** | Plugin | No | Plugin |
| **Spectrum analyzer** | **48-band FFT** | Plugin | No | Plugin |
| **Tag editing** | Soon | **Yes** | Yes | **Yes** |
| **DSP / EQ** | Soon | **Yes (VST)** | Yes | Yes |
| **Platforms** | macOS | Windows/macOS | All | All |

<img width="768" height="612" alt="Screenshot 2026-03-04 at 18 31 01" src="https://github.com/user-attachments/assets/0ad4879e-815f-42f3-8ebe-f6d01616bc96" />

## Documentation

| Guide | What it covers |
|-------|---------------|
| **[Getting Started](docs/getting-started.md)** | First-time setup, local and remote libraries, your first session |
| **[Radio Mode](docs/guide/radio-mode.md)** | Infinite play, similarity scoring, tuning discovery |
| **[Remote Servers](docs/guide/remote-servers.md)** | Navidrome/Subsonic setup, sync, streaming, cache management |
| **[File Organization](docs/guide/file-organization.md)** | Rename and reorganize your library from the TUI |
| **[GraphQL API](docs/guide/graphql-api.md)** | Headless operation, queries, mutations, daemon mode |
| **[MCP Integration](docs/guide/mcp-integration.md)** | Claude Desktop setup, example prompts |
| **[Headless Server](docs/guide/headless-server.md)** | Running koan as a background music server |
| **[Configuration](docs/reference/configuration.md)** | All config fields, layered config, env var overrides |
| **[Keybindings](docs/reference/keybindings.md)** | Every key in every mode |
| **[CLI Reference](docs/reference/cli.md)** | All commands, flags, and shell completions |
| **[Format Strings](docs/format-strings.md)** | fb2k-compatible template syntax and all 55+ functions |
| **[Troubleshooting](docs/recipes/troubleshooting.md)** | Common issues and fixes |
| **[Cache Management](docs/recipes/cache-management.md)** | Download cache, eviction, disk usage |

## Architecture

```
File -> Symphonia -> f32 samples -> rtrb ring buffer -> CoreAudio/cpal callback -> DAC
```

Two crates: `koan-core` (audio engine, player, database, indexer) and `koan-music` (`koan` binary, TUI). See [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical manual.

## Coming soon

- **Linux support** -- ALSA/PipeWire backends via trait-based audio abstraction ([plan](/.claude/plans/01-linux-and-audio-backends.md))
- **DSP pipeline** -- EQ, headphone correction profiles, crossfeed ([plan](/.claude/plans/02-dsp-and-profiles.md))
- **Tag editing** -- inline editing, bulk operations, vimv-style external editor ([plan](/.claude/plans/04-tagging.md))
- **Artist metadata** -- bios, images, similar artists from MusicBrainz/Last.fm ([plan](/.claude/plans/09-artist-metadata.md))

## Dev

```bash
just check    # test + clippy
just fmt      # cargo fmt
just cli      # cargo run -p koan-music -- <args>
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

MIT
