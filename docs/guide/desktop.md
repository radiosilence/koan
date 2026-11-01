# The desktop app

kōan's desktop app runs on macOS and Linux. It is drawn with GPUI and links the same engine the native apps do, so it plays bit-perfect through CoreAudio on a Mac and through ALSA, PipeWire or PulseAudio on Linux. Windows is waiting on the engine building there ([#1600](https://github.com/radiosilence/koan/issues/1600)).

## Installing

Each release carries a DMG for macOS 26 or later on Apple silicon, and an AppImage and a `.deb` for Linux on x86-64 and ARM64, which need glibc 2.35 or later (Ubuntu 22.04, Debian 12 and their successors). On a Mac it installs as **kōan desktop**, beside the native app rather than over it.

## Beside the other apps

On one machine the desktop app, the macOS app and the terminal UI can all run at once on the same library. Each is a device of its own to the server and the network, named for what it is (`studio (desktop)`, `studio (terminal)`), so one can control another and none takes another's place. Only one desktop app runs per configuration directory: opening it again, or opening files with it, brings the running one to the front and hands it the files.

The desktop app keeps its own saved session, the queue and where it had got to, apart from the macOS app's and the terminal UI's on the same configuration directory. It picks it up at launch and saves it as it goes and as it quits, so the apps can run side by side without one replacing the other's queue.

## Files from outside

Files and folders dropped on the queue are added to the library where they lie and appended to the queue; dropped on a playlist, they are added to it. Folders are walked, so dropping a rip queues the album. On Linux, files opened with the app from the file manager, and on both, files named on the command line (`koan-gui ~/Music/new/*.flac`), are queued the same way. Nothing is moved: [organize](file-organization.md) is what puts them under a library folder.

## Media keys and now playing

The app shows what is playing in the system's controls, cover included, and takes their commands: Now Playing and the media keys on a Mac, MPRIS on Linux, so `playerctl` and the desktop's own media controls work. While it controls another device, the media keys control that device.

## Keys

The macOS app's shortcuts, with `Ctrl` for `⌘` on Linux. See [Keybindings](../reference/keybindings.md#desktop-app), or press `?` in the app.

## Configuration and the log

The app reads the configuration every kōan front end does, `~/.config/koan/` (or `KOAN_CONFIG_DIR`), and writes its log to `koan.log` there, and to stderr when started from a terminal.

## Running a build from source

```bash
KOAN_CONFIG_DIR=$(mktemp -d) cargo run --release -p koan-gui
```

A build from `main` can move the library database to a newer schema than the released apps read, after which they refuse it; a throwaway `KOAN_CONFIG_DIR`, with `folders` in its `config.toml` pointed at some music, keeps your own library out of it.

## The window

The sidebar lists the queue, the library's sections and your playlists. Back and forward move through the pages visited, as a browser does, from the toolbar, with ⌘[ and ⌘] (Ctrl on Linux), or with a mouse's back and forward buttons. The transport runs along the foot: the record playing, the controls over the seek slider, the format, and the sleep timer and output menus. The window opens where it was left.

## The queue

The queue is grouped by record: a heading with the sleeve over each record's run, and a track standing alone with its own sleeve. The playing track is in the accent colour.

| Action | How |
|--------|-----|
| Play a track | Double-click it, or select it and press Return |
| Play or pause | Space, anywhere in the window |
| Select | Click; ⌘-click adds or removes a row, Shift-click selects a range. A heading selects its record |
| Move the selection | Up and Down; with Shift, extend it |
| Select everything | ⌘A |
| Remove | Delete or Backspace, or the context menu |
| Reorder | Drag rows; dragging a heading moves its record, and a selected row moves the whole selection |
| Undo, redo | ⌘Z, ⇧⌘Z |

The queue keeps its place and selection when you visit another page and come back.

## Settings

⌘, (Ctrl+, on Linux), or kōan → Settings…, opens Settings in a window of its own; `koan-gui --settings` opens it at launch. It holds what the Mac app's Settings does, in the same tabs: Library, Server, Account, People, Playback, Devices, Integrations and Appearance, the middle ones where a server you are signed in to offers them. EQ is not there yet.

Settings reads the configuration again when its window opens and whenever it comes to the front, so a change made in the terminal UI or the CLI shows rather than being overwritten. A change writes only the setting it changes. Anything that removes or replaces something (a folder's tracks, the library index, downloads, a key, an account, sharing) says what it will do and asks first. The line along the foot says what is under way, and then how it went.
