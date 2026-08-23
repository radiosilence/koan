use std::panic::{AssertUnwindSafe, catch_unwind};
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
    /// Directory entries walkdir could not read — unreadable subtrees, symlink
    /// loops. Their contents are absent from the scan entirely.
    pub unreadable: usize,
    /// Paths of the tracks deleted or demoted to remote-only, so a caller can
    /// show what a removal actually took.
    pub removed_paths: Vec<String>,
    pub errors: Vec<(PathBuf, String)>,
}

/// How a scan should behave.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    /// Re-read tags for every file, ignoring `scan_cache`.
    pub force: bool,
    /// Delete stale tracks even when the proportion missing looks like a mount
    /// failure. Lifts the removal-fraction brake only — a folder that yields no
    /// audio files is still left alone, and an IO error still never counts as
    /// "file gone".
    pub force_remove: bool,
}

/// Files per transaction. Bounds peak memory (only one chunk's metadata is
/// resident) and caps what an interrupted scan loses; committed chunks land in
/// `scan_cache`, so the next run resumes rather than restarting.
#[cfg(not(test))]
const CHUNK_SIZE: usize = 1000;
#[cfg(test)]
const CHUNK_SIZE: usize = 4;

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
    opts: ScanOptions,
    on_track: Option<&dyn Fn(ScanEvent)>,
) -> ScanResult {
    let mut result = ScanResult::default();

    // Collect audio files via walkdir. `follow_links` means a symlink pointing at
    // a sibling directory inside the library indexes its files under both paths.
    let mut audio_files: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(path).follow_links(true) {
        match entry {
            Ok(e) if e.file_type().is_file() && is_audio_file(e.path()) => {
                audio_files.push(e.path().to_path_buf())
            }
            Ok(_) => {}
            Err(e) => {
                result.unreadable += 1;
                log::warn!("skipping unreadable entry under {}: {}", path.display(), e);
            }
        }
    }

    let total_files = audio_files.len();
    log::info!("found {} audio files in {}", total_files, path.display());

    // Filter to files that need scanning.
    // Batch-load the entire scan_cache into a HashMap to avoid O(N) individual
    // DB lookups (one per file). For 100k+ file libraries this is dramatically faster.
    let files_to_scan: Vec<PathBuf> = if opts.force {
        std::mem::take(&mut audio_files)
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

    result.skipped = total_files - files_to_scan.len();

    // Tag reads and database writes run at the same time.
    //
    // Reading the whole library up front and writing it in one transaction
    // blocks every other writer for the length of the scan and loses all of it
    // on interrupt, so writes stay chunked. But doing that as read-chunk,
    // write-chunk, read-chunk leaves the disk idle for every write and the CPU
    // idle for every read — on a library of any size that is most of the run.
    //
    // Instead the reads stream: a worker pool walks every file and pushes
    // results down a bounded channel while this thread batches them into
    // transactions. The bound is what caps memory, in place of the chunking.
    let (send, recv) = crossbeam_channel::bounded::<(PathBuf, Result<TrackMeta, String>)>(
        CHUNK_SIZE.saturating_mul(2),
    );
    let reader = std::thread::Builder::new()
        .name("koan-scan-read".into())
        .spawn(move || {
            files_to_scan.par_iter().for_each(|file_path| {
                // A send error means the consumer is gone; nothing left to do.
                let _ = send.send((
                    file_path.clone(),
                    isolate_read(file_path, metadata::read_metadata),
                ));
            });
        });
    if let Err(e) = &reader {
        log::error!("failed to spawn scan reader: {}", e);
        result
            .errors
            .push((path.to_path_buf(), format!("scan error: {}", e)));
        return result;
    }

    loop {
        // Blocks until a full batch is ready or the readers have finished.
        let batch: Vec<(PathBuf, Result<TrackMeta, String>)> =
            recv.iter().take(CHUNK_SIZE).collect();
        if batch.is_empty() {
            break;
        }

        let tx = match db.conn.unchecked_transaction() {
            Ok(tx) => tx,
            Err(e) => {
                log::error!("failed to begin scan transaction: {}", e);
                result
                    .errors
                    .push((path.to_path_buf(), format!("db error: {}", e)));
                return result;
            }
        };

        let (mut added, mut updated) = (0usize, 0usize);
        for (file_path, meta_result) in batch {
            match meta_result {
                Ok(meta) => match queries::upsert_track_status(&tx, &meta) {
                    Ok((track_id, is_new)) => {
                        if is_new {
                            added += 1;
                        } else {
                            updated += 1;
                        }
                        if let Some(cb) = &on_track {
                            cb(ScanEvent {
                                artist: &meta.artist,
                                album: &meta.album,
                                title: &meta.title,
                                path: &file_path,
                                is_new,
                            });
                        }
                        if let Err(e) = queries::update_scan_cache(
                            &tx,
                            meta.path.as_deref().unwrap_or(""),
                            meta.mtime.unwrap_or(0),
                            meta.size_bytes.unwrap_or(0),
                            track_id,
                        ) {
                            // Not fatal, but every future scan re-reads this file's tags.
                            log::warn!("failed to cache {}: {}", file_path.display(), e);
                        }
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

        match tx.commit() {
            Ok(()) => {
                result.added += added;
                result.updated += updated;
            }
            Err(e) => {
                log::error!("failed to commit scan transaction: {}", e);
                result
                    .errors
                    .push((path.to_path_buf(), format!("db error: {}", e)));
            }
        }
    }

    if let Ok(handle) = reader
        && handle.join().is_err()
    {
        log::error!("scan reader thread panicked");
    }

    // Remove tracks for files that no longer exist. A folder that yielded nothing
    // is far more likely to be an unmounted volume than a library someone emptied,
    // and stale rows are recoverable where deleted play history is not.
    if total_files == 0 {
        log::error!(
            "{} contains no audio files — skipping stale-track removal. \
             If this folder should have music in it, it is probably not mounted or not readable.",
            path.display()
        );
        return result;
    }

    let tx = match db.conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(e) => {
            log::error!("failed to begin stale-removal transaction: {}", e);
            result
                .errors
                .push((path.to_path_buf(), format!("db error: {}", e)));
            return result;
        }
    };
    match queries::remove_stale_tracks(&tx, path, opts.force_remove) {
        Ok(removed) => {
            result.removed = removed.len();
            result.removed_paths = removed;
            if let Err(e) = tx.commit() {
                log::error!("failed to commit stale removals: {}", e);
                result.removed = 0;
                result.removed_paths.clear();
                result
                    .errors
                    .push((path.to_path_buf(), format!("db error: {}", e)));
            }
        }
        Err(e) => {
            log::error!("failed to remove stale tracks: {}", e);
            result.errors.push((path.to_path_buf(), e.to_string()));
        }
    }

    result
}

/// Run a tag read, containing a panic from the parsers. Hostile input (a bogus
/// ID3v2 frame size, a pathological MP4 atom tree) can panic inside lofty or
/// symphonia; rayon re-raises that at `collect()`, which would otherwise abort
/// the whole scan over one file and not even name it.
fn isolate_read(
    path: &Path,
    read: impl FnOnce(&Path) -> Result<TrackMeta, metadata::MetadataError>,
) -> Result<TrackMeta, String> {
    match catch_unwind(AssertUnwindSafe(|| read(path))) {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(_) => Err(format!("panicked while reading tags: {}", path.display())),
    }
}

/// What an import of specific files produced.
#[derive(Debug, Default)]
pub struct ImportResult {
    /// Library rows for the imported files, in the order their paths were
    /// walked. This is what a caller queues.
    pub track_ids: Vec<i64>,
    pub added: usize,
    pub updated: usize,
    pub errors: Vec<(PathBuf, String)>,
}

/// Index specific files into the library, wherever they live.
///
/// This is the drop-a-folder-on-the-queue path: the files named here are not
/// under a configured library folder, and organize is what moves them there
/// afterwards. Nothing is ever removed — the caller named these paths, so there
/// is no directory listing to reconcile against and nothing to prune, which is
/// what separates this from `scan_folder`.
///
/// Directories are walked recursively. Order is by path, so an album lands in
/// the order its files are numbered.
pub fn import_paths(db: &Database, paths: &[PathBuf]) -> ImportResult {
    let mut result = ImportResult::default();

    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        let mut found: Vec<PathBuf> = walkdir::WalkDir::new(path)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && is_audio_file(e.path()))
            .map(|e| e.path().to_path_buf())
            .collect();
        found.sort();
        // A drop can name both a folder and a file inside it.
        files.extend(found.into_iter().filter(|f| seen.insert(f.clone())));
    }

    if files.is_empty() {
        return result;
    }

    // Tag reads are the slow part and independent per file; the writes are not.
    let read: Vec<(PathBuf, Result<TrackMeta, String>)> = files
        .par_iter()
        .map(|path| (path.clone(), isolate_read(path, metadata::read_metadata)))
        .collect();

    let tx = match db.conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(e) => {
            result
                .errors
                .push((PathBuf::new(), format!("db error: {e}")));
            return result;
        }
    };

    for (path, meta_result) in read {
        let meta = match meta_result {
            Ok(meta) => meta,
            Err(e) => {
                result.errors.push((path, e));
                continue;
            }
        };
        match queries::upsert_track_status(&tx, &meta) {
            Ok((track_id, is_new)) => {
                if is_new {
                    result.added += 1;
                } else {
                    result.updated += 1;
                }
                result.track_ids.push(track_id);
                if let Err(e) = queries::update_scan_cache(
                    &tx,
                    meta.path.as_deref().unwrap_or(""),
                    meta.mtime.unwrap_or(0),
                    meta.size_bytes.unwrap_or(0),
                    track_id,
                ) {
                    // Not fatal, but every future scan re-reads this file's tags.
                    log::warn!("failed to cache {}: {}", path.display(), e);
                }
            }
            Err(e) => result.errors.push((path, format!("db error: {e}"))),
        }
    }

    if let Err(e) = tx.commit() {
        result.track_ids.clear();
        result.added = 0;
        result.updated = 0;
        result
            .errors
            .push((PathBuf::new(), format!("db error: {e}")));
    }

    result
}

/// Scan all configured library folders.
pub fn full_scan(
    db: &Database,
    folders: &[PathBuf],
    opts: ScanOptions,
    on_track: Option<&dyn Fn(ScanEvent)>,
) -> ScanResult {
    let mut total = ScanResult::default();
    for folder in folders {
        if !folder.exists() {
            log::warn!("library folder does not exist: {}", folder.display());
            continue;
        }
        let r = scan_folder(db, folder, opts, on_track);
        total.added += r.added;
        total.updated += r.updated;
        total.removed += r.removed;
        total.skipped += r.skipped;
        total.unreadable += r.unreadable;
        total.removed_paths.extend(r.removed_paths);
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
            let result = match catch_unwind(AssertUnwindSafe(|| {
                features::analyze_track(Path::new(path))
            })) {
                Ok(r) => r,
                Err(_) => Err(features::AnalysisError::Bliss(format!(
                    "panicked while analyzing {}",
                    path
                ))),
            };
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
        let result = scan_folder(&db, &music_dir, ScanOptions::default(), None);

        assert_eq!(result.added, 1, "expected 1 track added");
        assert_eq!(result.skipped, 0);
        assert_eq!(result.removed, 0);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // Verify the track exists in the DB.
        let stats = queries::library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 1, "expected 1 track in DB");
    }

    #[test]
    fn import_paths_indexes_files_where_they_lie() {
        let dir = tempfile::tempdir().unwrap();
        // Deliberately nothing to do with a library folder — this is the
        // drag-a-rip-onto-the-queue case.
        let drop = dir.path().join("Downloads/rip");
        std::fs::create_dir_all(&drop).unwrap();
        test_utils::generate_wav(&drop.join("01.wav"), 44100, 1, 0.2, 16);
        test_utils::generate_wav(&drop.join("02.wav"), 44100, 1, 0.2, 16);
        std::fs::write(drop.join("notes.txt"), b"not music").unwrap();

        let db = test_db(dir.path());
        let result = import_paths(&db, std::slice::from_ref(&drop));

        assert_eq!(result.added, 2);
        assert_eq!(result.track_ids.len(), 2, "errors: {:?}", result.errors);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // The rows point at where the files still are; organize is what moves them.
        for id in &result.track_ids {
            let row = queries::get_track_row(&db.conn, *id).unwrap().unwrap();
            assert!(row.path.unwrap().starts_with(drop.to_str().unwrap()));
        }
    }

    /// Dropping the same rip twice queues it again without duplicating rows.
    #[test]
    fn import_paths_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let drop = dir.path().join("rip");
        std::fs::create_dir_all(&drop).unwrap();
        test_utils::generate_wav(&drop.join("a.wav"), 44100, 1, 0.2, 16);

        let db = test_db(dir.path());
        let first = import_paths(&db, std::slice::from_ref(&drop));
        let second = import_paths(&db, std::slice::from_ref(&drop));

        assert_eq!(first.added, 1);
        assert_eq!(second.added, 0);
        assert_eq!(second.updated, 1);
        assert_eq!(first.track_ids, second.track_ids);
        assert_eq!(queries::library_stats(&db.conn).unwrap().total_tracks, 1);
    }

    /// A drop can name a folder and a file inside it; the file is imported once.
    #[test]
    fn import_paths_deduplicates_overlapping_selections() {
        let dir = tempfile::tempdir().unwrap();
        let drop = dir.path().join("rip");
        std::fs::create_dir_all(&drop).unwrap();
        let track = drop.join("a.wav");
        test_utils::generate_wav(&track, 44100, 1, 0.2, 16);

        let db = test_db(dir.path());
        let result = import_paths(&db, &[drop.clone(), track.clone()]);

        assert_eq!(result.track_ids.len(), 1);
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
        let r1 = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        assert_eq!(r1.added, 1);

        // Second scan: file unchanged, should be skipped.
        let r2 = scan_folder(&db, &music_dir, ScanOptions::default(), None);
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
        test_utils::generate_wav(&music_dir.join("keeper.wav"), 44100, 1, 1.0, 16);

        let db = test_db(dir.path());

        // First scan: adds both files.
        let r1 = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        assert_eq!(r1.added, 2);

        // Delete one of them.
        std::fs::remove_file(&wav_path).unwrap();

        // Second scan: should detect removal.
        let r2 = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        assert_eq!(
            r2.removed, 1,
            "expected 1 track removed after file deletion"
        );

        let stats = queries::library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 1, "the surviving file must be kept");
    }

    #[test]
    fn empty_folder_does_not_wipe_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        std::fs::create_dir_all(&music_dir).unwrap();
        test_utils::generate_wav(&music_dir.join("a.wav"), 44100, 1, 1.0, 16);
        test_utils::generate_wav(&music_dir.join("b.wav"), 44100, 1, 1.0, 16);

        let db = test_db(dir.path());
        assert_eq!(
            scan_folder(&db, &music_dir, ScanOptions::default(), None).added,
            2
        );

        // The folder is still there but yields nothing — an unmounted NAS, a
        // detached volume, a Docker volume that failed to attach.
        std::fs::remove_file(music_dir.join("a.wav")).unwrap();
        std::fs::remove_file(music_dir.join("b.wav")).unwrap();

        let r = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        assert_eq!(r.removed, 0, "stale removal must be skipped entirely");
        assert_eq!(queries::library_stats(&db.conn).unwrap().total_tracks, 2);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_folder_is_not_a_deletion() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        let locked_dir = music_dir.join("locked");
        std::fs::create_dir_all(&locked_dir).unwrap();
        test_utils::generate_wav(&music_dir.join("keep.wav"), 44100, 1, 1.0, 16);
        let locked_file = locked_dir.join("locked.wav");
        test_utils::generate_wav(&locked_file, 44100, 1, 1.0, 16);

        let db = test_db(dir.path());
        assert_eq!(
            scan_folder(&db, &music_dir, ScanOptions::default(), None).added,
            2
        );

        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        if locked_file.try_exists().is_ok() {
            // Running as root — the permission bits mean nothing here.
            std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let r = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            r.unreadable >= 1,
            "the unreadable subtree should be counted"
        );
        assert_eq!(r.removed, 0, "an IO error is not a deletion");
        assert_eq!(queries::library_stats(&db.conn).unwrap().total_tracks, 2);
    }

    #[test]
    fn interrupted_scan_keeps_committed_chunks_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        std::fs::create_dir_all(&music_dir).unwrap();
        for i in 0..6 {
            test_utils::generate_wav(&music_dir.join(format!("{}.wav", i)), 44100, 1, 1.0, 16);
        }

        let db = test_db(dir.path());

        // Abort partway through the second chunk, the way Ctrl-C would.
        let seen = std::cell::Cell::new(0usize);
        let abort = |_: ScanEvent| {
            seen.set(seen.get() + 1);
            assert!(seen.get() <= CHUNK_SIZE, "simulated interrupt");
        };
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scan_folder(&db, &music_dir, ScanOptions::default(), Some(&abort));
        }));
        assert!(panicked.is_err());

        // The first chunk is on disk; the interrupted one is not.
        assert_eq!(
            queries::library_stats(&db.conn).unwrap().total_tracks,
            CHUNK_SIZE as i64
        );

        // And the next run picks up where it left off instead of restarting.
        let r = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        assert_eq!(r.skipped, CHUNK_SIZE, "committed files should be cached");
        assert_eq!(r.added, 6 - CHUNK_SIZE);
        assert_eq!(queries::library_stats(&db.conn).unwrap().total_tracks, 6);
    }

    #[test]
    fn failing_file_is_named_and_the_scan_continues() {
        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        std::fs::create_dir_all(&music_dir).unwrap();
        test_utils::generate_wav(&music_dir.join("good.wav"), 44100, 1, 1.0, 16);
        let broken = music_dir.join("broken.flac");
        std::fs::write(&broken, b"").unwrap();

        let db = test_db(dir.path());
        let r = scan_folder(&db, &music_dir, ScanOptions::default(), None);

        assert_eq!(r.added, 1, "the good file must still be indexed");
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].0, broken, "the failing file must be named");
        assert_eq!(queries::library_stats(&db.conn).unwrap().total_tracks, 1);
    }

    #[test]
    fn a_panicking_tag_read_becomes_an_error() {
        let path = Path::new("/music/hostile.mp3");
        let err = isolate_read(path, |_| panic!("bogus ID3v2 frame size")).unwrap_err();
        assert!(err.contains("hostile.mp3"), "should name the file: {}", err);
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
        let r1 = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        assert_eq!(r1.added, 1);

        // Modify the file (rewrite with different duration → different size + mtime).
        // Sleep briefly to ensure mtime changes (some FS have 1s resolution).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        test_utils::generate_wav(&wav_path, 44100, 1, 2.0, 16);

        // Second scan: should detect the modification.
        let r2 = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        assert_eq!(r2.updated, 1, "modified file should be re-indexed");
        assert_eq!(r2.added, 0, "the row already exists");
        assert_eq!(r2.skipped, 0, "modified file should not be skipped");
    }
}
