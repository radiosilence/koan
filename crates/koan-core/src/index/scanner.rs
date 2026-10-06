use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::UNIX_EPOCH;

use rayon::prelude::*;

use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};

use super::metadata::{self, is_audio_file};
use super::playlist_files::is_playlist_file;

/// Result of a folder scan.
#[derive(Debug, Default)]
pub struct ScanResult {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// Files found at a new path that kept the track they had at the old one,
    /// and so are counted neither as added nor as removed.
    pub moved: usize,
    pub skipped: usize,
    /// Directory entries walkdir could not read — unreadable subtrees, symlink
    /// loops. Their contents are absent from the scan entirely.
    pub unreadable: usize,
    /// Playlist files read into new or changed playlists.
    pub playlists: usize,
    /// Paths of the tracks deleted or demoted to remote-only, so a caller can
    /// show what a removal actually took.
    pub removed_paths: Vec<String>,
    pub errors: Vec<(PathBuf, String)>,
    /// Stopped early because someone asked. What it had done is still done.
    pub cancelled: bool,
    /// Tracks this scan made, which a file gone from elsewhere may turn out to
    /// be (see `queries::adopt_moved_files`).
    arrived: Vec<i64>,
}

/// How a scan should behave.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Re-read tags for every file, ignoring `scan_cache`.
    pub force: bool,
    /// Set from another thread to stop early.
    ///
    /// Checked between transactions, so a cancelled scan keeps everything it
    /// had already committed rather than throwing the work away — stopping is
    /// "stop here", not "undo".
    pub cancel: Option<Arc<AtomicBool>>,
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
    scan_folders(
        db,
        std::slice::from_ref(&path.to_path_buf()),
        opts,
        None,
        on_track,
    )
}

/// [`scan_folder`] over several folders, telling `on_started` how many audio
/// files there are once every folder has been walked. The count comes from the
/// same walk the scan uses: a separate counting walk would double the directory
/// traversal, which on a network mount is most of the cost of a rescan.
pub fn scan_folders(
    db: &Database,
    folders: &[PathBuf],
    opts: ScanOptions,
    on_started: Option<&dyn Fn(u64)>,
    on_track: Option<&dyn Fn(ScanEvent)>,
) -> ScanResult {
    let walked: Vec<(PathBuf, Walked, ScanResult)> = folders
        .iter()
        .map(|folder| {
            // The files under it are stored as the directory spells them; the
            // root has to agree, or nothing under it matches a path an earlier
            // scan stored.
            let path = super::spelling::on_disk(folder);
            let mut result = ScanResult::default();
            let found = walk(&path, &mut result);
            (path, found, result)
        })
        .collect();
    if let Some(started) = on_started {
        started(
            walked
                .iter()
                .map(|(_, found, _)| found.audio.len() as u64)
                .sum(),
        );
    }

    // Every folder is indexed before any is pruned, so a file moved from one
    // library folder to another is found at its new path before it is missed
    // at its old one, and keeps its track.
    let mut indexed = Vec::new();
    let mut cancelled = false;
    for (path, found, mut result) in walked {
        if cancelled {
            break;
        }
        // A folder with no audio is taken for one not mounted, as stale
        // removal takes it: its playlists are read but none forgotten.
        let settled = if found.audio.is_empty() || result.unreadable > 0 {
            Vec::new()
        } else {
            vec![path.clone()]
        };
        let prune = index_folder(db, &path, found.audio, &opts, on_track, &mut result);
        cancelled |= result.cancelled;
        indexed.push((path, found.playlists, settled, prune, result));
    }

    let arrived: Vec<i64> = indexed
        .iter()
        .flat_map(|(.., result)| result.arrived.iter().copied())
        .collect();
    let mut total = ScanResult::default();
    for (path, playlists, settled, prune, mut result) in indexed {
        if prune && !cancelled {
            remove_stale(db, &path, opts.force_remove, &arrived, &mut result);
        }
        if !result.cancelled {
            result.playlists += super::playlist_files::import(db, &playlists, &settled);
        }
        merge(&mut total, result);
    }
    // A moved file was counted as added when its new path was indexed.
    total.added = total.added.saturating_sub(total.moved);
    if total.added > 0 {
        super::playlist_files::refresh_m3u(db);
    }
    total
}

