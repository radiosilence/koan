# Keybindings

Every key in every mode. The hint bar at the bottom of the TUI shows available keys for the current mode.

## Normal mode (default)

### Playback

| Key | Action |
|-----|--------|
| `space` | Pause / resume |
| `<` | Previous track |
| `>` | Next track |
| `s` | Shuffle on / off ([plays the tracks not yet played in a random order](../guide/shuffle-and-repeat.md); the queue keeps its order) |
| `R` | Repeat: off, the queue, one track |
| `T` | [Sleep timer](../guide/sleep-timer.md): 15, 30, 45, 60 minutes, end of track, end of record, off |
| `,` or `<-` | Seek -10 seconds |
| `.` or `->` | Seek +10 seconds |
| `Shift+D` | Device selector (switch audio output) |

### Pages

See [The terminal UI](../guide/terminal-ui.md). These work from any page.

| Key | Action |
|-----|--------|
| `1` to `8` | Queue, Now playing, Albums, Artists, Favourites, Recent, History, Settings |
| `Tab` / `Shift+Tab` | Next / previous page |
| `[` / `]` or `Alt+Left` / `Alt+Right` | Back / forward through the pages visited |
| `Esc` | Leave the page for the one before it |
| `I` | Draw covers as images or in half blocks |
| `E` | Choose the EQ the output plays through |
| `A` / `O` | Open the artist / the record of the selection |

### Navigation

| Key | Action |
|-----|--------|
| `p` | Track picker (fuzzy search) |
| `a` | Album picker |
| `r` | Artist picker |
| `l` | Artists page (the library tree) |
| `/` | Search queue (jump to matching track) |
| `g` | Jump to start of queue |
| `G` | Jump to end of queue |
| `PgUp` or `Ctrl+U` | Page up |
| `PgDn` or `Ctrl+D` | Page down |

### Modes & panels

| Key | Action |
|-----|--------|
| `e` | Enter queue edit mode |
| `i` | Track info modal (codec, sample rate, cover art) |
| `z` | Zoom album art |
| `v` | Visualizer picker (22 modes with live preview) |
| `V` | Show / hide the visualizer |
| `X` | Toggle matrix overlay on visualizer |
| `S` | Toggle bass shake on visualizer |
| `L` | Toggle lyrics panel |
| `f` | Favourite / unfavourite current track |
| `?` | Open help modal |
| `n` | Next track |
| `q` | Quit |

### Queue changes

| Key | Action |
|-----|--------|
| `Ctrl+Z` | Undo last queue change |
| `Ctrl+Shift+Z` | Redo last undone change |

---

## Picker mode (track/album/artist)

Active when a fuzzy picker overlay is open (`p`, `a`, or `r`).

| Key | Action |
|-----|--------|
| Type | Filter results (fuzzy matching) |
| `Up` / `Down` | Navigate results |
| `Enter` | Append selected to queue |
| `Ctrl+Enter` | Append and start playing |
| `Ctrl+R` | Replace entire queue and play |
| `Esc` | Close picker |

### Mouse in pickers

- Click items to select
- Double-click to confirm (same as `Enter`)
- Click outside the picker to dismiss

---

## Artists page (`4` or `l`)

Tree view: artist -> album -> track.

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate |
| `Enter` | Expand node, or append the track and play it |
| `a` | Append what is under the cursor to the queue |
| `N` | Put it next, after the track playing |
| `/` or `f` | Filter library (type to search) |
| `Esc` | Clear the filter, then leave the page |

---

## History (`7`)

| Key | Action |
|-----|--------|
| `↑` `↓` or `j` `k` | Move between plays (the day headings are skipped) |
| `Enter` | Play the track |
| `a` | Append the track to the queue |
| `N` | Put the track next, after the one playing |
| `i` | Show what the library holds of the track |
| `d` / `u` | Download the track to this device / remove the download |
| `A` / `O` | Open the artist / the record |
| `x`, `Delete` or `Backspace` | Forget the play |
| `D` | Forget every play (press again to confirm) |
| `/` | Filter by title, artist or record |

Click a play to select it and again to play it; click the artist or the record to open it, `×` to forget the play, `forget all` to arm the second press, `/ filter` to filter. The wheel and the scrollbar scroll.

---

## Settings (`8`)

| Key | Action |
|-----|--------|
| `↑` `↓` or `j` `k` | Move between settings |
| `→`, `Enter` or `Space` | Next value (flips a toggle), or run the task on the row |
| `←` | Previous value |

Click the arrows beside a value, or the value, to change it, or `run` to start a task. Changes apply at once, except the frame rate, which applies at the next start. `scan library` and `sync now` run in the background and report how they went in the status line; offline mode narrows the library to what is on this device.

---

## Albums, Favourites and Recent (`3`, `5`, `6`)

