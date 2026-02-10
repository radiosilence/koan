use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;

use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};

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

    // Filter to files that need scanning (check scan_cache on the main thread).
    let files_to_scan: Vec<PathBuf> = if force {
        audio_files.clone()
    } else {
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
                queries::needs_rescan(&db.conn, &path_str, mtime, size).unwrap_or(true)
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
    let _ = db.conn.execute_batch("BEGIN");

    for (file_path, meta_result) in metadata_results {
        match meta_result {
            Ok(meta) => match queries::upsert_track(&db.conn, &meta) {
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
                        &db.conn,
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
    match queries::remove_stale_tracks(&db.conn, path) {
        Ok(removed) => result.removed = removed,
        Err(e) => log::error!("failed to remove stale tracks: {}", e),
    }

    let _ = db.conn.execute_batch("COMMIT");

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
