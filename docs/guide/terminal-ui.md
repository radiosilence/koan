# The terminal UI

The TUI follows the shape of the Mac and mobile apps: pages you move between, a sidebar or tab bar to do it with, and a transport bar that stays put. Everything is reachable by keyboard and by mouse.

## Pages

| Key | Page | What it shows |
|-----|------|---------------|
| `1` | Queue | The queue, grouped by album. Edit, reorder and select here |
| `2` | Now playing | The cover, the signal path, levels, the spectrum, lyrics and the EQ curve |
| `3` | Albums | Every record in the library, as tiles of cover art or as rows |
| `4` | Artists | The library as a tree: artist, album, track |
| `5` | Favourites | Favourited records, or the tracks themselves |
| `6` | Recent | What was played in the last 30 days |

`Tab` and `Shift+Tab` step through the pages. Pages keep a history like a browser: `[` and `]`, or `Alt+Left` and `Alt+Right`, go back and forward, and `Esc` leaves a page for the one before it. Albums, Favourites and Recent are the library's own listings with a filter on, the same ones the apps use, so the three agree about what is on a shelf.

## Layout

The layout follows the width of the terminal.

- **100 columns or more:** a sidebar beside the page, with back and forward at its foot.
- **56 to 99 columns:** a tab bar over the page. Page names shorten to fit.
- **Under 56 columns:** the tab bar shows page numbers and names only the current page.

The transport bar sits at the foot. Drag the hairline above it to resize the cover; the size is saved to `config.local.toml`.

## Browsing records

Albums, Favourites and Recent draw each record as a tile. `v` switches to rows (tracks, on Favourites and Recent). `o` cycles the order on Albums: artist, date added, title, year. `/` filters what is shown.

| Key | Action |
|-----|--------|
| Arrows or `hjkl` | Move |
| `Enter` | Add to the queue and play |
| `a` | Add to the end of the queue |
| `Ctrl+R` | Replace the queue and play |
| `f` | On a list of tracks, favourite or unfavourite |

With the mouse: click selects, a second click plays, right-click adds to the queue, the wheel scrolls, and the sort and view labels in the heading are buttons.

## Covers

Covers are drawn as real images where the terminal supports the kitty, iTerm2 or sixel graphics protocol (Ghostty, kitty, WezTerm and iTerm2 do). Elsewhere, and whenever `playback.art_graphics` is off, they are drawn in half blocks: two pixels to a cell, which is blocky and has a charm of its own. `I` switches between the two for the session and saves the choice.

Graphics are detected once at startup by asking the terminal, which can add a moment on a terminal that does not answer. Setting `art_graphics = false` skips the question.

## Colour

The palette is the [kōan theme](../design/koan-theme.md). While a record plays, the accent takes its colour from the cover; a sleeve with no colour in it leaves the accent mint. The `record` visualizer palette, the default, follows the accent too. Where the terminal reports 24-bit colour the theme is drawn exactly, and in the nearest of the 256 colours otherwise. `playback.tui_theme = "ansi"` returns to the terminal's own sixteen colours. The theme never paints a background, so the terminal's own shows through.

## Now playing

The signal path is the part to read. **source** is the file's format. **output** is the rate the device was given, and says `bit-perfect ✓` only when nothing stands between the file and the device. If the output is resampled, DSP is running, or ReplayGain is on, it says which instead. When a DSP profile is active, its frequency response is plotted beside the lyrics.

The transport bar carries a small spectrum at the end of its album line.
