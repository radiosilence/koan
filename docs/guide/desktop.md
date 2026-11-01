# The desktop app

`koan-gui`, the kōan GUI app, is kōan for macOS and Linux, drawn with GPUI in the kōan look on every platform. It links the engine in the same process, as the Mac app does, and shares its library and configuration (`~/.config/koan`, or `KOAN_CONFIG_DIR`). It is in progress: the queue and the transport are there, and the library pages follow. Windows follows once the core builds there.

```bash
KOAN_CONFIG_DIR=$(mktemp -d) cargo run --release -p koan-gui
```

A build from `main` can move the library database to a newer schema than the released apps read, after which they refuse it; a throwaway `KOAN_CONFIG_DIR`, with `folders` in its `config.toml` pointed at some music, keeps your own library out of it.

On macOS, do not run it on the Mac app's configuration while the Mac app is running: for now it signs in as the same device, so the two take each other's place on your server and the network, and share one saved session.

On Linux it needs the xkbcommon, fontconfig and freetype libraries, and a Vulkan driver.

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
