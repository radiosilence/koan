# Plan 04: Tag Editing

Full tag editing from within the TUI: inline editing, bulk operations, vimv-style external editor, auto-operations, and MusicBrainz lookups.

## Summary

lofty 0.23 already handles both reading and writing for every format koan supports. The write API is straightforward: read file, mutate the Tag object, call `save_to_path()`. With `remove_others: false` (the default), existing tags/pictures we don't touch are preserved. The hard parts are (a) the TUI text input UX, (b) the $EDITOR suspend/resume lifecycle, (c) safely writing to files that might be playing, and (d) keeping the DB + queue in sync after writes.

The recommended approach: build the vimv-style external editor path first (highest power-to-effort ratio), then inline TUI editing, then auto-operations.

---

## 1. lofty Write Capabilities

### Format support

lofty writes all major tag formats koan encounters:
- **ID3v2** (MP3, AIFF, WAV) — v2.3 or v2.4 via `use_id3v23` option
- **Vorbis Comments** (FLAC, OGG Vorbis, Opus)
- **MP4/iTunes ilst** (M4A, AAC, ALAC)
- **APE tags** (APE, WavPack)
- ID3v1 (legacy, but supported)

### Write workflow

```rust
use lofty::prelude::*;
use lofty::config::WriteOptions;

// 1. Read the file (preserves all existing tag data in memory)
let mut tagged_file = lofty::read_from_path(path)?;

// 2. Get the primary tag mutably (or create one)
let tag = tagged_file.primary_tag_mut()
    .unwrap_or_else(|| {
        let tag_type = tagged_file.primary_tag_type();
        tagged_file.insert_tag(Tag::new(tag_type));
        tagged_file.primary_tag_mut().unwrap()
    });

// 3. Modify fields via Accessor trait
tag.set_title("New Title".into());
tag.set_artist("New Artist".into());
tag.set_album("New Album".into());
tag.set_track(5);
tag.set_disk(1);

// For non-standard fields:
tag.insert_text(ItemKey::AlbumArtist, "Various Artists".into());
tag.insert_text(ItemKey::Year, "2024".into());
tag.insert_text(ItemKey::Genre, "Electronic".into());
tag.insert_text(ItemKey::Label, "Warp Records".into());

// 4. Save — default WriteOptions preserves other tags and pictures
tag.save_to_path(path, WriteOptions::default())?;
```

### Tag preservation

- **`remove_others: false` (default)**: Only the tag type being written is modified. If a file has both ID3v2 and APE tags, writing to ID3v2 leaves APE untouched.
- **Pictures**: Pictures stored in the Tag object persist through save. If we read the file, modify text fields only, and save, album art is preserved. We never need to explicitly handle art unless we're changing it.
- **Unknown/custom frames**: Items lofty doesn't parse are preserved as raw bytes in the underlying format-specific tag.

### Caveats

- **Opus + pictures**: There's a known lofty issue (#130) where adding pictures to Opus files can corrupt the Ogg stream. We should avoid picture writes on Opus for now, or test thoroughly. Text-only writes are fine.
- **File size changes**: Writing tags changes the file's mtime and potentially its size, which invalidates our scan_cache. This is expected and handled (see DB sync section).
- **Thread safety**: `Tag` and `TaggedFile` are not `Sync`. All tag writing must happen on a single thread (the background write thread). No concerns for the read-only UI thread.

### Fields we can write

| Field | Accessor method | ItemKey fallback |
|---|---|---|
| Title | `set_title()` | — |
| Artist | `set_artist()` | — |
| Album | `set_album()` | — |
| Album Artist | — | `ItemKey::AlbumArtist` |
| Track # | `set_track()` | — |
| Track Total | `set_track_total()` | — |
| Disc # | `set_disk()` | — |
| Disc Total | `set_disk_total()` | — |
| Year/Date | — | `ItemKey::Year` or `ItemKey::RecordingDate` |
| Genre | `set_genre()` | — |
| Label | — | `ItemKey::Label` |
| Comment | `set_comment()` | — |

---

## 2. TUI Inline Editor

### Current state

