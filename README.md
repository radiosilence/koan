<img width="1413" height="956" alt="An album in the macOS app" src="https://github.com/user-attachments/assets/8ec2f049-524a-4437-8bf3-91172c6b4f26" />

# kōan

[koan.rocks](https://koan.rocks)

It's a music player and server, for local collections and remote ones (anything OpenSubsonic). Remote libraries sit behind a fairly aggressive local cache. It handles multi-terabyte libraries with ease and has all the core audio features you'd want, like gapless and bit-perfect output (where the system allows). It's built from 25 years of messing about with music, being annoyed with pretty much everything, and wanting my dream application.

The idea is to be fully compatible with the existing ecosystem while bringing the decent UX and modern ideas that professionally made streaming services have. It started as a little cross-platform CLI and TUI app on a Rust core. Now there's a native SwiftUI macOS app (no Electron) that links that core directly, an iOS app, a server, and soon a tvOS app. The UX takes what I like about Apple Music and fb2k and fixes the things I thought were dumb. The point is to do the basics properly before adding features, and I'm really proud of it.

I wanted UX that makes it easy as hell to do what you want, while staying SUPER low on resources (and now battery). And I wanted the stuff you don't really see in the self-hosted space: device control and handoff, EQ profiles and convolution, DLNA output (with EQ!), and a cache built for a commuter who often has no signal, so you never have to remember to download your whole queue first. The best of every world, why not?

My philosophy is that you should lead with your opinions, but let people customise and tweak them to match theirs.

Because it grew organically, and I've insisted on staying compatible with OpenSubsonic, every part works on its own. You can use the macOS app with Navidrome. You can use the server with Arpeggi. Or you can use kōan all the way down and get the non-standard (sorry) features such as remote control. I think a shared standard like OpenSubsonic is incredibly important so everyone plays nicely together, but it shouldn't stop us experimenting to compete with how well the professional streaming platforms hang together.

One thing I've noticed: when you mix self-hosted apps with proprietary tech, say AirPlay from the fantastically solid play:Sub, you're treated as a second-class citizen. The audio has to stream over the wire from your phone, which is laggy. With kōan's remote features, the device you send it to plays its own copy.

I did use AI-assisted coding for this project. I've been building fairly high-quality software for a *loooong* time (decades) before AI existed, and I'd like to think the decisions reflect that rather than vibing slop. I probably could have written it myself, but I wanted to step back and be more of an architect, technical lead and product owner than the person typing out the code, as I'm just one person.

If it gets traction I'll happily look at more platforms like Android, but I'm already out $99 for an Apple Developer account, so I'm not shelling out for an Android phone just yet.

— [@radiosilence](https://github.com/radiosilence)


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
Alternatively, [download `Koan.dmg`](https://github.com/radiosilence/koan/releases/latest/download/Koan.dmg) from the latest release.

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


Single binary. macOS needs nothing else. Linux needs the ALSA and D-Bus development headers:

```bash
# Debian/Ubuntu
sudo apt install libasound2-dev libdbus-1-dev

# Fedora
sudo dnf install alsa-lib-devel dbus-devel

# Arch
sudo pacman -S alsa-lib dbus
```

## Quickstart (CLI)

```bash
koan config init                            # create config dir + commented template
# edit ~/.config/koan/config.local.toml:
#   [library]
#   folders = ["/path/to/your/music"]
koan scan                                   # index your library
koan                                        # launch the TUI
```

`space` to pause, `<`/`>` to skip, `p` to pick tracks, `a` for albums, `q` to quit.

The macOS app needs none of this: **Settings → Library** adds folders, **Settings → Server** signs in to a kōan, Navidrome or Subsonic server.

To run a server, play from Navidrome, or move off it, see the [documentation](https://koan.rocks/docs/).

## What it does

- **Bit-perfect playback** -- CoreAudio AUHAL / ALSA via cpal, the device switched to the source rate rather than resampled to reach it. When a device refuses the switch, the format badge says the output is resampled instead of claiming otherwise
- **Gapless transitions** -- decode thread keeps the ring buffer alive across track boundaries
- **Format support** -- FLAC, MP3, AAC, Vorbis, Opus, ALAC, ADPCM, WAV/AIFF/CAF, Ogg, MKV/WebM, MP4. Opus is decoded by `opus-decoder` rather than symphonia, which ships no Opus codec — mono and stereo, in Ogg, Matroska or WebM
- **Native macOS app** -- SwiftUI, built out of Liquid Glass. Album-grouped queue with drag reorder, playlists, library and artist browsing, ⌘K search, synced lyrics, play history, file organization, and first-run setup
- **Full-screen TUI** -- transport bar with album art, album-grouped queue, fuzzy picker, library browser, track info modal, visualizer, lyrics panel, mouse support
- **Authentication** -- Ed25519 JWT tokens, three roles (admin/user/readonly), 1Password CLI integration
- **Subsonic/Navidrome** -- library sync that runs when the server changes, unified local+remote browsing, streaming playback, two-way sync of favourites and playlists
- **Music server** -- run headless and kōan serves the library itself: a mobile-first web UI with gapless browser playback, favourites, history and Recently played, share links (a track shares its album cued to it) that unfurl with their cover, an OpenSubsonic API for Subsonic apps, signed in with a kōan account by password, app password or API key, with ratings, bookmarks and transcoding to Opus, MP3 or AAC for clients that ask, and ListenBrainz scrobbling per account. See [Running a server](https://koan.rocks/docs/headless-server/)
- **Playlists** -- ordered, named, reorderable; synced both ways with Navidrome, exportable as M3U8. Smart playlists hold whatever matches a set of rules, and Navidrome's `.nsp` files and `.m3u` files in the library folders are imported. See [Smart playlists](https://koan.rocks/docs/smart-playlists/)
- **Sleep timer** -- after a set time, fading out, or at the end of the track or record; it pauses and keeps the queue, and is set on whichever device is playing. See [Sleep timer](https://koan.rocks/docs/sleep-timer/)
- **Play history and Recently played** -- shared between an account's devices on a kōan server
- **ReplayGain** -- track and album modes with peak limiting and configurable pre-amp
- **EQ and convolution** -- parametric EQ (AutoEQ profiles import directly) and FIR room correction, chosen per output device. Impulse responses are used at their own rate, one per rate; a track without one is resampled to the nearest, and the badge says so. Off, or on a device without a profile, nothing runs. See [Equalisation and convolution](https://koan.rocks/docs/dsp/)
- **Format strings** -- fb2k-compatible `%field%`, `[conditionals]`, `$functions()` — 59 of them — for display and file organization
- **File organization** -- rename/reorganize your library from the macOS app or the TUI using format string patterns
- **GraphQL API** -- alongside the app and TUI, or headless. Relay pagination, filters, and mutations for playback, the queue, favourites, playlists and the library
- **MCP server** -- a server serves MCP at `/mcp` with its own OAuth sign-in, acting as the signed-in account; `koan mcp` runs the player for a desktop client over stdio
- **Queue management** -- undo/redo (100-deep), multi-select, drag-reorder, Finder drag & drop, session persistence
- **SQLite FTS5 search** -- full-text search across your entire library
- **Media keys** -- macOS Control Center and Linux MPRIS (play/pause, next/prev, now playing info)
- **Lyrics** -- synced (LRC) and plain lyrics from LRCLIB, current line highlighting
- **22 visualizer modes** -- spectrum bars, oscilloscope, radial, particles, lissajous, spectrogram, stereo waveform, VU meter, flame, plasma, tunnel, wireframe, metaballs, starfield, terrain, moiré, kaleidoscope, julia fractal, spiral, interference, wormhole, matrix rain. Picker with live preview (`v`), matrix overlay (`X`), bass shake (`S`), configurable reactivity

<img width="815" height="598" alt="Screenshot 2026-03-04 at 18 30 43" src="https://github.com/user-attachments/assets/9dab1d13-5d48-4e60-8625-7d72dd2e7957" />

## How it compares

[koan.rocks/compare](https://koan.rocks/compare/) has feature tables against Subsonic, Jellyfin and Plex apps on Apple platforms, desktop players, room-correction software and self-hosted servers. The tables below cover terminal and desktop players only.

No TUI player combines bit-perfect audio, Subsonic streaming, album art, fb2k-style format strings, and file organization in one binary. Most either need a daemon, lack remote support, or skip the audiophile bits.

### TUI / terminal players

| | kōan | ncmpcpp | cmus | musikcube | termusic | rmpc | stmp |
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
| **Tag editing** | No | Via MPD | No | Yes | Yes | Via MPD | No |
| **DSP / EQ** | **EQ + FIR** | Via MPD | Yes | Yes | No | Via MPD | No |
| **Auth** | **JWT + roles** | No | No | No | No | No | No |
| **Platforms** | macOS, Linux | Linux/macOS | Linux/macOS/BSD | Linux/macOS/Win | Linux/macOS/Win | Linux/macOS | Linux/macOS |


### Desktop players (GUI)

| | kōan | foobar2000 | Strawberry | DeaDBeeF |
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
| **Tag editing** | No | **Yes** | Yes | **Yes** |
| **DSP / EQ** | **EQ + FIR, per device** | **Yes (VST)** | Yes | Yes |
| **Platforms** | macOS (app + TUI), Linux (TUI) | Windows/macOS | All | All |

<img width="768" height="612" alt="Screenshot 2026-03-04 at 18 31 01" src="https://github.com/user-attachments/assets/0ad4879e-815f-42f3-8ebe-f6d01616bc96" />

## Documentation

At [koan.rocks/docs](https://koan.rocks/docs/), rendered from [`docs/`](docs). Start with [Getting started](https://koan.rocks/docs/getting-started/), [Running a server](https://koan.rocks/docs/headless-server/) or [Migrating from Navidrome](https://koan.rocks/docs/migrating-from-navidrome/).

## Architecture

```
File -> Symphonia -> f32 samples -> rtrb ring buffer -> CoreAudio/cpal callback -> DAC
```

Five crates: `koan-core` (audio engine, player, database, indexer), `koan-tui` (Ratatui TUI, visualizers, media keys), `koan-server` (GraphQL, Subsonic REST, MCP), `koan-ffi` (uniffi bindings for native clients), and `koan-cli` (the `koan` binary). See [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical manual.

## macOS app

A SwiftUI app in [`apps/macos`](apps/macos). It links `koan-core` in-process through `koan-ffi` rather than talking to a server, and shares one library and config with the TUI.

```bash
just macos-run     # build and launch
just macos-dmg     # package for release
```

Requires Swift 6 and macOS 26+.

## iOS app

<img alt="Now Playing, lyrics, a record and the queue on iPhone" src="docs/images/koan-ios.png" />

The same SwiftUI app and engine in a phone's shell, playing from a server. Output crosses the system mixer, so bit-perfect is a claim for the Mac and Linux only.

```bash
just ios-run      # build and launch on a simulator
just ios-phone    # install on the iPhone plugged in, signed with your personal team
just ios-walk     # screenshot every page on a simulator
```

Requires iOS 26+.

## Playing on another device

Any kōan app can control another, and hand its queue to it. The Mac app and the TUI also play to UPnP/DLNA amplifiers and streamers: the original files, or, for an amplifier with its own EQ or convolution profile, one stream koan has processed. See [Playing on another device](https://koan.rocks/docs/devices/).

## Planned

- **Tag editing** -- inline editing, bulk operations, vimv-style external editor ([plan](/.claude/plans/04-tagging.md))
- **Similar artists** -- from MusicBrainz/Last.fm ([plan](/.claude/plans/09-artist-metadata.md))

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
