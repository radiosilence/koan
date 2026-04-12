use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;

use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};

use super::features;
use super::metadata::{self, is_audio_file};

/// Result of a folder scan.
#[derive(Debug, Default)]
pub struct ScanResult {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub errors: Vec<(PathBuf, String)>,
}

/// Info about a scanned track, passed to the progress callback.
pub struct ScanEvent<'a> {
    pub artist: &'a str,
    pub album: &'a str,
    pub title: &'a str,
    pub path: &'a Path,
    pub is_new: bool,
}

/// Scan a folder recursively for audio files and index them into the database.
/// The optional `on_track` callback is invoked for each successfully indexed track.
pub fn scan_folder(
    db: &Database,
    path: &Path,
    force: bool,
    on_track: Option<&dyn Fn(ScanEvent)>,
) -> ScanResult {
    let mut result = ScanResult::default();

    // Collect audio files via walkdir.
    let audio_files: Vec<PathBuf> = walkdir::WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_audio_file(e.path()))
        .map(|e| e.path().to_path_buf())
        .collect();

    log::info!(
        "found {} audio files in {}",
        audio_files.len(),
        path.display()
    );

    // Filter to files that need scanning.
    // Batch-load the entire scan_cache into a HashMap to avoid O(N) individual
    // DB lookups (one per file). For 100k+ file libraries this is dramatically faster.
    let files_to_scan: Vec<PathBuf> = if force {
        audio_files.clone()
    } else {
        let scan_cache = queries::load_scan_cache(&db.conn).unwrap_or_default();
        audio_files
            .iter()
            .filter(|file_path| {
                let Ok(file_meta) = std::fs::metadata(file_path) else {
                    return true;
                };
                let mtime = file_meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let size = file_meta.len() as i64;
                let path_str = file_path.to_string_lossy();
                match scan_cache.get(path_str.as_ref()) {
                    Some(&(cached_mtime, cached_size)) => {
                        mtime != cached_mtime || size != cached_size
                    }
                    None => true,
                }
            })
            .cloned()
            .collect()
    };

    result.skipped = audio_files.len() - files_to_scan.len();

    // Read metadata in parallel (CPU-bound tag parsing — no DB access here).
    let metadata_results: Vec<(PathBuf, Result<TrackMeta, String>)> = files_to_scan
        .par_iter()
        .map(|file_path| {
            let meta = metadata::read_metadata(file_path).map_err(|e| format!("{}", e));
            (file_path.clone(), meta)
        })
        .collect();

    // Insert into DB sequentially in a single transaction.
    let tx = match db.conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(e) => {
            log::error!("failed to begin scan transaction: {}", e);
            return result;
        }
    };

    for (file_path, meta_result) in metadata_results {
        match meta_result {
            Ok(meta) => match queries::upsert_track(&tx, &meta) {
                Ok(track_id) => {
                    result.added += 1;
                    if let Some(cb) = &on_track {
                        cb(ScanEvent {
                            artist: &meta.artist,
                            album: &meta.album,
                            title: &meta.title,
                            path: &file_path,
                            is_new: true,
                        });
                    }
                    let _ = queries::update_scan_cache(
                        &tx,
                        meta.path.as_deref().unwrap_or(""),
                        meta.mtime.unwrap_or(0),
                        meta.size_bytes.unwrap_or(0),
                        track_id,
                    );
                }
                Err(e) => {
                    result.errors.push((file_path, format!("db error: {}", e)));
                }
            },
            Err(e) => {
                result.errors.push((file_path, e));
            }
        }
    }

    // Remove tracks for files that no longer exist.
    match queries::remove_stale_tracks(&tx, path) {
        Ok(removed) => result.removed = removed,
        Err(e) => log::error!("failed to remove stale tracks: {}", e),
    }

    if let Err(e) = tx.commit() {
        log::error!("failed to commit scan transaction: {}", e);
    }

    result
}

/// Scan all configured library folders.
pub fn full_scan(
    db: &Database,
    folders: &[PathBuf],
    force: bool,
    on_track: Option<&dyn Fn(ScanEvent)>,
) -> ScanResult {
    let mut total = ScanResult::default();
    for folder in folders {
        if !folder.exists() {
            log::warn!("library folder does not exist: {}", folder.display());
            continue;
        }
        let r = scan_folder(db, folder, force, on_track);
        total.added += r.added;
        total.updated += r.updated;
        total.removed += r.removed;
        total.skipped += r.skipped;
        total.errors.extend(r.errors);
    }
    total
}

/// Info about an analyzed track, passed to the progress callback.
pub struct AnalysisEvent<'a> {
    pub path: &'a str,
    pub success: bool,
    pub current: usize,
    pub total: usize,
}

