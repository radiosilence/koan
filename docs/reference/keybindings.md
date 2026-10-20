# Keybindings

The keys of the terminal UI, page by page, and of the macOS app. The hint bar along the foot of the TUI shows the keys for the page in view, and `?` lists them all.

## Every page

These work on any page whose own keys leave them free, and never while a text field has the keys. A page takes some of them for itself: `S` shuffles an album or playlist page, `D` on History forgets every play (after asking), `X` on a playlist's page deletes it, and `D` and `R` on the EQ page delete and reset the profile.

### Playback

| Key | Action |
|-----|--------|
| `space` | Pause / resume |
| `<` | Previous track |
| `>` or `n` | Next track |
| `,` or `←` | Seek back 10 seconds (`←` only where the page does not move its cursor with it) |
| `.` or `→` | Seek forward 10 seconds (likewise) |
| `s` | Shuffle on / off ([plays the tracks not yet played in a random order](../guide/shuffle-and-repeat.md); the queue keeps its order) |
| `R` | Repeat: off, the queue, one track |
| `T` | Sleep timer: 15, 30, 45, 60 minutes, end of track, end of record, off |
| `D` | Output device |
| `C` | Play on: control another kōan, see its queue, or move the music between them |
| `E` | The EQ page (also a click on `EQ` in the transport bar, the signal path's `dsp` row or the curve on Now playing) |
| `B` | Bypass the EQ, or put back the one bypassed |

### Pages

See [The terminal UI](../guide/terminal-ui.md).

| Key | Action |
|-----|--------|
| `1` to `9`, `0` | Queue, Now playing, Albums, Artists, Favourites, Recent, History, Playlists, Settings, Search |
| `Tab` / `Shift+Tab` | Next / previous page in the sidebar's order |
| `[` / `]` or `Alt+←` / `Alt+→` | Back / forward through the pages visited |
| `Esc` | Leave the page for the one before it |
| `I` | Draw covers as images or in half blocks |

Tracks, Downloaded, Downloads and the EQ page have no number; they are reached from the sidebar, or `E` for the EQ.

### Pickers, panels and the visualiser

| Key | Action |
|-----|--------|
| `p` | Track picker (fuzzy search) |
| `z` | Zoom the cover |
| `L` | Lyrics panel |
| `V` | Visualiser picker: off, or one of 22 modes, with a live preview |
| `M` | Next visualiser mode |
| `F` | Visualiser full screen |
| `S` | Bass shake on / off (on an album or playlist page, `S` shuffles) |
| `X` | Matrix overlay on / off |
| `w` / `W` | On Now playing: put the playing track in a playlist (other pages' `w` uses what they show) |
| `?` | Help |
| `q` | Quit |

---

## Queue (`1`)

There is no mode to enter: the cursor and the selection are always live, and the queue's keys act on the selection, or with none on the row under the cursor.

| Key | Action |
|-----|--------|
| `↑` `↓` or `j` `k` | Move the cursor |
| `Shift+↑` / `Shift+↓`, `Shift+PgUp` / `Shift+PgDn` | Extend the selection |
| `t` | Tag the row under the cursor, or let it go |
| `Ctrl+A` / `Esc` | Select everything / let the selection go |
| `Enter` | Play the row |
| `J` / `K` | Move the selection down / up |
| `d`, `Delete` or `Backspace` | Remove the selection |
| `m` | The selection's menu |
| `v` | Albums, or a row per track |
| `o` | Scroll to what is playing |
| `Ctrl+Z` / `Ctrl+Y` (or `Ctrl+Shift+Z`) | Undo / redo |
| `Ctrl+X` | Clear the queue |
| `g` / `G` | Jump to start / end |
| `PgUp` / `PgDn`, `Ctrl+U` / `Ctrl+D` | Page up / page down |
| `a` / `r` | Album / artist picker |
| `l` | The Artists page |
| `/` | Jump to a track in the queue |
| `i` | Track info |
| `f` | Favourite the track under the cursor |
| `A` / `O` | Open the artist / the record of the selection |
| `w` / `W` | Put the selection in a playlist / save the queue as one |

| Action | Effect |
|--------|--------|
| Click | Select the track |
| `Option`-click | Add or take away one track |
| `Shift`- or `Ctrl`-click | Select a range |
| Drag | Move the selection, as one block |
| Double-click | Skip to that track; on one still downloading, fetch it first and play it when ready |

---

## Pickers (`p`, `a`, `r`)

| Key | Action |
|-----|--------|
| Typing | Filter the results (fuzzy matching) |
| `↑` / `↓` | Move through the results |
| `←` / `→` | Tracks, albums or artists |
| `Tab` | Pick the row, or let it go |
| `Enter` | Add to the queue |
| `Ctrl+Enter` | Add and play |
| `Ctrl+R` | Replace the queue and play |
| `Esc` | Close |

Click a result to select it and double-click to add it; a click outside the picker closes it.

---

## Albums, Artists, Tracks and an artist's page (`3`, `4`)

| Key | Action |
|-----|--------|
| Arrows or `h` `j` `k` `l` | Move through the tiles or rows |
| `PgUp` / `PgDn`, `g` / `G` | Page, start, end |
| `Enter` | Open the record or artist (a track row plays the track) |
| `O` / `A` | Open the record / the artist |
| `P` | Append and play |
| `a` | Append to the queue |
| `N` | Put it next, after the track playing |
| `Ctrl+R` | Replace the queue and play |
| `f` | Favourite the record, or the track on a track row |
| `H` | Favourite the artist (on an artist's page, that artist) |
| `i` | What the library holds of the track (Tracks) |
| `w` | Put it in a playlist |
| `d` / `u` | Download it to this device / remove the download |
| `m` | Menu of everything that can be done with it |
| `/` | Filter by name |
| `Ctrl+F` | The filters panel: lossless, downloaded, codec, genre, decade |
| `o` | Cycle the order (Albums, Tracks) |
| `S` | Shuffle again, while albums are in random order |
| `v` | Tiles or rows (Albums, an artist's page) |

Playing from a row asks before replacing a queue that has music in it.

### Mouse

| Action | Effect |
|--------|--------|
| Click | Select |
| Second click | Open the record |
| Click a title or artist under a tile | Open the record / the artist |
| Right-click | Append to the queue |
| Wheel | Scroll |
| Click `sort:` or `view:` in the heading | Cycle the order, switch tiles and rows |
| Click `⋯ actions` in the heading | Open the menu for what the cursor is on (also `m`) |
| Click the sidebar or tab bar | Open that page, or go back or forward |
| Click a key hint along the foot | The same as pressing that key |
| Drag the scrollbar | Scroll |

---

## Favourites, Recent and Downloaded (`5`, `6`)

The shelves list artists, records and tracks in sections.

| Key | Action |
|-----|--------|
| Arrows or `h` `j` `k` `l` | Move through the rows and tiles |
| `Enter` | Open an artist or a record; play a track |
| `O` / `A` | Open the record / the artist |
| `P` / `a` / `N` | Play / append / put next |
| `Ctrl+R` | Replace the queue and play |
| `f` / `H` | Favourite it / its artist |
| `i` | Track info |
| `w` | Put it in a playlist |
| `d` / `u` | Download it / remove the download |
| `m` | Menu |
| `v` | Tiles or rows |

---

## An album's page

| Key | Action |
|-----|--------|
| `↑` `↓` or `j` `k` | Move |
| `Enter` | Play from the track under the cursor |
| `P` / `S` | Play the record / play it shuffled |
| `a` / `N` | Append the record / put it next |
| `Ctrl+R` | Replace the queue with it |
| `f` | Favourite the track |
| `H` | Favourite the record |
| `i` | Track info |
| `w` | Put the record in a playlist |
| `d` / `u` | Download the record / remove the download |
| `m` | Menu |
| `A` | Open the artist; on a record credited to several, choose which |

---

## Search (`0`)

| Key | Action |
|-----|--------|
| Typing | Search the library; every key is text while the box has focus |
| `Enter`, `↓` or `Tab` | Move to the results; `Tab` with none, and `Shift+Tab`, change page |
| `Esc` | Clear the box, then leave the page |
| `/` | Back to the box |
| `Enter` | Open an artist or a record; play a track |
| `P` / `a` / `N` | Play / add to the queue / put next |
| `w` | Put the result in a playlist |
| `f` | Favourite it |
| `A` / `O` | Open its artist / its record |
| `i` | Track info (tracks) |
| `v` | Tiles or rows |
| `m` | Menu of everything above |

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
| `w` | Put the track in a playlist |
| `m` | Menu of everything that can be done with the track |
| `A` / `O` | Open the artist / the record |
| `x`, `Delete` or `Backspace` | Forget the play |
| `D` | Forget every play (press again to confirm) |
| `t` | Pick out the play, or put it back |
| `Shift+↑` `Shift+↓` | Pick out a run of plays |
| `Ctrl+A` | Pick out every play shown |
| `/` | Filter by title, artist or record |

Click a play to select it and again to play it; click the artist or the record to open it, `×` to forget the play, `clear…` to arm the second press, `/ filter` to filter. The wheel and the scrollbar scroll.

---

## Playlists (`8`)

| Key | Action |
|-----|--------|
| `↑` `↓` or `j` `k` | Move |
| `Enter`, `→`, `l` or `O` | Open the playlist |
| `P` | Play it |
| `a` / `N` | Add it to the queue / put it next |
| `w` | Put its tracks in another playlist |
| `c` or `+` | New playlist |
| `r` | Rename |
| `x` or `Delete` | Delete (press again to confirm) |
| `m` | Menu |
| `/` | Filter by name |

On a playlist's page:

| Key | Action |
|-----|--------|
| `Enter` | Play from the track under the cursor |
| `P` / `S` | Play all / play it shuffled |
| `a` / `N` | Add all to the queue / put them next |
| `K` / `J` | Move the track up / down |
| `Y` | Shuffle the playlist's order |
| `v` | Albums, or a row per track |
| `x`, `Delete` or `Backspace` | Take the track out |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo the last edit to this playlist |
| `r` | Rename |
| `e` | Export as M3U8 |
| `X` | Delete the playlist |
| `w` | Put the track in another playlist |
| `f` | Favourite the track |
| `i` | Track info |
| `m` | Menu |
| `A` / `O` | Open the artist / the record |

Everything on both pages is also a click: the row, `▸`, `×`, `▴`, `▾`, the artist and the record in a row, and the buttons under the playlist's name (play, shuffle, add to queue, undo, redo). The wheel and the scrollbar scroll.

## Add to a playlist (`w`, `W`)

`w` opens a list of playlists that can be filled, headed by `+ new playlist…`, for what the page has under the cursor or selected: the queue's selection, the playing track on Now playing, a record, a track, a playlist's tracks or a play in History. `Enter` or a click chooses one; `Esc` or a click outside cancels. `W` asks for a name and saves the whole queue as a playlist.

---

## Settings (`9`)

| Key | Action |
|-----|--------|
| `↑` `↓` or `j` `k` | Move between sections, or between rows in the pane |
| `Enter`, `→` or `l` | Go into the section |
| `Esc` or `←` | Back to the sections; from the sections, leave the page |
| `→` `←`, `+` `-` | Change the value under the cursor |
| `Enter` or `Space` | Change a value, start typing in a field, run a button, or open a row's actions |
| `m` | The actions of the list row under the cursor |
| `x` or `Delete` | Remove or revoke what the list row names, after asking |

While typing in a field, every key goes to the field: `Enter` keeps what is typed and runs what it is for, `Esc` leaves it as typed, `Ctrl+U` clears it. A question asked over the page takes `←` `→` or `Tab` to choose and `Enter` to answer; `Esc` cancels. Click a section, a value, a field, a button or a row's action to use it.

Changes apply at once, except the frame rate, which applies at the next start. `scan library` and `sync now` run in the background and report how they went in the status line; offline mode narrows the browsing pages and the pickers to what is on this device.

---

## EQ (`E`)

| Key | Action |
|-----|--------|
| `↑` `↓` or `j` `k` | Move |
| `Enter` | Choose: the device, a preset, the correction, or open an EQ |
| `Space` | Switch an EQ on or off |
| `K` / `J` (or `Shift+↑` / `Shift+↓`) | Move an EQ up / down the chain |
| `x` or `Delete` | Take an EQ out of the filters, or remove a band |
| `o` | Open the correction or EQ under the cursor |
| `←` `→` or `Tab` | Move between a band's fields; on the device row, change device |
| `+` `-` / `]` `[` | Nudge the figure, fine / coarse |
| `0`–`9` | Type a figure (`1k`, `-3.5`, `0.7`) |
| `a` | Add a band |
| `n` / `i` | New EQ / import one |
| `p` / `s` | Choose a preset / save as a preset |
| `d` | Next device |
| `u` | Use the EQ |
| `m` | Manage every correction, EQ and preset |
| `r` / `c` / `R` / `D` | Rename / save as new / reset to file / delete |
| `B` | Bypass the EQ |

---

## Downloads

| Key | Action |
|-----|--------|
| `↑` `↓` or `j` `k` | Move |
| `Enter` or `O` | Show the track's record |
| `u` | Remove a finished download |
| `c` | Clear the finished and failed transfers from the list |

---

## Mouse

Mouse controls are always available.

| Action | Effect |
|--------|--------|
| Click the seek bar | Jump to that position |
| Wheel | Scroll the list under the pointer |
| Click or drag a scrollbar | Scroll to that position |

### Finder drag and drop

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
| `⌘1`–`⌘7` | Queue, Albums, Artists, Favourites, History, Recently Played, Tracks |
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

Keys the TUI spends on its own panels — the visualizer, the help overlay — have no counterpart: there is no visualizer here.
