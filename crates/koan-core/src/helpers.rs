//! Shared helpers used by downstream crates (koan-tui, koan-server, koan-cli).
//!
//! These functions provide common functionality for building playlist items,
//! resolving track paths, downloading remote tracks, and building Subsonic clients.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::db::queries::shares::{ShareKind, Slice};
use crate::player::commands::PlayerCommand;
use crate::player::state::{ItemState, PlaylistItem, QueueItemId, SharedPlayerState};
use crate::remote::client::{SubsonicAuth, SubsonicClient, SubsonicError};
use crate::remote::download::DownloadError;

// ---------------------------------------------------------------------------
// Subsonic client builder
// ---------------------------------------------------------------------------

/// The remote password, from `config.local.toml` or a `KOAN_REMOTE__PASSWORD`
/// layered over it.
pub fn get_remote_password(cfg: &Config) -> Option<String> {
    (!cfg.remote.password.is_empty()).then(|| cfg.remote.password.clone())
}

/// Index files that appear in the library folders while koan is running.
///
/// One incremental scan shortly after startup — the walk is a fraction of a
/// second even across fifty thousand files, and everything unchanged is skipped
/// on its mtime and size — then a scan of whatever the folders say changed.
///
/// Only the directories events name are scanned: walking the whole library for
/// one new album costs a spinning disk minutes. Events that cannot change the
/// index — access, metadata, Syncthing's bookkeeping, partial downloads — are
/// dropped before they count (see `index::watch`). The whole library is scanned
/// only when the watcher reports it lost events, or when so many directories
/// changed at once that walking them one by one would cost more.
///
/// Changes are debounced: copying an album in produces a burst of events, and
/// scanning once per file would be both slow and pointless. A scan that lands
/// halfway through a move is corrected by the scan the rest of the move's
/// events bring, since a directory scan removes what is no longer under it.
///
/// The folder list is re-read and each folder's identity checked every half
/// minute, so a folder added in settings is watched without a restart, and a
/// volume unmounted and mounted again is watched afresh and rescanned.
///
/// `on_state` reports whether a scan is running, so a UI can show it.
pub fn spawn_library_watch(
    db_path: std::path::PathBuf,
    on_state: impl Fn(bool) + Send + Sync + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    use std::collections::BTreeSet;
    use std::time::{Duration, Instant};

    use notify::{RecursiveMode, Watcher};

    use crate::index::scanner::{self, ScanOptions};
    use crate::index::watch::{WatchedRoot, scan_target};

    // Copying an album in is a burst of events. Wait for it to stop before
    // scanning, rather than scanning per file.
    const SETTLE: Duration = Duration::from_secs(5);
    // How often the folder list and each folder's identity are checked.
    const CHECK: Duration = Duration::from_secs(30);
    // Past this many directories one walk of the library is cheaper than many.
    const MAX_DIRS: usize = 200;
    // A full scan now and then, for the events a platform drops without
    // asking for a rescan. Unchanged files are skipped on mtime and size, so an
    // idle one is a second's work.
    const RESCAN: Duration = Duration::from_secs(15 * 60);

    std::thread::Builder::new()
        .name("koan-library-watch".into())
        .spawn(move || {
            let scan = |reason: &str, folders: &[PathBuf], dirs: Option<&[PathBuf]>| {
                if folders.is_empty() {
                    return;
                }
                let Ok(db) = Database::open(&db_path) else {
                    return;
                };
                on_state(true);
                let result = match dirs {
                    Some(dirs) => {
                        scanner::scan_dirs(&db, folders, dirs, ScanOptions::default(), None)
                    }
                    None => scanner::full_scan(&db, folders, ScanOptions::default(), None),
                };
                on_state(false);
                log::info!(
                    "{reason} scan: {} added, {} updated, {} removed, {} unchanged",
                    result.added,
                    result.updated,
                    result.removed,
                    result.skipped
                );
            };
            let folders = || Config::cached().library.folders.clone();

            let (tx, rx) = std::sync::mpsc::channel();
            let Ok(mut watcher) = notify::recommended_watcher(move |event| {
                let _ = tx.send(event);
            }) else {
                log::warn!("could not watch the library folders");
                return;
            };

            // Brings the watches in line with the configured folders as they
            // are now, returning the ones newly watched — added in settings, or
            // back from being unmounted — whose changes nobody heard.
            let mut roots: Vec<WatchedRoot> = Vec::new();
            let mut rewatch = |roots: &mut Vec<WatchedRoot>| {
                let wanted: Vec<WatchedRoot> = folders()
                    .iter()
                    .filter_map(|f| WatchedRoot::resolve(f))
                    .collect();
                roots.retain(|root| {
                    let keep = wanted.contains(root);
                    if !keep {
                        let _ = watcher.unwatch(&root.path);
                    }
                    keep
                });
                let mut fresh = Vec::new();
                for root in wanted {
                    if roots.contains(&root) {
                        continue;
                    }
                    match watcher.watch(&root.path, RecursiveMode::Recursive) {
                        Ok(()) => {
                            fresh.push(root.path.clone());
                            roots.push(root);
                        }
                        Err(e) => log::warn!("could not watch {}: {e}", root.path.display()),
                    }
                }
                fresh
            };

            // After the first frame and the first track, not competing with them.
            std::thread::sleep(Duration::from_secs(3));
            rewatch(&mut roots);
            scan("startup", &folders(), None);

            let mut dirs = BTreeSet::new();
            let mut everything = false;
            let mut settle_at: Option<Instant> = None;
            let mut check_at = Instant::now() + CHECK;
            let mut rescan_at = Instant::now() + RESCAN;
            loop {
                let now = Instant::now();
                let wake = settle_at
                    .map_or(check_at, |at| at.min(check_at))
                    .min(rescan_at);
                match rx.recv_timeout(wake.saturating_duration_since(now)) {
                    Ok(Ok(event)) if event.need_rescan() => {
                        everything = true;
                        settle_at = Some(Instant::now() + SETTLE);
                    }
                    Ok(Ok(event)) => {
                        let mut heard = false;
                        for dir in event
                            .paths
                            .iter()
                            .filter_map(|p| scan_target(&event.kind, p, &roots))
                        {
                            dirs.insert(dir);
                            heard = true;
                        }
                        if heard {
                            settle_at = Some(Instant::now() + SETTLE);
                        }
                    }
                    Ok(Err(e)) => log::debug!("library watch: {e}"),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }

                let now = Instant::now();
                if settle_at.is_some_and(|at| now >= at) {
                    let changed =
                        scanner::minimal_dirs(std::mem::take(&mut dirs).into_iter().collect());
                    if everything || changed.len() > MAX_DIRS {
                        scan("watched change", &folders(), None);
                        rescan_at = Instant::now() + RESCAN;
                    } else {
                        scan("watched change", &folders(), Some(&changed));
                    }
                    everything = false;
                    settle_at = None;
                }
                if now >= rescan_at {
                    scan("periodic", &folders(), None);
                    rescan_at = Instant::now() + RESCAN;
                }
                if now >= check_at {
                    let fresh = rewatch(&mut roots);
                    if !fresh.is_empty() {
                        scan("newly watched", &fresh, None);
                    }
                    check_at = Instant::now() + CHECK;
                }
            }
        })
        .ok()
}

/// Keep the library in step with the server, without being asked.
///
/// One sync shortly after startup, then every `auto_sync_interval_mins`. Always
/// incremental: it asks the server what changed rather than walking the whole
/// library, which is what makes it cheap enough to run unattended. A full sync
/// stays a deliberate action.
///
/// The startup run is delayed a few seconds so it is not competing with the
/// first frame and the first track for the disk.
///
/// `on_state` reports whether a sync is running, so a UI can say so rather than
/// appearing to do nothing, and `on_progress` how far it has got. The first
/// sync against a server has no watermark and walks the whole library.
pub fn spawn_auto_sync(
    db_path: std::path::PathBuf,
    on_state: impl Fn(bool) + Send + 'static,
    on_progress: impl Fn(crate::remote::sync::SyncProgress) + Send + Sync + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("koan-auto-sync".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            loop {
                let cfg = Config::load().unwrap_or_default();
                if !cfg.remote.enabled || !cfg.remote.auto_sync {
                    // Re-read rather than exit: the setting can be turned on
                    // while the app is running.
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    continue;
                }

                if let Some(client) = subsonic_client(&cfg)
                    && let Ok(db) = Database::open(&db_path)
                {
                    on_state(true);
                    match sync_remote(
                        &db,
                        &client,
                        false,
                        &cfg.remote.url,
                        &cfg.remote.username,
                        &on_progress,
                    ) {
                        Ok(s) => log::info!(
                            "auto sync: {} artists, {} albums, {} tracks ({} albums failed); \
                             favourites {}↑ {}↓; playlists {}↓ {}↑",
                            s.library.artists_synced,
                            s.library.albums_synced,
                            s.library.tracks_synced,
                            s.library.albums_failed,
                            s.favourites.pushed,
                            s.favourites.imported,
                            s.playlists.pulled,
                            s.playlists.pushed,
                        ),
                        Err(e) => log::warn!("auto sync failed: {e}"),
                    }
                    on_state(false);
                }

                match cfg.remote.auto_sync_interval_mins {
                    // Once at startup and no more.
                    0 => return,
                    mins => std::thread::sleep(std::time::Duration::from_secs(mins * 60)),
                }
            }
        })
        .ok()
}