| Key | Action |
|-----|--------|
| Arrows or `h` `j` `k` `l` | Move through the tiles or rows |
| `PgUp` / `PgDn`, `g` / `G` | Page, start, end |
| `Enter` or `O` | Open the record (a track row plays the track) |
| `A` | Open the artist |
| `P` | Append the record (or track) and play it |
| `a` | Append to the queue |
| `N` | Put it next, after the track playing |
| `Ctrl+R` | Replace the queue and play |
| `/` | Filter what is shown |
| `v` | Tiles or rows (tracks, on Favourites and Recent) |
| `o` | Cycle the order (Albums) |
| `f` | Favourite the record under the cursor, or the track on a track row |
| `H` | Favourite the artist (on an artist's page, that artist) |
| `i` | Show what the library holds of the track (track rows) |
| `d` / `u` | Download the record or track to this device / remove the download |

### Mouse

| Action | Effect |
|--------|--------|
| Click | Select |
| Second click | Open the record |
| Click a title or artist under a tile | Open the record / the artist |
| Right-click | Append to the queue (also `a`, and the hint bar) |
| Wheel | Scroll |
| Click `sort:` or `view:` in the heading | Cycle the order, switch tiles and rows |
| Click the sidebar or tab bar | Open that page, or go back or forward |
| Click a key hint along the foot | The same as pressing that key |
| Drag the scrollbar | Scroll |

---

## Queue edit mode (`e`)

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate |
| `Shift+Up` / `Shift+Down` or `J` / `K` | Extend selection |
| `d` | Remove selected track(s) |
| `j` | Move selected down |
| `k` | Move selected up |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |
| `space` | Context menu (organize) |
| `g` | Jump to start |
| `G` | Jump to end (shift-extends selection) |
| `PgUp` / `PgDn` | Page up / page down |
| `Esc` | Exit edit mode |

### Mouse in edit mode

| Action | Effect |
|--------|--------|
| Click | Select track |
| `Option`-click | Toggle individual selection |
| `Ctrl`-click | Range select |
| Drag | Reorder selected tracks |
| Drag group | Move all selected together |

---

## Mouse (works in any mode)

Mouse controls are always available -- keyboard modality only affects keyboard shortcuts.

| Action | Effect |
|--------|--------|
| Double-click queue track | Skip to that track (forward or backward) |
| Double-click downloading track | Prioritize download and play when ready |
| Click seek bar | Jump to position |
| Scroll wheel in queue | Scroll queue |
| Click scrollbar | Jump scroll position |
| Drag scrollbar | Drag scroll position |
| Single-click in queue | Select track |
| Drag in queue | Reorder |

### Finder drag & drop

Drag files or folders from macOS Finder into the terminal window to add them to the queue.

---

## Queue display reference

Tracks are grouped by album with headers showing album artist, year, album title, and codec. Track artist is shown inline only when it differs from the album artist (compilations, VA albums). Downloading tracks show progress percentage, waiting tracks show braille spinners.

```
 Limewax -- (2007) Therapy Session 4 [FLAC]
   > 01 Agent Orange                              4:56
     02 Pigeons and Marshmellows feat. The Panacea 2:53
     03 SPL -- Fade                                1:52
     04 Icicle                                     2:27
```

---

## macOS app

Two kinds of shortcut. Anything with ⌘ is a menu command and shows in the menu bar; the single-key ones follow the TUI and are listed under Help ▸ Keyboard Shortcuts (`?`).

Single keys never fire while a text field has focus, so typing an `f` into the search box types an f. `Esc` gets you back out of a field.

### Single keys

| Key | Action |
|-----|--------|
| `space` | Play / pause |
| `<` | Previous track |
| `>` or `n` | Next track |
| `,` | Back 10 seconds |
| `.` | Forward 10 seconds |
| `f` | Favourite the current track |
| `p` | Add music (the ⇧⌘K picker) |
| `/` | Search the library |
| `l` or `a` | Albums |
| `r` | Artists |
| `g` / `G` | Top / end of the queue |
| `L` | Lyrics panel |
| `z` | Zoom the cover |
| `?` | The shortcuts list |

### Menu commands

| Key | Action |
|-----|--------|
| `⌘K` | Search the library |
| `⇧⌘K` | Add music |
| `⌘1`–`⌘5` | Queue, Albums, Artists, Favourites, History |
| `⌘[` / `⌘]` | Back / forward |
| `⌘F` | Filter what's on screen (albums, artists), otherwise search the library |
| `⌘←` / `⌘→` | Previous / next track |
| `⌥←` / `⌥→` | Seek ∓10 seconds |
| `⌘D` | Favourite the current track |
| `⌥⌘L` | Lyrics panel |
| `⌘+` (or `⌘=`) / `⌘−` / `⌘0` | Bigger / smaller / actual text size (kōan theme) |
| `⌘Z` / `⇧⌘Z` | Undo / redo a queue change |
| `⌘X` `⌘C` `⌘V` `⌫` `⌘A` | Queue editing, or the ordinary thing while typing |
| `⇧⌘R` | Rescan local folders |
| `⌘/` | Keyboard shortcuts |

Keys the TUI spends on its own modes — edit mode, the visualizer, the help overlay — have no counterpart: the queue is always editable here, and there is no visualizer.
