# The desktop app

`koan-desktop` is kōan for macOS and Linux, drawn with GPUI in the kōan look on every platform. It links the engine in the same process, as the Mac app does, and shares its library and configuration (`~/.config/koan`, or `KOAN_CONFIG_DIR`). It is in progress: the queue and the transport are there, and the library pages follow. Windows follows once the core builds there.

```bash
cargo run --release -p koan-desktop
```

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
