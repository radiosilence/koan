<img width="1413" height="956" alt="An album in the macOS app" src="https://github.com/user-attachments/assets/8ec2f049-524a-4437-8bf3-91172c6b4f26" />

# kōan

> It is a music player. Designed for both local and remote collections (subsonic/navidrome). Remote works with a fairly aggressive local cache. It is super fast and handles 1TB+ libraries with ease and has all the things you'd want like gapless, queue management, "bit-perfect" (so much as the audio stack allows), combined search...etc. Built from 25 years of experience messing about with music and being annoyed with pretty much everything and wanting my dream application. There are some organisational features such as file renaming, which is compatible with fb2k syntax, and I plan to add a decent well thought out tagger once I have pondered the UX more.
>
> Originally built as a Rust TUI and core, I've now added a beautiful macOS SwiftUI app (no Electron) that uses FFI to bridge to the rust. It's fast, it's pretty, it has these lush transitions, and the point is to do all the basics properly and well before adding features, I'm really proud of it. The UX is somewhat inspired by taking the things I like about Apple Music and fb2k, but also fixing things I thought were dumb.
>
> Full disclaimer: AI assisted coding was used. I have been building somewhat high quality software for a long time (decades) before AI existed, and I'd like to think the decisions I've been making reflect this as opposed to just vibing slop. I probably could have written it myself, but I kind of wanted to take a step back and be more of an architect/technical lead/product owner rather than a coder for this.
>
> — [@radiosilence](https://github.com/radiosilence)


<img width="1630" height="1167" alt="The library in the macOS app" src="https://github.com/user-attachments/assets/cb7f9ca0-61eb-4e7e-bebc-43fbc11a7c78" />

<img width="1405" height="905" alt="The macOS app albums view" src="https://github.com/user-attachments/assets/c0ac41f2-3cde-4ad4-8aa4-e53859d6559d" />

<img width="874" height="942" alt="The TUI" src="https://github.com/user-attachments/assets/99782de3-5683-4dd9-97b6-10782e8e4099" />

<img width="1824" height="1355" alt="Screenshot 2026-08-25 at 00 06 31" src="https://github.com/user-attachments/assets/e6d734f1-f2a7-4364-a914-ad953ead7da5" />


<img width="406" height="182" alt="Screenshot 2026-03-04 at 18 30 32" src="https://github.com/user-attachments/assets/d4fff1f7-7c1f-4aaa-87aa-41bd2b9c22f7" />

## Install

macOS App:

```bash
brew install --cask radiosilence/koan/koan-app
```

You may have to update brew's trust settings to trust the tap.
Alternatively you can get `Koan.dmg` from the releases page. The app
is signed but not notarised because Apple wants $$ — so macOS refuses the first
open and offers only "Move to Trash". Drag it to Applications and run
`xattr -dr com.apple.quarantine /Applications/kōan.app` once. The cask does this
for you.

CLI/TUI:

```bash
# mise (recommended)
mise use -g github:radiosilence/koan@latest

# homebrew
brew install radiosilence/koan/koan

# or via cargo
cargo install koan-cli

# or build from source
git clone https://github.com/radiosilence/koan.git && cd koan
cargo install --path crates/koan-cli
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

## 30-second quickstart (for CLI - for GUI just open settings and mess about)

```bash
koan config init                            # create config dir + commented template
# edit ~/.config/koan/config.local.toml:
#   [library]
#   folders = ["/path/to/your/music"]
koan scan                                   # index your library
koan                                        # launch the TUI
```

`space` to pause, `<`/`>` to skip, `p` to pick tracks, `a` for albums, `q` to quit. That's it.

**Rather not touch a terminal?** Install the app instead and do all of the above
inside it: **Settings → Library** points koan at your music and scans it,
**Settings → Server** signs you in to Navidrome or Subsonic, and playback,
output device and radio have their own panes. Everything in this quickstart can
be done from the app; the headless server and the MCP endpoint still want a
shell.

**Remote server?** If you run Navidrome or Subsonic:

```bash
koan remote login https://music.example.com admin
koan remote sync
koan
```

Local and remote tracks merge into one library. Local files take playback priority; remote tracks stream with progressive download.

## What it does

- **Bit-perfect playback** -- CoreAudio AUHAL / ALSA via cpal, the device switched to the source rate rather than resampled to reach it. When a device refuses the switch, the format badge says the output is resampled instead of claiming otherwise
- **Gapless transitions** -- decode thread keeps the ring buffer alive across track boundaries
- **Format support** -- FLAC, MP3, AAC, Vorbis, ALAC, ADPCM, WAV/AIFF/CAF, Ogg, MKV/WebM, MP4
- **Native macOS app** -- SwiftUI, built out of Liquid Glass. Album-grouped queue with drag reorder, library and artist browsing, ⌘K search, synced lyrics, play history, snapshots, file organization, and first-run setup — no terminal required
- **Full-screen TUI** -- transport bar with album art, album-grouped queue, fuzzy picker, library browser, track info modal, visualizer, lyrics panel, mouse support
- **Authentication** -- Ed25519 JWT tokens, three roles (admin/user/readonly), 1Password CLI integration
- **Subsonic/Navidrome** -- incremental sync, unified local+remote browsing, streaming playback, two-way favourite sync for tracks, albums and artists
- **Radio mode** -- infinite play using Subsonic similarity, cached artist relationships, and genre matching
- **ReplayGain** -- track and album modes with peak limiting and configurable pre-amp
- **Format strings** -- fb2k-compatible `%field%`, `[conditionals]`, `$functions()` — 59 of them — for display and file organization
- **File organization** -- rename/reorganize your library from the macOS app or the TUI using format string patterns
- **GraphQL API** -- full programmatic control alongside the app and TUI, or headless. Relay pagination, rich filters, mutations for everything
- **MCP server** -- `koan mcp` exposes the player to Claude Desktop via Model Context Protocol
- **Queue management** -- undo/redo (100-deep), multi-select, drag-reorder, Finder drag & drop, session persistence
- **SQLite FTS5 search** -- full-text search across your entire library
- **Media keys** -- macOS Control Center and Linux MPRIS (play/pause, next/prev, now playing info)
- **Lyrics** -- synced (LRC) and plain lyrics from LRCLIB, current line highlighting
- **22 visualizer modes** -- spectrum bars, oscilloscope, radial, particles, lissajous, spectrogram, stereo waveform, VU meter, flame, plasma, tunnel, wireframe, metaballs, starfield, terrain, moiré, kaleidoscope, julia fractal, spiral, interference, wormhole, matrix rain. Picker with live preview (`v`), matrix overlay (`X`), bass shake (`S`), configurable reactivity

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
| **fb2k format strings** | **59 functions** | Column fmt | Basic | No | No | Basic | No |
| **File organization** | **Yes** | No | No | No | No | No | No |
| **FTS search** | **SQLite FTS5** | MPD search | Filter | Text | Filter | MPD search | Basic |
| **Queue undo/redo** | **100-deep** | No | No | No | No | No | No |
| **Mouse support** | **Full** | Yes | Yes | Basic | Yes | Yes | No |
| **Media keys** | **macOS CC + MPRIS** | Via MPRIS | Via MPRIS | -- | Via MPRIS | Via MPRIS | -- |
| **Drag & drop** | **Finder -> TUI** | No | No | No | No | No | No |
| **Lyrics** | **Synced + plain** | Via MPD | No | Plugin | No | Via MPD | No |
| **Visualizer** | **22 modes** | No | No | No | No | No | No |
| **Favourites** | **Yes (syncs)** | Via MPD | No | Yes | No | Via MPD | **Yes** |
| **Streaming playback** | **Yes (256KB)** | Via MPD | No | No | No | Via MPD | **Yes** |
| **API / MCP** | **GraphQL + MCP** | MPD protocol | No | No | No | MPD protocol | No |
| **Tag editing** | Soon | Via MPD | No | Yes | Yes | Via MPD | No |
| **DSP / EQ** | No | Via MPD | Yes | Yes | No | Via MPD | No |
| **Auth** | **JWT + roles** | No | No | No | No | No | No |
| **Platforms** | macOS, Linux | Linux/macOS | Linux/macOS/BSD | Linux/macOS/Win | Linux/macOS/Win | Linux/macOS | Linux/macOS |
| **Maintained** | Yes | Yes | Yes (2.12.0) | Slowing | Yes | Very active | Stale |

### Desktop players (GUI)

| | koan | foobar2000 | Strawberry | DeaDBeeF |
|---|:---:|:---:|:---:|:---:|
| **Type** | **Native GUI + TUI** | GUI | GUI (Qt) | GUI (GTK) |
| **Bit-perfect** | **Yes** | Yes (WASAPI/ASIO) | Yes (Linux) | Yes (ALSA) |
| **Gapless** | **Yes** | Yes | Yes | Yes |
| **Subsonic** | **Built-in** | Plugin | **Built-in** | No |
| **ReplayGain** | **Track + album** | Scan + apply | Yes | Scan + apply |
| **Format strings** | **fb2k-compat** | **The original** | Organizer only | fb2k-like |
| **File organization** | **Yes** | Yes (component) | **Yes** | No |
| **Queue undo/redo** | **100-deep** | Partial | No | Yes |
| **Lyrics** | **Synced + plain** | Plugin | No | Plugin |
| **Visualizer** | **22 modes** | Plugin | No | Plugin |
| **Tag editing** | Soon | **Yes** | Yes | **Yes** |
| **DSP / EQ** | No | **Yes (VST)** | Yes | Yes |
| **Platforms** | macOS (app + TUI), Linux (TUI) | Windows/macOS | All | All |

<img width="768" height="612" alt="Screenshot 2026-03-04 at 18 31 01" src="https://github.com/user-attachments/assets/0ad4879e-815f-42f3-8ebe-f6d01616bc96" />

## Documentation

| Guide | What it covers |
|-------|---------------|
| **[Getting Started](docs/getting-started.md)** | First-time setup, local and remote libraries, your first session |
| **[Authentication](docs/guide/authentication.md)** | JWT auth, user management, 1Password integration, recovery |
| **[Radio Mode](docs/guide/radio-mode.md)** | Infinite play, similarity scoring, tuning discovery |
| **[Remote Servers](docs/guide/remote-servers.md)** | Navidrome/Subsonic setup, sync, streaming, cache management |
| **[File Organization](docs/guide/file-organization.md)** | Rename and reorganize your library from the TUI |
| **[GraphQL API](docs/guide/graphql-api.md)** | Headless operation, queries, mutations, daemon mode |
| **[MCP Integration](docs/guide/mcp-integration.md)** | Claude Desktop setup, example prompts |
| **[Headless Server](docs/guide/headless-server.md)** | Running koan as a background music server |
| **[Configuration](docs/reference/configuration.md)** | All config fields, layered config, env var overrides |
| **[Keybindings](docs/reference/keybindings.md)** | Every key in every mode |
| **[CLI Reference](docs/reference/cli.md)** | All commands, flags, and shell completions |
| **[Format Strings](docs/format-strings.md)** | fb2k-compatible template syntax and all 59 functions |
| **[Troubleshooting](docs/recipes/troubleshooting.md)** | Common issues and fixes |
| **[Cache Management](docs/recipes/cache-management.md)** | Download cache, eviction, disk usage |

## Architecture

```
File -> Symphonia -> f32 samples -> rtrb ring buffer -> CoreAudio/cpal callback -> DAC
```

Five crates: `koan-core` (audio engine, player, database, indexer), `koan-tui` (Ratatui TUI, visualizers, media keys), `koan-server` (GraphQL, Subsonic REST, MCP), `koan-ffi` (uniffi bindings for native clients), and `koan-cli` (the `koan` binary). See [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical manual.

## macOS app

A native SwiftUI app lives in [`apps/macos`](apps/macos), and it is a way to use
koan rather than a viewer bolted onto the side of one. Browse and search the
library, build and reorder the queue, favourite tracks, albums and artists, save
and restore snapshots, read synced lyrics, look through play history, and
reorganize files on disk — and set the whole thing up on first run, library
folders and remote sign-in included, without opening a terminal.

It links `koan-core` directly through `koan-ffi` rather than talking to `koan
serve` — the app is sitting on top of the audio engine, so round-tripping HTTP to
reach it would buy nothing and cost a daemon, a port, and an auth surface.
Playback stays bit-perfect because CoreAudio output never leaves Rust.

One library and one config with the CLI and TUI, so a queue saved in one shows up
in the others, and a scan run in either is a scan for both.

Two things it deliberately leaves alone: visualizers, which are what the TUI is
for, and running the server, which is a `koan serve` job. GraphQL remains the
surface for clients that genuinely *can't* link the core — the web SPA, iOS, and
jukebox-style remotes.

Dropping a folder from Finder onto the queue indexes it into the library and
plays it; **Organize Files…** then previews where a pattern puts each file —
collisions included — before moving anything. See
[File Organization](docs/guide/file-organization.md).

```bash
just macos-run     # build and launch
just macos-dmg     # package for release
```

Requires Swift 6 and macOS 26+.

## Coming soon

- **Tag editing** -- inline editing, bulk operations, vimv-style external editor ([plan](/.claude/plans/04-tagging.md))
- **Artist metadata** -- bios, images, similar artists from MusicBrainz/Last.fm ([plan](/.claude/plans/09-artist-metadata.md))
- **Playlists** -- proper playlists, beyond the queue snapshots that stand in for them today

## Dev

```bash
just check    # test + clippy
just fmt      # cargo fmt
just cli      # cargo run -p koan-cli -- <args>
just macos-run # build + launch the macOS app
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

MIT
