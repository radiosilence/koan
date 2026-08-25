use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::db::connection::{Database, DbError};
use crate::db::queries::{self, PersistedQueueItem, TrackRow};
use crate::format::{self, FormatError, MetadataProvider};
use crate::helpers::{sanitise_filename, truncate_bytes};

/// Ancillary file patterns we move alongside audio files.
const ANCILLARY_PATTERNS: &[&str] = &[
    "cover.jpg",
    "cover.png",
    "cover.webp",
    "folder.jpg",
    "folder.png",
    "front.jpg",
    "front.png",
];

const ANCILLARY_EXTENSIONS: &[&str] = &["cue", "log", "m3u", "m3u8"];

/// Byte ceiling for a destination file name, extension included. Filesystems we
/// target cap a single name at 255 bytes.
const MAX_FILE_NAME_BYTES: usize = 250;

#[derive(Debug, thiserror::Error)]
pub enum OrganizeError {
    #[error("database error: {0}")]
    Db(#[from] DbError),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("format error: {0}")]
    Format(#[from] FormatError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no tracks with local paths found")]
    NoLocalTracks,
    #[error("no organize batches to undo")]
    NothingToUndo,
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("copied {copied} of {expected} bytes from {path}")]
    ShortCopy {
        path: PathBuf,
        expected: u64,
        copied: u64,
    },
    #[error("not enough free space: {needed} bytes needed, {available} available")]
    NotEnoughSpace { needed: u64, available: u64 },
}

/// What the pattern means for one file.
///
/// Conflicts are an outcome rather than an error off to one side: "this would
/// overwrite something" is the single most important thing a preview can tell
/// you, and it belongs on the row it concerns, next to the destination it would
/// have landed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOutcome {
    /// The file will be moved, or was.
    Move,
    /// Already exactly where the pattern puts it. Nothing to do.
    Unchanged,
    /// Something holds the destination — a file already there, or another file
    /// in the same run that claimed it first. Nothing is ever overwritten, so
    /// this file stays where it is.
    Conflict(String),
    /// The pattern produced nothing usable for this file, or the move failed.
    Error(String),
}

impl PlanOutcome {
    /// The reason a file isn't moving, for anything rendering it as text.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Conflict(reason) | Self::Error(reason) => Some(reason),
            _ => None,
        }
    }
}

/// One file's place in a plan: where it is, where the pattern puts it, and
/// whether that can happen.
#[derive(Debug, Clone)]
pub struct PlanEntry {
    /// The library track this file belongs to, or `None` for a file the library
    /// doesn't know about. Either way the move is logged and can be undone.
    pub track_id: Option<i64>,
    pub from: PathBuf,
    /// Where the pattern puts it. `None` only when the pattern failed before it
    /// produced a path at all.
    pub to: Option<PathBuf>,
    pub ancillary: Vec<(PathBuf, PathBuf)>,
    pub outcome: PlanOutcome,
}

impl PlanEntry {
    /// Where this file is headed. Only call it on an entry that has a
    /// destination — a plan that failed before producing one has none.
    #[cfg(test)]
    fn dest(&self) -> &Path {
        self.to.as_deref().expect("plan entry has no destination")
    }

    /// The executable move this entry stands for, if it is one.
    fn as_move(&self) -> Option<FileMove> {
        match (&self.outcome, &self.to) {
            (PlanOutcome::Move, Some(to)) => Some(FileMove {
                track_id: self.track_id,
                from: self.from.clone(),
                to: to.clone(),
                ancillary: self.ancillary.clone(),
            }),
            _ => None,
        }
    }
}

/// Every selected file and what happens to it, in the order it was planned.
///
/// One ordered list rather than separate buckets: a preview is a table with a
/// row per file, and splitting the failures out of it loses both their place in
/// the run and the destination they were headed for.
#[derive(Debug, Default)]
pub struct OrganizeResult {
    pub entries: Vec<PlanEntry>,
}

impl OrganizeResult {
    pub fn moves(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries
            .iter()
            .filter(|e| e.outcome == PlanOutcome::Move)
    }

    pub fn moved_count(&self) -> usize {
        self.moves().count()
    }

    /// Files already where the pattern puts them.
    pub fn unchanged_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.outcome == PlanOutcome::Unchanged)
            .count()
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e.outcome, PlanOutcome::Conflict(_)))
    }

    /// Everything that isn't happening and isn't already right — conflicts and
    /// errors together, since a caller reporting failures wants both.
    pub fn failures(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e.outcome, PlanOutcome::Conflict(_) | PlanOutcome::Error(_)))
    }

    /// One line per failure, for callers that render a flat list. Includes the
    /// destination where there was one — for a conflict that is the whole point.
    pub fn failure_messages(&self) -> Vec<String> {
        self.failures()
            .map(|e| {
                let reason = e.outcome.reason().unwrap_or("unknown");
                match &e.to {
                    Some(to) => format!("{}: {reason} ({})", e.from.display(), to.display()),
                    None => format!("{}: {reason}", e.from.display()),
                }
            })
            .collect()
    }
}

/// An executable move, extracted from a plan entry that can proceed.
#[derive(Debug)]
pub struct FileMove {
    pub track_id: Option<i64>,
    pub from: PathBuf,
    pub to: PathBuf,
    pub ancillary: Vec<(PathBuf, PathBuf)>,
}

/// One `organize_log` row: id, original path, moved-to path, and the size and
/// modification time the file had when it was moved.
type UndoEntry = (i64, String, String, Option<i64>, Option<i64>);

#[derive(Debug, Default)]
pub struct UndoResult {
    pub restored: usize,
    pub errors: Vec<(PathBuf, String)>,
}

/// Which files an organize run covers.
enum Selection<'a> {
    All,
    TrackIds(&'a [i64]),
    Paths(&'a [PathBuf]),
}

/// Album fields a track inherits: both come from the album row, not the track row.
#[derive(Default, Clone)]
struct AlbumFacts {
    date: Option<String>,
    label: Option<String>,
}

/// A source file with the metadata its destination will be built from.
struct ResolvedTrack {
    source: PathBuf,
    track_id: Option<i64>,
    metadata: Result<TrackMetadata, String>,
}

/// Metadata provider backed by a HashMap, for evaluating format strings against track data.
struct TrackMetadata {
    fields: HashMap<String, String>,
}

impl TrackMetadata {
    fn from_track_row(track: &TrackRow, album: &AlbumFacts) -> Self {
        let mut fields = HashMap::new();
        // Sanitize all field values so they can't inject path separators or illegal chars.
        let s = sanitise_filename;
        fields.insert("title".into(), s(&track.title));
        fields.insert("artist".into(), s(&track.artist_name));
        fields.insert("album artist".into(), s(&track.album_artist_name));
        fields.insert("album".into(), s(&track.album_title));
        if let Some(n) = track.track_number {
            fields.insert("tracknumber".into(), format!("{n:02}"));
        }
        if let Some(d) = track.disc {
            fields.insert("discnumber".into(), d.to_string());
        }
        if let Some(ref date) = album.date {
            fields.insert("date".into(), s(date));
        }
        if let Some(ref label) = album.label {
            fields.insert("label".into(), s(label));
        }
        if let Some(ref codec) = track.codec {
            fields.insert("codec".into(), s(codec));
        }
        if let Some(ref genre) = track.genre {
            fields.insert("genre".into(), s(genre));
        }
        Self { fields }
    }

    /// Build metadata directly from file tags, for files the library doesn't know about.
    /// Populates exactly the same field set as `from_track_row` so a preview and the
    /// move it authorises can never resolve to different paths.
    fn from_file_meta(meta: &queries::TrackMeta) -> Self {
        let mut fields = HashMap::new();
        let s = sanitise_filename;
        fields.insert("title".into(), s(&meta.title));
        fields.insert("artist".into(), s(&meta.artist));
        fields.insert(
            "album artist".into(),
            s(meta.album_artist.as_deref().unwrap_or(&meta.artist)),
        );
        fields.insert("album".into(), s(&meta.album));
        if let Some(n) = meta.track_number {
            fields.insert("tracknumber".into(), format!("{n:02}"));
        }
        if let Some(d) = meta.disc {
            fields.insert("discnumber".into(), d.to_string());
        }
        if let Some(ref date) = meta.date {
            fields.insert("date".into(), s(date));
        }
        if let Some(ref label) = meta.label {
            fields.insert("label".into(), s(label));
        }
        if let Some(ref codec) = meta.codec {
            fields.insert("codec".into(), s(codec));
        }
        if let Some(ref genre) = meta.genre {
            fields.insert("genre".into(), s(genre));
        }
        Self { fields }
    }
}

impl MetadataProvider for TrackMetadata {
    fn get_field(&self, name: &str) -> Option<String> {
        self.fields.get(name).cloned()
    }
}

/// Sanitize each component of a relative path independently.
///
/// An empty, `.` or `..` component is an error, not something to skip: dropping one
/// silently collapses a whole album onto a single filename, and the tracks that land
/// there overwrite each other.
fn sanitize_relative_path(rel: &str) -> Result<PathBuf, String> {
    let mut result = PathBuf::new();
    for part in rel.split(['/', std::path::MAIN_SEPARATOR]) {
        let sanitized = sanitise_filename(part);
        if sanitized.is_empty() {
            return Err(format!(
                "format string produced an empty path component: {rel:?}"
            ));
        }
        if sanitized == "." || sanitized == ".." {
            return Err(format!(
                "format string produced a relative path component: {rel:?}"
            ));
        }
        result.push(sanitized);
    }
    if result.as_os_str().is_empty() {
        return Err("format string produced an empty path".into());
    }
    Ok(result)
}

/// Load every album's date and label in one query — both are fields a format string
/// can reference, and both live on the album row.
fn load_album_facts(conn: &Connection) -> Result<HashMap<i64, AlbumFacts>, OrganizeError> {
    let mut stmt = conn.prepare("SELECT id, date, label FROM albums")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            AlbumFacts {
                date: row.get(1)?,
                label: row.get(2)?,
            },
        ))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (id, facts) = row?;
        map.insert(id, facts);
    }
    Ok(map)
}

