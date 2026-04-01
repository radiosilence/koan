# Getting Started

This guide walks you through installing koan, setting up your music library, and playing your first track.

## Install

### Homebrew (recommended)

```bash
brew install radiosilence/koan/koan
```

### mise

```bash
mise use -g github:radiosilence/koan@latest
```

### Cargo

```bash
cargo install koan-music
```

### Build from source

```bash
git clone https://github.com/radiosilence/koan.git && cd koan
cargo install --path crates/koan-music
```

### Linux dependencies

macOS works out of the box (CoreAudio). Linux needs ALSA and D-Bus dev headers:

```bash
# Debian/Ubuntu
sudo apt install libasound2-dev libdbus-1-dev

# Fedora
sudo dnf install alsa-lib-devel dbus-devel

# Arch
sudo pacman -S alsa-lib dbus
```

## Create your config

```bash
koan config init
```

This creates `~/.config/koan/` with:

| File | Purpose |
|------|---------|
| `config.toml` | Commented template -- all defaults shown as comments, uncomment to customize. Safe to commit to dotfiles |
| `config.local.toml` | Machine-specific settings (library paths, credentials) -- gitignored |
| `.gitignore` | Ignores logs, database, local config, cache |
| `koan.db` | SQLite database (created on first use) |
| `cache/` | Download cache for remote tracks |

Running `koan config init` on an existing setup is safe -- it merges new defaults without overwriting your customizations, and skips `config.local.toml` if it already exists. `[library]` and `[remote]` sections are excluded from `config.toml` (they belong in `config.local.toml`).

## Add your music

koan needs at least one music source -- local files, a remote server, or both.

### Option A: Local files

Edit `~/.config/koan/config.local.toml`:

```toml
[library]
folders = ["/path/to/your/music"]
```

Then scan your library:

```bash
koan scan
```

Scanning runs in parallel -- fast even for large collections (tens of thousands of tracks). koan reads metadata from FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV, and AIFF files.

### Option B: Remote server (Navidrome/Subsonic)

If you run [Navidrome](https://www.navidrome.org/), Subsonic, or anything with a Subsonic-compatible API:

```bash
koan remote login https://music.example.com admin
koan remote sync
```

The first sync fetches your entire library. Subsequent syncs are **incremental** -- only new albums since the last sync. Use `--full` to force a complete re-sync.

See [Remote Servers](guide/remote-servers.md) for the full setup guide.

### Option C: Both

Use both together. Local and remote tracks merge seamlessly -- if the same track exists in both sources (matched by artist + album + title + track number), it becomes a single entry. Local files always take playback priority; remote tracks stream on demand and cache locally.

Run `koan remote sync` periodically (or after adding music to your server) to pull new tracks.

## Play something

```bash
# Launch the TUI
koan

# Play files or directories directly
koan play ~/Music/Aphex\ Twin/
koan play ~/Music/album/*.flac

# Play by album or artist ID (use tab completion)
koan play --album 5
koan play --artist 3
```

The TUI launches immediately. If tracks need downloading (remote library), they appear in the queue with animated spinners while loading in the background.

## Your first session

The TUI has three areas: a **transport bar** at the top (album art, track info, seek bar, spectrum analyzer), a **content area** in the middle (queue, library, or picker), and a **hint bar** at the bottom showing available keys.

### Finding music

| Key | What it opens |
|-----|--------------|
| `p` | Track picker -- fuzzy search across all tracks |
| `a` | Album picker -- fuzzy search by album name |
| `r` | Artist picker -- fuzzy search by artist |
| `l` | Library browser -- tree view (artist -> album -> track) |

Type to filter. In any picker, press `Enter` to add to queue, `Ctrl+Enter` to add and start playing, or `Ctrl+R` to replace the entire queue.

### Controlling playback

| Key | Action |
|-----|--------|
| `space` | Pause / resume |
| `<` `>` | Previous / next track |
| `,` `.` or `<-` `->` | Seek +/-10 seconds |
| `i` | Track info (codec, sample rate, bit depth, cover art) |
| `L` | Toggle lyrics panel |
| `f` | Favourite / unfavourite |
| `R` | Toggle radio mode (infinite play) |

### Managing the queue

Press `e` to enter edit mode. Select tracks with shift-arrows, `d` to delete, `j`/`k` to reorder. `Ctrl+Z` undoes any queue change (100-deep stack). `Esc` to exit edit mode.

Mouse works everywhere too -- click, drag, scroll wheel. Double-click a track to jump to it.

See [Keybindings](reference/keybindings.md) for the complete key reference.

## Shell completions

Dynamic completions that know your library -- artist/album IDs tab-complete from the database.

```bash
# zsh (add to .zshrc)
source <(COMPLETE=zsh koan)

# bash
source <(COMPLETE=bash koan)

# fish
COMPLETE=fish koan | source
```

Then `koan play --album <TAB>` shows your actual albums with artist names.

## What's next?

- **[Configuration](reference/configuration.md)** -- customize playback, visualizer, organize patterns, and more
- **[Radio Mode](guide/radio-mode.md)** -- let koan pick tracks for you
- **[File Organization](guide/file-organization.md)** -- rename your library using format string patterns
- **[Remote Servers](guide/remote-servers.md)** -- advanced Subsonic/Navidrome setup
- **[GraphQL API](guide/graphql-api.md)** -- programmatic control and headless operation
