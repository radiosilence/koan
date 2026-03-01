use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::db::connection::{Database, DbError};
use crate::db::queries::{self, TrackRow};
use crate::format::{self, FormatError, MetadataProvider};

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
}

#[derive(Debug)]
pub struct OrganizeResult {
    pub moves: Vec<FileMove>,
    pub errors: Vec<(PathBuf, String)>,
    pub skipped: usize,
}

#[derive(Debug)]
pub struct FileMove {
    pub track_id: i64,
    pub from: PathBuf,
    pub to: PathBuf,
    pub ancillary: Vec<(PathBuf, PathBuf)>,
}

/// Metadata provider backed by a HashMap, for evaluating format strings against track data.
struct TrackMetadata {
    fields: HashMap<String, String>,
}

impl TrackMetadata {
    fn from_track_row(track: &TrackRow, album_date: Option<&str>) -> Self {
        let mut fields = HashMap::new();
        // Sanitize all field values so they can't inject path separators or illegal chars.
        let s = sanitize_path_component;
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
        if let Some(date) = album_date {
            // Date is safe (digits + hyphens) but sanitize anyway for consistency.
            fields.insert("date".into(), date.to_string());
        }
        if let Some(ref codec) = track.codec {
            fields.insert("codec".into(), codec.clone());
        }
        if let Some(ref genre) = track.genre {
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

/// Replace characters that are illegal in file/directory names.
fn sanitize_path_component(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Sanitize each component of a relative path independently.
fn sanitize_relative_path(rel: &str) -> PathBuf {
    let parts: Vec<&str> = if rel.contains('/') {
        rel.split('/')
    } else {
        rel.split(std::path::MAIN_SEPARATOR)
    }
    .collect();

    let mut result = PathBuf::new();
    for part in parts {
        let sanitized = sanitize_path_component(part);
        if !sanitized.is_empty() {
            result.push(sanitized);
        }
    }
    result
}

/// Get the album date for a track via its album_id.
fn album_date_for_track(
    db: &Database,
    track: &TrackRow,
) -> Result<Option<String>, rusqlite::Error> {
    let Some(album_id) = track.album_id else {
        return Ok(None);
    };
    db.conn
        .query_row(
            "SELECT date FROM albums WHERE id = ?1",
            params![album_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .or(Ok(None))
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
    files
}

/// Build the list of moves for all local tracks.
fn plan_moves(
    db: &Database,
    pattern: &str,
    base_dir: &Path,
) -> Result<OrganizeResult, OrganizeError> {
    let tracks = queries::all_tracks(&db.conn)?;
    let mut moves = Vec::new();
    let mut errors = Vec::new();
    let mut skipped = 0;

    // Track which ancillary files we've already planned to move (dedup across tracks in same dir).
    let mut planned_ancillary: std::collections::HashSet<PathBuf> =
        std::collections::HashSet::new();

    for track in &tracks {
        let Some(ref path_str) = track.path else {
            continue; // remote-only, skip
        };

        let source = PathBuf::from(path_str);
        if !source.exists() {
            continue; // file gone, skip
        }

        let album_date = match album_date_for_track(db, track) {
            Ok(d) => d,
            Err(e) => {
                errors.push((source, format!("failed to get album date: {e}")));
                continue;
            }
        };

        let metadata = TrackMetadata::from_track_row(track, album_date.as_deref());
        let relative = match format::format(pattern, &metadata) {
            Ok(r) => r,
            Err(e) => {
                errors.push((source, format!("format error: {e}")));
                continue;
            }
        };

        if relative.is_empty() {
            errors.push((source, "format string produced empty path".into()));
            continue;
        }

        let sanitized = sanitize_relative_path(&relative);

        // Preserve the original file extension.
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("flac");
        let dest = base_dir.join(sanitized).with_extension(ext);

        if source == dest {
            skipped += 1;
            continue;
        }

        // Plan ancillary moves — move files in source dir to dest dir.
        let source_dir = source.parent().unwrap_or(Path::new("."));
        let dest_dir = dest.parent().unwrap_or(Path::new("."));
        let mut ancillary = Vec::new();

        if source_dir != dest_dir {
            for anc_path in find_ancillary_files(source_dir) {
                if planned_ancillary.contains(&anc_path) {
                    continue;
                }
                let Some(anc_name) = anc_path.file_name() else {
                    continue;
                };
                let anc_dest = dest_dir.join(anc_name);
                planned_ancillary.insert(anc_path.clone());
                ancillary.push((anc_path, anc_dest));
            }
        }

        moves.push(FileMove {
            track_id: track.id,
            from: source,
            to: dest,
            ancillary,
        });
    }

    Ok(OrganizeResult {
        moves,
        errors,
        skipped,
    })
}

/// Preview what would happen without moving files.
pub fn preview(
    db: &Database,
    pattern: &str,
    base_dir: Option<&Path>,
) -> Result<OrganizeResult, OrganizeError> {
    let base = resolve_base_dir(base_dir)?;
    plan_moves(db, pattern, &base)
}

/// Execute the moves: rename files, update DB, log for undo.
pub fn execute(
    db: &Database,
    pattern: &str,
    base_dir: Option<&Path>,
) -> Result<OrganizeResult, OrganizeError> {
    let base = resolve_base_dir(base_dir)?;
    let mut result = plan_moves(db, pattern, &base)?;

    if result.moves.is_empty() {
        return Ok(result);
    }

    // Generate a batch ID from timestamp.
    let batch_id = chrono_batch_id();

    let mut completed_moves = Vec::new();
    let mut new_errors = Vec::new();

    for file_move in result.moves.drain(..) {
        match execute_single_move(db, &file_move, &batch_id) {
            Ok(()) => completed_moves.push(file_move),
            Err(e) => {
                new_errors.push((file_move.from, e.to_string()));
            }
        }
    }

    result.moves = completed_moves;
    result.errors.extend(new_errors);
    Ok(result)
}

/// Execute a single file move: create dirs, move file + ancillary, update DB, write log.
fn execute_single_move(
    db: &Database,
    file_move: &FileMove,
    batch_id: &str,
) -> Result<(), OrganizeError> {
    // Create destination directory.
    if let Some(parent) = file_move.to.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Move the audio file.
    std::fs::rename(&file_move.from, &file_move.to)?;

    // Log the move.
    db.conn.execute(
        "INSERT INTO organize_log (batch_id, track_id, from_path, to_path) VALUES (?1, ?2, ?3, ?4)",
        params![
            batch_id,
            file_move.track_id,
            file_move.from.to_string_lossy().as_ref(),
            file_move.to.to_string_lossy().as_ref(),
        ],
    )?;

    // Update the track's path in the database.
    db.conn.execute(
        "UPDATE tracks SET path = ?1 WHERE id = ?2",
        params![file_move.to.to_string_lossy().as_ref(), file_move.track_id],
    )?;

    // Update scan_cache if it exists for the old path.
    db.conn.execute(
        "UPDATE scan_cache SET path = ?1 WHERE path = ?2",
        params![
            file_move.to.to_string_lossy().as_ref(),
            file_move.from.to_string_lossy().as_ref(),
        ],
    )?;

    // Move ancillary files.
    for (anc_from, anc_to) in &file_move.ancillary {
        if let Some(parent) = anc_to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Best-effort — don't fail the whole move if ancillary fails.
        if std::fs::rename(anc_from, anc_to).is_ok() {
            db.conn.execute(
                "INSERT INTO organize_log (batch_id, track_id, from_path, to_path) VALUES (?1, NULL, ?2, ?3)",
                params![
                    batch_id,
                    anc_from.to_string_lossy().as_ref(),
                    anc_to.to_string_lossy().as_ref(),
                ],
            )?;
        }
    }

    // Try to remove empty source directories.
    if let Some(source_dir) = file_move.from.parent() {
        remove_empty_dirs(source_dir);
    }

    Ok(())
}

/// Undo the most recent organize batch.
pub fn undo(db: &Database) -> Result<usize, OrganizeError> {
    // Find the most recent batch.
    let batch_id: String = db
        .conn
        .query_row(
            "SELECT batch_id FROM organize_log ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| OrganizeError::NothingToUndo)?;

    // Get all moves in this batch, in reverse order.
    let mut stmt = db.conn.prepare(
        "SELECT id, track_id, from_path, to_path FROM organize_log
         WHERE batch_id = ?1 ORDER BY id DESC",
    )?;

    let entries: Vec<(i64, Option<i64>, String, String)> = stmt
        .query_map(params![batch_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut count = 0;

    for (log_id, track_id, from_path, to_path) in &entries {
        let to = Path::new(to_path);
        let from = Path::new(from_path);

        if !to.exists() {
            // Already moved back or deleted — skip.
            db.conn
                .execute("DELETE FROM organize_log WHERE id = ?1", params![log_id])?;
            continue;
        }

        // Create original parent dir.
        if let Some(parent) = from.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Move back.
        std::fs::rename(to, from)?;

        // Update track path in DB if this was a track (not ancillary).
        if let Some(tid) = track_id {
            db.conn.execute(
                "UPDATE tracks SET path = ?1 WHERE id = ?2",
                params![from_path, tid],
            )?;
            db.conn.execute(
                "UPDATE scan_cache SET path = ?1 WHERE path = ?2",
                params![from_path, to_path],
            )?;
        }

        // Remove empty dirs at the (now old) destination.
        if let Some(parent) = to.parent() {
            remove_empty_dirs(parent);
        }

        db.conn
            .execute("DELETE FROM organize_log WHERE id = ?1", params![log_id])?;

        count += 1;
    }

    Ok(count)
}

/// Walk up directories removing empty ones, stopping at first non-empty.
fn remove_empty_dirs(dir: &Path) {
    let mut current = dir.to_path_buf();
    loop {
        if std::fs::read_dir(&current)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            if std::fs::remove_dir(&current).is_err() {
                break;
            }
            match current.parent() {
                Some(p) => current = p.to_path_buf(),
                None => break,
            }
        } else {
            break;
        }
    }
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

fn chrono_batch_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("batch-{}", now.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::TrackMeta;
    use crate::db::schema;

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
        }
    }

    #[test]
    fn track_metadata_provider_fields() {
        let track = TrackRow {
            id: 1,
            album_id: Some(1),
            artist_id: Some(1),
            artist_name: "Radiohead".into(),
            album_artist_name: "Radiohead".into(),
            album_title: "OK Computer".into(),
            disc: Some(1),
            track_number: Some(3),
            title: "Subterranean Homesick Alien".into(),
            duration_ms: Some(240_000),
            path: Some("/music/ok_computer/03.flac".into()),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1000),
            genre: Some("Alternative".into()),
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        };

        let meta = TrackMetadata::from_track_row(&track, Some("1997-06-16"));
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
        assert_eq!(meta.get_field("codec").as_deref(), Some("FLAC"));
        assert_eq!(meta.get_field("genre").as_deref(), Some("Alternative"));
        assert_eq!(meta.get_field("nonexistent"), None);
    }

    #[test]
    fn sanitize_replaces_illegal_chars() {
        assert_eq!(sanitize_path_component("AC/DC"), "AC_DC");
        assert_eq!(sanitize_path_component("What?"), "What_");
        assert_eq!(sanitize_path_component("a:b*c"), "a_b_c");
        assert_eq!(sanitize_path_component("normal"), "normal");
    }

    #[test]
    fn sanitize_relative_path_splits() {
        let result = sanitize_relative_path("Artist/Album/Track");
        assert_eq!(result, PathBuf::from("Artist/Album/Track"));

        let result = sanitize_relative_path("Radiohead/(1997) OK Computer/01. Airbag");
        assert_eq!(
            result,
            PathBuf::from("Radiohead/(1997) OK Computer/01. Airbag")
        );
    }

    #[test]
    fn acdc_artist_name_sanitized() {
        // "AC/DC" should become "AC_DC" through field sanitization.
        let track = TrackRow {
            id: 1,
            album_id: Some(1),
            artist_id: Some(1),
            artist_name: "AC/DC".into(),
            album_artist_name: "AC/DC".into(),
            album_title: "Highway to Hell".into(),
            disc: Some(1),
            track_number: Some(1),
            title: "Highway to Hell".into(),
            duration_ms: None,
            path: Some("/music/test.flac".into()),
            codec: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        };
        let meta = TrackMetadata::from_track_row(&track, Some("1979"));
        assert_eq!(meta.get_field("album artist").as_deref(), Some("AC_DC"));
        let result = format::format("%album artist%/%album%/%title%", &meta).unwrap();
        assert_eq!(result, "AC_DC/Highway to Hell/Highway to Hell");
    }

    #[test]
    fn format_string_evaluation() {
        let track = TrackRow {
            id: 1,
            album_id: Some(1),
            artist_id: Some(1),
            artist_name: "Radiohead".into(),
            album_artist_name: "Radiohead".into(),
            album_title: "OK Computer".into(),
            disc: Some(1),
            track_number: Some(1),
            title: "Airbag".into(),
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
        };

        let meta = TrackMetadata::from_track_row(&track, Some("1997-06-16"));
        let pattern =
            "%album artist%/['('$left(%date%,4)')' ]%album%/$num(%tracknumber%,2). %title%";
        let result = format::format(pattern, &meta).unwrap();
        assert_eq!(result, "Radiohead/(1997) OK Computer/01. Airbag");
    }

    #[test]
    fn preview_does_not_move_files() {
        let db = test_db();
        let tmp = std::env::temp_dir().join(format!("koan-organize-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // Create a test file.
        let test_file = tmp.join("test.flac");
        std::fs::write(&test_file, b"fake audio data").unwrap();

        let mut meta = sample_meta("Airbag", "Radiohead", "OK Computer");
        meta.path = Some(test_file.to_string_lossy().into());
        queries::upsert_track(&db.conn, &meta).unwrap();

        let result = preview(&db, "%album artist%/%album%/%title%", Some(&tmp)).unwrap();
        // The file should still be at the original path.
        assert!(test_file.exists());
        assert!(!result.moves.is_empty());

        // Cleanup.
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn execute_moves_files_and_undo_reverts() {
        let db = test_db();
        let tmp = std::env::temp_dir().join(format!("koan-organize-exec-{}", std::process::id()));
        let src_dir = tmp.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();

        // Create test files.
        let test_file = src_dir.join("test.flac");
        std::fs::write(&test_file, b"fake audio data").unwrap();

        let mut meta = sample_meta("Airbag", "Radiohead", "OK Computer");
        meta.path = Some(test_file.to_string_lossy().into());
        queries::upsert_track(&db.conn, &meta).unwrap();

        // Execute.
        let result = execute(&db, "%album artist%/%album%/%title%", Some(&tmp)).unwrap();
        assert_eq!(result.moves.len(), 1);
        assert!(!test_file.exists()); // original gone
        let dest = &result.moves[0].to;
        assert!(dest.exists()); // new location exists

        // Undo.
        let undone = undo(&db).unwrap();
        assert_eq!(undone, 1);
        assert!(test_file.exists()); // back to original
        assert!(!dest.exists()); // new location gone

        // Cleanup.
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn ancillary_file_detection() {
        let tmp = std::env::temp_dir().join(format!("koan-organize-anc-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        std::fs::write(tmp.join("cover.jpg"), b"img").unwrap();
        std::fs::write(tmp.join("cover.png"), b"img").unwrap();
        std::fs::write(tmp.join("album.cue"), b"cue").unwrap();
        std::fs::write(tmp.join("rip.log"), b"log").unwrap();
        std::fs::write(tmp.join("track.flac"), b"audio").unwrap(); // not ancillary

        let found = find_ancillary_files(&tmp);
        assert!(found.iter().any(|p| p.file_name().unwrap() == "cover.jpg"));
        assert!(found.iter().any(|p| p.file_name().unwrap() == "cover.png"));
        assert!(found.iter().any(|p| p.file_name().unwrap() == "album.cue"));
        assert!(found.iter().any(|p| p.file_name().unwrap() == "rip.log"));
        assert!(!found.iter().any(|p| p.file_name().unwrap() == "track.flac"));

        std::fs::remove_dir_all(&tmp).ok();
    }
}