`track_info.rs` renders a read-only modal (TrackInfoOverlay) showing metadata fields. The natural evolution is making those fields editable.

### Design: TagEditOverlay

New mode: `Mode::TagEdit(TagEditState)`. Entered from:
- Track info modal (press `e` to switch to edit mode)
- Queue edit mode (press `e` on selected track, or `E` for bulk edit)
- Context menu action

```rust
pub struct TagEditState {
    /// Tracks being edited (1 for single, N for bulk).
    tracks: Vec<TagEditEntry>,
    /// Which field the cursor is on.
    field_cursor: usize,
    /// The text input buffer for the active field.
    input: String,
    /// Cursor position within the input string.
    input_cursor: usize,
    /// Whether we're actively editing the current field (insert mode).
    editing: bool,
    /// Preview of changes (field name -> old value -> new value).
    changes: Vec<TagChange>,
    /// Focus: field list vs. confirm button.
    focus: TagEditFocus,
}

pub struct TagEditEntry {
    path: PathBuf,
    queue_item_id: Option<QueueItemId>,
    original: TagFieldSet,
    modified: TagFieldSet,
}

pub struct TagFieldSet {
    title: String,
    artist: String,
    album_artist: Option<String>,
    album: String,
    track_number: Option<i32>,
    disc: Option<i32>,
    year: Option<String>,
    genre: Option<String>,
    label: Option<String>,
}
```

### Field editing UX

- **Navigation**: Up/Down arrows move between fields.
- **Enter/Tab**: Start editing the focused field. The field value appears in a text input with cursor.
- **Text input**: Standard behavior — left/right to move cursor, backspace/delete, Home/End. Crossterm already handles this in the picker.
- **Escape**: Cancel editing current field (revert to original value) or exit modal if not editing.
- **Ctrl+S or Enter on confirm**: Apply changes.

### Bulk edit behavior

When multiple tracks are selected:
- Fields where all tracks have the same value show that value.
- Fields where tracks differ show `<mixed>` (grayed out).
- Editing a `<mixed>` field applies the new value to ALL selected tracks.
- Leaving a `<mixed>` field untouched preserves each track's original value.
- Special fields for bulk: "Track # (auto-number)" which assigns sequential numbers.

### Text input widget

The picker already uses nucleo for fuzzy matching, but the tag editor needs simple text input. Options:
- **tui-input** crate: Lightweight, handles cursor movement, supports unicode. Good fit.
- **Roll our own**: Not much to it — a String buffer + cursor position + basic keymap. The picker's input handling is close but coupled to nucleo. Probably cleanest to use tui-input or a minimal inline implementation.

Recommendation: Use `tui-input` (or `tui-textarea` for a slightly richer widget). It integrates cleanly with ratatui.

---

## 3. vimv-Style External Editor (Primary Approach)

This is the highest-value feature. Dump tags to a structured file, open $EDITOR, parse changes back. Gives users the full power of their editor for bulk operations.

### File format: TSV

TSV is the best format for this. It's tabular (tracks are rows, fields are columns), editable in any text editor, and trivially parseable. TOML/YAML would work for single tracks but becomes unwieldy for 50+ tracks.

```
# koan tag edit — modify fields, save and exit
# Columns: PATH	TITLE	ARTIST	ALBUM_ARTIST	ALBUM	TRACK	DISC	YEAR	GENRE
/music/album/01.flac	Opening	Aphex Twin	Aphex Twin	SAW 85-92	1	1	1992	Electronic
/music/album/02.flac	Tha	Aphex Twin	Aphex Twin	SAW 85-92	2	1	1992	Electronic
/music/album/03.flac	Pulsewidth	Aphex Twin	Aphex Twin	SAW 85-92	3	1	1992	Electronic
```

The PATH column is read-only (used to identify which file to modify). All other columns are editable.

Alternative: One-track-per-block TOML for when you want rich single-track editing:

```toml
[[track]]
path = "/music/album/01.flac"  # read-only
title = "Opening"
artist = "Aphex Twin"
album_artist = "Aphex Twin"
album = "SAW 85-92"
track = 1
disc = 1
year = "1992"
genre = "Electronic"
```

