# Getting started

kōan plays a music folder on this machine, a server's library, or both merged into one. Which piece to install depends on where the music is:

- **On the computer you listen at:** the macOS app, or the terminal UI on macOS and Linux.
- **On another machine:** run the [server](guide/headless-server.md) there, and sign in from the apps, the browser or any Subsonic client.
- **On a Navidrome or Subsonic server you already run:** the apps and terminal UI play from it directly. See [Remote servers](guide/remote-servers.md), or [Migrating from Navidrome](guide/migrating-from-navidrome.md) to replace it.

## The macOS app

Download [`Koan.dmg`](https://github.com/radiosilence/koan/releases/latest/download/Koan.dmg) or install with Homebrew:

```bash
brew install --cask radiosilence/koan/koan-app
```

Everything is set up in Settings: **Library** adds folders and scans them, **Server** signs in to a kōan, Navidrome or Subsonic server. It needs macOS 26 or later, and shares its config and library with the terminal UI.

## The iOS app

The iOS app plays from a server; a phone has no music folder to scan. Sign in under Settings → Server, or open an invite link from the server's admin. It needs iOS 26 or later.

## History and Recently played

Both apps keep what you play. **History** lists every play by day, and is where a play is forgotten. **Recently Played**, beside it in the Mac's sidebar and the iOS Library tab, answers "what was that record I had on yesterday": the records, artists and tracks of the last 30 days, each once however often it played, newest first. Both follow each play as it is recorded.

Recently Played and Favourites show the first few of each kind. Each section's heading gives how many there are in all and opens the Albums, Artists or Tracks browser filtered to the shelf, in the shelf's order; the filter shows in the browser's filter control and is cleared there. Search's sections do the same. **Tracks**, beside Albums and Artists, lists every track in the library with the same filters.

## The terminal UI

```bash
mise use -g github:radiosilence/koan@latest   # or: brew install radiosilence/koan/koan
                                              # or: cargo install koan-cli
```

Linux needs the ALSA and D-Bus headers to build: `libasound2-dev libdbus-1-dev` on Debian and Ubuntu, `alsa-lib-devel dbus-devel` on Fedora, `alsa-lib dbus` on Arch.

```bash
koan config init   # creates ~/.config/koan/ with a commented config.toml
```

Add your music to `~/.config/koan/config.local.toml`:

```toml
[library]
folders = ["/path/to/your/music"]
```

Then index it and start the player:

```bash
koan scan
koan
```

Or sign in to a server instead of, or as well as, scanning:

```bash
koan remote login https://music.example.com alice
koan remote sync
```

### Playing

`p` searches tracks, `a` albums, `r` artists, and `l` browses the library. In a picker, `Enter` adds to the queue, `Ctrl+Enter` adds and plays, and `Ctrl+R` replaces the queue. `space` pauses, `<` and `>` skip, `,` and `.` seek, and `e` edits the queue, with `Ctrl+Z` to undo. The hint bar shows the keys for the current mode; [Keybindings](reference/keybindings.md) lists them all.

`koan play` also takes paths: `koan play ~/Music/some-album/`.

### Shell completions

Completions read the library, so `koan play --album <TAB>` lists your albums:

```bash
source <(COMPLETE=zsh koan)    # zsh; bash and fish take COMPLETE=bash and COMPLETE=fish
```

## Removed files

A scan removes tracks whose files have gone, along with their play history. It refuses when the pattern looks like a missing mount rather than a deletion: a folder with no audio files, a path it cannot read, or more than 20% of a folder of at least 100 tracks gone at once. `koan scan --force-remove` lifts the last of these after a deliberate mass deletion.

## Local and remote together

A track present both locally and on a server is one entry in the library, matched on artist, album, disc, track number and title, or on MusicBrainz recording and release. The local file plays; the remote copy is the fallback when the drive is not mounted.