/// What a library rebuild removed.
#[derive(Debug, Clone, Copy, Default)]
pub struct RebuildSummary {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
}

/// Drop the index so the next scan rebuilds it from the files.
///
/// Favourites are keyed on the file path rather than a row id, so they survive
/// this and re-attach when the paths come back. Everything keyed on a track id
/// cannot: lyrics, play history and acoustic embeddings go, and the foreign keys
/// would refuse the delete otherwise. Lyrics and embeddings are re-derivable;
/// play counts are not, which is worth saying out loud wherever this is offered.
///
/// The remote half of the library comes back on the next sync, the local half on
/// the next scan.
pub fn rebuild_index(db: &Database) -> Result<RebuildSummary, crate::db::connection::DbError> {
    let count = |sql: &str| -> u64 {
        db.conn
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64
    };
    let summary = RebuildSummary {
        tracks: count("SELECT COUNT(*) FROM tracks"),
        albums: count("SELECT COUNT(*) FROM albums"),
        artists: count("SELECT COUNT(*) FROM artists"),
    };

    // Children before parents; the FTS index has no foreign keys but is derived
    // from tracks and would otherwise keep answering for rows that are gone.
    db.conn.execute_batch(
        "BEGIN;
         DELETE FROM track_vectors;
         DELETE FROM lyrics_cache;
         DELETE FROM play_history;
         DELETE FROM scan_cache;
         DELETE FROM tracks_fts;
         DELETE FROM tracks;
         DELETE FROM similar_artists;
         DELETE FROM albums;
         DELETE FROM artists;
         COMMIT;",
    )?;
    let _ = db.conn.execute_batch("VACUUM");
    Ok(summary)
}

/// Remove whole albums from the download cache, least recently played first,
/// until it is under the configured limit. Never an album with a favourite
/// in it, nor one with a track in `keep`: the queue, whose files the player
/// may be reading. Returns the bytes freed.
pub fn evict_cache(
    db: &Database,
    cfg: &Config,
    keep: &std::collections::HashSet<i64>,
    verbose: bool,
) -> u64 {
    let Some(limit) = cfg.cache_limit_bytes().map(|l| l as i64) else {
        return 0;
    };
    let mut current = match queries::total_cache_size(&db.conn) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("cache eviction: failed to query cache size: {e}");
            return 0;
        }
    };
    if current <= limit {
        if verbose {
            log::info!("cache within limit: {current} / {limit} bytes");
        }
        return 0;
    }
    let albums = match queries::cached_albums_lru(&db.conn) {
        Ok(a) => a,
        Err(e) => {
            log::warn!("cache eviction: failed to query cached albums: {e}");
            return 0;
        }
    };
    let mut freed: i64 = 0;
    for album in &albums {
        if current <= limit {
            break;
        }
        if album.track_ids.iter().any(|id| keep.contains(id)) {
            continue;
        }
        for path in &album.cached_paths {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => log::warn!("cache eviction: failed to delete {path}: {e}"),
            }
        }
        if let Err(e) = queries::clear_cache_for_tracks(&db.conn, &album.track_ids) {
            log::warn!("cache eviction: failed to clear DB for album: {e}");
        }
        log::info!(
            "evicted: {} — {} ({} bytes)",
            album.artist_name,
            album.album_title,
            album.total_size
        );
        current -= album.total_size;
        freed += album.total_size;
    }
    remove_empty_dirs(&cfg.cache_dir());
    if freed > 0 {
        log::info!("cache eviction freed {freed} bytes");
    }
    freed as u64
}

/// Remove empty directories under `dir`, leaving `dir` itself.
fn remove_empty_dirs(dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    for entry in walkdir::WalkDir::new(dir)
        .contents_first(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_dir() && e.path() != dir)
    {
        let _ = std::fs::remove_dir(entry.path());
    }
}

/// Bytes currently held in the download cache.
pub fn cache_size_bytes(cfg: &Config) -> u64 {
    walkdir::WalkDir::new(cfg.cache_dir())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// How many tracks came from this folder.
///
/// The trailing separator matters: without it `/Volumes/Music` also counts
/// `/Volumes/Music Backup`.
pub fn tracks_under(db: &Database, folder: &Path) -> u64 {
    let (lower, upper) = queries::folder_prefix_range(folder);
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE path >= ?1 AND path < ?2",
            [&lower, &upper],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
}

/// How many tracks the server accounts for.
pub fn tracks_from_server(db: &Database) -> u64 {
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE remote_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
}

/// Forget every track under a folder.
///
/// Removing a folder from the library should remove what it put there —
/// otherwise the library keeps showing records whose files it will never look
/// at again, and there is no way back to an empty library short of clearing the
/// whole index.
///
/// A track that also exists on the server keeps its row and loses only its local
/// path: it is still playable, just by download rather than from disk.
///
/// Albums and artists left holding nothing go too, or the browser fills with
/// empty shelves.
pub fn forget_folder(db: &Database, folder: &Path) -> Result<u64, crate::db::connection::DbError> {
    // Rows are keyed by the disk's spelling; a folder named the other way would forget nothing.
    let folder = &crate::index::spelling::on_disk(folder);
    let (lower, upper) = queries::folder_prefix_range(folder);

    let tx = db.conn.unchecked_transaction()?;
    // Still on the server: keep the row, drop the local file.
    tx.execute(
        "UPDATE tracks SET path = NULL, source = 'remote'
          WHERE path >= ?1 AND path < ?2 AND remote_id IS NOT NULL",
        [&lower, &upper],
    )?;

    let ids: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM tracks WHERE path >= ?1 AND path < ?2")?;
        let rows = stmt.query_map([&lower, &upper], |r| r.get(0))?;
        rows.filter_map(Result::ok).collect()
    };
    for id in &ids {
        tx.execute("DELETE FROM track_vectors WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM lyrics_cache WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM play_history WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM scan_cache WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM tracks_fts WHERE rowid = ?1", [id])?;
        tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
    }
    prune_empty_albums_and_artists(&tx)?;
    tx.commit()?;
    Ok(ids.len() as u64)
}

/// Forget everything that only existed on the server.
///
/// Signing out should leave the library with what is actually on this machine.
/// A track held both locally and remotely keeps its row and loses its remote id;
/// one that only ever came from the server goes.
pub fn forget_remote(db: &Database) -> Result<u64, crate::db::connection::DbError> {
    let tx = db.conn.unchecked_transaction()?;

    let ids: Vec<i64> = {
        let mut stmt =
            tx.prepare("SELECT id FROM tracks WHERE remote_id IS NOT NULL AND path IS NULL")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.filter_map(Result::ok).collect()
    };
    for id in &ids {
        tx.execute("DELETE FROM track_vectors WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM lyrics_cache WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM play_history WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM scan_cache WHERE track_id = ?1", [id])?;
        tx.execute("DELETE FROM tracks_fts WHERE rowid = ?1", [id])?;
        tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
    }
    // Local copies stay, minus the server they were also on.
    tx.execute(
        "UPDATE tracks SET remote_id = NULL, remote_url = NULL, source = 'local'
          WHERE remote_id IS NOT NULL",
        [],
    )?;
    tx.execute("DELETE FROM similar_artists", [])?;
    prune_empty_albums_and_artists(&tx)?;
    tx.commit()?;
    Ok(ids.len() as u64)
}

