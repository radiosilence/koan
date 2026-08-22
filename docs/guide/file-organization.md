# File Organization

koan can rename and reorganize your music library using fb2k-compatible format strings, directly from the TUI. No external tools needed.

## Quick start

1. Open the TUI: `koan`
2. Press `e` to enter queue edit mode
3. Select tracks (shift-arrows for multi-select, or `Ctrl`-click)
4. Press `space` to open the context menu
5. Select **Organize**
6. Pick a named pattern from your config
7. Preview the file moves
8. Execute

Playlist paths update automatically. Playback continues uninterrupted (Unix rename preserves open file descriptors). Ancillary files (cover.jpg, .cue, .log) move with the music. Empty directories are cleaned up.

## Destination

Files are organized into the **first configured library folder** (from `[library] folders` in your config). If you have multiple library folders, the first one is always the destination. The format pattern generates the relative path within that folder.

For example, with `folders = ["/Volumes/Music/library"]` and the `standard` pattern, a track becomes:

```
/Volumes/Music/library/Aphex Twin/(1999) Windowlicker EP/01. Windowlicker.flac
```

## Configuring patterns

Define named patterns in your config:

```toml
[organize]
default = "standard"      # pattern selected by default in the modal

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

- **Nothing is ever overwritten.** Two tracks that resolve to the same destination, or a destination that already holds a file, are reported as errors and skipped -- the second file stays exactly where it is. On macOS the check is case-insensitive, because `Rain.flac` and `RAIN.flac` are one file there.
- **Preview matches execute.** Both read metadata through the same resolver, so the paths you confirm are the paths that get used.
- **A pattern that produces an empty path component is refused.** An unknown function (`$nun` for `$num`) is a parse error rather than a silently empty result, and `..` or `.` components are errors, not something to strip.
- **Long titles are shortened, not rejected.** A destination name is capped below the filesystem's limit, extension included.
- **The database moves with the file.** Track paths, the scan cache, favourites, queue snapshots and saved playback state are all rewritten in the same transaction as the move. A constraint violation aborts before the file is touched.
- **Undo.** Every move -- including files the library has no row for -- is written to `organize_log`, newest batch first. Undo restores a file only if its original path is still free and the moved file is still the one that was logged; anything else is reported and left alone, with its log entry intact.
- **Cross-filesystem moves are copied, flushed and verified before the original is deleted.** A run that won't fit is refused before it starts.
- **Empty directories are cleaned up, but never a configured library root** or anything above one.
- **Ancillary files move with music.** Cover art, cue sheets, and log files in the same directory are moved alongside the music files. Artwork already at the destination is left alone.