Recommendation: Support both. TSV for bulk (default when >1 track), TOML for single track. User can override via config.

### Terminal suspend/resume

The `run_tui` function in `play.rs` owns the terminal lifecycle. To spawn $EDITOR, we need to:

1. Leave alternate screen + disable raw mode + disable mouse capture
2. Run `$EDITOR tempfile` as a child process, wait for exit
3. Re-enter alternate screen + enable raw mode + enable mouse capture + clear terminal

Ratatui documents this pattern explicitly. The implementation:

```rust
// In the main event loop, when the user triggers vimv edit:
fn spawn_editor(terminal: &mut Terminal<impl Backend>, path: &Path) -> io::Result<bool> {
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use crossterm::event::{DisableMouseCapture, EnableMouseCapture,
                           DisableBracketedPaste, EnableBracketedPaste};
    use crossterm::execute;

    // Suspend TUI
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;

    // Spawn editor
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vim".into());

    let status = std::process::Command::new(&editor)
        .arg(path)
        .status()?;

    // Resume TUI
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    terminal.clear()?;

    Ok(status.success())
}
```

**Critical detail**: The terminal reference must be passed through the App to the event handler. Currently `run_tui` owns the terminal and the app doesn't have access to it. Two options:
1. Add a field `pending_editor_action: Option<EditorAction>` to App. The main loop checks this after handling events, suspends, runs the editor, resumes, then processes the result.
2. Pass `&mut Terminal` into `handle_key`. This is messy and couples the app to the terminal backend.

Option 1 is cleaner and matches the existing pattern (like `picker_result`).

### Full vimv workflow

1. **User selects tracks** in queue (multi-select) and hits `T` (tag-edit hotkey).
2. **App sets** `pending_editor_action = Some(EditorAction::TagEdit(selected_paths))`.
3. **Main loop** detects pending action:
   a. Generates TSV from current tags (read from files, not DB — files are the source of truth).
   b. Writes to a temp file (`tempfile` crate, `.tsv` extension).
   c. Suspends TUI, opens $EDITOR, waits.
   d. Resumes TUI.
   e. Parses the edited TSV.
   f. Diffs against original — builds a list of `TagChange` structs.
   g. Shows a confirmation modal (like organize preview): "N tracks, M fields changed".
   h. On confirm, spawns background thread to write tags.
4. **Background thread** writes tags, updates DB, signals completion.
5. **Main loop** picks up completion, updates queue entries if needed.

### Diff and preview

```rust
pub struct TagChange {
    path: PathBuf,
    field: TagField,
    old_value: String,
    new_value: String,
}

pub enum TagField {
    Title,
    Artist,
    AlbumArtist,
    Album,
    TrackNumber,
    Disc,
    Year,
    Genre,
    Label,
}
```

The preview modal (reuse organize pattern) shows:
```
 3 tracks, 7 fields changed

 01.flac  Artist: "Aphex Twin" → "AFX"
 01.flac  Album: "SAW 85-92" → "Selected Ambient Works 85-92"
 02.flac  Artist: "Aphex Twin" → "AFX"
 ...

 [Apply]  [Cancel]
```

---

## 4. Auto-Operations

### Track numbering

Select tracks in queue, trigger auto-number. Assigns track numbers 1..N based on:
- **By position**: Order in the current selection (queue order).
- **By filename**: Sort by filename, then number.
- **By existing order**: Preserve relative order but renumber from 1.

Implementation: Generate the same `TagChange` vec, feed into the standard preview + apply pipeline.

### Case conversion presets

These operate on the `TagFieldSet` before writing:

```rust
pub enum CasePreset {
    TitleCase,       // "the quick brown fox" → "The Quick Brown Fox"
    SentenceCase,    // "THE QUICK BROWN FOX" → "The quick brown fox"
    Lowercase,
    Uppercase,
}

pub enum CleanupPreset {
    StripLeadingTrackNumbers,  // "01 - Opening" → "Opening"
    StripTrailingSpaces,
    NormalizeWhitespace,       // collapse multiple spaces
    ArtistFromDirectory,       // parent dir name → artist field
    AlbumFromDirectory,        // grandparent dir name → album
}
```