/// Find ancillary files in the same directory as a track.
fn find_ancillary_files(track_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(track_dir) else {
        return files;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_lowercase();

        // Check exact name matches.
        if ANCILLARY_PATTERNS.iter().any(|p| name == *p) {
            files.push(path);
            continue;
        }
        // Check extension matches.
        if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && ANCILLARY_EXTENSIONS
                .iter()
                .any(|e| ext.eq_ignore_ascii_case(e))
        {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// The destination names a run has already committed to, so two files can never be
/// planned onto the same path.
#[derive(Default)]
struct DestinationLedger {
    taken: HashSet<String>,
}

impl DestinationLedger {
    /// macOS, iOS and Windows filesystems are case-insensitive by default, so
    /// `Rain.flac` and `RAIN.flac` are one file there and must collide here too.
    fn key(path: &Path) -> String {
        let key = path.to_string_lossy().into_owned();
        if cfg!(any(target_os = "macos", target_os = "ios", target_os = "windows")) {
            key.to_lowercase()
        } else {
            key
        }
    }

    /// Returns false if this destination is already spoken for.
    fn claim(&mut self, path: &Path) -> bool {
        self.taken.insert(Self::key(path))
    }
}

/// Plan one file: format the pattern, sanitize it into a path, and decide
/// whether the file can actually go there.
///
/// Always yields an entry. A file that can't move is still a row in the plan,
/// carrying the destination it was headed for and why it isn't going.
fn plan_single_move(
    source: &Path,
    track_id: Option<i64>,
    metadata: &TrackMetadata,
    pattern: &str,
    base_dir: &Path,
    dests: &mut DestinationLedger,
) -> PlanEntry {
    let entry = |to: Option<PathBuf>, outcome: PlanOutcome| PlanEntry {
        track_id,
        from: source.to_path_buf(),
        to,
        ancillary: Vec::new(),
        outcome,
    };
    // Everything before a destination exists is an error with nothing to point at.
    macro_rules! bail {
        ($reason:expr) => {
            return entry(None, PlanOutcome::Error($reason))
        };
    }

    let relative = match format::format(pattern, metadata) {
        Ok(r) => r,
        Err(e) => bail!(format!("format error: {e}")),
    };

    if relative.is_empty() {
        bail!("format string produced empty path".to_string());
    }

    let sanitized = match sanitize_relative_path(&relative) {
        Ok(p) => p,
        Err(e) => bail!(e),
    };

    // Preserve the original file extension.
    // Don't use with_extension() — it replaces after the LAST dot, which
    // destroys titles containing dots (e.g. "0111. Bicep - TANGZ II" → "0111.flac").
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("flac");
    let Some(stem) = sanitized.file_name().and_then(|n| n.to_str()) else {
        bail!("format string produced an unusable file name".to_string());
    };
    // Leave room for the extension, so a long title is shortened rather than
    // previewing cleanly and failing with ENAMETOOLONG at move time.
    let stem = truncate_bytes(stem, MAX_FILE_NAME_BYTES.saturating_sub(ext.len() + 1)).trim_end();
    if stem.is_empty() {
        bail!("format string produced an empty file name".to_string());
    }
    let mut dest = base_dir.to_path_buf();
    if let Some(parent) = sanitized.parent() {
        dest.push(parent);
    }
    dest.push(format!("{stem}.{ext}"));

    // Safety: verify dest stays under base_dir (defense-in-depth against path traversal).
    if !dest.starts_with(base_dir) {
        let reason = format!(
            "path traversal blocked: destination {} escapes base dir {}",
            dest.display(),
            base_dir.display()
        );
        return entry(Some(dest), PlanOutcome::Error(reason));
    }

    if source == dest {
        // Already in place — claim the name anyway so nothing else targets it.
        dests.claim(&dest);
        return entry(Some(dest), PlanOutcome::Unchanged);
    }

    if !dests.claim(&dest) {
        return entry(
            Some(dest),
            PlanOutcome::Conflict("another file in this run is already going here".into()),
        );
    }

    // Whether something is *already* sitting at the destination is a question
    // for the filesystem, and this function deliberately does not ask one —
    // see `check_against_disk`.
    PlanEntry {
        track_id,
        from: source.to_path_buf(),
        to: Some(dest),
        ancillary: Vec::new(),
        outcome: PlanOutcome::Move,
    }
}

/// Ask the filesystem the two questions formatting cannot answer: whether a
/// destination is already occupied, and what ancillary files travel with each
/// move.
///
/// Separate from planning because it is the only part that touches the disk. A
/// preview that reruns on every keystroke wants the pure half immediately and
/// this afterwards; an execute wants both before it moves anything.
pub fn check_against_disk(result: &mut OrganizeResult, move_ancillary: bool) {
    let mut dests = DestinationLedger::default();
    let mut planned_ancillary: HashSet<PathBuf> = HashSet::new();
    // One directory read per source folder. An album is one folder and a dozen
    // tracks, so doing this per file repeated the same readdir a dozen times.
    let mut ancillary_by_dir: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    for entry in &mut result.entries {
        let (PlanOutcome::Move, Some(dest)) = (&entry.outcome, entry.to.clone()) else {
            continue;
        };

        // A destination that resolves to the source itself is a case-only
        // rename, which is a real move; anything else already there would be
        // overwritten.
        if dest.exists() && !paths_equal(&entry.from, &dest) {
            entry.outcome =
                PlanOutcome::Conflict("a file is already here — it would be overwritten".into());
            continue;
        }
        dests.claim(&dest);

        if !move_ancillary {
            continue;
        }
        let source_dir = entry.from.parent().unwrap_or(Path::new("."));
        let dest_dir = dest.parent().unwrap_or(Path::new("."));
        if source_dir == dest_dir {
            continue;
        }
        let candidates = ancillary_by_dir
            .entry(source_dir.to_path_buf())
            .or_insert_with(|| find_ancillary_files(source_dir))
            .clone();
        for anc_path in candidates {
            if planned_ancillary.contains(&anc_path) {
                continue;
            }
            let Some(anc_name) = anc_path.file_name() else {
                continue;
            };
            let anc_dest = dest_dir.join(anc_name);
            // Artwork already at the destination is left alone rather than
            // overwritten; the audio file is what matters here.
            if anc_dest.exists() || !dests.claim(&anc_dest) {
                continue;
            }
            planned_ancillary.insert(anc_path.clone());
            entry.ancillary.push((anc_path, anc_dest));
        }
    }
}

fn resolve_from_rows(rows: Vec<TrackRow>, albums: &HashMap<i64, AlbumFacts>) -> Vec<ResolvedTrack> {
    let fallback = AlbumFacts::default();
    rows.into_iter()
        .filter_map(|track| {
            let source = PathBuf::from(track.path.as_ref()?);
            if !source.exists() {
                return None; // file gone, skip
            }
            let facts = track
                .album_id
                .and_then(|id| albums.get(&id))
                .unwrap_or(&fallback);
            Some(ResolvedTrack {
                source,
                track_id: Some(track.id),
                metadata: Ok(TrackMetadata::from_track_row(&track, facts)),
            })
        })
        .collect()
}

fn read_tag_metadata(source: &Path) -> Result<TrackMetadata, String> {
    if !source.exists() {
        return Err("file not found".to_string());
    }
    crate::index::metadata::read_metadata(source)
        .map(|m| TrackMetadata::from_file_meta(&m))
        .map_err(|e| format!("metadata error: {e}"))
}

/// Resolve arbitrary paths: library rows where we have them, file tags otherwise.
/// Preview and execute both come through here, so both see the same metadata.
fn resolve_from_paths(
    db: &Database,
    paths: &[PathBuf],
    albums: &HashMap<i64, AlbumFacts>,
) -> Result<Vec<ResolvedTrack>, OrganizeError> {
    use rayon::prelude::*;

    let path_strings: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let known = queries::tracks_by_paths(&db.conn, &path_strings)?;

    // Tag reads are the expensive part, so only the unknown files pay for them.
    let mut tagged: HashMap<PathBuf, Result<TrackMetadata, String>> = paths
        .par_iter()
        .filter(|p| !known.contains_key(p.to_string_lossy().as_ref()))
        .map(|p| (p.clone(), read_tag_metadata(p)))
        .collect();

    let fallback = AlbumFacts::default();
    let mut resolved = Vec::with_capacity(paths.len());
    for (path, path_str) in paths.iter().zip(&path_strings) {
        let entry = match known.get(path_str) {
            Some(track) => {
                let facts = track
                    .album_id
                    .and_then(|id| albums.get(&id))
                    .unwrap_or(&fallback);
                ResolvedTrack {
                    source: path.clone(),
                    track_id: Some(track.id),
                    metadata: Ok(TrackMetadata::from_track_row(track, facts)),
                }
            }
            None => ResolvedTrack {
                source: path.clone(),
                track_id: None,
                metadata: tagged
                    .remove(path)
                    .unwrap_or_else(|| Err("duplicate path in selection".to_string())),
            },
        };
        resolved.push(entry);
    }
    Ok(resolved)
}

/// A selection with every read already done: library rows, album facts, and
/// tags for files the library has never seen.
///
/// This is the half that costs something. Generating destinations from it is
/// pure string work, so a preview that reruns as a pattern is typed resolves
/// once here and formats many times against the result.
pub struct ResolvedSelection {
    tracks: Vec<ResolvedTrack>,
}

impl ResolvedSelection {
    /// How many files resolved to something with a local path. Fewer than were
    /// asked for means the rest are remote-only or gone from disk.
    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
}

/// Read a selection out of the library. `track_ids` of `None` means all of it.
///
/// Database reads and a `stat` per file, so it belongs off whatever thread is
/// drawing — but it only has to happen once per selection.
pub fn resolve(
    db: &Database,
    track_ids: Option<&[i64]>,
) -> Result<ResolvedSelection, OrganizeError> {
    let selection = match track_ids {
        Some(ids) => Selection::TrackIds(ids),
        None => Selection::All,
    };
    resolve_selection(db, selection)
}

/// Read a selection of file paths, which may or may not be in the library.
/// Unknown files pay for a tag read; known ones come from their row.
pub fn resolve_paths(db: &Database, paths: &[PathBuf]) -> Result<ResolvedSelection, OrganizeError> {
    resolve_selection(db, Selection::Paths(paths))
}

fn resolve_selection(
    db: &Database,
    selection: Selection<'_>,
) -> Result<ResolvedSelection, OrganizeError> {
    let albums = load_album_facts(&db.conn)?;
    let tracks = match selection {
        Selection::All => resolve_from_rows(queries::all_tracks(&db.conn)?, &albums),
        Selection::TrackIds(ids) => {
            let mut rows = Vec::with_capacity(ids.len());
            for &id in ids {
                if let Some(row) = queries::get_track_row(&db.conn, id)? {
                    rows.push(row);
                }
            }
            resolve_from_rows(rows, &albums)
        }
        Selection::Paths(paths) => resolve_from_paths(db, paths, &albums)?,
    };
    Ok(ResolvedSelection { tracks })
}

/// Turn a pattern into destinations. **Touches no files at all.**
///
/// Everything here is formatting the pattern, sanitising what it produced, and
/// checking the result against the destinations this same run has already
/// claimed. That is fast enough to run on every keystroke, which is the whole
/// reason it is separate from `check_against_disk`.
pub fn generate(selection: &ResolvedSelection, pattern: &str, base_dir: &Path) -> OrganizeResult {
    let mut entries = Vec::with_capacity(selection.tracks.len());
    let mut dests = DestinationLedger::default();

    for track in &selection.tracks {
        let metadata = match &track.metadata {
            Ok(m) => m,
            Err(msg) => {
                entries.push(PlanEntry {
                    track_id: track.track_id,
                    from: track.source.clone(),
                    to: None,
                    ancillary: Vec::new(),
                    outcome: PlanOutcome::Error(msg.clone()),
                });
                continue;
            }
        };
        entries.push(plan_single_move(
            &track.source,
            track.track_id,
            metadata,
            pattern,
            base_dir,
            &mut dests,
        ));
    }

    OrganizeResult { entries }
}

/// Resolve, generate, and optionally ask the disk. Every entry point plans
/// through here, so a preview and the execute that follows it produce the same
/// destinations from the same metadata.
fn plan(
    db: &Database,
    selection: Selection<'_>,
    pattern: &str,
    base_dir: &Path,
    check_disk: bool,
) -> Result<OrganizeResult, OrganizeError> {
    let resolved = resolve_selection(db, selection)?;
    let mut result = generate(&resolved, pattern, base_dir);
    if check_disk {
        check_against_disk(&mut result, move_ancillary());
    }
    Ok(result)
}

/// Plan, then carry out the moves: each file's database rows and its rename land
/// together or not at all.
fn run(
    db: &Database,
    selection: Selection<'_>,
    pattern: &str,
    base_dir: &Path,
) -> Result<OrganizeResult, OrganizeError> {
    let mut result = plan(db, selection, pattern, base_dir, true)?;

    let pending: Vec<FileMove> = result
        .entries
        .iter()
        .filter_map(PlanEntry::as_move)
        .collect();
    if pending.is_empty() {
        return Ok(result);
    }

    check_free_space(&pending, base_dir)?;

    let batch_id = batch_id();
    let floors = cleanup_floors(Some(base_dir));

    // The plan is the report: a move that fails has its own row demoted to an
    // error, so the caller sees the same table it confirmed, now saying what
    // actually happened to each file.
    for file_move in pending {
        let failure = match execute_single_move(db, &file_move, &batch_id, &floors) {
            Ok(()) => verify_move(&file_move).err(),
            Err(e) => Some(e.to_string()),
        };
        let Some(reason) = failure else { continue };
        if let Some(entry) = result.entries.iter_mut().find(|e| e.from == file_move.from) {
            entry.outcome = PlanOutcome::Error(reason);
        }
    }

    Ok(result)
}

/// Preview what would happen without moving files.
///
/// `check_disk` is what finds destinations that are already occupied and the
/// ancillary files travelling with each move. It is a `stat` per file and a
/// directory read per source folder, and it changes none of the destinations —
/// so a preview that reruns as a pattern is typed leaves it off and fills it in
/// afterwards.
pub fn preview(
    db: &Database,
    pattern: &str,
    base_dir: Option<&Path>,
    check_disk: bool,
) -> Result<OrganizeResult, OrganizeError> {
    let base = resolve_base_dir(base_dir)?;
    plan(db, Selection::All, pattern, &base, check_disk)
}

/// Execute the moves: rename files, update DB, log for undo.
pub fn execute(
    db: &Database,
    pattern: &str,
    base_dir: Option<&Path>,
) -> Result<OrganizeResult, OrganizeError> {
    let base = resolve_base_dir(base_dir)?;
    run(db, Selection::All, pattern, &base)
}

/// Preview organize for a specific set of tracks.
pub fn preview_for_tracks(
    db: &Database,
    track_ids: &[i64],
    pattern: &str,
    base_dir: Option<&Path>,
    check_disk: bool,
) -> Result<OrganizeResult, OrganizeError> {
    let base = resolve_base_dir(base_dir)?;
    plan(
        db,
        Selection::TrackIds(track_ids),
        pattern,
        &base,
        check_disk,
    )
}

/// Execute organize for a specific set of tracks.
pub fn execute_for_tracks(
    db: &Database,
    track_ids: &[i64],
    pattern: &str,
    base_dir: Option<&Path>,
) -> Result<OrganizeResult, OrganizeError> {
    let base = resolve_base_dir(base_dir)?;
    run(db, Selection::TrackIds(track_ids), pattern, &base)
}

/// Preview organize for file paths, which may or may not be in the library.
pub fn preview_for_paths(
    paths: &[PathBuf],
    pattern: &str,
    base_dir: Option<&Path>,
    check_disk: bool,
) -> Result<OrganizeResult, OrganizeError> {
    let db = Database::open_default()?;
    let base = resolve_base_dir(base_dir)?;
    plan(&db, Selection::Paths(paths), pattern, &base, check_disk)
}

/// Execute organize for file paths. Requires the library database: without it there is
/// nowhere to record the moves, and an organize that can't be undone isn't offered.
pub fn execute_for_paths(
    paths: &[PathBuf],
    pattern: &str,
    base_dir: Option<&Path>,
) -> Result<OrganizeResult, OrganizeError> {
    let db = Database::open_default()?;
    let base = resolve_base_dir(base_dir)?;
    run(&db, Selection::Paths(paths), pattern, &base)
}

/// Verify a move actually happened — dest exists and source is gone.
fn verify_move(file_move: &FileMove) -> Result<(), String> {
    if !file_move.to.exists() {
        return Err(format!(
            "destination not found after move: {}",
            file_move.to.display()
        ));
    }
    if file_move.from.exists() && !paths_equal(&file_move.from, &file_move.to) {
        return Err(format!(
            "source still exists after move: {}",
            file_move.from.display()
        ));
    }
    Ok(())
}

fn log_move(
    conn: &Connection,
    batch_id: &str,
    track_id: Option<i64>,
    from: &Path,
    to: &Path,
    size: Option<u64>,
    mtime: Option<i64>,
) -> Result<(), OrganizeError> {
    conn.execute(
        "INSERT INTO organize_log (batch_id, track_id, from_path, to_path, size_bytes, mtime)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            batch_id,
            track_id,
            from.to_string_lossy().as_ref(),
            to.to_string_lossy().as_ref(),
            size.map(|s| s as i64),
            mtime,
        ],
    )?;
    Ok(())
}

/// Point every path-keyed row at the file's new location.
///
/// `tracks.path` and `scan_cache.path` are UNIQUE, so a move onto a path another row
/// already claims fails here — inside the caller's transaction, before the file itself
/// is touched.
fn rewrite_path_references(conn: &Connection, old: &Path, new: &Path) -> Result<(), OrganizeError> {
    let old_lossy = old.to_string_lossy();
    let new_lossy = new.to_string_lossy();
    let old_path = old_lossy.as_ref();
    let new_path = new_lossy.as_ref();

    conn.execute(
        "UPDATE tracks SET path = ?1 WHERE path = ?2",
        params![new_path, old_path],
    )?;
    conn.execute(
        "UPDATE tracks SET cached_path = ?1 WHERE cached_path = ?2",
        params![new_path, old_path],
    )?;
    conn.execute(
        "UPDATE scan_cache SET path = ?1 WHERE path = ?2",
        params![new_path, old_path],
    )?;
    // The destination may already be starred from an earlier move; OR REPLACE
    // leaves exactly one favourite row rather than failing on the primary key.
    conn.execute(
        "UPDATE OR REPLACE favourites SET track_path = ?1 WHERE track_path = ?2",
        params![new_path, old_path],
    )?;
    conn.execute(
        "UPDATE playback_state SET cursor_id = ?1 WHERE cursor_id = ?2",
        params![new_path, old_path],
    )?;
    rewrite_queue_json(conn, old_path, new_path)?;
    Ok(())
}

/// Rewrite paths inside the saved session's serialized queue.
///
/// Playlists need no equivalent: they point at library rows, and a row's path
/// changing is a column this function has already updated.
fn rewrite_queue_json(
    conn: &Connection,
    old_path: &str,
    new_path: &str,
) -> Result<(), OrganizeError> {
    let mut stmt =
        conn.prepare("SELECT id, queue_json FROM playback_state WHERE instr(queue_json, ?1) > 0")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map(params![old_path], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    for (id, json) in rows {
        let Ok(mut items) = serde_json::from_str::<Vec<PersistedQueueItem>>(&json) else {
            continue;
        };
        let mut changed = false;
        for item in &mut items {
            if item.path == old_path {
                item.path = new_path.to_string();
                changed = true;
            }
        }
        if !changed {
            continue;
        }
        let Ok(updated) = serde_json::to_string(&items) else {
            continue;
        };
        conn.execute(
            "UPDATE playback_state SET queue_json = ?1 WHERE id = ?2",
            params![updated, id],
        )?;
    }
    Ok(())
}

/// Execute a single file move: write the database rows first, then move the file.
/// A constraint violation therefore aborts before anything on disk changes, and a
/// failed rename rolls the rows back.
fn execute_single_move(
    db: &Database,
    file_move: &FileMove,
    batch_id: &str,
    floors: &[PathBuf],
) -> Result<(), OrganizeError> {
    if let Some(parent) = file_move.to.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let source_meta = std::fs::metadata(&file_move.from)?;
    let size = source_meta.len();
    let mtime = mtime_secs(&source_meta);

    let tx = db.conn.unchecked_transaction()?;
    log_move(
        &tx,
        batch_id,
        file_move.track_id,
        &file_move.from,
        &file_move.to,
        Some(size),
        mtime,
    )?;
    rewrite_path_references(&tx, &file_move.from, &file_move.to)?;

    // Dropping `tx` on the way out of this `?` rolls the rows back.
    move_file(&file_move.from, &file_move.to)?;

    let mut moved_ancillary: Vec<(&PathBuf, &PathBuf)> = Vec::new();
    let mut failure = None;
    for (anc_from, anc_to) in &file_move.ancillary {
        if let Some(parent) = anc_to.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            continue;
        }
        // Best-effort — artwork that won't move doesn't hold up the audio file.
        match move_file(anc_from, anc_to) {
            Ok(()) => {
                moved_ancillary.push((anc_from, anc_to));
                let meta = std::fs::metadata(anc_to).ok();
                if let Err(e) = log_move(
                    &tx,
                    batch_id,
                    None,
                    anc_from,
                    anc_to,
                    meta.as_ref().map(|m| m.len()),
                    meta.as_ref().and_then(mtime_secs),
                ) {
                    failure = Some(e);
                    break;
                }
            }
            Err(e) => log::warn!(
                "failed to move ancillary file {}: {}",
                anc_from.display(),
                e
            ),
        }
    }

    let outcome = match failure {
        Some(e) => Err(e),
        None => tx.commit().map_err(OrganizeError::from),
    };

    if let Err(e) = outcome {
        // The rows rolled back, so nothing records these files as moved and nothing
        // could undo them. Put them back.
        for (anc_from, anc_to) in moved_ancillary {
            let _ = move_file(anc_to, anc_from);
        }
        let _ = move_file(&file_move.to, &file_move.from);
        return Err(e);
    }

    if let Some(source_dir) = file_move.from.parent() {
        remove_empty_dirs(source_dir, floors);
    }

    Ok(())
}

/// Undo the most recent organize batch.
///
/// Each entry is restored only when the original path is still free and the moved file
/// is still the one that was logged. Anything else is reported and left in the log, so
/// a single blocked file doesn't strand the rest of the batch.
pub fn undo(db: &Database) -> Result<UndoResult, OrganizeError> {
    // Newest batch by primary key: created_at only has one-second resolution, so two
    // batches in the same second would tie.
    let batch_id: String = db
        .conn
        .query_row(
            "SELECT batch_id FROM organize_log ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| OrganizeError::NothingToUndo)?;

    let mut stmt = db.conn.prepare(
        "SELECT id, from_path, to_path, size_bytes, mtime FROM organize_log
         WHERE batch_id = ?1 ORDER BY id DESC",
    )?;

    let entries: Vec<UndoEntry> = stmt
        .query_map(params![batch_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let floors = cleanup_floors(None);
    let mut result = UndoResult::default();

    for (log_id, from_path, to_path, size, mtime) in &entries {
        let to = Path::new(to_path);
        let from = Path::new(from_path);

        if !to.exists() {
            // Already moved back or deleted — drop the log row.
            db.conn
                .execute("DELETE FROM organize_log WHERE id = ?1", params![log_id])?;
            continue;
        }

        if from.exists() && !paths_equal(from, to) {
            result.errors.push((
                from.to_path_buf(),
                format!(
                    "another file now occupies the original path; {} left in place",
                    to.display()
                ),
            ));
            continue;
        }

        if let Err(msg) = matches_logged_file(to, *size, *mtime) {
            result.errors.push((to.to_path_buf(), msg));
            continue;
        }

        if let Some(parent) = from.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            result.errors.push((from.to_path_buf(), e.to_string()));
            continue;
        }

        let tx = db.conn.unchecked_transaction()?;
        if let Err(e) = rewrite_path_references(&tx, to, from) {
            result.errors.push((to.to_path_buf(), e.to_string()));
            continue;
        }
        if let Err(e) = move_file(to, from) {
            result.errors.push((to.to_path_buf(), e.to_string()));
            continue;
        }
        if let Err(e) = tx.execute("DELETE FROM organize_log WHERE id = ?1", params![log_id]) {
            let _ = move_file(from, to);
            result.errors.push((to.to_path_buf(), e.to_string()));
            continue;
        }
        if let Err(e) = tx.commit() {
            let _ = move_file(from, to);
            result.errors.push((to.to_path_buf(), e.to_string()));
            continue;
        }

        if let Some(parent) = to.parent() {
            remove_empty_dirs(parent, &floors);
        }

        result.restored += 1;
    }

    Ok(result)
}

/// Confirm the file at a logged destination is still the file that was moved there.
/// Rows written before size/mtime were recorded carry neither and are accepted.
fn matches_logged_file(path: &Path, size: Option<i64>, mtime: Option<i64>) -> Result<(), String> {
    let (Some(size), Some(mtime)) = (size, mtime) else {
        return Ok(());
    };
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() != size as u64 {
        return Err(format!(
            "{} has changed since it was moved (size differs); left in place",
            path.display()
        ));
    }
    if mtime_secs(&meta).is_some_and(|current| current != mtime) {
        return Err(format!(
            "{} has changed since it was moved (modification time differs); left in place",
            path.display()
        ));
    }
    Ok(())
}

fn mtime_secs(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Directories an empty-directory sweep must never remove or climb past.
fn cleanup_floors(base: Option<&Path>) -> Vec<PathBuf> {
    let mut floors: Vec<PathBuf> = base.map(Path::to_path_buf).into_iter().collect();
    if let Ok(config) = crate::config::Config::load() {
        floors.extend(config.library.folders);
    }
    floors
}

/// Remove the directory a file just left, and its now-empty parents — but never a
/// configured library root, and never anything above one.
fn remove_empty_dirs(start: &Path, floors: &[PathBuf]) {
    let mut current = start.to_path_buf();
    loop {
        if floors.iter().any(|floor| floor == &current) {
            break;
        }
        let empty = std::fs::read_dir(&current)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if !empty || std::fs::remove_dir(&current).is_err() {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        // Only keep climbing inside a directory the run was told about.
        if !floors
            .iter()
            .any(|floor| parent.starts_with(floor) && parent != floor.as_path())
        {
            break;
        }
        current = parent.to_path_buf();
    }
}

/// Move a file, never overwriting whatever is already at the destination.
fn move_file(from: &Path, to: &Path) -> Result<(), OrganizeError> {
    if from == to {
        return Ok(());
    }
    if paths_equal(from, to) {
        // Same file under a different spelling — a case-only rename on a
        // case-insensitive filesystem. Reserving the destination would land on
        // the source itself, so it goes via a temporary name.
        return rename_via_temp(from, to);
    }

    // Claim the name atomically: nothing can slip into the destination between
    // this check and the rename below.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
    {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            return Err(OrganizeError::DestinationExists(to.to_path_buf()));
        }
        Err(e) => return Err(e.into()),
    }

    match transfer(from, to) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Don't leave the empty placeholder behind.
            let _ = std::fs::remove_file(to);
            Err(e)
        }
    }
}

fn rename_via_temp(from: &Path, to: &Path) -> Result<(), OrganizeError> {
    let temp = temp_sibling(to);
    std::fs::rename(from, &temp)?;
    match std::fs::rename(&temp, to) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::rename(&temp, from);
            Err(e.into())
        }
    }
}

/// Rename, falling back to a verified copy when the destination is on another filesystem.
fn transfer(from: &Path, to: &Path) -> Result<(), OrganizeError> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // EXDEV (18): cross-device link.
        Err(e) if e.raw_os_error() == Some(18) => copy_across_devices(from, to),
        Err(e) => Err(e.into()),
    }
}