/// Albums and artists with nothing left in them.
fn prune_empty_albums_and_artists(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), crate::db::connection::DbError> {
    tx.execute(
        "DELETE FROM albums WHERE NOT EXISTS
           (SELECT 1 FROM tracks WHERE tracks.album_id = albums.id)",
        [],
    )?;
    tx.execute(
        "DELETE FROM similar_artists WHERE NOT EXISTS
           (SELECT 1 FROM albums WHERE albums.artist_id = similar_artists.artist_id)",
        [],
    )?;
    tx.execute(
        "DELETE FROM artists WHERE NOT EXISTS
             (SELECT 1 FROM albums WHERE albums.artist_id = artists.id)
           AND NOT EXISTS
             (SELECT 1 FROM tracks WHERE tracks.artist_id = artists.id)",
        [],
    )?;
    Ok(())
}

/// What clearing the download cache removed.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheCleared {
    pub files: u64,
    pub bytes: u64,
}

/// Delete every downloaded remote track and forget where they were.
///
/// The rows stay — a remote track is still in the library, it just has to be
/// fetched again to play.
pub fn clear_download_cache(db: &Database, cfg: &Config) -> CacheCleared {
    let dir = cfg.cache_dir();
    let mut cleared = CacheCleared::default();
    for entry in walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(meta) = entry.metadata() {
            cleared.bytes += meta.len();
            cleared.files += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let _ = queries::clear_cached_paths(&db.conn);
    cleared
}

/// Delete the downloaded copies of just these tracks.
///
/// The per-track counterpart of `clear_download_cache`, for throwing away one
/// record rather than the lot. A track playing from a copy being removed keeps
/// playing — the decoder holds the file open, and unlinking it only takes the
/// name away — but the next play fetches it again.
pub fn clear_downloads_for(db: &Database, track_ids: &[i64]) -> CacheCleared {
    let mut cleared = CacheCleared::default();
    let paths = match queries::cached_paths_for(&db.conn, track_ids) {
        Ok(paths) => paths,
        Err(e) => {
            log::warn!("could not read cached paths: {e}");
            return cleared;
        }
    };
    for path in &paths {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(path) {
            Ok(()) => {
                cleared.files += 1;
                cleared.bytes += size;
            }
            // Already gone is the outcome asked for, so it is not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("could not remove {path}: {e}"),
        }
    }
    if let Err(e) = queries::clear_cached_paths_for(&db.conn, track_ids) {
        log::warn!("removed downloads but failed to forget them ({e})");
    }
    cleared
}

/// Throw away half-finished downloads left behind by a previous run.
///
/// A `.part` file only means something to the transfer writing it. koan does
/// not resume — the file is written straight through and renamed at the end —
/// so one still on disk at startup is from a run that did not finish, and it
/// will be truncated and rewritten the next time that track is wanted anyway.
/// Until then it is bytes nothing knows about: cache eviction only tracks what
/// finished, so an interrupted download of a nine-hour recording is half a
/// gigabyte that never gets reclaimed.
///
/// At startup rather than at exit, because a run that ends without getting to
/// its own cleanup is exactly the run that leaves these behind.
pub fn sweep_partial_downloads(cfg: &Config) -> CacheCleared {
    let mut swept = CacheCleared::default();
    for entry in walkdir::WalkDir::new(cfg.cache_dir())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "part"))
    {
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                swept.files += 1;
                swept.bytes += size;
            }
            Err(e) => log::warn!("could not remove {}: {e}", entry.path().display()),
        }
    }
    if swept.files > 0 {
        log::info!(
            "swept {} unfinished download(s), {} bytes",
            swept.files,
            swept.bytes
        );
    }
    swept
}

/// Fetch again anything in the queue whose downloaded copy has just been
/// removed.
///
/// Clearing downloads deletes files the queue is still pointing at, and an item
/// that goes on claiming to be ready plays nothing at all. Call this after
/// either clearing function, from anywhere with a player attached.
pub fn requeue_cleared_downloads(
    state: &Arc<SharedPlayerState>,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
) {
    let stale = state.reset_items_with_missing_files();
    if stale.is_empty() {
        return;
    }
    log::info!(
        "{} queued tracks lost their copy — fetching again",
        stale.len()
    );
    spawn_downloads(stale, tx.clone(), state.clone());
}

/// Push a favourite to the remote server, if this track came from one.
///
/// Fire and forget on its own thread: starring is a courtesy to the server, and
/// a slow or unreachable one should not hold up the click that caused it. The
/// local favourite is already written by the time this runs.
///
/// Silently does nothing for a track with no `remote_id` — including a local
/// file whose copy on the server failed to merge with it (#221), which is the
/// one case where the silence is wrong.
///
/// Shared by the TUI, the server and the app, which each had their own copy.
pub fn sync_favourite_to_remote(db: &Database, path: &Path, star: bool) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let Ok(Some(remote_id)) = queries::remote_id_for_path(&db.conn, path) else {
        log::warn!("not syncing favourite: {} has no remote id", path.display());
        return;
    };
    let Some(client) = subsonic_client(&cfg) else {
        log::warn!("not syncing favourite: no usable server credentials");
        return;
    };
    std::thread::Builder::new()
        .name("koan-fav-sync".into())
        .spawn(move || {
            let result = if star {
                client.star(&remote_id)
            } else {
                client.unstar(&remote_id)
            };
            match result {
                Ok(()) => log::info!("synced favourite to remote: {remote_id} = {star}"),
                Err(e) => log::warn!("failed to sync favourite to remote: {e}"),
            }
        })
        .ok();
}

/// Everything a sync is.
#[derive(Debug, Default)]
pub struct FullSync {
    pub library: crate::remote::sync::SyncResult,
    pub favourites: FavouriteSync,
    pub playlists: crate::playlists::PlaylistSync,
}

/// Pull the library, then reconcile favourites and playlists.
///
/// One function because there are four callers — the app, the CLI, the GraphQL
/// job and koan's own auto-sync — and they had each been told separately what a
/// sync consists of. Two of them never heard about playlists, and the auto-sync
/// had never heard about favourites either, so a star made on the server only
/// arrived if you happened to press the button yourself.
///
/// The library comes first: favourites and playlists both name tracks by the
/// server's ids, and neither can find a track the library has not seen yet.
pub fn sync_remote(
    db: &Database,
    client: &SubsonicClient,
    full: bool,
    url: &str,
    username: &str,
    progress: &(dyn Fn(crate::remote::sync::SyncProgress) + Sync),
) -> Result<FullSync, crate::remote::sync::SyncError> {
    let library = crate::remote::sync::sync_library(db, client, full, url, username, progress)?;
    Ok(FullSync {
        library,
        favourites: reconcile_favourites(db, client),
        playlists: crate::playlists::reconcile_playlists(db, client, username),
    })
}

/// What a favourites reconciliation did.
#[derive(Debug, Default, Clone, Copy)]
pub struct FavouriteSync {
    pub pushed: usize,
    pub imported: usize,
}

/// Reconcile favourites with the server, both directions.
///
/// Pushes every local favourite that the server knows about, then imports
/// everything the server has starred. Union rather than mirror: neither side
/// records an unstar, so treating one as authoritative would silently delete
/// favourites made on the other.
///
/// Covers albums and artists as well as tracks — `getStarred2` returns all
/// three from one request, and reading only songs left a starred album
/// invisible to koan.
pub fn reconcile_favourites(db: &Database, client: &SubsonicClient) -> FavouriteSync {
    let mut out = FavouriteSync::default();

    let tracks =
        queries::favourites_with_remote_id(&db.conn, queries::LOCAL_USER).unwrap_or_default();
    for (_path, remote_id) in &tracks {
        if client.star(remote_id).is_ok() {
            out.pushed += 1;
        }
    }
    for (_id, remote_id) in
        queries::favourite_albums_with_remote_id(&db.conn, queries::LOCAL_USER).unwrap_or_default()
    {
        if client.star_album(&remote_id).is_ok() {
            out.pushed += 1;
        }
    }
    for (_id, remote_id) in
        queries::favourite_artists_with_remote_id(&db.conn, queries::LOCAL_USER).unwrap_or_default()
    {
        if client.star_artist(&remote_id).is_ok() {
            out.pushed += 1;
        }
    }

    let starred = match client.get_starred_all() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("could not fetch starred items from the server: {e}");
            return out;
        }
    };

    let songs: Vec<String> = starred.song.into_iter().map(|s| s.id).collect();
    let albums: Vec<String> = starred.album.into_iter().map(|a| a.id).collect();
    let artists: Vec<String> = starred.artist.into_iter().map(|a| a.id).collect();
    out.imported +=
        queries::import_remote_favourites(&db.conn, queries::LOCAL_USER, &songs).unwrap_or(0);
    out.imported += queries::import_remote_favourite_albums(&db.conn, queries::LOCAL_USER, &albums)
        .unwrap_or(0);
    out.imported +=
        queries::import_remote_favourite_artists(&db.conn, queries::LOCAL_USER, &artists)
            .unwrap_or(0);
    out
}

