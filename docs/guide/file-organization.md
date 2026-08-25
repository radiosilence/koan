# File Organization

koan can rename and reorganize your music library using fb2k-compatible format strings, from the TUI or the macOS app. No external tools needed.

## In the macOS app

1. Select tracks in the queue, or an album or artist in the library
2. **Organize Files…** from the context menu
3. Pick a pattern, and a library folder if you have more than one
4. Read the preview
5. **Move Files**

Dropping a folder of files from Finder onto the queue indexes them into the library where they are and queues them, which is the usual way in: drop a rip, listen to it, then organize it into the tree once you are happy. Importing does not move anything -- files land in the music tree only after you have seen where they are going.

## In the TUI

1. Open the TUI: `koan`
2. Press `e` to enter queue edit mode
3. Select tracks (shift-arrows for multi-select, or `Ctrl`-click)
4. Press `space` to open the context menu
5. Select **Organize**
6. Pick a named pattern from your config
7. Preview the file moves
8. Execute

Playlist paths update automatically. Playback continues uninterrupted (Unix rename preserves open file descriptors). Ancillary files (cover.jpg, .cue, .log) move with the music. Empty directories are cleaned up.

## Reading the preview

Every selected file gets a row, whatever happens to it:

| | |
|---|---|
| **→** | Moving. The destination is shown relative to the library folder. |
| **✓** | Already exactly where the pattern puts it. Nothing to do. |
| **⚠** | Blocked. Something already holds that path, or two files in this run resolve to it. The file stays where it is. |
| **✗** | The pattern produced nothing usable for this file. |

A blocked file keeps its row next to the destination it collided with, rather than being counted up underneath. Nothing is ever overwritten, and that guarantee is only worth something if you can see what it saved you from before you commit.

## Destination

Files are organized into a **configured library folder** (from `[library] folders` in your config). The CLI and TUI use the first one; the macOS app lets you pick when there is more than one. The format pattern generates the relative path within that folder.

For example, with `folders = ["/Volumes/Music/library"]` and the `standard` pattern, a track becomes:

```
/Volumes/Music/library/Aphex Twin/(1999) Windowlicker EP/01. Windowlicker.flac
```

## Configuring patterns

The macOS app edits them in place: **Edit** next to the pattern picker turns it into a field, the preview follows what you type, and **Save** writes it back to `config.toml` under its name. A pattern you have edited but not saved still previews and still runs, so trying one out costs nothing.

Or define them in your config directly:

```toml
[organize]
default = "standard"       # preselected in the TUI modal and the macOS sheet
move_ancillary = true      # cover art, .cue and .log travel with the music

[organize.patterns]
standard = "%album artist%/(%date%) %album%/%tracknumber%. %title%"
va-aware = "%album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%"
flat = "%artist% - %title%"
```

### Pattern breakdown

**`standard`** -- simple artist/album/track hierarchy:
```
Aphex Twin/(1999) Windowlicker EP/01. Windowlicker.flac
```

**`va-aware`** -- handles compilations intelligently:
- Normal album: `Aphex Twin/(1999) Windowlicker EP [FLAC]/01. Windowlicker.flac`
- VA compilation: `Various Artists/Ministry of Sound [FLAC]/01. DJ Shadow - Building Steam.flac`

When the album artist is "Various Artists", the per-track artist is included in the filename and the redundant year prefix is omitted.

**`flat`** -- everything in one directory:
```
Aphex Twin - Windowlicker.flac
```

## Format string syntax

Patterns use fb2k-compatible syntax:

- `%field%` -- metadata value (artist, title, album, date, tracknumber, etc.)
- `[...]` -- conditional block, only included if all fields inside have values
- `$function()` -- transform functions ($if, $stricmp, $left, $num, etc.)
- `/` -- directory separator

See [Format Strings](../format-strings.md) for the complete syntax reference and all 55+ functions.

## Safety

Music files are irreplaceable, so organize refuses anything it can't do without risk rather than doing its best.

- **Nothing is ever overwritten.** Two tracks that resolve to the same destination, or a destination that already holds a file, are flagged as conflicts in the preview -- the second file stays exactly where it is. On macOS the check is case-insensitive, because `Rain.flac` and `RAIN.flac` are one file there.
- **Preview matches execute.** Both read metadata through the same resolver, so the paths you confirm are the paths that get used.
- **A pattern that produces an empty path component is refused.** An unknown function (`$nun` for `$num`) is a parse error rather than a silently empty result, and `..` or `.` components are errors, not something to strip.
- **Long titles are shortened, not rejected.** A destination name is capped below the filesystem's limit, extension included.
- **The database moves with the file.** Track paths, the scan cache, favourites and saved playback state are all rewritten in the same transaction as the move. A constraint violation aborts before the file is touched. Playlists are untouched, because they name library rows rather than paths.
- **Undo.** Every move -- including files the library has no row for -- is written to `organize_log`, newest batch first. Undo restores a file only if its original path is still free and the moved file is still the one that was logged; anything else is reported and left alone, with its log entry intact.
- **Cross-filesystem moves are copied, flushed and verified before the original is deleted.** A run that won't fit is refused before it starts.
- **Empty directories are cleaned up, but never a configured library root** or anything above one.
- **Ancillary files move with music.** Cover art, cue sheets, and log files in the same directory are moved alongside the music files. Artwork already at the destination is left alone. Turn it off with `[organize] move_ancillary = false`, or the checkbox in the macOS sheet, which writes the same setting.