/// Copy to a temporary file, flush it to disk, verify its length, and only then
/// drop the original. A crash at any point leaves the source intact.
fn copy_across_devices(from: &Path, to: &Path) -> Result<(), OrganizeError> {
    let source_meta = std::fs::metadata(from)?;
    let expected = source_meta.len();
    let temp = temp_sibling(to);

    let copied = {
        let mut reader = std::fs::File::open(from)?;
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let copied = std::io::copy(&mut reader, &mut writer)?;
        // std::io::copy returning Ok only means the bytes reached the page cache.
        writer.sync_all()?;
        if let Ok(modified) = source_meta.modified() {
            let _ = writer.set_modified(modified);
        }
        copied
    };

    let written = std::fs::metadata(&temp).map(|m| m.len()).unwrap_or(0);
    if copied != expected || written != expected {
        let _ = std::fs::remove_file(&temp);
        return Err(OrganizeError::ShortCopy {
            path: from.to_path_buf(),
            expected,
            copied: copied.min(written),
        });
    }

    if let Err(e) = std::fs::rename(&temp, to) {
        let _ = std::fs::remove_file(&temp);
        return Err(e.into());
    }
    std::fs::remove_file(from)?;
    Ok(())
}

fn temp_sibling(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_file_name(format!(".koan-{}-{}.tmp", std::process::id(), nanos))
}