/// What a favourite applies to. Subsonic stars all three, under different
/// parameter names — passing an album id as `id` silently stars nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FavouriteKind {
    Track,
    Album,
    Artist,
}

/// Push an album or artist favourite to the server.
///
/// Same shape as [`sync_favourite_to_remote`], but the remote id comes from the
/// album or artist row rather than the track's path.
pub fn sync_collection_favourite_to_remote(
    db: &Database,
    kind: FavouriteKind,
    id: i64,
    star: bool,
) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let remote_id = match kind {
        FavouriteKind::Album => queries::album_remote_id(&db.conn, id),
        FavouriteKind::Artist => queries::artist_remote_id(&db.conn, id),
        FavouriteKind::Track => return,
    };
    let Ok(Some(remote_id)) = remote_id else {
        log::warn!("not syncing favourite: {kind:?} {id} has no remote id");
        return;
    };
    let Some(client) = subsonic_client(&cfg) else {
        log::warn!("not syncing favourite: no usable server credentials");
        return;
    };
    std::thread::Builder::new()
        .name("koan-fav-sync".into())
        .spawn(move || {
            let result = match (kind, star) {
                (FavouriteKind::Album, true) => client.star_album(&remote_id),
                (FavouriteKind::Album, false) => client.unstar_album(&remote_id),
                (FavouriteKind::Artist, true) => client.star_artist(&remote_id),
                (FavouriteKind::Artist, false) => client.unstar_artist(&remote_id),
                (FavouriteKind::Track, _) => Ok(()),
            };
            match result {
                Ok(()) => log::info!("synced favourite to remote: {kind:?} {remote_id} = {star}"),
                Err(e) => log::warn!("failed to sync favourite to remote: {e}"),
            }
        })
        .ok();
}

/// Why signing in to a remote server failed.
#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error("the server did not accept those credentials: {0}")]
    Rejected(#[from] crate::remote::client::SubsonicError),
    #[error("could not write the configuration: {0}")]
    Config(#[from] crate::config::ConfigError),
}

/// Sign in to a Subsonic/Navidrome server and remember it.
///
/// The password goes to `config.local.toml`, which is gitignored and written
/// `0600`. Subsonic authenticates every request with the password or a salted
/// MD5 of it, so there is no token to hold instead — whatever koan keeps is
/// password-equivalent wherever it is kept.
///
/// The credentials are checked against the server before anything is written; a
/// stored password that does not work is worse than none.
///
/// Shared by the CLI and the app so the two cannot disagree about where
/// credentials live.
pub fn set_remote_credentials(
    url: &str,
    username: &str,
    password: &str,
) -> Result<(), SignInError> {
    let url = url.trim_end_matches('/');
    SubsonicClient::new(url, username, password).ping()?;

    Config::persist(|cfg| {
        cfg.remote.enabled = true;
        cfg.remote.url = url.to_string();
        cfg.remote.username = username.to_string();
        cfg.remote.password = password.to_string();
    })?;
    Ok(())
}

/// Shared secret for koan's own Subsonic API.
///
/// Deliberately not the same secret as `get_remote_password` — see `SubsonicConfig`.
pub fn get_subsonic_password(cfg: &Config) -> Option<String> {
    (!cfg.subsonic.password.is_empty()).then(|| cfg.subsonic.password.clone())
}

/// Upstream Subsonic credentials from the merged config, returning `None` if
/// remote is disabled or has no URL configured.
///
/// Prefer this over `subsonic_client` when only a signed URL is needed:
/// building a client constructs blocking `reqwest` clients, which panics from
/// inside a tokio runtime.
pub fn subsonic_auth(cfg: &Config) -> Option<SubsonicAuth> {
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        return None;
    }
    let password = get_remote_password(cfg)?;
    Some(SubsonicAuth::new(
        &cfg.remote.url,
        &cfg.remote.username,
        &password,
    ))
}

/// One `SubsonicClient` per set of credentials, shared process-wide.
///
/// Constructing one builds two blocking `reqwest` clients, each carrying its
/// own runtime on its own thread, and each starting with a cold connection
/// pool — so a client per call means a fresh TLS handshake for every cover art
/// request. The download queue had already worked this out and kept a client
/// of its own for the app's lifetime; this is that, for everyone.
///
/// Keyed on the credentials, so logging in as someone else replaces the client
/// rather than serving the old one. Never call from async code: building the
/// inner clients panics inside a tokio runtime.
pub fn subsonic_client(cfg: &Config) -> Option<Arc<SubsonicClient>> {
    let auth = subsonic_auth(cfg)?;

    let mut slot = SUBSONIC_CLIENT.lock();
    if let Some((cached, client)) = slot.as_ref()
        && *cached == auth
    {
        return Some(client.clone());
    }

    let client = Arc::new(SubsonicClient::from_auth(auth.clone()));
    *slot = Some((auth, client.clone()));
    Some(client)
}

type CachedClient = Option<(SubsonicAuth, Arc<SubsonicClient>)>;

static SUBSONIC_CLIENT: std::sync::LazyLock<parking_lot::Mutex<CachedClient>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(None));

// ---------------------------------------------------------------------------
// Sharing
// ---------------------------------------------------------------------------

/// Why a share link could not be made. Each variant is something the user can
/// act on, which is the point — every caller used to collapse these into
/// "local-only tracks can't be shared" and send people looking in the wrong
/// place.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error("no remote server is configured")]
    NoRemote,
    #[error("sharing.public_url is not set, so there is no address to give out")]
    NoPublicUrl,
    #[error("none of these tracks are in the library")]
    NothingToShare,
    #[error("none of these tracks are on the server, so a link has nothing to point at")]
    NothingRemote,
    #[error("the server refused to share these: {0}")]
    Server(#[from] crate::remote::client::SubsonicError),
    #[error(transparent)]
    Database(#[from] crate::db::connection::DbError),
}

/// A created share link, and how much of the request it covers.
#[derive(Debug, Clone)]
pub struct ShareOutcome {
    pub url: String,
    /// The server's own ID for the share, for callers that manage them.
    pub id: String,
    /// Tracks the server knows about, which went into the link.
    pub shared: usize,
    /// Tracks with no copy on the server, left out of it.
    pub skipped: usize,
}

/// What a share link is asked to cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareTarget {
    /// Loose tracks. A single track shares its album, cued to that track.
    Tracks(Vec<i64>),
    /// An album, optionally cued to one of its tracks.
    Album {
        album_id: i64,
        start_track_id: Option<i64>,
    },
    /// An artist's albums, in release order.
    Artist(i64),
}

/// The slice a target makes and the tracks it covers, in play order. Only
/// tracks in the library are included; the list is fixed from here on.
///
/// A single track becomes its album cued to it: a song is heard in the
/// record it belongs to, the way the app shows it.
pub fn resolve_share(
    conn: &rusqlite::Connection,
    target: &ShareTarget,
) -> Result<(Slice, Vec<i64>), ShareError> {
    let album_tracks = |album_id| -> Result<Vec<i64>, ShareError> {
        Ok(queries::tracks_for_album(conn, album_id)?
            .into_iter()
            .map(|t| t.id)
            .collect())
    };
    let (slice, ids) = match target {
        ShareTarget::Tracks(ids) => {
            let rows = queries::tracks_by_ids(conn, ids)?;
            match (ids.as_slice(), rows.first().and_then(|t| t.album_id)) {
                ([one], Some(album_id)) => {
                    return resolve_share(
                        conn,
                        &ShareTarget::Album {
                            album_id,
                            start_track_id: Some(*one),
                        },
                    );
                }
                _ => {
                    // The order asked for, which is the order the page plays them in.
                    let ids = ids
                        .iter()
                        .copied()
                        .filter(|id| rows.iter().any(|t| t.id == *id))
                        .collect();
                    (Slice::TRACKS, ids)
                }
            }
        }
        ShareTarget::Album {
            album_id,
            start_track_id,
        } => {
            let ids = album_tracks(*album_id)?;
            let slice = Slice {
                kind: ShareKind::Album,
                subject_id: Some(*album_id),
                start_track_id: start_track_id.filter(|s| ids.contains(s)),
            };
            (slice, ids)
        }
        ShareTarget::Artist(artist_id) => {
            let mut ids = Vec::new();
            for album in queries::albums_for_artist(conn, *artist_id)? {
                ids.extend(album_tracks(album.id)?);
            }
            let slice = Slice {
                kind: ShareKind::Artist,
                subject_id: Some(*artist_id),
                start_track_id: None,
            };
            (slice, ids)
        }
    };
    if ids.is_empty() {
        return Err(ShareError::NothingToShare);
    }
    Ok((slice, ids))
}