/// Index a library folder's files. Whether its stale rows may then be
/// removed.
fn index_folder(
    db: &Database,
    path: &Path,
    audio_files: Vec<PathBuf>,
    opts: &ScanOptions,
    on_track: Option<&dyn Fn(ScanEvent)>,
    result: &mut ScanResult,
) -> bool {
    let total_files = audio_files.len();
    log::info!("found {} audio files in {}", total_files, path.display());

    if !index_files(db, audio_files, opts, on_track, result, path) {
        return false;
    }

    if result.cancelled {
        // Stale removal decides what is missing by what the scan did *not* see.
        // After a cancellation that is most of the folder, so it would delete a
        // library rather than tidy one.
        return false;
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
        return false;
    }
    true
}

/// Rescan directories inside the library folders, and nothing else.
///
/// What the folder watcher runs: a change touches a handful of directories,
/// and walking the whole library to find them costs a spinning disk minutes.
/// Each directory is authoritative for what lies under it — files found are
/// indexed, rows for files no longer there are removed — which is what
/// separates this from `import_paths`. A directory that no longer exists
/// removes everything that was under it.
///
/// Removal is only trusted while the library folder holding the directory is
/// itself readable and non-empty: an unmounted volume leaves an empty mount
/// point, and every directory under it would otherwise read as deleted. A
/// directory that could not be read in full, or one reached through a symlink
/// whose target is gone, removes nothing. Directories
/// outside every library folder are ignored; one that is a library folder
/// gets the same treatment as `scan_folder`.
pub fn scan_dirs(
    db: &Database,
    library: &[PathBuf],
    dirs: &[PathBuf],
    opts: ScanOptions,
    on_track: Option<&dyn Fn(ScanEvent)>,
) -> ScanResult {
    let mut result = ScanResult::default();
    let mut spelling = super::spelling::Spelling::default();
    let library: Vec<PathBuf> = library.iter().map(|f| spelling.on_disk(f)).collect();
    let dirs: Vec<PathBuf> = dirs.iter().map(|d| spelling.on_disk(d)).collect();

    let mut files = Vec::new();
    let mut playlists = Vec::new();
    let mut settled = Vec::new();
    for dir in minimal_dirs(dirs) {
        let Some(root) = library.iter().find(|root| dir.starts_with(root)) else {
            log::warn!("not in a library folder, not scanning: {}", dir.display());
            continue;
        };
        if dir == *root {
            merge(&mut result, scan_folder(db, root, opts.clone(), on_track));
            continue;
        }
        if !is_populated(root) {
            log::warn!(
                "{} is empty or unreadable — not scanning {}",
                root.display(),
                dir.display()
            );
            continue;
        }
        match dir.try_exists() {
            Ok(true) => {
                let before = result.unreadable;
                let found = walk(&dir, &mut result);
                files.extend(found.audio);
                playlists.extend(found.playlists);
                if result.unreadable == before {
                    settled.push(dir);
                }
            }
            Ok(false) if super::known_missing(&dir) => settled.push(dir),
            Ok(false) => log::warn!(
                "{} is behind a link that leads nowhere — not scanning it",
                dir.display()
            ),
            Err(e) => log::warn!("cannot tell whether {} exists: {e}", dir.display()),
        }
    }

    if !index_files(db, files, &opts, on_track, &mut result, Path::new("")) || result.cancelled {
        return result;
    }
    let arrived = std::mem::take(&mut result.arrived);
    for dir in &settled {
        remove_stale(db, dir, true, &arrived, &mut result);
        // A cover image changing is a reason to be here that no row records.
        if let Err(e) = queries::evict_art_under(&db.conn, dir) {
            log::warn!("cover art under {} not refreshed: {e}", dir.display());
        }
    }
    result.playlists += super::playlist_files::import(db, &playlists, &settled);
    result.added = result.added.saturating_sub(result.moved);
    if result.added > 0 {
        super::playlist_files::refresh_m3u(db);
    }
    result
}