These can be applied as "transforms" in the tag edit modal, or as a pre-processing step before the vimv editor opens.

### Extract from directory structure (reverse-organize)

The organize feature uses format strings like `{album_artist}/{album}/{track:02} - {title}`. Reversing this: given a format pattern and a file path, extract metadata fields.

This is a regex generation problem:
1. Parse the format string into segments.
2. Replace `{field}` placeholders with named capture groups.
3. Match against the file's relative path.
4. Extract captured values as metadata.

```
Pattern: "{album_artist}/{album}/{track:02} - {title}"
Path:    "Aphex Twin/SAW 85-92/03 - Pulsewidth.flac"
Result:  album_artist="Aphex Twin", album="SAW 85-92", track=3, title="Pulsewidth"
```

Reuse the organize patterns from config.toml — they're already defined. The module already has a parser for these format strings.

### MusicBrainz / AcoustID lookup (future phase)

This requires:
1. **Chromaprint** for audio fingerprinting. Available as `chromaprint-sys-next` crate (Rust FFI bindings). However, this introduces a C dependency — against the "pure Rust" philosophy. Alternative: shell out to `fpcalc` binary if installed.
2. **AcoustID API** to submit fingerprint, get MusicBrainz recording IDs.
3. **MusicBrainz API** (`musicbrainz` crate) to fetch rich metadata.

This is a phase 3+ feature. The fingerprinting adds significant complexity and a non-Rust dependency. Consider making it opt-in (feature flag) or external-tool-based (`fpcalc` in PATH).

---

## 5. Database Sync Strategy

After writing tags to a file, the database must reflect the new metadata.

### Approach: Re-read + upsert

The simplest and most correct approach: after writing tags, read the metadata back from the file (using the existing `read_metadata()`) and upsert it. This guarantees the DB matches the file exactly.

```rust
// After tag write succeeds:
let meta = metadata::read_metadata(&path)?;
queries::upsert_track(&db.conn, &meta)?;
// Update scan cache with new mtime/size
let file_meta = std::fs::metadata(&path)?;
let mtime = file_meta.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64;
let size = file_meta.len() as i64;
queries::update_scan_cache(&db.conn, path_str, mtime, size, track_id)?;
```

Why not update the DB directly? Because:
- The DB has normalized artist/album tables (get_or_create_artist, get_or_create_album). Changing an artist name might create a new artist row, orphan the old one, etc. `upsert_track` already handles all this complexity.
- Re-reading ensures we don't get out of sync if lofty normalizes/transforms anything during write.

### Queue entry updates

After DB update, we need to update the in-memory QueueEntry for any affected tracks. New command:

```rust
PlayerCommand::UpdateTrackMetadata(Vec<(QueueItemId, TrackMetadataUpdate)>)
```

Where `TrackMetadataUpdate` contains the fields that changed. The player thread applies these to matching playlist items. This is similar to the existing `UpdatePaths` command but for metadata fields.

### Currently-playing tracks

If a track is currently playing when its tags are modified:
- **Audio is unaffected.** Tag data lives in the file header, not in the audio stream. Symphonia already read the audio data into the decode pipeline. Modifying tags doesn't invalidate the decoded audio.
- **Display updates immediately** via the PlayerCommand above.
- **No need to stop/restart playback.**

However, if the file is being actively read by the decode thread (e.g., streaming read), there's a theoretical race. In practice:
- FLAC/Vorbis/Opus: Tags are at the beginning of the file. By the time we're playing audio, the decoder has moved past the tag block. Writing tags might rewrite the header, but the decoder is reading from later in the file.
- MP3: ID3v2 is at the start, ID3v1 at the end. Same reasoning — the decoder reads the audio frames in the middle.
- **lofty rewrites the file** (for formats without padding, it truncates + rewrites). This could cause issues if the decode thread has the file open with a read FD. On macOS, the file descriptor remains valid even if the inode changes (rename-based atomic write) — but if lofty truncates in place, the FD's offset might become invalid.

