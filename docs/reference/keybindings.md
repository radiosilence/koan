# Keybindings

Every key in every mode. The hint bar at the bottom of the TUI shows available keys for the current mode.

## Normal mode (default)

### Playback

| Key | Action |
|-----|--------|
| `space` | Pause / resume |
| `<` | Previous track |
| `>` | Next track |
| `,` or `<-` | Seek -10 seconds |
| `.` or `->` | Seek +10 seconds |
| `Shift+D` | Device selector (switch audio output) |

### Navigation

| Key | Action |
|-----|--------|
| `p` | Track picker (fuzzy search) |
| `a` | Album picker |
| `r` | Artist picker |
| `l` | Library browser |
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
| `L` | Toggle lyrics panel |
| `R` | Toggle radio mode |
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

## Library browser (`l`)

Tree view: artist -> album -> track.

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate |
| `Enter` | Expand node or enqueue track |
| `f` | Filter library (type to search) |
| `Esc` | Exit library browser |

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