/// Compare paths for equality, including two spellings of one file on a
/// case-insensitive filesystem.
fn paths_equal(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return ma.dev() == mb.dev() && ma.ino() == mb.ino();
        }
    }
    false
}

/// Refuse a run that can't fit, rather than discovering it partway through.
/// Only files landing on a different filesystem need space.
fn check_free_space(moves: &[FileMove], base_dir: &Path) -> Result<(), OrganizeError> {
    let Some(target) = existing_ancestor(base_dir) else {
        return Ok(());
    };
    let Some(target_device) = device_id(&target) else {
        return Ok(());
    };

    let mut needed = 0u64;
    for file_move in moves {
        if device_id(&file_move.from).is_some_and(|d| d == target_device) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&file_move.from) {
            needed = needed.saturating_add(meta.len());
        }
    }
    if needed == 0 {
        return Ok(());
    }

    match available_bytes(&target) {
        Some(available) if available < needed => {
            Err(OrganizeError::NotEnoughSpace { needed, available })
        }
        _ => Ok(()),
    }
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| p.exists()).map(Path::to_path_buf)
}

#[cfg(unix)]
fn device_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_id(_path: &Path) -> Option<u64> {
    None
}

#[cfg(unix)]
fn available_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // Widths of these fields differ between macOS and Linux.
    (stat.f_bavail as u64).checked_mul(stat.f_frsize as u64)
}