/// Run acoustic analysis on all tracks missing vectors.
/// Uses rayon for parallel analysis, stores results sequentially.
pub fn analyze_missing(
    db: &Database,
    on_track: Option<&(dyn Fn(AnalysisEvent) + Sync)>,
) -> (usize, usize) {
    let missing = match queries::tracks_missing_vectors(&db.conn) {
        Ok(m) => m,
        Err(e) => {
            log::error!("failed to query missing vectors: {}", e);
            return (0, 0);
        }
    };

    if missing.is_empty() {
        return (0, 0);
    }

    let total = missing.len();
    log::info!("analyzing {} tracks for acoustic features", total);

    // Analyze in parallel.
    let results: Vec<(i64, String, Result<Vec<f32>, features::AnalysisError>)> = missing
        .par_iter()
        .enumerate()
        .map(|(i, (track_id, path))| {
            let result = features::analyze_track(Path::new(path));
            if let Some(cb) = &on_track {
                cb(AnalysisEvent {
                    path,
                    success: result.is_ok(),
                    current: i + 1,
                    total,
                });
            }
            (*track_id, path.clone(), result)
        })
        .collect();

    // Store sequentially.
    let mut analyzed = 0usize;
    let mut errors = 0usize;
    let tx = match db.conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(e) => {
            log::error!("failed to begin analysis transaction: {}", e);
            return (0, 0);
        }
    };
    for (track_id, path, result) in results {
        match result {
            Ok(embedding) => {
                if let Err(e) = queries::store_vector(&tx, track_id, &embedding) {
                    log::warn!("failed to store vector for {}: {}", path, e);
                    errors += 1;
                } else {
                    analyzed += 1;
                }
            }
            Err(e) => {
                log::warn!("analysis failed for {}: {}", path, e);
                errors += 1;
            }
        }
    }
    if let Err(e) = tx.commit() {
        log::error!("failed to commit analysis transaction: {}", e);
    }

    log::info!("analysis complete: {} ok, {} errors", analyzed, errors);
    (analyzed, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries;
    use crate::test_utils;

    fn test_db(dir: &Path) -> Database {
        let db_path = dir.join("test.db");
        Database::open(&db_path).unwrap()
    }

    #[test]
    fn scan_folder_indexes_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        std::fs::create_dir_all(&music_dir).unwrap();

        // Generate a valid WAV file (1 second, 44100 Hz, mono, 16-bit).
        let wav_path = music_dir.join("silence.wav");
        test_utils::generate_wav(&wav_path, 44100, 1, 1.0, 16);

        let db = test_db(dir.path());
        let result = scan_folder(&db, &music_dir, false, None);

        assert_eq!(result.added, 1, "expected 1 track added");
        assert_eq!(result.skipped, 0);
        assert_eq!(result.removed, 0);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // Verify the track exists in the DB.
        let stats = queries::library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 1, "expected 1 track in DB");
    }

    #[test]
    fn scan_folder_skips_unchanged_files() {
        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        std::fs::create_dir_all(&music_dir).unwrap();

        let wav_path = music_dir.join("unchanged.wav");
        test_utils::generate_wav(&wav_path, 44100, 1, 1.0, 16);

        let db = test_db(dir.path());

        // First scan: adds the file.
        let r1 = scan_folder(&db, &music_dir, false, None);
        assert_eq!(r1.added, 1);

        // Second scan: file unchanged, should be skipped.
        let r2 = scan_folder(&db, &music_dir, false, None);
        assert_eq!(r2.skipped, 1, "expected unchanged file to be skipped");
        assert_eq!(r2.added, 0, "no new files should be added");
    }

    #[test]
    fn scan_folder_removes_deleted_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        std::fs::create_dir_all(&music_dir).unwrap();

        let wav_path = music_dir.join("ephemeral.wav");
        test_utils::generate_wav(&wav_path, 44100, 1, 1.0, 16);

        let db = test_db(dir.path());

        // First scan: adds the file.
        let r1 = scan_folder(&db, &music_dir, false, None);
        assert_eq!(r1.added, 1);

        // Delete the file.
        std::fs::remove_file(&wav_path).unwrap();

        // Second scan: should detect removal.
        let r2 = scan_folder(&db, &music_dir, false, None);
        assert_eq!(
            r2.removed, 1,
            "expected 1 track removed after file deletion"
        );

        // DB should have 0 tracks.
        let stats = queries::library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 0, "expected 0 tracks after removal");
    }

    #[test]
    fn scan_folder_updates_modified_files() {
        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        std::fs::create_dir_all(&music_dir).unwrap();

        let wav_path = music_dir.join("modified.wav");
        test_utils::generate_wav(&wav_path, 44100, 1, 1.0, 16);

        let db = test_db(dir.path());

        // First scan.
        let r1 = scan_folder(&db, &music_dir, false, None);
        assert_eq!(r1.added, 1);

        // Modify the file (rewrite with different duration → different size + mtime).
        // Sleep briefly to ensure mtime changes (some FS have 1s resolution).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        test_utils::generate_wav(&wav_path, 44100, 1, 2.0, 16);

        // Second scan: should detect the modification.
        let r2 = scan_folder(&db, &music_dir, false, None);
        assert_eq!(
            r2.added, 1,
            "modified file should be re-indexed (counted as added by upsert)"
        );
        assert_eq!(r2.skipped, 0, "modified file should not be skipped");
    }
}