**Safe approach**: Before writing tags to a currently-playing file, check if the file is in the active decode pipeline. If so, either:
1. Queue the write for after playback moves past that file, OR
2. Warn the user and let them decide, OR
3. Just do it — the decode thread handles read errors gracefully (it'll fail and move to the next track, which is acceptable for an explicit user action).

Recommendation: Option 3 with a log warning. Users editing tags on a playing track expect some disruption. The worst case is the current track restarts or skips — not a crash.

---

## 6. Safety / Undo Design

### Backup strategy

Before modifying any file, save the original tag data. Two approaches:

**Option A: In-memory backup (recommended for undo)**

```rust
pub struct TagBackup {
    path: PathBuf,
    /// Complete original Tag, serialized.
    original_tag: Vec<u8>,  // dumped via tag.dump_to()
    tag_type: TagType,
}
```

Store backups in an `UndoEntry` variant. The existing `UndoStack` (100-depth, push/pop semantics) is the right place, but it currently only handles playlist operations. We can either:
- Add tag-specific variants to `UndoEntry`, or
- Create a separate `TagUndoStack` that lives in `koan-core`.

Recommendation: Separate stack. Tag undo is fundamentally different from playlist undo — it modifies files on disk, not in-memory state. Mixing them would be confusing (Ctrl+Z might unexpectedly revert file changes vs. queue changes).

**Option B: Backup directory**

Write the original tag bytes to a `.koan-backup/` directory (or `~/.local/share/koan/tag-backups/`) before each edit session. Allows recovery even after restart.

```
~/.local/share/koan/tag-backups/
  2024-01-15T14:30:00/
    01-opening.flac.tag-backup   # raw tag bytes
    02-tha.flac.tag-backup
```

This is the belt-and-suspenders approach. Implement both: in-memory for Ctrl+Z, persistent backup for disaster recovery.

### Undo implementation

```rust
pub enum TagUndoEntry {
    /// One or more tags were modified. Stores the original tag data.
    Modified {
        backups: Vec<TagBackup>,
    },
}
```

To undo: for each backup, open the file, remove the current tag, restore the backup bytes via `tag.save_to_path()` (or more precisely, reconstruct the Tag from the dumped bytes and save it).

**Restoring from dump_to bytes**: `dump_to` writes the tag in its native format. To restore, we'd need to parse those bytes back into a Tag and save. lofty can do this — read the bytes as if they were a tag-only stream. Alternatively, just keep the entire `Tag` object in memory (it's Clone) rather than serialized bytes.

```rust
pub struct TagBackup {
    path: PathBuf,
    original_tag: Tag,  // Clone of the tag before modification
}
```

This is simpler and avoids serialization roundtrip issues.

### File-in-use protection

The player's decode thread may have an open file descriptor. On macOS/Unix, this is safe for most operations because:
- `rename()` doesn't invalidate open FDs (the old inode stays alive).
- Truncate + rewrite CAN invalidate the FD's read position.

**Implementation**: Add a method to check if a path is in the active decode pipeline:

```rust
impl SharedPlayerState {
    pub fn is_file_in_decode_pipeline(&self, path: &Path) -> bool {
        // Check if path matches current or next track being decoded
    }
}
```

If the file is being decoded, show a warning in the preview: "Warning: track 3 is currently playing. Tag edit may interrupt playback."

---

## 7. Implementation Phases

### Phase 1: Core write infrastructure + vimv editor

**Scope**: Write tags from a CLI command first, then wire into TUI.

1. **`koan-core/src/index/tag_writer.rs`** — New module:
   - `write_tags(path, changes: &[TagChange]) -> Result<(), TagWriteError>`
   - `read_tag_fields(path) -> Result<TagFieldSet, _>`
   - `backup_tag(path) -> Result<TagBackup, _>`
   - `restore_tag(backup: &TagBackup) -> Result<(), _>`

2. **`koan-core/src/index/tag_edit.rs`** — TSV/TOML serialization:
   - `generate_tsv(tracks: &[TagFieldSet]) -> String`
   - `parse_tsv(content: &str) -> Result<Vec<TagFieldSet>, _>`
   - `diff_fields(original: &TagFieldSet, modified: &TagFieldSet) -> Vec<TagChange>`
   - Same for TOML format.

