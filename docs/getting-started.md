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

On an iPad with room for it, the iOS app's sidebar is the Mac's: Queue, the library's sections, the playlists and Settings, each opening its page beside it. Narrower, in Split View or Slide Over, the app folds back into the iPhone's tabs, with the sections under Library.

### Sample rates on iOS

The Mac switches the output device to each track's sample rate, so playback is bit-perfect. On iOS koan asks the audio session for each track's rate, and the route decides. A USB DAC (USB-C, or Lightning with the Camera Adapter) that supports the track's rate is switched to it, so there is no resampling in koan or iOS. The built-in speaker stays at its own rate, Apple's headphone adapters go to 48 kHz at most, and iOS resamples anything they do not take. Bluetooth is lossy (AAC, or SBC on other headphones) and never bit-perfect, and AirPlay carries 16-bit/44.1 kHz Apple Lossless.

The format badge shows the rate the hardware runs at, read back from the session, so a route that did not take the track's rate shows as a conversion. The Apple TV app does the same; over HDMI the rate is the one the television or receiver negotiates.

## History and Recently played

Both apps keep what you play. **History** lists every play by day, and is where a play is forgotten. **Recently Played**, beside it in the Mac's and the iPad's sidebar and the iPhone's Library tab, answers "what was that record I had on yesterday": the records, artists and tracks of the last 30 days, each once however often it played, newest first. Both follow each play as it is recorded.

Recently Played and Favourites show the first few of each kind. Each section's heading gives how many there are in all and opens the Albums, Artists or Tracks browser filtered to the shelf, in the shelf's order; the filter shows in the browser's filter control and is cleared there. Search's sections do the same. On the Mac, **Tracks**, beside Albums and Artists, lists every track in the library with the same filters; the iOS and Apple TV apps reach the track listing only filtered, from a "See all", since the whole library as one list is more than a phone or a television should draw and queue at once. On Apple TV the Albums, Artists, History and Playlists pages narrow by name from a field above the listing, as the other apps' filter fields do.

## Downloaded and offline

**Downloaded**, in the Mac's and the iPad's sidebar and the iPhone's Library tab, is a shelf like Favourites: the artists, records and tracks with files on this device, records fully there first, each with a bar along the foot of its sleeve showing how much of it is. On the Mac every record in the library folder is whole.

On iOS, when the server cannot be reached for a few seconds while the app is open, kōan goes offline: every list, from Albums and search to Favourites and playlists, shows only what is on the device, the queue greys out tracks that are not, and the Library tab (on an iPad, the sidebar) says so. It goes back online by itself when the server answers again. **Settings → Offline mode** turns it on by hand, for a train with a signal that comes and goes. A server without kōan's link (Navidrome) gives no signal for this, so there only the switch applies.

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

A scan removes tracks whose files have gone, along with their play history. It refuses when the pattern looks like a missing mount rather than a deletion: a folder with no audio files, a path it cannot read, or more than 20% of a folder of at least 100 tracks gone at once. `koan scan --force-remove` lifts the last of these after a deliberate mass deletion. A file behind a symlink whose target has gone, such as an unmounted network share linked into the library, counts as unreachable rather than deleted.

A file moved or renamed outside kōan keeps its track, and with it its play history, favourites and playlist places, when the scan that finds it at the new path also finds it gone from the old one: it must be the same MusicBrainz recording, the same disc and number on the same release with the same size or length, or, untagged, the same size, length and modification time. When more than one missing file fits, none is taken, and the new file is indexed as a new track. A full scan matches moves between library folders too; the folder watcher matches them within one batch of changes.

On Linux, a file whose name is not valid UTF-8 is skipped with a warning, since its path cannot be stored as it is.

## Local and remote together

A track present both locally and on a server is one entry in the library, matched on artist, album, disc, track number and title, or on MusicBrainz recording and release. The local file plays; the remote copy is the fallback when the drive is not mounted.