/// The fewest directories that cover every one given: duplicates dropped, and
/// any directory inside another dropped, since a scan of the outer one walks it.
pub fn minimal_dirs(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    // Sorted by component, everything inside a directory comes straight after it.
    dirs.sort();
    dirs.dedup();
    let mut kept: Vec<PathBuf> = Vec::with_capacity(dirs.len());
    for dir in dirs {
        if !kept.last().is_some_and(|outer| dir.starts_with(outer)) {
            kept.push(dir);
        }
    }
    kept
}

/// Whether a library folder is there to scan: readable, with something in it.
fn is_populated(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_some())
}

fn merge(total: &mut ScanResult, r: ScanResult) {
    total.cancelled |= r.cancelled;
    total.added += r.added;
    total.updated += r.updated;
    total.removed += r.removed;
    total.moved += r.moved;
    total.skipped += r.skipped;
    total.unreadable += r.unreadable;
    total.playlists += r.playlists;
    total.removed_paths.extend(r.removed_paths);
    total.errors.extend(r.errors);
}

/// What a walk of a directory found.
#[derive(Default)]
struct Walked {
    audio: Vec<PathBuf>,
    /// Playlist files: see `playlist_files`.
    playlists: Vec<PathBuf>,
}

/// Every audio and playlist file under `path`. `follow_links` means a symlink
/// pointing at a sibling directory inside the library indexes its files under
/// both paths.
///
/// A path that is not UTF-8 is left out: the database stores paths as text, so
/// it would be stored under a name no stat finds, and the same scan would
/// remove it again along with its play history. So is anything under a name
/// the watcher ignores (see [`super::watch::is_ignored`]), below `path` itself.
fn walk(path: &Path, result: &mut ScanResult) -> Walked {
    let mut found = Walked::default();
    let entries = walkdir::WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !super::watch::is_ignored(e.file_name()));
    for entry in entries {
        match entry {
            Ok(e) if e.path().to_str().is_none() => {
                if e.file_type().is_file() {
                    log::warn!("skipping a path that is not UTF-8: {}", e.path().display());
                }
            }
            Ok(e) if e.file_type().is_file() && is_audio_file(e.path()) => {
                found.audio.push(e.path().to_path_buf())
            }
            Ok(e) if e.file_type().is_file() && is_playlist_file(e.path()) => {
                found.playlists.push(e.path().to_path_buf())
            }
            Ok(_) => {}
            Err(e) => {
                result.unreadable += 1;
                log::warn!("skipping unreadable entry under {}: {}", path.display(), e);
            }
        }
    }
    found
}

