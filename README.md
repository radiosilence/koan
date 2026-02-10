# kōan

A music player for people who give a shit about audio quality.

macOS-native (SwiftUI shell, Rust core). Bit-perfect playback, gapless transitions, fast library indexing, Subsonic/Navidrome integration. No Electron. No subscriptions. No bullshit.

## What works

- **Bit-perfect playback** — CoreAudio AUHAL, no resampling, automatic device sample rate switching
- **Gapless** — decode thread keeps the ring buffer alive across track boundaries, AudioUnit never stops
- **Format support** — FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV/AIFF (via Symphonia)
- **Library indexing** — parallel metadata scanning with rayon, SQLite FTS5 full-text search
- **File watching** — FSEvents via notify, debounced 500ms, auto-updates DB on changes
- **Subsonic/Navidrome** — parallel remote library sync (rayon), unified local+remote browsing, Keychain credential storage
- **CLI** — play, scan, search, probe, device listing, remote management

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

## Usage

```bash
# play files (gapless across tracks)
koan play ~/Music/album/*.flac

# scan library (path or configured folders)
koan scan /path/to/music
koan scan              # uses folders from config

# search (FTS5, prefix matching)
koan search "radiohead"

# library stats
koan library

# show config sources + resolved values
koan config

# list audio devices
koan devices

# probe file format
koan probe track.flac
```

### Playback controls

| Key             | Action         |
| --------------- | -------------- |
| `space`         | pause / resume |
| `n`             | next track     |
| `< >` or arrows | seek ±10s      |
| `q`             | quit           |

### Remote (Subsonic/Navidrome)

```bash
# authenticate (password stored in macOS Keychain)
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