/// Create a public share link for a slice of the library.
///
/// With a remote Subsonic server configured, the link is made there: a laptop
/// or phone shares through the server it plays from, which may be another
/// koan. Without one this koan is the server, and makes the link itself.
///
/// A link points at the server, so only tracks the server knows about can go in
/// it. A mixed selection shares the part that can be shared and reports the
/// rest rather than failing whole — half a link beats none, as long as the
/// caller says which half.
///
/// `user` is who is sharing, recorded on a link this koan makes itself.
///
/// May be network-bound. Callers keep it off whatever thread draws.
pub fn create_share(
    db: &Database,
    user: i64,
    cfg: &Config,
    target: &ShareTarget,
    description: Option<&str>,
) -> Result<ShareOutcome, ShareError> {
    let Some(client) = subsonic_client(cfg) else {
        return create_native_share(db, user, cfg, target, description);
    };
    // A remote server makes its own kind of link from what it is given, so it
    // is given exactly what was picked.
    let resolved;
    let track_ids = match target {
        ShareTarget::Tracks(ids) => ids.as_slice(),
        _ => {
            resolved = resolve_share(&db.conn, target)?.1;
            resolved.as_slice()
        }
    };

    // One query, not one per track: sharing an artist is thousands of tracks.
    let rows = queries::tracks_by_ids(&db.conn, track_ids)?;

    let shared = rows.iter().filter(|t| t.remote_id.is_some()).count();
    if shared == 0 {
        return Err(ShareError::NothingRemote);
    }

    // A whole record shares as one album rather than as N tracks — the server
    // renders it as the album it is, and the link survives the user adding to
    // it. Only when the selection is genuinely the whole thing.
    let one_album = rows
        .first()
        .and_then(|f| f.album_id)
        .filter(|first| rows.iter().all(|t| t.album_id == Some(*first)))
        .and_then(|album_id| album_remote_id(&db.conn, album_id, rows.len()));

    let remote_ids: Vec<String> = match one_album {
        Some(rid) => vec![album_share_id(&client, rid)],
        None => rows.into_iter().filter_map(|t| t.remote_id).collect(),
    };

    let refs: Vec<&str> = remote_ids.iter().map(String::as_str).collect();
    let share = client.create_share(&refs, description)?;

    // Navidrome does not always hand back a URL, and a share with no link is
    // useless to the caller — the ID is enough to build it.
    let url = share
        .url
        .clone()
        .unwrap_or_else(|| format!("{}/s/{}", client.base_url(), share.id));

    Ok(ShareOutcome {
        url,
        id: share.id,
        shared,
        skipped: track_ids.len().saturating_sub(shared),
    })
}

/// A share this koan serves at `{sharing.public_url}/share/{id}`.
fn create_native_share(
    db: &Database,
    user: i64,
    cfg: &Config,
    target: &ShareTarget,
    description: Option<&str>,
) -> Result<ShareOutcome, ShareError> {
    let base = cfg
        .sharing
        .public_url
        .as_deref()
        .filter(|u| !u.trim().is_empty())
        .ok_or(ShareError::NoPublicUrl)?;
    let (slice, ids) = resolve_share(&db.conn, target)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let share = queries::shares::create_share(&db.conn, user, slice, &ids, description, now, None)?;
    Ok(ShareOutcome {
        url: share_url(base, &share.id),
        id: share.id,
        shared: ids.len(),
        // Only loose tracks are named one by one, so only they can be missing.
        skipped: match (target, slice.kind) {
            (ShareTarget::Tracks(asked), ShareKind::Tracks) => asked.len() - ids.len(),
            _ => 0,
        },
    })
}

/// A native share's public address.
pub fn share_url(public_url: &str, id: &str) -> String {
    format!("{}/share/{id}", public_url.trim_end_matches('/'))
}

/// The album's own remote ID, but only when `selected` covers every track on
/// it. Sharing an album link for half an album would hand out more than the
/// user picked.
/// An album's id as `createShare` should be given it.
///
/// koan numbers albums and songs separately, publishes album ids bare, and
/// reads a bare id in `createShare` as a song, so album 5 would share song 5.
/// Its `al-` prefix says which is meant. Other servers get the id as they
/// issued it: some also number albums, and would not know the prefix.
fn album_share_id(client: &crate::remote::client::SubsonicClient, remote_id: String) -> String {
    let koan = matches!(client.server_type(), Ok(Some(t)) if t == "koan");
    album_share_id_for(koan, remote_id)
}

fn album_share_id_for(koan: bool, remote_id: String) -> String {
    if koan && remote_id.parse::<i64>().is_ok() {
        format!("al-{remote_id}")
    } else {
        remote_id
    }
}