/// Read and store the files that changed since they were last indexed.
///
/// Returns false when the scan could not run at all, with the reason in
/// `result.errors` against `context`.
fn index_files(
    db: &Database,
    mut audio_files: Vec<PathBuf>,
    opts: &ScanOptions,
    on_track: Option<&dyn Fn(ScanEvent)>,
    result: &mut ScanResult,
    context: &Path,
) -> bool {
    let total_files = audio_files.len();
    if total_files == 0 {
        return true;
    }

    // Filter to files that need scanning.
    // Batch-load the entire scan_cache into a HashMap to avoid O(N) individual
    // DB lookups (one per file). For 100k+ file libraries this is dramatically faster.
    let files_to_scan: Vec<PathBuf> = if opts.force {
        std::mem::take(&mut audio_files)
    } else {
        let scan_cache = queries::load_scan_cache(&db.conn).unwrap_or_default();
        // One stat per file; in parallel, since on a network mount each is a round trip.
        audio_files
            .par_iter()
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

    result.skipped += total_files - files_to_scan.len();

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
    let cancel = opts.cancel.clone();
    let reader = std::thread::Builder::new()
        .name("koan-scan-read".into())
        .spawn(move || {
            // Stops at the first failed send (the consumer is gone) or cancel.
            let _ = files_to_scan.par_iter().try_for_each(|file_path| {
                if cancel
                    .as_ref()
                    .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
                {
                    return Err(());
                }
                send.send((
                    file_path.clone(),
                    isolate_read(file_path, metadata::read_metadata),
                ))
                .map_err(|_| ())
            });
        });
    if let Err(e) = &reader {
        log::error!("failed to spawn scan reader: {}", e);
        result
            .errors
            .push((context.to_path_buf(), format!("scan error: {}", e)));
        return false;
    }

    loop {
        // Blocks until a full batch is ready or the readers have finished.
        let batch: Vec<(PathBuf, Result<TrackMeta, String>)> =
            recv.iter().take(CHUNK_SIZE).collect();
        if batch.is_empty() {
            break;
        }
        if opts
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        {
            log::info!("scan cancelled — keeping what was already committed");
            result.cancelled = true;
            break;
        }

        let tx = match crate::db::queries::write_transaction(&db.conn) {
            Ok(tx) => tx,
            Err(e) => {
                log::error!("failed to begin scan transaction: {}", e);
                result
                    .errors
                    .push((context.to_path_buf(), format!("db error: {}", e)));
                return false;
            }
        };

        let (mut added, mut updated) = (0usize, 0usize);
        let mut arrived = Vec::new();
        for (file_path, meta_result) in batch {
            match meta_result {
                Ok(meta) => match queries::upsert_track_status(&tx, &meta) {
                    Ok((track_id, is_new)) => {
                        if is_new {
                            added += 1;
                            arrived.push(track_id);
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
                result.arrived.extend(arrived);
            }
            Err(e) => {
                log::error!("failed to commit scan transaction: {}", e);
                result
                    .errors
                    .push((context.to_path_buf(), format!("db error: {}", e)));
            }
        }
    }

    // Dropped before the join: readers parked on a full channel only wake
    // when their send fails.
    drop(recv);
    if let Ok(handle) = reader
        && handle.join().is_err()
    {
        log::error!("scan reader thread panicked");
    }
    true
}

/// Remove the rows under `path` whose files are gone, in one transaction,
/// after giving those that moved (to one of `arrived`) their tracks back.
fn remove_stale(
    db: &Database,
    path: &Path,
    force_remove: bool,
    arrived: &[i64],
    result: &mut ScanResult,
) {
    let tx = match crate::db::queries::write_transaction(&db.conn) {
        Ok(tx) => tx,
        Err(e) => {
            log::error!("failed to begin stale-removal transaction: {}", e);
            result
                .errors
                .push((path.to_path_buf(), format!("db error: {}", e)));
            return;
        }
    };
    let moved = match queries::adopt_moved_files(&tx, path, arrived) {
        Ok(moved) => moved,
        Err(e) => {
            log::error!("failed to match moved files: {}", e);
            result.errors.push((path.to_path_buf(), e.to_string()));
            return;
        }
    };
    result.moved += moved;
    match queries::remove_stale_tracks(&tx, path, force_remove) {
        Ok(removed) => {
            if let Err(e) = tx.commit() {
                log::error!("failed to commit stale removals: {}", e);
                result
                    .errors
                    .push((path.to_path_buf(), format!("db error: {}", e)));
                return;
            }
            result.removed += removed.len();
            result.removed_paths.extend(removed);
        }
        Err(e) => {
            log::error!("failed to remove stale tracks: {}", e);
            result.errors.push((path.to_path_buf(), e.to_string()));
            // The brake refuses before removing anything; the moves found
            // stand.
            if matches!(e, crate::db::connection::DbError::UnsafeBulkDelete(_))
                && let Err(e) = tx.commit()
            {
                log::error!("failed to commit moved files: {}", e);
            }
        }
    }
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
    // Dropped paths are spelled by whoever dropped them, and Foundation spells
    // accents differently from the disk. Walked children come from the
    // directory itself and need nothing.
    let mut spelling = super::spelling::Spelling::default();
    for path in paths {
        let path = spelling.on_disk(path);
        let mut found: Vec<PathBuf> = walkdir::WalkDir::new(&path)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && is_audio_file(e.path()))
            .filter(|e| {
                let utf8 = e.path().to_str().is_some();
                if !utf8 {
                    log::warn!("skipping a path that is not UTF-8: {}", e.path().display());
                }
                utf8
            })
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

    let tx = match crate::db::queries::write_transaction(&db.conn) {
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
    if result.added > 0 {
        super::playlist_files::refresh_m3u(db);
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
    let existing: Vec<PathBuf> = folders
        .iter()
        .filter(|folder| {
            let exists = folder.exists();
            if !exists {
                log::warn!("library folder does not exist: {}", folder.display());
            }
            exists
        })
        .cloned()
        .collect();
    let total = scan_folders(db, &existing, opts, None, on_track);
    db.optimize();
    total
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
    fn cancelling_a_scan_returns_with_readers_still_pending() {
        let dir = tempfile::tempdir().unwrap();
        let music_dir = dir.path().join("music");
        std::fs::create_dir_all(&music_dir).unwrap();
        // Far more than the channel holds (2 * CHUNK_SIZE), so readers are
        // parked on a full channel when the consumer stops.
        for i in 0..CHUNK_SIZE * 12 {
            test_utils::generate_wav(&music_dir.join(format!("{i:03}.wav")), 8000, 1, 0.01, 16);
        }

        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let root = dir.path().to_path_buf();
        std::thread::spawn(move || {
            let db = test_db(&root);
            let cancel = Arc::new(AtomicBool::new(false));
            let opts = ScanOptions {
                cancel: Some(cancel.clone()),
                ..Default::default()
            };
            let on_track = |_: ScanEvent| cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            let result = scan_folder(&db, &music_dir, opts, Some(&on_track));
            done_tx.send(result).unwrap();
        });

        let result = done_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("cancelled scan never returned");
        assert!(result.cancelled);
        assert!(result.added < CHUNK_SIZE * 12);
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
    fn a_renamed_file_keeps_its_track() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let album = music.join("Squire Of Gothos").join("Album");
        std::fs::create_dir_all(&album).unwrap();
        for i in 1..=3 {
            test_utils::generate_wav(
                &album.join(format!("0{i}. Squire Of Gothos - T{i}.wav")),
                44100,
                1,
                0.5 + i as f32 * 0.1,
                16,
            );
        }
        let db = test_db(dir.path());
        scan_folder(&db, &music, ScanOptions::default(), None);
        let before = tracks_with_uids(&db);
        let first = before[0].0;
        queries::record_play(&db.conn, queries::LOCAL_USER, first, Some(1000)).unwrap();
        queries::add_favourite(&db.conn, queries::LOCAL_USER, first).unwrap();

        // What a retag that re-files does: the artist folder and file names
        // change case.
        let moved = music.join("The Squire of Gothos").join("Album");
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::rename(&album, &moved).unwrap();
        std::fs::remove_dir(music.join("Squire Of Gothos")).unwrap();
        for i in 1..=3 {
            std::fs::rename(
                moved.join(format!("0{i}. Squire Of Gothos - T{i}.wav")),
                moved.join(format!("0{i}. The Squire of Gothos - T{i}.wav")),
            )
            .unwrap();
        }
        let result = scan_folder(&db, &music, ScanOptions::default(), None);

        assert_eq!(
            (result.added, result.removed, result.moved),
            (0, 0, 3),
            "{:?}",
            result.errors
        );
        let paths = track_paths(&db);
        assert_eq!(paths.len(), 3, "{paths:#?}");
        assert!(paths.iter().all(|p| Path::new(p).exists()), "{paths:#?}");
        let after = tracks_with_uids(&db);
        assert_eq!(
            before.iter().map(|t| (t.0, &t.1)).collect::<Vec<_>>(),
            after.iter().map(|t| (t.0, &t.1)).collect::<Vec<_>>(),
            "same rows, same uids"
        );
        let kept: (i64, i64) = db
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM play_history WHERE track_id = ?1),
                        (SELECT COUNT(*) FROM favourites WHERE track_id = ?1)",
                [first],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kept, (1, 1), "history and favourite kept");
    }

    /// Moved from one library folder to another: every folder is indexed
    /// before any is pruned, so the old path is not missed first.
    #[test]
    fn a_file_moved_between_library_folders_keeps_its_track() {
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (dir.path().join("one"), dir.path().join("two"));
        std::fs::create_dir_all(one.join("Album")).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        test_utils::generate_wav(&one.join("Album/a.wav"), 44100, 1, 0.2, 16);
        test_utils::generate_wav(&one.join("b.wav"), 44100, 1, 0.3, 16);
        test_utils::generate_wav(&two.join("c.wav"), 44100, 1, 0.4, 16);
        let db = test_db(dir.path());
        let folders = [one.clone(), two.clone()];
        scan_folders(&db, &folders, ScanOptions::default(), None, None);
        let before = tracks_with_uids(&db);

        std::fs::rename(one.join("Album"), two.join("Album")).unwrap();
        let r = scan_folders(&db, &folders, ScanOptions::default(), None, None);
        assert_eq!((r.added, r.removed, r.moved), (0, 0, 1), "{:?}", r.errors);
        let after = tracks_with_uids(&db);
        assert_eq!(
            before.iter().map(|t| (t.0, &t.1)).collect::<Vec<_>>(),
            after.iter().map(|t| (t.0, &t.1)).collect::<Vec<_>>()
        );
        assert!(after.iter().any(|t| t.2.ends_with("two/Album/a.wav")));
    }

    /// Track ids, uids and paths, by id.
    fn tracks_with_uids(db: &Database) -> Vec<(i64, String, String)> {
        db.conn
            .prepare("SELECT id, uid, path FROM tracks ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn track_paths(db: &Database) -> Vec<String> {
        db.conn
            .prepare("SELECT path FROM tracks ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// A track also on the server outlives its folder being forgotten, as the
    /// server's copy. Added back, the folder's file must be read again.
    #[test]
    fn a_forgotten_folder_added_back_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let file = music.join("kept.wav");
        test_utils::generate_wav(&file, 8000, 1, 0.1, 16);
        let db = test_db(dir.path());
        let mut both = metadata::read_metadata(&file).unwrap();
        both.remote_id = Some("sub-1".into());
        let track = queries::upsert_track(&db.conn, &both).unwrap();
        let local = |db: &Database| -> Option<String> {
            db.conn
                .query_row("SELECT path FROM tracks WHERE id = ?1", [track], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        scan_folder(&db, &music, ScanOptions::default(), None);

        assert_eq!(crate::helpers::forget_folder(&db, &music).unwrap(), 1);
        assert_eq!(local(&db), None, "kept as the server's copy");

        let again = scan_folder(&db, &music, ScanOptions::default(), None);
        assert_eq!(again.skipped, 0, "{:?}", again.errors);
        assert_eq!(local(&db), Some(file.to_string_lossy().into_owned()));
    }

    #[test]
    fn scans_skip_what_the_watcher_ignores() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let album = music.join("Artist/Album");
        let dotted = music.join("Britney Spears/...Baby One More Time");
        for sub in [
            album.join(".stversions"),
            album.join("Disc 2.part"),
            dotted.clone(),
        ] {
            std::fs::create_dir_all(sub).unwrap();
        }
        let wav = |p: PathBuf| test_utils::generate_wav(&p, 8000, 1, 0.1, 16);
        wav(album.join("01.wav"));
        wav(dotted.join("01.wav"));
        wav(album.join(".stversions/01~20261006-120000.wav"));
        wav(album.join("._01.wav"));
        wav(album.join("Disc 2.part/02.wav"));
        wav(album.join("~syncthing~03.wav"));
        let db = test_db(dir.path());

        let full = scan_folder(&db, &music, ScanOptions::default(), None);
        assert_eq!(full.added, 2, "{:?}", full.errors);
        let dirs = scan_dirs(
            &db,
            std::slice::from_ref(&music),
            std::slice::from_ref(&album),
            ScanOptions::default(),
            None,
        );
        assert_eq!(dirs.added, 0, "{:?}", dirs.errors);
        assert_eq!(
            track_paths(&db),
            [album.join("01.wav"), dotted.join("01.wav")].map(|p| p.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn minimal_dirs_keeps_only_the_outermost() {
        let dirs = [
            "/m/A/Album 2",
            "/m/A/Album",
            "/m/A/Album/CD1",
            "/m/A/Album",
            "/m/AB",
            "/m/B/Album/CD2",
        ]
        .map(PathBuf::from)
        .to_vec();
        assert_eq!(
            minimal_dirs(dirs),
            ["/m/A/Album", "/m/A/Album 2", "/m/AB", "/m/B/Album/CD2"].map(PathBuf::from)
        );
        assert_eq!(
            minimal_dirs(["/m/A/x", "/m/A"].map(PathBuf::from).to_vec()),
            [PathBuf::from("/m/A")]
        );
    }

    #[test]
    fn scan_dirs_covers_only_the_directories_named() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let (old, new) = (music.join("Old"), music.join("New"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        test_utils::generate_wav(&old.join("a.wav"), 44100, 1, 0.2, 16);

        let db = test_db(dir.path());
        scan_folder(&db, &music, ScanOptions::default(), None);
        test_utils::generate_wav(&new.join("b.wav"), 44100, 1, 0.2, 16);
        // Not named, so not seen: this scan is not a walk of the library.
        test_utils::generate_wav(&old.join("c.wav"), 44100, 1, 0.3, 16);

        let r = scan_dirs(
            &db,
            std::slice::from_ref(&music),
            std::slice::from_ref(&new),
            ScanOptions::default(),
            None,
        );
        assert_eq!((r.added, r.skipped, r.removed), (1, 0, 0), "{:?}", r.errors);
        assert_eq!(track_paths(&db).len(), 2);
    }

    #[test]
    fn rescanning_a_directory_tells_the_apps_its_art_may_have_changed() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let (album, other) = (music.join("Album"), music.join("Other"));
        std::fs::create_dir_all(album.join("CD1")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        test_utils::generate_wav(&album.join("CD1/a.wav"), 44100, 1, 0.2, 16);
        test_utils::generate_wav(&other.join("b.wav"), 44100, 1, 0.2, 16);
        let db = test_db(dir.path());
        scan_folder(&db, &music, ScanOptions::default(), None);
        let evicted = |db: &Database| -> Vec<(String, i64)> {
            db.conn
                .prepare("SELECT kind, id FROM art_evictions ORDER BY seq")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert!(evicted(&db).is_empty());

        // The watcher names the album's folder when a cover lands in it.
        std::fs::write(album.join("cover.jpg"), b"").unwrap();
        scan_dirs(
            &db,
            std::slice::from_ref(&music),
            std::slice::from_ref(&album),
            ScanOptions::default(),
            None,
        );
        let (album_id, track_id): (i64, i64) = db
            .conn
            .query_row(
                "SELECT album_id, id FROM tracks WHERE path LIKE '%/CD1/a.wav'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            evicted(&db),
            vec![("album".into(), album_id), ("track".into(), track_id)],
            "only the records under the folder rescanned"
        );
    }

    #[test]
    fn scan_dirs_removes_what_left_a_directory_and_a_directory_that_left() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let (kept, gone) = (music.join("Artist/Kept"), music.join("Artist/Gone"));
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::create_dir_all(&gone).unwrap();
        for i in 0..3 {
            test_utils::generate_wav(&kept.join(format!("{i}.wav")), 44100, 1, 0.2, 16);
            test_utils::generate_wav(&gone.join(format!("{i}.wav")), 44100, 1, 0.3, 16);
        }
        let db = test_db(dir.path());
        assert_eq!(
            scan_folder(&db, &music, ScanOptions::default(), None).added,
            6
        );

        std::fs::remove_file(kept.join("0.wav")).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        let r = scan_dirs(
            &db,
            std::slice::from_ref(&music),
            &[kept.clone(), gone.clone()],
            ScanOptions::default(),
            None,
        );

        assert_eq!((r.removed, r.skipped), (4, 2), "{:?}", r.errors);
        let paths = track_paths(&db);
        assert_eq!(paths.len(), 2, "{paths:#?}");
        assert!(paths.iter().all(|p| Path::new(p).exists()), "{paths:#?}");
    }

    /// An unmounted volume leaves an empty mount point, under which every
    /// directory reads as deleted.
    #[test]
    fn scan_dirs_removes_nothing_under_an_empty_library_folder() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let album = music.join("Album");
        std::fs::create_dir_all(&album).unwrap();
        test_utils::generate_wav(&album.join("a.wav"), 44100, 1, 0.2, 16);
        let db = test_db(dir.path());
        scan_folder(&db, &music, ScanOptions::default(), None);

        std::fs::remove_dir_all(&album).unwrap();
        let r = scan_dirs(
            &db,
            std::slice::from_ref(&music),
            std::slice::from_ref(&album),
            ScanOptions::default(),
            None,
        );
        assert_eq!(r.removed, 0);
        assert_eq!(track_paths(&db).len(), 1);
    }

    /// A share linked into the library and gone away leaves a dangling
    /// symlink, through which every file reads as deleted. Neither a full
    /// scan nor the watcher's forced rescan may take that as a deletion.
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_is_not_a_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let local = music.join("Local");
        let share = dir.path().join("share");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(share.join("Album")).unwrap();
        test_utils::generate_wav(&local.join("a.wav"), 44100, 1, 0.2, 16);
        test_utils::generate_wav(&share.join("Album/b.wav"), 44100, 1, 0.2, 16);
        std::os::unix::fs::symlink(&share, music.join("nas")).unwrap();
        let db = test_db(dir.path());
        assert_eq!(
            scan_folder(&db, &music, ScanOptions::default(), None).added,
            2
        );

        std::fs::remove_dir_all(&share).unwrap();
        let full = scan_folder(&db, &music, ScanOptions::default(), None);
        assert_eq!(full.removed, 0, "{:?}", full.removed_paths);
        let watched = scan_dirs(
            &db,
            std::slice::from_ref(&music),
            &[music.join("nas/Album")],
            ScanOptions::default(),
            None,
        );
        assert_eq!(watched.removed, 0, "{:?}", watched.removed_paths);
        assert_eq!(track_paths(&db).len(), 2);

        // The link itself removed is a deletion like any other.
        std::fs::remove_file(music.join("nas")).unwrap();
        let removed = scan_folder(&db, &music, ScanOptions::default(), None);
        assert_eq!(removed.removed, 1);
    }

    /// A name that is not UTF-8 would be stored under a different one, which
    /// no stat finds: the same scan would add it and remove it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_path_that_is_not_utf8_is_skipped() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        test_utils::generate_wav(&music.join("a.wav"), 44100, 1, 0.2, 16);
        let odd = music.join(std::ffi::OsStr::from_bytes(b"caf\xe9.wav"));
        test_utils::generate_wav(&odd, 44100, 1, 0.2, 16);
        let db = test_db(dir.path());

        let r = scan_folder(&db, &music, ScanOptions::default(), None);
        assert_eq!((r.added, r.removed), (1, 0));
        assert_eq!(track_paths(&db).len(), 1);
    }

    #[test]
    fn scan_dirs_ignores_directories_outside_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        test_utils::generate_wav(&elsewhere.join("a.wav"), 44100, 1, 0.2, 16);
        let db = test_db(dir.path());

        let r = scan_dirs(
            &db,
            std::slice::from_ref(&music),
            std::slice::from_ref(&elsewhere),
            ScanOptions::default(),
            None,
        );
        assert_eq!(r.added, 0);
        assert!(track_paths(&db).is_empty());
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

    #[test]
    fn a_drop_and_a_scan_spell_a_file_the_same_way() {
        use unicode_normalization::UnicodeNormalization;
        let dir = tempfile::tempdir().unwrap();
        let nfd: String = "Roman Flügel".nfd().collect();
        let nfc: String = "Roman Flügel".nfc().collect();
        let music_dir = dir.path().join(&nfd);
        std::fs::create_dir_all(&music_dir).unwrap();
        test_utils::generate_wav(&music_dir.join("softice.wav"), 44100, 1, 0.2, 16);
        let db = test_db(dir.path());

        // Dropped from Finder: the path arrives precomposed.
        let dropped = import_paths(&db, &[dir.path().join(&nfc).join("softice.wav")]);
        assert_eq!(dropped.added, 1, "errors: {:?}", dropped.errors);

        // Rescanned from the folder: the walker reads the directory's own bytes.
        let scanned = scan_folder(&db, &music_dir, ScanOptions::default(), None);
        assert_eq!(scanned.added, 0, "the same file, not a second one");

        let stats = queries::library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 1);
        let stored: String = db
            .conn
            .query_row("SELECT path FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, music_dir.join("softice.wav").to_string_lossy());
    }
}
