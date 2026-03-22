use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;

use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};

use super::features;
use super::metadata::{self, is_audio_file};
use super::neural;

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

/// Info about an analyzed track, passed to the progress callback.
pub struct AnalysisEvent<'a> {
    pub path: &'a str,
    pub success: bool,
    pub current: usize,
    pub total: usize,
}

/// Type alias for a vector store function (track_id, embedding) → Result.
type StoreFn =
    dyn Fn(&rusqlite::Connection, i64, &[f32]) -> Result<(), crate::db::connection::DbError>;

/// Generic batch analysis: query missing tracks, analyze in parallel, store sequentially.
fn run_batch_analysis<E: Send + std::fmt::Display>(
    db: &Database,
    missing: Vec<(i64, String)>,
    label: &str,
    analyze_fn: &(dyn Fn(&Path) -> Result<Vec<f32>, E> + Sync),
    store_fn: &StoreFn,
    on_track: Option<&(dyn Fn(AnalysisEvent) + Sync)>,
) -> (usize, usize) {
    if missing.is_empty() {
        return (0, 0);
    }

    let total = missing.len();
    log::info!("analyzing {} tracks for {}", total, label);

    let results: Vec<(i64, String, bool, Option<Vec<f32>>)> = missing
        .par_iter()
        .enumerate()
        .map(|(i, (track_id, path))| {
            let result = analyze_fn(Path::new(path));
            let success = result.is_ok();
            if let Some(cb) = &on_track {
                cb(AnalysisEvent {
                    path,
                    success,
                    current: i + 1,
                    total,
                });
            }
            match result {
                Ok(emb) => (*track_id, path.clone(), true, Some(emb)),
                Err(e) => {
                    log::warn!("{} analysis failed for {}: {}", label, path, e);
                    (*track_id, path.clone(), false, None)
                }
            }
        })
        .collect();

    let mut analyzed = 0usize;
    let mut errors = 0usize;
    let _ = db.conn.execute_batch("BEGIN");
    for (track_id, path, success, embedding) in results {
        if !success {
            errors += 1;
            continue;
        }
        let embedding = embedding.unwrap();
        if let Err(e) = store_fn(&db.conn, track_id, &embedding) {
            log::warn!("failed to store {} vector for {}: {}", label, path, e);
            errors += 1;
        } else {
            analyzed += 1;
        }
    }
    let _ = db.conn.execute_batch("COMMIT");

    log::info!("{} complete: {} ok, {} errors", label, analyzed, errors);
    (analyzed, errors)
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

    run_batch_analysis(
        db,
        missing,
        "acoustic",
        &|path| features::analyze_track(path).map_err(|e| e.to_string()),
        &queries::store_vector,
        on_track,
    )
}

/// Run neural analysis on all tracks missing neural vectors.
/// Returns (ok_count, error_count). If the model is missing or the feature
/// is disabled, returns (0, 0) after logging a warning.
pub fn analyze_missing_neural(
    db: &Database,
    model_dir: &Path,
    on_track: Option<&(dyn Fn(AnalysisEvent) + Sync)>,
) -> (usize, usize) {
    if !neural::is_audio_model_available(model_dir) {
        log::warn!(
            "neural model not found at {}. Download DCLAP ONNX models and place them at: {}/",
            neural::audio_model_path(model_dir).display(),
            model_dir.display(),
        );
        return (0, 0);
    }

    let missing = match queries::tracks_missing_neural_vectors(&db.conn) {
        Ok(m) => m,
        Err(e) => {
            log::error!("failed to query missing neural vectors: {}", e);
            return (0, 0);
        }
    };

    let model_dir = model_dir.to_path_buf();
    run_batch_analysis(
        db,
        missing,
        "neural",
        &|path| neural::analyze_track_neural(path, &model_dir).map_err(|e| e.to_string()),
        &queries::store_neural_vector,
        on_track,
    )
}