fn album_remote_id(conn: &rusqlite::Connection, album_id: i64, selected: usize) -> Option<String> {
    let (remote_id, total): (Option<String>, i64) = conn
        .query_row(
            "SELECT al.remote_id, (SELECT COUNT(*) FROM tracks WHERE album_id = al.id)
             FROM albums al WHERE al.id = ?1",
            [album_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    (total == selected as i64).then_some(remote_id).flatten()
}

// ---------------------------------------------------------------------------
// Path utilities
// ---------------------------------------------------------------------------

/// Fisher-Yates over a fresh seed, so consecutive calls differ.
///
/// Deliberately not seeded from anything stable: "shuffle again" has to
/// actually produce a new order, which a process-lifetime seed wouldn't.
pub fn shuffle<T>(items: &mut [T]) {
    let mut seed = [0u8; 8];
    if getrandom::fill(&mut seed).is_err() {
        return; // Leave the order alone rather than pretending to shuffle.
    }
    let mut state = u64::from_le_bytes(seed) | 1;
    for i in (1..items.len()).rev() {
        // xorshift64 — plenty for shuffling a list nobody is betting on.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        items.swap(i, (state % (i as u64 + 1)) as usize);
    }
}

/// Truncate a string to at most `max` bytes, cutting on a char boundary.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Sanitise and truncate a string for use as a path component.
/// Strips illegal chars and caps at 240 bytes (macOS 255-byte filename limit minus room for ext).
pub fn sanitise_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string();

    truncate_bytes(&cleaned, 240).trim_end().to_string()
}

/// The year a tag date starts with. `get`, not a slice: a date is free text,
/// and a multibyte character in its first four bytes would panic a slice.
pub fn year_of(date: &str) -> Option<&str> {
    date.get(..4)
}

/// Build a structured cache path for a track:
///   cache_dir/Album Artist/(Year) Album [Codec]/01. Track Artist - Title.ext
pub fn cache_path_for_track(
    cache_dir: &Path,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> PathBuf {
    let artist_dir = sanitise_filename(&track.artist_name);

    let year = album_date
        .and_then(year_of)
        .map(|y| format!("({}) ", y))
        .unwrap_or_default();
    let codec = track
        .codec
        .as_deref()
        .map(|c| format!(" [{}]", c))
        .unwrap_or_default();
    let album_dir = sanitise_filename(&format!("{}{}{}", year, track.album_title, codec));

    let disc_prefix = match track.disc {
        Some(d) if d > 1 => format!("{}-", d),
        _ => String::new(),
    };
    let track_num = track
        .track_number
        .map(|n| format!("{:02}. ", n))
        .unwrap_or_default();

    let ext = track
        .codec
        .as_deref()
        .map(|c| c.to_lowercase())
        .unwrap_or_else(|| "flac".into());

    let filename = sanitise_filename(&format!(
        "{}{}{} - {}",
        disc_prefix, track_num, track.artist_name, track.title
    ));

    cache_dir
        .join(artist_dir)
        .join(album_dir)
        .join(format!("{}.{}", filename, ext))
}

// ---------------------------------------------------------------------------
// Track resolution
// ---------------------------------------------------------------------------

/// Resolve a track to its path + load state (without downloading).
/// Returns (path, `ItemState::Ready`) for local/cached, (cache path, `ItemState::Pending`)
/// for remote — a track with no copy here yet has to be fetched before it plays.
pub fn resolve_item_path(
    db: &Database,
    cfg: &Config,
    id: i64,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> (PathBuf, ItemState) {
    match queries::resolve_playback_path(&db.conn, id) {
        Ok(Some(queries::PlaybackSource::Local(p))) => (p, ItemState::Ready),
        // A cache entry is only as good as its contents. Older builds could
        // store a Subsonic error body here, which reports Ready and then fails
        // to decode forever; treating it as Pending sends it back through the
        // download path, which discards it and re-fetches.
        Ok(Some(queries::PlaybackSource::Cached(p))) => {
            let state = if is_cached_audio(&p) {
                ItemState::Ready
            } else {
                ItemState::Pending
            };
            (p, state)
        }
        Ok(Some(queries::PlaybackSource::Remote(_))) => {
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            if dest.exists() && is_cached_audio(&dest) {
                (dest, ItemState::Ready)
            } else {
                (dest, ItemState::Pending)
            }
        }
        _ => {
            // Fallback: construct a cache path and mark pending.
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            (dest, ItemState::Pending)
        }
    }
}

/// Build a PlaylistItem from a TrackRow + album date + resolved path + load state.
pub fn playlist_item_from_track(
    track: &queries::TrackRow,
    album_date: Option<&str>,
    dest: PathBuf,
    state: ItemState,
) -> PlaylistItem {
    let year = album_date.and_then(year_of).map(str::to_string);
    PlaylistItem {
        playlist_entry_id: None,
        id: QueueItemId::new(),
        db_id: Some(track.id),
        path: dest,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        state,
    }
}

/// Build playlist items for many tracks at once.
///
/// `track_to_playlist_item` loads the config on every call, which means
/// reading and parsing `config.toml` and `config.local.toml` once per track —
/// the reason a large add crawled. This loads it once and memoises album dates,
/// so a thousand-track add costs one config read instead of a thousand.
pub fn playlist_items_for_tracks(db: &Database, tracks: &[queries::TrackRow]) -> Vec<PlaylistItem> {
    use std::collections::HashMap;

    let cfg = Config::load().unwrap_or_default();
    let mut album_dates: HashMap<i64, Option<String>> = HashMap::new();

    tracks
        .iter()
        .map(|track| {
            let album_date = match track.album_id {
                Some(aid) => album_dates
                    .entry(aid)
                    .or_insert_with(|| queries::album_date(&db.conn, aid).ok().flatten())
                    .clone(),
                None => None,
            };
            let (path, state) = resolve_item_path(db, &cfg, track.id, track, album_date.as_deref());
            playlist_item_from_track(track, album_date.as_deref(), path, state)
        })
        .collect()
}

/// Build a PlaylistItem from a TrackRow, resolving its path automatically.
pub fn track_to_playlist_item(track: &queries::TrackRow, db: &Database) -> PlaylistItem {
    let album_date = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let cfg = Config::load().unwrap_or_default();
    let (path, state) = resolve_item_path(db, &cfg, track.id, track, album_date.as_deref());

    let year = album_date.as_deref().and_then(year_of).map(str::to_string);

    PlaylistItem {
        playlist_entry_id: None,
        id: QueueItemId::new(),
        db_id: Some(track.id),
        path,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        state,
    }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Whether a cached file plausibly holds audio.
///
/// A stored Subsonic error is a few hundred bytes of JSON or XML; no real
/// encoded track comes close to that, so the size check alone settles almost
/// every case and the leading byte covers the rest.
fn is_cached_audio(path: &std::path::Path) -> bool {
    const MIN_PLAUSIBLE_BYTES: u64 = 4096;
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() >= MIN_PLAUSIBLE_BYTES => true,
        Ok(_) => {
            let mut first = [0u8; 1];
            match std::fs::File::open(path)
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut first).map(|_| first[0]))
            {
                Ok(b) => b != b'{' && b != b'<',
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

/// Resolve a track to a playable file, downloading from remote if needed.
///
/// Resolution order:
/// 1. Local library path (DB `path` field) -- use directly if file exists
/// 2. Cache path -- use if already downloaded
/// 3. Download from remote to cache -- stream while downloading
pub fn download_track(
    db_id: i64,
    queue_id: QueueItemId,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
    log_buf: &Arc<Mutex<Vec<String>>>,
    state: &Arc<SharedPlayerState>,
    cfg: &Config,
    client: &SubsonicClient,
) {
    // From the pool. This runs once per track fetched, and opening a
    // connection here re-ran the schema DDL and attempted a WAL checkpoint —
    // with several transfers going, several init cycles contending with each
    // other and with whatever the library was trying to read.
    let db = match crate::db::pool::shared().get() {
        Ok(db) => db,
        Err(e) => {
            fail_track(state, tx, queue_id, format!("db error: {}", e));
            return;
        }
    };
    let track = match queries::get_track_row(&db.conn, db_id) {
        Ok(Some(t)) => t,
        _ => {
            fail_track(state, tx, queue_id, "track not found".into());
            return;
        }
    };

    let remote_id = match &track.remote_id {
        Some(rid) => rid.clone(),
        None => {
            // No remote_id -- check if the local file exists.
            if let Some(ref path) = track.path {
                let p = std::path::PathBuf::from(path);
                if p.exists() {
                    state.update_paths(&[(queue_id, p)]);
                    state.update_item_state(queue_id, ItemState::Ready);
                    if state.is_cursor(queue_id) {
                        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
                    }
                    return;
                }
            }
            fail_track(
                state,
                tx,
                queue_id,
                "not in the library folder, and no remote copy to fetch".into(),
            );
            return;
        }
    };

    // 1. Check if the local library file exists.
    if let Some(ref local_path) = track.path {
        let p = std::path::PathBuf::from(local_path);
        if p.exists() {
            log::info!("download_track: local file exists, using {}", p.display());
            state.update_paths(&[(queue_id, p)]);
            state.update_item_state(queue_id, ItemState::Ready);
            if state.is_cursor(queue_id) {
                tx.send(PlayerCommand::TrackReady(queue_id)).ok();
            }
            return;
        }
    }

    let album_date: Option<String> = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let dest = cache_path_for_track(&cfg.cache_dir(), &track, album_date.as_deref());

    // 2. Already cached.
    //
    // Older builds could write a Subsonic error body here as if it were audio,
    // leaving a tiny JSON file that reports Ready and then fails to decode
    // forever. Treat those as absent so they get re-fetched.
    if dest.exists() && !is_cached_audio(&dest) {
        log::warn!(
            "discarding non-audio cache entry {} (likely a stored server error)",
            dest.display()
        );
        let _ = std::fs::remove_file(&dest);
    }
    if dest.exists() {
        state.update_paths(&[(queue_id, dest)]);
        state.update_item_state(queue_id, ItemState::Ready);
        if state.is_cursor(queue_id) {
            tx.send(PlayerCommand::TrackReady(queue_id)).ok();
        }
        return;
    }

    // 3. Download from remote. The queue item points at the in-progress file so
    // the decoder reads bytes as they land; it flips to `dest` on success.
    state.update_paths(&[(queue_id, crate::remote::download::part_path(&dest))]);

    let bytes_written = crate::remote::downloads::ByteFeed::new();

    // Announce it before a byte moves, so a queue of six shows six rows rather
    // than one row and five tracks that look like nothing is happening to them.
    let store = crate::remote::downloads::store();
    store.queued(crate::remote::downloads::Download {
        id: queue_id,
        track_id: db_id,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        source: crate::remote::download::part_path(&dest),
        dest: dest.clone(),
        total: 0,
        written: bytes_written.clone(),
        state: crate::remote::downloads::DownloadState::Queued,
        bytes_per_second: 0,
    });

    let progress_qid = queue_id;
    let bytes_written_progress = bytes_written.clone();
    let progress_tx = tx.clone();
    let stream_ready_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stream_ready_flag = stream_ready_sent.clone();
    // A retry restarts the byte count from zero, so a changed total re-announces.
    let announced_total = AtomicU64::new(u64::MAX);
    // Taken out of the queue, cleared or removed, while waiting out an outage.
    let gone = || state.get_item(queue_id).is_none();
    let result =
        client.download_with_progress(&remote_id, &dest, &gone, move |downloaded, total| {
            bytes_written_progress.set(downloaded);
            // What knows a transfer moved is the code moving it. Held to a reading
            // every 250ms inside, so a chunk landing costs an atomic and a compare.
            store.progressed();
            if announced_total.swap(total, Ordering::Relaxed) != total {
                // The store, and only the store. The item's state says whether its
                // file can be played, which a transfer in flight has not changed.
                store.started(progress_qid, total, bytes_written_progress.clone());
            }
            if !stream_ready_flag.load(Ordering::Relaxed)
                && downloaded >= crate::player::state::STREAM_THRESHOLD
            {
                stream_ready_flag.store(true, Ordering::Relaxed);
                progress_tx
                    .send(PlayerCommand::TrackStreamReady(progress_qid))
                    .ok();
            }
        });

    if let Err(SubsonicError::Download(DownloadError::Cancelled)) = result {
        store.withdrawn(queue_id);
        bytes_written.done();
        return;
    }

    // However it ended, a decoder reading the `.part` file may be parked at the
    // write head. It waits on the feed, so the feed has to wake it — and only
    // once the item says how it ended, or it looks, sees a download, and parks
    // again with nothing left to wake it.
    if let Err(e) = result {
        store.failed(queue_id, e.to_string());
        fail_track(state, tx, queue_id, e.to_string());
        bytes_written.done();
        push_log(log_buf, format!("x {} — {}", track.title, e));
        return;
    }
    store.finished(queue_id);

    state.update_paths(&[(queue_id, dest.clone())]);
    state.update_item_state(queue_id, ItemState::Ready);
    bytes_written.done();
    // Without this row the file is invisible to cache eviction and never reclaimed.
    if let Err(e) = queries::set_cached_path(&db.conn, db_id, &dest.to_string_lossy()) {
        log::warn!(
            "cached {} but failed to record it ({}) — it will not be evicted",
            dest.display(),
            e
        );
    }

    push_log(
        log_buf,
        format!("+ {} — {}", track.title, track.artist_name),
    );

    if state.is_cursor(queue_id) {
        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
    }
}

/// Mark a queue item unplayable and tell the player, if it is waiting on it.
///
/// Setting `ItemState::Failed` alone is not enough: the player only wakes for
/// `TrackReady`, so a cursor parked on the item would wait for a download that
/// has already given up.
pub(crate) fn fail_track(
    state: &Arc<SharedPlayerState>,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
    queue_id: QueueItemId,
    reason: String,
) {
    state.update_item_state(queue_id, ItemState::Failed(reason));
    if state.is_cursor(queue_id) {
        tx.send(PlayerCommand::TrackFailed(queue_id)).ok();
    }
}

/// Append to the TUI log pane, tolerating a poisoned lock — a download worker
/// must not die because some other thread panicked while holding it.
fn push_log(log_buf: &Arc<Mutex<Vec<String>>>, msg: String) {
    match log_buf.lock() {
        Ok(mut buf) => buf.push(msg),
        Err(_) => log::info!("{}", msg),
    }
}

/// Why there is no remote client, in words worth showing someone.
///
/// Every caller of `subsonic_client` gets `None` for three different reasons and
/// used to report the same one — so "koan has no password", which sends you to
/// sign in, arrived looking like a server that was merely down.
pub fn remote_unavailable(cfg: &Config) -> String {
    if !cfg.remote.enabled {
        return "no remote server is configured".into();
    }
    if cfg.remote.url.is_empty() {
        return "the remote server has no address".into();
    }
    if get_remote_password(cfg).is_none() {
        return "no password is stored for the remote server".into();
    }
    // A password resolved, so the client should have built. Nothing else
    // returns `None`, but saying so beats claiming a cause that is wrong.
    "the remote server could not be reached".into()
}

/// Spawn background downloads for remote tracks with ItemState::Pending.
/// Submit tracks for download.
///
/// Everything that is not the TUI reaches downloads through here — the FFI, the
/// GraphQL server and radio's auto-extend. It used to spawn a thread per batch
/// and walk it with a `for` loop, which meant one track at a time no matter
/// what `download_workers` said, and no reordering when the cursor moved. It
/// hands the batch to the shared queue now, which is the same pool, priority
/// lane and cursor watcher the TUI has always used.
pub fn spawn_downloads(
    pending: Vec<(i64, QueueItemId)>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    state: Arc<SharedPlayerState>,
) {
    if pending.is_empty() {
        return;
    }
    crate::remote::queue::shared(&tx, &state, None).enqueue(pending);
}

#[cfg(test)]
mod year_tests {
    use super::year_of;

    #[test]
    fn a_year_is_the_first_four_characters_when_they_are_bytes_too() {
        assert_eq!(year_of("1997-05-21"), Some("1997"));
        assert_eq!(year_of("199"), None);
        // Full-width digits: four bytes in is mid-character.
        assert_eq!(year_of("１９９７"), None);
    }
}

#[cfg(test)]
mod rebuild_tests {
    use super::*;
    use crate::db::queries::sample_meta;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn clearing_one_download_leaves_the_others_and_the_library_alone() {
        let dir = tempfile::tempdir().unwrap();
        let db = test_db();

        let mut cached = Vec::new();
        for name in ["one", "two"] {
            let mut meta = sample_meta(name, "Artist", "Album");
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(name.into());
            let id = queries::upsert_track(&db.conn, &meta).unwrap();
            let file = dir.path().join(format!("{name}.opus"));
            std::fs::write(&file, vec![0u8; 2048]).unwrap();
            queries::set_cached_path(&db.conn, id, &file.to_string_lossy()).unwrap();
            cached.push((id, file));
        }

        let cleared = clear_downloads_for(&db, &[cached[0].0]);
        assert_eq!(cleared.files, 1);
        assert_eq!(cleared.bytes, 2048);
        assert!(!cached[0].1.exists(), "the copy asked for is gone");
        assert!(cached[1].1.exists(), "the other one is untouched");

        // The row survives — a remote track is still in the library, it just
        // has to be fetched again.
        assert_eq!(queries::library_stats(&db.conn).unwrap().remote_tracks, 2);
        assert_eq!(queries::library_stats(&db.conn).unwrap().cached_tracks, 1);
        assert!(
            queries::cached_paths_for(&db.conn, &[cached[0].0])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn clearing_a_download_that_is_already_gone_is_not_a_failure() {
        let db = test_db();
        let mut meta = sample_meta("ghost", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("ghost".into());
        let id = queries::upsert_track(&db.conn, &meta).unwrap();
        queries::set_cached_path(&db.conn, id, "/nowhere/at/all.opus").unwrap();

        let cleared = clear_downloads_for(&db, &[id]);
        assert_eq!(cleared.files, 0, "nothing was there to remove");
        // Forgotten regardless: the row claimed a copy that does not exist.
        assert!(
            queries::cached_paths_for(&db.conn, &[id])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn sweeping_removes_half_finished_downloads_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(cache.join("Artist")).unwrap();

        let finished = cache.join("Artist/whole.opus");
        let half = cache.join("Artist/half.opus.part");
        std::fs::write(&finished, vec![0u8; 1024]).unwrap();
        std::fs::write(&half, vec![0u8; 4096]).unwrap();

        let cfg = Config {
            remote: crate::config::RemoteConfig {
                cache_dir: Some(cache.clone()),
                ..Default::default()
            },
            ..Default::default()
        };

        let swept = sweep_partial_downloads(&cfg);
        assert_eq!(swept.files, 1);
        assert_eq!(swept.bytes, 4096);
        assert!(!half.exists(), "the unfinished one is gone");
        assert!(finished.exists(), "a downloaded track is not touched");
    }

    #[test]
    fn sweeping_an_empty_cache_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            remote: crate::config::RemoteConfig {
                cache_dir: Some(dir.path().join("nothing-here")),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(sweep_partial_downloads(&cfg).files, 0);
    }

    #[test]
    fn clearing_no_tracks_does_nothing() {
        let db = test_db();
        assert_eq!(clear_downloads_for(&db, &[]).files, 0);
    }

    #[test]
    fn rebuild_drops_the_index_and_keeps_favourites() {
        let db = test_db();
        let mut meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        meta.path = Some("/music/windowlicker.flac".into());
        let track_id = queries::upsert_track(&db.conn, &meta).unwrap();

        // Favourites key on the path; lyrics key on the row id.
        queries::toggle_favourite(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            Path::new("/music/windowlicker.flac"),
        )
        .unwrap();
        db.conn
            .execute(
                "INSERT INTO lyrics_cache (track_id, source, content, fetched_at)
                 VALUES (?1, 'test', 'la la la', 0)",
                [track_id],
            )
            .unwrap();

        let summary = rebuild_index(&db).unwrap();
        assert_eq!(summary.tracks, 1);
        assert_eq!(summary.albums, 1);

        let tracks: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tracks, 0, "the index is gone");

        let favourites: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM favourites", [], |r| r.get(0))
            .unwrap();
        assert_eq!(favourites, 1, "favourites survive — they key on the path");

        let lyrics: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM lyrics_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(lyrics, 0, "anything keyed on a track id cannot survive");
    }

    #[test]
    fn rebuilding_an_empty_library_is_not_an_error() {
        let db = test_db();
        let summary = rebuild_index(&db).unwrap();
        assert_eq!(summary.tracks, 0);
    }
}

#[cfg(test)]
mod share_tests {
    use super::*;
    use crate::db::queries::sample_meta;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    /// Three tracks on one album; the album carries a remote ID.
    fn album_of_three(db: &Database) -> (i64, Vec<i64>) {
        let ids: Vec<i64> = ["One", "Two", "Three"]
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let mut meta = sample_meta(title, "Boards of Canada", "Geogaddi");
                meta.path = Some(format!("/music/geogaddi/{i}.flac"));
                meta.track_number = Some(i as i32 + 1);
                queries::upsert_track(&db.conn, &meta).unwrap()
            })
            .collect();
        let album_id: i64 = db
            .conn
            .query_row("SELECT album_id FROM tracks WHERE id = ?1", [ids[0]], |r| {
                r.get(0)
            })
            .unwrap();
        db.conn
            .execute(
                "UPDATE albums SET remote_id = 'al-1' WHERE id = ?1",
                [album_id],
            )
            .unwrap();
        (album_id, ids)
    }

    #[test]
    fn whole_album_collapses_to_the_album_link() {
        let db = test_db();
        let (album_id, ids) = album_of_three(&db);
        assert_eq!(
            album_remote_id(&db.conn, album_id, ids.len()),
            Some("al-1".into())
        );
    }

    #[test]
    fn part_of_an_album_does_not() {
        let db = test_db();
        let (album_id, _) = album_of_three(&db);
        // Sharing an album link for two of three tracks would hand out a track
        // the user did not pick.
        assert_eq!(album_remote_id(&db.conn, album_id, 2), None);
    }

    #[test]
    fn a_local_only_album_has_no_link_to_collapse_to() {
        let db = test_db();
        let (album_id, ids) = album_of_three(&db);
        db.conn
            .execute(
                "UPDATE albums SET remote_id = NULL WHERE id = ?1",
                [album_id],
            )
            .unwrap();
        assert_eq!(album_remote_id(&db.conn, album_id, ids.len()), None);
    }
}

#[cfg(test)]
mod client_cache_tests {
    use super::*;

    #[test]
    fn one_subsonic_client_is_shared_per_credentials() {
        crate::config::isolate_config_for_tests();
        let mut cfg = Config::default();
        cfg.remote.enabled = true;
        cfg.remote.url = "https://shared-client.invalid".into();
        cfg.remote.username = "koan".into();
        cfg.remote.password = "first".into();

        let first = subsonic_client(&cfg).expect("a configured remote yields a client");
        let again = subsonic_client(&cfg).expect("a configured remote yields a client");
        assert!(
            Arc::ptr_eq(&first, &again),
            "rebuilding drops the connection pool and re-handshakes TLS per request"
        );

        cfg.remote.password = "second".into();
        let relogged = subsonic_client(&cfg).expect("a configured remote yields a client");
        assert!(
            !Arc::ptr_eq(&first, &relogged),
            "new credentials must not keep serving the client signed with the old ones"
        );
    }
}

#[cfg(test)]
mod native_share_tests {
    use super::*;
    use crate::db::queries::{sample_meta, upsert_track};

    #[test]
    fn a_standalone_server_shares_natively_in_the_order_asked() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let db = Database { conn };
        let a = upsert_track(&db.conn, &sample_meta("A", "X", "Y")).unwrap();
        let b = upsert_track(&db.conn, &sample_meta("B", "X", "Y")).unwrap();
        let mut cfg = Config::default();
        assert!(matches!(
            create_share(
                &db,
                queries::LOCAL_USER,
                &cfg,
                &ShareTarget::Tracks(vec![a]),
                None
            ),
            Err(ShareError::NoPublicUrl)
        ));
        cfg.sharing.public_url = Some("https://koan.example/".into());
        let out = create_share(
            &db,
            queries::LOCAL_USER,
            &cfg,
            &ShareTarget::Tracks(vec![b, 9999, a]),
            Some("mix"),
        )
        .unwrap();
        assert_eq!(out.url, format!("https://koan.example/share/{}", out.id));
        assert_eq!((out.shared, out.skipped), (2, 1));
        let share = queries::shares::get_share(&db.conn, &out.id)
            .unwrap()
            .unwrap();
        assert_eq!(share.track_ids, [b, a]);
        assert!(matches!(
            create_share(
                &db,
                queries::LOCAL_USER,
                &cfg,
                &ShareTarget::Tracks(vec![9999]),
                None
            ),
            Err(ShareError::NothingToShare)
        ));
    }

    fn album_track(db: &Database, title: &str, album: &str, n: i32, date: &str) -> i64 {
        let mut meta = sample_meta(title, "Rrose", album);
        meta.track_number = Some(n);
        meta.date = Some(date.into());
        upsert_track(&db.conn, &meta).unwrap()
    }

    #[test]
    fn shares_are_slices_fixed_when_made() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let db = Database { conn };
        let later = album_track(&db, "L1", "Later", 1, "2021");
        let a1 = album_track(&db, "E1", "Earlier", 1, "2015");
        let a2 = album_track(&db, "E2", "Earlier", 2, "2015");
        let album_of = |t| {
            queries::tracks_by_ids(&db.conn, &[t]).unwrap()[0]
                .album_id
                .unwrap()
        };
        let (earlier, later_album) = (album_of(a1), album_of(later));
        let artist = queries::tracks_by_ids(&db.conn, &[a1]).unwrap()[0]
            .artist_id
            .unwrap();

        // One track: its album, cued to it.
        let (slice, ids) = resolve_share(&db.conn, &ShareTarget::Tracks(vec![a2])).unwrap();
        assert_eq!(
            (slice.kind, slice.subject_id, slice.start_track_id),
            (ShareKind::Album, Some(earlier), Some(a2))
        );
        assert_eq!(ids, [a1, a2]);

        // An album, with a cue that is not on it dropped.
        let (slice, ids) = resolve_share(
            &db.conn,
            &ShareTarget::Album {
                album_id: later_album,
                start_track_id: Some(a1),
            },
        )
        .unwrap();
        assert_eq!((slice.kind, slice.start_track_id), (ShareKind::Album, None));
        assert_eq!(ids, [later]);

        // An artist: every album, in release order.
        let (slice, ids) = resolve_share(&db.conn, &ShareTarget::Artist(artist)).unwrap();
        assert_eq!(
            (slice.kind, slice.subject_id),
            (ShareKind::Artist, Some(artist))
        );
        assert_eq!(ids, [a1, a2, later]);

        // Several tracks stay a list, in the order given.
        let (slice, ids) = resolve_share(&db.conn, &ShareTarget::Tracks(vec![later, a1])).unwrap();
        assert_eq!(slice, Slice::TRACKS);
        assert_eq!(ids, [later, a1]);

        assert!(matches!(
            resolve_share(&db.conn, &ShareTarget::Artist(9999)),
            Err(ShareError::NothingToShare)
        ));
    }
}

#[cfg(test)]
mod album_share_id_tests {
    use super::album_share_id_for;

    #[test]
    fn a_koan_album_is_named_as_an_album() {
        assert_eq!(album_share_id_for(true, "46215".into()), "al-46215");
        // Already prefixed, or not koan's numbering: left as issued.
        assert_eq!(album_share_id_for(true, "al-7".into()), "al-7");
        assert_eq!(album_share_id_for(false, "46215".into()), "46215");
        assert_eq!(album_share_id_for(false, "3xJ9kQ2pZ".into()), "3xJ9kQ2pZ");
    }
}