#[cfg(not(unix))]
fn available_bytes(_path: &Path) -> Option<u64> {
    None
}

fn resolve_base_dir(base_dir: Option<&Path>) -> Result<PathBuf, OrganizeError> {
    if let Some(dir) = base_dir {
        return Ok(dir.to_path_buf());
    }

    // Use first configured library folder.
    let config = crate::config::Config::load()
        .map_err(|e| OrganizeError::Io(std::io::Error::other(e.to_string())))?;

    config.library.folders.into_iter().next().ok_or_else(|| {
        OrganizeError::Io(std::io::Error::other(
            "no library folders configured; use --base-dir",
        ))
    })
}

/// Whether cover art and cue sheets travel with the music. A preference, so it
/// is read where it is used rather than threaded through every signature.
fn move_ancillary() -> bool {
    crate::config::Config::load()
        .map(|c| c.organize.move_ancillary)
        .unwrap_or(true)
}

fn batch_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("batch-{}", now.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::TrackMeta;
    use crate::db::schema;
    use tempfile::TempDir;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    fn sample_meta(title: &str, artist: &str, album: &str) -> TrackMeta {
        TrackMeta {
            title: title.into(),
            artist: artist.into(),
            album_artist: Some(artist.into()),
            album: album.into(),
            date: Some("1997-06-16".into()),
            disc: Some(1),
            track_number: Some(1),
            genre: Some("Rock".into()),
            label: None,
            duration_ms: Some(240_000),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1000),
            size_bytes: Some(30_000_000),
            mtime: Some(1700000000),
            path: None,
            source: "local".into(),
            remote_id: None,
            remote_url: None,
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            album_added_at: None,
        }
    }

    fn sample_track_row(title: &str, artist: &str, album: &str) -> TrackRow {
        TrackRow {
            id: 1,
            album_id: Some(1),
            artist_id: Some(1),
            artist_name: artist.into(),
            album_artist_name: artist.into(),
            album_title: album.into(),
            disc: Some(1),
            track_number: Some(1),
            title: title.into(),
            duration_ms: Some(240_000),
            path: Some("/music/test.flac".into()),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1000),
            genre: None,
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        }
    }

    /// Write a file with recognisable contents and register it in the library.
    fn add_track(db: &Database, path: &Path, title: &str, track_number: i32) -> i64 {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("audio bytes for {title}")).unwrap();
        let mut meta = sample_meta(title, "Radiohead", "OK Computer");
        meta.track_number = Some(track_number);
        meta.path = Some(path.to_string_lossy().into_owned());
        queries::upsert_track(&db.conn, &meta).unwrap()
    }

    fn db_path_of(db: &Database, track_id: i64) -> Option<String> {
        db.conn
            .query_row(
                "SELECT path FROM tracks WHERE id = ?1",
                params![track_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn log_rows(db: &Database) -> Vec<(Option<i64>, String, String)> {
        let mut stmt = db
            .conn
            .prepare("SELECT track_id, from_path, to_path FROM organize_log ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    // ---- Metadata + sanitisation ----

    #[test]
    fn track_metadata_provider_fields() {
        let mut track = sample_track_row("Subterranean Homesick Alien", "Radiohead", "OK Computer");
        track.track_number = Some(3);
        track.genre = Some("Alternative".into());

        let album = AlbumFacts {
            date: Some("1997-06-16".into()),
            label: Some("Parlophone".into()),
        };
        let meta = TrackMetadata::from_track_row(&track, &album);
        assert_eq!(
            meta.get_field("title").as_deref(),
            Some("Subterranean Homesick Alien")
        );
        assert_eq!(meta.get_field("artist").as_deref(), Some("Radiohead"));
        assert_eq!(meta.get_field("album artist").as_deref(), Some("Radiohead"));
        assert_eq!(meta.get_field("album").as_deref(), Some("OK Computer"));
        assert_eq!(meta.get_field("tracknumber").as_deref(), Some("03"));
        assert_eq!(meta.get_field("discnumber").as_deref(), Some("1"));
        assert_eq!(meta.get_field("date").as_deref(), Some("1997-06-16"));
        assert_eq!(meta.get_field("label").as_deref(), Some("Parlophone"));
        assert_eq!(meta.get_field("codec").as_deref(), Some("FLAC"));
        assert_eq!(meta.get_field("genre").as_deref(), Some("Alternative"));
        assert_eq!(meta.get_field("nonexistent"), None);
    }

    /// Both providers must populate the same field names, or a preview taken from one
    /// authorises a move planned by the other.
    #[test]
    fn both_metadata_sources_expose_the_same_fields() {
        let mut track = sample_track_row("Airbag", "Radiohead", "OK Computer");
        track.genre = Some("Rock".into());
        let album = AlbumFacts {
            date: Some("1997-06-16".into()),
            label: Some("Parlophone".into()),
        };
        let from_db = TrackMetadata::from_track_row(&track, &album);

        let mut meta = sample_meta("Airbag", "Radiohead", "OK Computer");
        meta.label = Some("Parlophone".into());
        let from_tags = TrackMetadata::from_file_meta(&meta);

        let mut db_fields: Vec<&String> = from_db.fields.keys().collect();
        let mut tag_fields: Vec<&String> = from_tags.fields.keys().collect();
        db_fields.sort();
        tag_fields.sort();
        assert_eq!(db_fields, tag_fields);
    }

    #[test]
    fn sanitize_replaces_illegal_chars() {
        assert_eq!(sanitise_filename("AC/DC"), "AC_DC");
        assert_eq!(sanitise_filename("What?"), "What_");
        assert_eq!(sanitise_filename("a:b*c"), "a_b_c");
        assert_eq!(sanitise_filename("normal"), "normal");
    }

    #[test]
    fn sanitize_relative_path_splits() {
        assert_eq!(
            sanitize_relative_path("Artist/Album/Track").unwrap(),
            PathBuf::from("Artist/Album/Track")
        );
        assert_eq!(
            sanitize_relative_path("Radiohead/(1997) OK Computer/01. Airbag").unwrap(),
            PathBuf::from("Radiohead/(1997) OK Computer/01. Airbag")
        );
    }

    #[test]
    fn sanitize_relative_path_refuses_traversal_and_gaps() {
        // Reinterpreting these silently is what turns one bad pattern into a
        // directory full of overwritten files.
        assert!(sanitize_relative_path("../../../../etc/passwd").is_err());
        assert!(sanitize_relative_path("Artist/../../../outside").is_err());
        assert!(sanitize_relative_path("./Artist/./Album").is_err());
        assert!(sanitize_relative_path("Radiohead/OK Computer/").is_err());
        assert!(sanitize_relative_path("Radiohead//Airbag").is_err());
        assert!(sanitize_relative_path("   /Airbag").is_err());
    }

    #[test]
    fn acdc_artist_name_sanitized() {
        let track = sample_track_row("Highway to Hell", "AC/DC", "Highway to Hell");
        let meta = TrackMetadata::from_track_row(&track, &AlbumFacts::default());
        assert_eq!(meta.get_field("album artist").as_deref(), Some("AC_DC"));
        let result = format::format("%album artist%/%album%/%title%", &meta).unwrap();
        assert_eq!(result, "AC_DC/Highway to Hell/Highway to Hell");
    }

    #[test]
    fn format_string_evaluation() {
        let track = sample_track_row("Airbag", "Radiohead", "OK Computer");
        let album = AlbumFacts {
            date: Some("1997-06-16".into()),
            label: None,
        };
        let meta = TrackMetadata::from_track_row(&track, &album);
        let pattern =
            "%album artist%/['('$left(%date%,4)')' ]%album%/$num(%tracknumber%,2). %title%";
        assert_eq!(
            format::format(pattern, &meta).unwrap(),
            "Radiohead/(1997) OK Computer/01. Airbag"
        );
    }

    #[test]
    fn ancillary_file_detection() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("cover.jpg"), b"img").unwrap();
        std::fs::write(dir.join("cover.png"), b"img").unwrap();
        std::fs::write(dir.join("album.cue"), b"cue").unwrap();
        std::fs::write(dir.join("rip.log"), b"log").unwrap();
        std::fs::write(dir.join("track.flac"), b"audio").unwrap();

        let found = find_ancillary_files(dir);
        assert!(found.iter().any(|p| p.file_name().unwrap() == "cover.jpg"));
        assert!(found.iter().any(|p| p.file_name().unwrap() == "cover.png"));
        assert!(found.iter().any(|p| p.file_name().unwrap() == "album.cue"));
        assert!(found.iter().any(|p| p.file_name().unwrap() == "rip.log"));
        assert!(!found.iter().any(|p| p.file_name().unwrap() == "track.flac"));
    }

    // ---- Preview / execute ----

    #[test]
    fn preview_does_not_move_files() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        add_track(&db, &source, "Airbag", 1);

        let result = preview(
            &db,
            "%album artist%/%album%/%title%",
            Some(tmp.path()),
            true,
        )
        .unwrap();
        assert!(source.exists());
        assert_eq!(result.moved_count(), 1);
    }

    #[test]
    fn execute_moves_files_and_undo_reverts() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        let id = add_track(&db, &source, "Airbag", 1);

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        assert_eq!(result.moved_count(), 1);
        assert_eq!(result.failures().count(), 0);
        assert!(!source.exists());
        let dest = result.moves().next().unwrap().dest().to_path_buf();
        assert!(dest.exists());
        assert_eq!(db_path_of(&db, id).as_deref(), Some(dest.to_str().unwrap()));

        let undone = undo(&db).unwrap();
        assert_eq!(undone.restored, 1);
        assert!(undone.errors.is_empty());
        assert!(source.exists());
        assert!(!dest.exists());
        assert_eq!(
            db_path_of(&db, id).as_deref(),
            Some(source.to_str().unwrap())
        );
    }

    /// The preview a user confirms and the moves that follow must agree. They read
    /// metadata through the same resolver, so a pattern using an album-level field
    /// (here `%label%`) resolves identically in both.
    #[test]
    fn preview_and_execute_agree_on_destinations() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let pattern = "$if2(%label%,%album artist%)/%album%/[$num(%tracknumber%,2). ]%title%";

        let source = tmp.path().join("src/aphex.flac");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"audio").unwrap();
        let mut meta = sample_meta("Xtal", "Aphex Twin", "Selected Ambient Works");
        meta.label = Some("Warp Records".into());
        meta.path = Some(source.to_string_lossy().into_owned());
        queries::upsert_track(&db.conn, &meta).unwrap();

        let previewed = preview(&db, pattern, Some(tmp.path()), true).unwrap();
        assert_eq!(previewed.moved_count(), 1);
        let expected = previewed.moves().next().unwrap().dest().to_path_buf();
        assert!(expected.starts_with(tmp.path().join("Warp Records")));

        let executed = execute(&db, pattern, Some(tmp.path())).unwrap();
        assert_eq!(executed.moved_count(), 1);
        assert_eq!(executed.moves().next().unwrap().dest(), expected);
        assert!(expected.exists());
    }

    /// The whole macOS flow, end to end: files land from outside the library,
    /// get rows where they lie, and organize is what puts them under the music
    /// tree. Nothing about the import knows where they will end up.
    #[test]
    fn imported_files_organize_into_the_library_folder() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("Downloads/rip");
        let library = tmp.path().join("Music");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&library).unwrap();

        let dropped = outside.join("track.wav");
        crate::test_utils::generate_wav(&dropped, 44100, 1, 0.2, 16);

        let imported = crate::index::scanner::import_paths(&db, std::slice::from_ref(&outside));
        assert_eq!(imported.track_ids.len(), 1, "errors: {:?}", imported.errors);

        let result = execute_for_tracks(
            &db,
            &imported.track_ids,
            "%album artist%/%album%/%title%",
            Some(&library),
        )
        .unwrap();

        assert_eq!(result.moved_count(), 1);
        let dest = result.moves().next().unwrap().dest();
        assert!(dest.starts_with(&library), "landed at {}", dest.display());
        assert!(dest.exists());
        assert!(
            !dropped.exists(),
            "the original should have moved, not copied"
        );

        // The row followed the file, so playing it afterwards still works.
        assert_eq!(
            db_path_of(&db, imported.track_ids[0]).as_deref(),
            dest.to_str()
        );
    }

    /// Generation is pure. It is what reruns on every keystroke, so if it ever
    /// starts touching the filesystem this is what says so: the destination is
    /// occupied and the source directory is full of cover art, and neither
    /// shows up until the disk is actually asked.
    #[test]
    fn generate_touches_no_files() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/track.flac");
        add_track(&db, &source, "Airbag", 1);
        std::fs::write(source.parent().unwrap().join("cover.jpg"), b"art").unwrap();

        // Something is already sitting where the pattern points.
        let occupied = tmp.path().join("Radiohead/OK Computer/Airbag.flac");
        std::fs::create_dir_all(occupied.parent().unwrap()).unwrap();
        std::fs::write(&occupied, b"the good rip").unwrap();

        let selection = resolve(&db, None).unwrap();
        let mut result = generate(&selection, "%album artist%/%album%/%title%", tmp.path());

        // Pure pass: a move, no conflict, no ancillary — it has not looked.
        assert_eq!(result.moved_count(), 1);
        assert_eq!(result.conflicts().count(), 0);
        assert!(result.entries[0].ancillary.is_empty());

        // Asking the disk is what finds both.
        check_against_disk(&mut result, true);
        assert_eq!(result.moved_count(), 0);
        assert_eq!(result.conflicts().count(), 1);
        assert_eq!(result.conflicts().next().unwrap().dest(), occupied);
    }

    /// Resolving once and generating many times must agree with planning from
    /// scratch, or the preview would be lying about what execute will do.
    #[test]
    fn generate_agrees_with_a_full_plan() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        add_track(&db, &tmp.path().join("src/a.flac"), "Airbag", 1);
        add_track(&db, &tmp.path().join("src/b.flac"), "Karma Police", 2);
        let pattern = "%album artist%/%album%/%tracknumber%. %title%";

        let selection = resolve(&db, None).unwrap();
        let mut generated = generate(&selection, pattern, tmp.path());
        check_against_disk(&mut generated, true);
        let planned = preview(&db, pattern, Some(tmp.path()), true).unwrap();

        assert_eq!(generated.entries.len(), planned.entries.len());
        for (a, b) in generated.entries.iter().zip(&planned.entries) {
            assert_eq!(a.from, b.from);
            assert_eq!(a.to, b.to);
            assert_eq!(a.outcome, b.outcome);
            assert_eq!(a.ancillary, b.ancillary);
        }
    }

    /// One readdir per source directory, not one per file — the thing that made
    /// a preview over an album on a slow volume cost what it did.
    #[test]
    fn ancillary_is_scanned_once_per_directory() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        for (i, title) in ["Airbag", "Karma Police", "Lucky"].iter().enumerate() {
            add_track(
                &db,
                &tmp.path().join(format!("src/{i}.flac")),
                title,
                i as i32 + 1,
            );
        }
        std::fs::write(tmp.path().join("src/cover.jpg"), b"art").unwrap();

        let result = preview(
            &db,
            "%album artist%/%album%/%title%",
            Some(tmp.path()),
            true,
        )
        .unwrap();

        // The cover travels with exactly one of them, not all three.
        let carrying: Vec<_> = result.moves().filter(|e| !e.ancillary.is_empty()).collect();
        assert_eq!(carrying.len(), 1);
        assert_eq!(carrying[0].ancillary.len(), 1);
    }

    /// The disk pass must not mistake a file for its own obstacle. Nothing
    /// stops a caller planning against paths a previous run already moved, and
    /// a bare `exists()` on the destination says "occupied" for every one of
    /// them.
    #[test]
    fn a_file_already_at_its_destination_is_unchanged_not_a_conflict() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let pattern = "%album artist%/%album%/%title%";
        add_track(&db, &tmp.path().join("src/a.flac"), "Airbag", 1);

        let moved = execute(&db, pattern, Some(tmp.path())).unwrap();
        assert_eq!(moved.moved_count(), 1);

        // Plan again, from the rows as they now stand.
        let again = preview(&db, pattern, Some(tmp.path()), true).unwrap();
        assert_eq!(again.conflicts().count(), 0);
        assert_eq!(again.unchanged_count(), 1);
    }

    #[test]
    fn ancillary_files_stay_put_when_they_are_turned_off() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/a.flac");
        add_track(&db, &source, "Airbag", 1);
        std::fs::write(source.parent().unwrap().join("cover.jpg"), b"art").unwrap();

        let selection = resolve(&db, None).unwrap();
        let mut off = generate(&selection, "%album artist%/%album%/%title%", tmp.path());
        check_against_disk(&mut off, false);
        assert!(off.moves().all(|e| e.ancillary.is_empty()));

        let mut on = generate(&selection, "%album artist%/%album%/%title%", tmp.path());
        check_against_disk(&mut on, true);
        assert_eq!(on.moves().next().unwrap().ancillary.len(), 1);
    }

    // ---- Collisions ----

    #[test]
    fn colliding_destinations_leave_both_files_intact() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let first = tmp.path().join("src/a.flac");
        let second = tmp.path().join("src/b.flac");
        // Same title, different track numbers: two library rows, one destination.
        let first_id = add_track(&db, &first, "Airbag", 1);
        let second_id = add_track(&db, &second, "Airbag", 2);
        let second_bytes = std::fs::read(&second).unwrap();

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();

        assert_eq!(result.moved_count(), 1);

        // The loser is a row in the plan, flagged as a conflict and still
        // carrying the destination it lost — that is what a preview shows.
        let blocked = result.conflicts().next().unwrap();
        assert_eq!(result.conflicts().count(), 1);
        assert_eq!(blocked.from, second);
        assert_eq!(blocked.dest(), result.moves().next().unwrap().dest());

        // The loser stays exactly where it was, byte for byte.
        assert!(second.exists());
        assert_eq!(std::fs::read(&second).unwrap(), second_bytes);
        assert_eq!(
            db_path_of(&db, second_id).as_deref(),
            Some(second.to_str().unwrap())
        );

        let dest = result.moves().next().unwrap().dest();
        assert_eq!(
            std::fs::read(dest).unwrap(),
            b"audio bytes for Airbag".to_vec()
        );
        assert_eq!(
            db_path_of(&db, first_id).as_deref(),
            Some(dest.to_str().unwrap())
        );
    }

    #[test]
    fn existing_destination_is_never_overwritten() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/new.flac");
        add_track(&db, &source, "Airbag", 1);

        // Something unrelated is already sitting at the destination.
        let dest = tmp.path().join("Radiohead/OK Computer/Airbag.flac");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"the good rip").unwrap();

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        assert_eq!(result.moved_count(), 0);

        // Flagged as a conflict against the occupied path, so a preview can say
        // what would have been overwritten before anyone presses the button.
        let blocked = result.conflicts().next().unwrap();
        assert_eq!(result.conflicts().count(), 1);
        assert_eq!(blocked.from, source);
        assert_eq!(blocked.dest(), dest);
        assert!(blocked.outcome.reason().unwrap().contains("overwritten"));

        assert_eq!(std::fs::read(&dest).unwrap(), b"the good rip".to_vec());
        assert!(source.exists());
    }

    /// `move_file` is the last line of defence: even handed a destination that exists,
    /// it refuses rather than replacing it.
    #[test]
    fn move_file_refuses_an_occupied_destination() {
        let tmp = TempDir::new().unwrap();
        let from = tmp.path().join("a.flac");
        let to = tmp.path().join("b.flac");
        std::fs::write(&from, b"source").unwrap();
        std::fs::write(&to, b"keep me").unwrap();

        let err = move_file(&from, &to).unwrap_err();
        assert!(matches!(err, OrganizeError::DestinationExists(_)));
        assert_eq!(std::fs::read(&to).unwrap(), b"keep me".to_vec());
        assert_eq!(std::fs::read(&from).unwrap(), b"source".to_vec());
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn case_only_difference_collides_on_a_case_insensitive_filesystem() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let first = tmp.path().join("src/1.flac");
        let second = tmp.path().join("src/2.flac");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        for (path, title, number) in [(&first, "Rain", 1i32), (&second, "RAIN", 2)] {
            std::fs::write(path, format!("audio bytes for {title}")).unwrap();
            let mut meta = sample_meta(title, "Radiohead", "OK Computer");
            meta.track_number = Some(number);
            meta.path = Some(path.to_string_lossy().into_owned());
            queries::upsert_track(&db.conn, &meta).unwrap();
        }

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        assert_eq!(result.moved_count(), 1);
        assert_eq!(result.failures().count(), 1);
        assert!(second.exists());
        assert_eq!(
            std::fs::read(&second).unwrap(),
            b"audio bytes for RAIN".to_vec()
        );
    }

    /// A rename that only changes case has to go via a temporary name: reserving the
    /// destination would otherwise open the source file itself.
    /// Not on iOS: the simulator's sandboxed filesystem answers `stat` for
    /// `Rain.flac` with ENOENT while `open(O_CREAT|O_EXCL)` on the same name
    /// answers EEXIST — measured, both, in one directory. `paths_equal` reads
    /// `stat`, so the case-only rename it exists to catch cannot be detected
    /// there. Unverified on a device, where the volume is ordinary APFS.
    #[cfg(not(target_os = "ios"))]
    #[test]
    fn case_only_rename_keeps_the_file() {
        let tmp = TempDir::new().unwrap();
        let from = tmp.path().join("rain.flac");
        let to = tmp.path().join("Rain.flac");
        std::fs::write(&from, b"audio bytes").unwrap();

        move_file(&from, &to).unwrap();

        assert_eq!(std::fs::read(&to).unwrap(), b"audio bytes".to_vec());
        let names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["Rain.flac".to_string()]);
    }

    /// The cross-device path copies, flushes and verifies before unlinking the
    /// original, so an interrupted move can never leave a truncated file and no source.
    #[test]
    fn cross_device_copy_verifies_before_dropping_the_source() {
        let tmp = TempDir::new().unwrap();
        let from = tmp.path().join("a.flac");
        let to = tmp.path().join("b.flac");
        let bytes: Vec<u8> = (0..64_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&from, &bytes).unwrap();
        let mtime = std::fs::metadata(&from).unwrap().modified().unwrap();

        copy_across_devices(&from, &to).unwrap();

        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), bytes);
        // Preserved, so scan_cache entries stay valid across a cross-device move.
        assert_eq!(std::fs::metadata(&to).unwrap().modified().unwrap(), mtime);
        // No temporary left behind.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".koan-")
            })
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn free_space_check_ignores_same_device_moves() {
        let tmp = TempDir::new().unwrap();
        let from = tmp.path().join("a.flac");
        std::fs::write(&from, b"audio").unwrap();
        let moves = vec![FileMove {
            track_id: None,
            from,
            to: tmp.path().join("b.flac"),
            ancillary: Vec::new(),
        }];
        // A rename within one filesystem consumes no space.
        assert!(check_free_space(&moves, tmp.path()).is_ok());
    }

    // ---- Bad patterns ----

    #[test]
    fn unknown_function_refuses_the_move() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        add_track(&db, &source, "Airbag", 1);

        // $nun instead of $num.
        let result = execute(
            &db,
            "%album artist%/%album%/$nun(%tracknumber%,2). %title%",
            Some(tmp.path()),
        )
        .unwrap();
        assert_eq!(result.moved_count(), 0);
        assert_eq!(result.failures().count(), 1);
        assert!(result.failure_messages()[0].contains("unknown function"));
        assert!(source.exists());
    }

    /// An empty last component used to append the extension to the parent directory,
    /// pointing every track on an album at one file.
    #[test]
    fn empty_final_component_refuses_the_move() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let first = tmp.path().join("src/a.flac");
        let second = tmp.path().join("src/b.flac");
        add_track(&db, &first, "Airbag", 1);
        add_track(&db, &second, "Karma Police", 2);

        // The conditional resolves to nothing, leaving a trailing separator.
        let result = execute(
            &db,
            "%album artist%/%album%/[%nonexistent field%]",
            Some(tmp.path()),
        )
        .unwrap();

        assert_eq!(result.moved_count(), 0);
        assert_eq!(result.failures().count(), 2);
        assert!(first.exists());
        assert!(second.exists());
        assert!(!tmp.path().join("Radiohead/OK Computer.flac").exists());
    }

    #[test]
    fn long_title_is_truncated_rather_than_failing() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        let title = "a".repeat(300);
        add_track(&db, &source, &title, 1);

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        assert_eq!(
            result.moved_count(),
            1,
            "errors: {:?}",
            result.failure_messages()
        );
        let name = result
            .moves()
            .next()
            .unwrap()
            .dest()
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert!(name.len() <= MAX_FILE_NAME_BYTES);
        assert!(name.ends_with(".flac"));
        assert!(result.moves().next().unwrap().dest().exists());
    }

    // ---- Directory cleanup ----

    #[test]
    fn remove_empty_dirs_never_climbs_past_a_floor() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("library");
        let nested = root.join("artist/album");
        std::fs::create_dir_all(&nested).unwrap();

        remove_empty_dirs(&nested, std::slice::from_ref(&root));

        assert!(!nested.exists());
        assert!(!root.join("artist").exists());
        assert!(root.exists(), "the library root must survive");
    }

    #[test]
    fn remove_empty_dirs_stays_put_outside_any_floor() {
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("incoming/rip");
        std::fs::create_dir_all(&outside).unwrap();

        remove_empty_dirs(&outside, &[tmp.path().join("library")]);

        assert!(!outside.exists());
        assert!(
            tmp.path().join("incoming").exists(),
            "no floor means no climbing"
        );
    }

    #[test]
    fn remove_empty_dirs_never_removes_a_floor_itself() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("library");
        std::fs::create_dir_all(&root).unwrap();

        remove_empty_dirs(&root, std::slice::from_ref(&root));

        assert!(root.exists());
    }

    // ---- Undo ----

    #[test]
    fn undo_refuses_when_the_original_path_is_occupied() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        add_track(&db, &source, "Airbag", 1);

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        let dest = result.moves().next().unwrap().dest().to_path_buf();

        // A different rip lands at the vacated path before the undo.
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"a completely different rip").unwrap();

        let undone = undo(&db).unwrap();
        assert_eq!(undone.restored, 0);
        assert_eq!(undone.errors.len(), 1);
        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"a completely different rip".to_vec()
        );
        assert!(dest.exists());
        // The entry stays in the log so it can be undone once the path is free.
        assert_eq!(log_rows(&db).len(), 1);
    }

    #[test]
    fn undo_refuses_when_the_moved_file_has_been_replaced() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        add_track(&db, &source, "Airbag", 1);

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        let dest = result.moves().next().unwrap().dest().to_path_buf();
        std::fs::write(&dest, b"replaced with something else entirely").unwrap();

        let undone = undo(&db).unwrap();
        assert_eq!(undone.restored, 0);
        assert_eq!(undone.errors.len(), 1);
        assert!(!source.exists());
        assert!(dest.exists());
    }

    /// `created_at` has one-second resolution, so batches are ordered by primary key.
    #[test]
    fn undo_takes_the_newest_batch_when_timestamps_tie() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let older = tmp.path().join("older.flac");
        let newer = tmp.path().join("newer.flac");
        std::fs::write(&older, b"older").unwrap();
        std::fs::write(&newer, b"newer").unwrap();
        let moved_older = tmp.path().join("moved-older.flac");
        let moved_newer = tmp.path().join("moved-newer.flac");
        std::fs::rename(&older, &moved_older).unwrap();
        std::fs::rename(&newer, &moved_newer).unwrap();

        for (batch, from, to) in [
            ("batch-1", &older, &moved_older),
            ("batch-2", &newer, &moved_newer),
        ] {
            db.conn
                .execute(
                    "INSERT INTO organize_log (batch_id, track_id, from_path, to_path, created_at)
                     VALUES (?1, NULL, ?2, ?3, '2025-01-01 00:00:00')",
                    params![
                        batch,
                        from.to_string_lossy().as_ref(),
                        to.to_string_lossy().as_ref()
                    ],
                )
                .unwrap();
        }

        let undone = undo(&db).unwrap();
        assert_eq!(undone.restored, 1);
        assert!(newer.exists(), "the newest batch is the one undone");
        assert!(!older.exists());
    }

    // ---- Database consistency ----

    #[test]
    fn favourites_and_queue_state_follow_the_move() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        add_track(&db, &source, "Airbag", 1);
        let source_str = source.to_string_lossy().into_owned();

        queries::add_favourite(&db.conn, &source).unwrap();
        let item = PersistedQueueItem {
            path: source_str.clone(),
            title: "Airbag".into(),
            artist: "Radiohead".into(),
            album_artist: "Radiohead".into(),
            album: "OK Computer".into(),
            year: None,
            codec: None,
            track_number: Some(1),
            disc: Some(1),
            duration_ms: None,
            db_id: None,
        };
        queries::save_playback_state(&db.conn, &[item], Some(&source_str), 0, false, false)
            .unwrap();

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        let dest = result.moves().next().unwrap().dest().to_path_buf();
        let dest_str = dest.to_string_lossy().into_owned();

        let favourites = queries::load_favourites(&db.conn).unwrap();
        assert!(favourites.contains(&dest));
        assert!(!favourites.contains(&source));

        let state = queries::load_playback_state(&db.conn).unwrap().unwrap();
        assert_eq!(state.items[0].path, dest_str);
        assert_eq!(state.cursor_path.as_deref(), Some(dest_str.as_str()));

        assert_eq!(undo(&db).unwrap().restored, 1);

        let favourites = queries::load_favourites(&db.conn).unwrap();
        assert!(favourites.contains(&source));
        assert!(!favourites.contains(&dest));
        let state = queries::load_playback_state(&db.conn).unwrap().unwrap();
        assert_eq!(state.items[0].path, source_str);
        assert_eq!(state.cursor_path.as_deref(), Some(source_str.as_str()));
    }

    #[test]
    fn scan_cache_follows_the_move() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        let id = add_track(&db, &source, "Airbag", 1);
        db.conn
            .execute(
                "INSERT INTO scan_cache (path, mtime, size, track_id) VALUES (?1, 1, 1, ?2)",
                params![source.to_string_lossy().as_ref(), id],
            )
            .unwrap();

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        let dest = result
            .moves()
            .next()
            .unwrap()
            .dest()
            .to_string_lossy()
            .into_owned();

        let cached: String = db
            .conn
            .query_row(
                "SELECT path FROM scan_cache WHERE track_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cached, dest);
    }

    /// A failure partway through a batch must leave the rest of the run truthful: the
    /// files that moved are in the result and the log, the one that didn't is in neither.
    #[test]
    fn partial_failure_leaves_the_database_and_result_consistent() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let first = tmp.path().join("src/a.flac");
        let clash = tmp.path().join("src/b.flac");
        let third = tmp.path().join("src/c.flac");
        let first_id = add_track(&db, &first, "Airbag", 1);
        let clash_id = add_track(&db, &clash, "Airbag", 2);
        let third_id = add_track(&db, &third, "Karma Police", 3);

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();

        assert_eq!(result.moved_count(), 2);
        assert_eq!(result.failures().count(), 1);

        let logged = log_rows(&db);
        assert_eq!(logged.len(), 2);
        for file_move in result.moves() {
            assert!(file_move.dest().exists());
            assert!(
                logged
                    .iter()
                    .any(|(_, _, to)| Path::new(to) == file_move.dest())
            );
        }

        // The failed file is untouched, in the filesystem and in the database.
        assert!(clash.exists());
        assert_eq!(
            db_path_of(&db, clash_id).as_deref(),
            Some(clash.to_str().unwrap())
        );
        assert_ne!(db_path_of(&db, first_id).as_deref(), first.to_str());
        assert_ne!(db_path_of(&db, third_id).as_deref(), third.to_str());
    }

    /// The TUI organizes a selection of paths. Files the library doesn't know about
    /// still get a log entry, so the whole run can be undone.
    #[test]
    fn unknown_paths_are_logged_and_undoable() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let known = tmp.path().join("src/known.flac");
        add_track(&db, &known, "Airbag", 1);

        let result = run(
            &db,
            Selection::Paths(std::slice::from_ref(&known)),
            "%album artist%/%album%/%title%",
            tmp.path(),
        )
        .unwrap();

        assert_eq!(result.moved_count(), 1);
        let logged = log_rows(&db);
        assert_eq!(logged.len(), 1);
        assert!(logged[0].0.is_some());

        assert_eq!(undo(&db).unwrap().restored, 1);
        assert!(known.exists());
    }

    #[test]
    fn ancillary_files_move_with_the_album() {
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/test.flac");
        add_track(&db, &source, "Airbag", 1);
        std::fs::write(source.parent().unwrap().join("cover.jpg"), b"art").unwrap();

        let result = execute(&db, "%album artist%/%album%/%title%", Some(tmp.path())).unwrap();
        assert_eq!(result.moved_count(), 1);
        let dest_dir = result.moves().next().unwrap().dest().parent().unwrap();
        assert!(dest_dir.join("cover.jpg").exists());

        // Both the audio and the artwork are in the log, so undo restores both.
        assert_eq!(log_rows(&db).len(), 2);
        assert_eq!(undo(&db).unwrap().restored, 2);
        assert!(source.parent().unwrap().join("cover.jpg").exists());
    }

    // ---- Extension handling ----

    #[test]
    fn extension_not_clobbered_by_dots_in_title() {
        // Regression: with_extension() replaces after the LAST dot,
        // destroying titles with dots ("0111. Bicep - TANGZ II" → "0111.flac").
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/CHROMA 011 A.L.O.E II.flac");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"fake").unwrap();

        let mut meta = sample_meta("CHROMA 011 A.L.O.E II", "Bicep", "CHROMA 000");
        meta.track_number = Some(10);
        meta.date = Some("2025-11-21".into());
        meta.path = Some(source.to_string_lossy().into_owned());
        queries::upsert_track(&db.conn, &meta).unwrap();

        let pattern = "%album artist%/['('$left(%date%,4)')' ]%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%";
        let result = preview(&db, pattern, Some(tmp.path()), true).unwrap();
        assert_eq!(result.moved_count(), 1);
        assert_eq!(
            result
                .moves()
                .next()
                .unwrap()
                .dest()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "0110. Bicep - CHROMA 011 A.L.O.E II.flac"
        );
    }

    #[test]
    fn extension_preserved_for_tracknumber_dot() {
        // "0111. Bicep - TANGZ II" must not become "0111.flac"
        let db = test_db();
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src/CHROMA 012 TANGZ II.flac");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"fake").unwrap();

        let mut meta = sample_meta("CHROMA 012 TANGZ II", "Bicep", "CHROMA 000");
        meta.track_number = Some(11);
        meta.date = Some("2025-11-21".into());
        meta.path = Some(source.to_string_lossy().into_owned());
        queries::upsert_track(&db.conn, &meta).unwrap();

        let pattern = "%album artist%/['('$left(%date%,4)')' ]%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%";
        let result = preview(&db, pattern, Some(tmp.path()), true).unwrap();
        assert_eq!(result.moved_count(), 1);
        assert_eq!(
            result
                .moves()
                .next()
                .unwrap()
                .dest()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "0111. Bicep - CHROMA 012 TANGZ II.flac"
        );
    }
}

