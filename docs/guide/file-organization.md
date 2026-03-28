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

- **Preview before execute.** The organize modal always shows you exactly what will be moved before doing anything.
- **Path traversal protection.** Malicious metadata containing `..` or `.` path components is stripped. Destinations are validated to stay under the library base directory.
- **Undo support.** Organize operations are tracked in the database (`organize_log` table) for potential reversal.
- **Ancillary files move with music.** Cover art, cue sheets, and log files in the same directory are moved alongside the music files.