3. **CLI command** `koan tag-edit [paths...]`:
   - Opens $EDITOR with TSV, applies changes. Good for testing the pipeline without TUI complexity.

4. **TUI integration**:
   - New `Mode::TagEditPreview(TagEditPreviewState)` for the confirmation modal.
   - `pending_editor_action` field on App for the suspend/resume lifecycle.
   - Wire into the main loop in `play.rs`.

5. **DB sync**: Re-read + upsert after writes. Update queue entries via new PlayerCommand.

**Estimated effort**: Medium-large. The lofty write code is trivial, the TSV parsing is straightforward, the hard part is the terminal suspend/resume plumbing and the preview modal.

### Phase 2: TUI inline editor

1. **`tui/tag_edit.rs`** — TagEditOverlay widget:
   - Field list with cursor.
   - Text input per field (tui-input or custom).
   - Single-track and bulk-edit modes.

2. **Keybindings**:
   - `e` in TrackInfo modal → enter edit mode.
   - `E` in QueueEdit mode → bulk edit selected tracks.
   - Navigation, editing, save/cancel.

3. **Auto-numbering**: Accessible from bulk edit modal as a button/action.

**Estimated effort**: Medium. Most of the write infrastructure exists from Phase 1. Main work is the TUI widget and text input handling.

### Phase 3: Auto-operations + presets

1. **Case conversion and cleanup presets**: Pure string transforms, exposed as actions in the edit modal.
2. **Reverse-organize** (extract from directory): Reuse format string parser from organize module.
3. **Tag backup directory**: Persistent backup with cleanup policy.

**Estimated effort**: Small-medium. Mostly string manipulation and UI actions.

### Phase 4: MusicBrainz / AcoustID (optional, future)

1. **fpcalc integration**: Shell out to `fpcalc` binary, parse JSON output.
2. **AcoustID API client**: Submit fingerprint, get recording IDs.
3. **MusicBrainz API client**: Fetch metadata, present as suggestions.
4. **UI**: "Lookup" button in tag editor, shows MusicBrainz results, user picks one.

**Estimated effort**: Large. External API integration, rate limiting, error handling, UI for results. Feature-flag it.

### Keybinding plan

| Key | Context | Action |
|---|---|---|
| `T` | Normal/QueueEdit | Open vimv tag editor for selected tracks |
| `e` | TrackInfo modal | Switch to inline tag editor |
| `E` | QueueEdit | Inline bulk tag editor for selection |
| `Ctrl+S` | Tag editor | Save/apply changes |
| `Esc` | Tag editor | Cancel |

---

## 8. New files and modules

```
crates/koan-core/src/index/
  tag_writer.rs    — lofty write operations, backup/restore
  tag_edit.rs      — TSV/TOML gen/parse, diff, TagFieldSet, TagChange

crates/koan-music/src/tui/
  tag_edit.rs      — TagEditOverlay widget, TagEditState
```

New PlayerCommand variant:
```rust
PlayerCommand::UpdateTrackMetadata(Vec<(QueueItemId, TrackMetadataUpdate)>)
```

New config options (config.toml):
```toml
[tagging]
editor_format = "tsv"  # or "toml"
backup_dir = "~/.local/share/koan/tag-backups"
backup_retention_days = 30
```

---

## 9. Open questions

1. **Should tag undo share the Ctrl+Z binding with queue undo?** If yes, we need a unified undo stack that knows the context. If no, we need a separate binding (Ctrl+Shift+Z is already redo). Recommendation: Unified stack — tag edits push onto the same undo stack, the Player knows how to reverse them.

2. **File locking on write**: Should we use advisory file locks (`flock`) when writing tags? Prevents concurrent writes from multiple koan instances. Probably overkill for a single-user music player.

3. **Album art editing**: Phase 1 deliberately excludes picture modification. Adding/removing/replacing cover art is a separate feature that needs its own UI (image preview, file picker). Defer.

4. **Remote tracks**: Tag editing only applies to local files. Remote-only tracks should have the edit option grayed out / hidden. DB-only metadata edits (without file writes) could be a future enhancement.
