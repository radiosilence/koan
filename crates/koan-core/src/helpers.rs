//! Helpers shared by every front end: koan-tui, koan-server, koan-ffi and koan-cli.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::db::queries::shares::{ShareKind, Slice};
use crate::player::commands::PlayerCommand;
use crate::player::state::{ItemState, PlaylistItem, QueueItemId, SharedPlayerState};
use crate::remote::client::{Credential, SubsonicAuth, SubsonicClient, SubsonicError};
use crate::remote::download::DownloadError;

// ---------------------------------------------------------------------------
// Subsonic client builder
// ---------------------------------------------------------------------------

/// What signs in to the remote server, from `config.local.toml` or a
/// `KOAN_REMOTE__*` variable layered over it: the API key when there is one,
/// else the password.
pub fn remote_credential(cfg: &Config) -> Option<Credential> {
    if !cfg.remote.api_key.is_empty() {
        return Some(Credential::ApiKey(cfg.remote.api_key.clone()));
    }
    (!cfg.remote.password.is_empty()).then(|| Credential::Password(cfg.remote.password.clone()))
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
                let Ok(db) = Database::open_existing(&db_path) else {
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
                // Changes heard meanwhile wait in the channel; a rescan that
                // fell due runs as soon as this returns.
                crate::quiet::wait_until_awake();
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
/// One sync shortly after startup, then every `auto_sync_interval_mins` for a
/// server that cannot say when it changes. A koan server can: it sends every
/// linked app a sync when its library or a playlist moves, and queues one for
/// an app that was away, so a timer would only ask a question already
/// answered. Each run walks the library only if the server says it moved —
/// see `Walk::IfChanged` — which is what makes it cheap enough to run
/// unattended.
///
/// The startup run is delayed a few seconds so it is not competing with the
/// first frame and the first track for the disk.
///
/// `on_state` reports whether a sync is running, so a UI can say so rather than
/// appearing to do nothing, and `on_progress` how far it has got.
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
                // Due while the app was in the background: runs when it is not.
                crate::quiet::wait_until_awake();
                let cfg = Config::load().unwrap_or_default();
                if !cfg.remote.enabled || !cfg.remote.auto_sync {
                    // Re-read rather than exit: the setting can be turned on
                    // while the app is running.
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    continue;
                }

                if let Some(client) = subsonic_client(&cfg)
                    && let Ok(db) = Database::open_existing(&db_path)
                {
                    on_state(true);
                    match sync_remote(
                        &db,
                        &client,
                        Walk::IfChanged,
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

                // Once at startup and no more, or on the interval. A koan
                // server's own syncs make the interval redundant; it is still
                // slept, since the server signed in to can change.
                let mins = cfg.remote.auto_sync_interval_mins;
                if mins == 0 {
                    return;
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(mins * 60));
                    if !crate::remote::profile::current().is_some_and(|p| p.links()) {
                        break;
                    }
                }
            }
        })
        .ok()
}

/// What a library rebuild re-reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct RebuildSummary {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
}

/// Read the whole library again from its sources.
///
/// What each file and server entry said is forgotten, so the next scan reads
/// every file and the next sync walks the whole server, and every track,
/// album and artist takes what its sources say now. The rows themselves stay,
/// and each source takes its own back by path or server id, so play history,
/// playlists, favourites and lyrics are kept. A row nothing claims again goes:
/// a file's when the scan of its folder finds it missing, a server entry's
/// when a complete sync does not list it.
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

    db.conn.execute_batch(
        "BEGIN;
         DELETE FROM local_files;
         DELETE FROM remote_entries;
         DELETE FROM scan_cache;
         UPDATE remote_servers SET library_version = NULL;
         COMMIT;",
    )?;
    Ok(summary)
}

/// Remove downloads until the cache is under the configured limit, a file at
/// a time: those fetched to play first, then those asked for, least recently
/// used first within each. Never a file whose album has a favourite in it,
/// nor a track in `keep` — the playback window, whose files the player may be
/// reading and which are what it wants next (`playback_window`). Returns the
/// bytes freed.
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
    let files = match queries::cached_files_lru(&db.conn) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("cache eviction: failed to query cached files: {e}");
            return 0;
        }
    };
    let mut gone = Vec::new();
    let mut freed: i64 = 0;
    for file in files.iter().filter(|f| !keep.contains(&f.track_id)) {
        if current <= limit {
            break;
        }
        match std::fs::remove_file(&file.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                log::warn!("cache eviction: failed to delete {}: {e}", file.path);
                continue;
            }
        }
        log::info!(
            "evicted: {} ({} bytes{})",
            file.path,
            file.size,
            if file.pinned { ", pinned" } else { "" }
        );
        gone.push(file.track_id);
        current -= file.size;
        freed += file.size;
    }
    if let Err(e) = queries::clear_cached_paths_for(&db.conn, &gone) {
        log::warn!("cache eviction: failed to clear DB: {e}");
    }
    remove_empty_dirs(&cfg.cache_dir());
    if freed > 0 {
        log::info!("cache eviction freed {freed} bytes");
    }
    freed as u64
}

/// How many of `upcoming` — the playing track, then the rest of the queue in
/// the order the player reaches it — the cache has room for. The playing
/// track and the next always: the decoder reads ahead into the next for
/// gapless. After them each track while the cache stays under `limit`,
/// counting favourites and pinned downloads first, since the queue does not
/// displace them. What is already downloaded costs its file, and what is not
/// its estimated size.
pub fn playback_window(
    db: &Database,
    limit: u64,
    upcoming: &[i64],
) -> Result<usize, crate::db::connection::DbError> {
    let total = queries::total_cache_size(&db.conn)?;
    let evictable: std::collections::HashMap<i64, i64> = queries::cached_files_lru(&db.conn)?
        .into_iter()
        .filter(|f| !f.pinned)
        .map(|f| (f.track_id, f.size))
        .collect();
    let estimates = queries::download_estimates(&db.conn, upcoming)?;

    let mut used = (total - evictable.values().sum::<i64>()).max(0) as u64;
    let mut counted = std::collections::HashSet::new();
    for (n, id) in upcoming.iter().enumerate() {
        if counted.insert(*id) {
            let cost = evictable.get(id).or_else(|| estimates.get(id));
            used += cost.copied().unwrap_or(0).max(0) as u64;
        }
        if n >= 2 && used > limit {
            return Ok(n);
        }
    }
    Ok(upcoming.len())
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

    let tx = crate::db::queries::write_transaction(&db.conn)?;
    let paths: Vec<String> = {
        let mut stmt = tx.prepare("SELECT path FROM local_files WHERE path >= ?1 AND path < ?2")?;
        let rows = stmt.query_map([&lower, &upper], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // A track also on the server keeps its row, minus the file.
    for path in &paths {
        queries::sources::remove(&tx, queries::sources::Kind::Local, path)?;
    }
    // Otherwise the folder added back would find its files cached as read and
    // skip them, leaving their tracks without a file.
    tx.execute(
        "DELETE FROM scan_cache WHERE path >= ?1 AND path < ?2",
        [&lower, &upper],
    )?;
    tx.commit()?;
    Ok(paths.len() as u64)
}

/// Forget everything that only existed on the server.
///
/// Signing out should leave the library with what is actually on this machine.
/// A track held both locally and remotely keeps its row and loses the server's
/// copy; one that only ever came from the server goes.
pub fn forget_remote(db: &Database) -> Result<u64, crate::db::connection::DbError> {
    let tx = crate::db::queries::write_transaction(&db.conn)?;
    let ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT remote_id FROM remote_entries")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut removed = 0;
    for id in &ids {
        let track: i64 = tx.query_row(
            "SELECT track_id FROM remote_entries WHERE remote_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        queries::sources::remove(&tx, queries::sources::Kind::Remote, id)?;
        let kept: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM tracks WHERE id = ?1)",
            [track],
            |r| r.get(0),
        )?;
        removed += u64::from(!kept);
    }
    // What waited for this server, and how far its history was read.
    tx.execute_batch(
        "DELETE FROM history_outbox;
         UPDATE remote_servers SET history_cursor = NULL;",
    )?;
    tx.commit()?;
    Ok(removed)
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

/// Re-root cached paths that name a cache directory other than `cache_dir`.
///
/// iOS gives an app a new container path when it is updated. The cache moves
/// with it, but the absolute paths stored for its files do not, so every
/// download looks missing: tracks are fetched again beside the copies already
/// there, and eviction cannot find the old ones to delete. The cache lays
/// files out as `<cache>/<artist>/<album>/<file>` (`cache_path_for_track`), so
/// a stale path is re-rooted on its last three components when that file
/// exists under `cache_dir`. Paths whose file is not there are left alone; the
/// next play resolves them as it would any missing download.
///
/// Returns the number of paths rewritten.
pub fn relocate_cached_paths(db: &Database, cache_dir: &Path) -> rusqlite::Result<usize> {
    let prefix = format!("{}/", cache_dir.to_string_lossy().trim_end_matches('/'));
    let stale: Vec<(i64, String)> = db
        .conn
        .prepare(
            "SELECT id, cached_path FROM tracks
             WHERE cached_path IS NOT NULL AND substr(cached_path, 1, ?2) != ?1",
        )?
        .query_map(
            rusqlite::params![prefix, prefix.chars().count() as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    if stale.is_empty() {
        return Ok(0);
    }

    let tx = crate::db::queries::write_transaction(&db.conn)?;
    let mut moved = 0;
    for (id, old) in &stale {
        let tail: Vec<_> = Path::new(old).components().rev().take(3).collect();
        if tail.len() < 3 {
            continue;
        }
        let new = tail
            .iter()
            .rev()
            .fold(cache_dir.to_path_buf(), |p, c| p.join(c));
        if new.is_file() {
            tx.execute(
                "UPDATE tracks SET cached_path = ?1 WHERE id = ?2",
                rusqlite::params![new.to_string_lossy(), id],
            )?;
            moved += 1;
        }
    }
    tx.commit()?;
    if moved > 0 {
        log::info!(
            "re-rooted {moved} cached path(s) under {}",
            cache_dir.display()
        );
    }
    Ok(moved)
}

/// Fetch again anything in the queue whose downloaded copy has just been
/// removed.
///
/// Clearing downloads deletes files the queue is still pointing at, and an item
/// that goes on claiming to be ready plays nothing at all. Call this after
/// either clearing function, from anywhere with a player attached. Putting the
/// items back to `Pending` is all it takes: the download queue follows the
/// playlist and fetches them.
pub fn requeue_cleared_downloads(state: &SharedPlayerState) {
    let stale = state.reset_items_with_missing_files();
    if !stale.is_empty() {
        log::info!(
            "{} queued tracks lost their copy — fetching again",
            stale.len()
        );
    }
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
/// Shared by the TUI, the server and the app.
pub fn sync_favourite_to_remote(db: &Database, track_id: i64, star: bool) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let Ok(Some(remote_id)) = queries::track_remote_id(&db.conn, track_id) else {
        log::warn!("not syncing favourite: track {track_id} has no remote id");
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
pub struct Synced {
    pub library: crate::remote::sync::SyncResult,
    pub favourites: FavouriteSync,
    pub playlists: crate::playlists::PlaylistSync,
    pub history: crate::remote::history::HistorySync,
}

/// Whether a sync walks the server's library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    /// Somebody asked: walk it, whatever the server says.
    Always,
    /// On koan's own account — a server saying something changed, a timer, a
    /// favourite on another device: walk it only if the server's library has
    /// moved since the last complete walk.
    IfChanged,
}

/// Pull the library, then reconcile favourites and playlists.
///
/// One function because there are four callers — the app, the CLI, the GraphQL
/// job and koan's own auto-sync — and each must sync the same things.
///
/// The library comes first: favourites and playlists both name tracks by the
/// server's ids, and neither can find a track the library has not seen yet.
/// Walking it is most of a sync's cost — fifty thousand tracks written for
/// every walk — so `Walk::IfChanged` asks the server first. Its version is
/// read before the walk, so a change landing mid-walk leaves it newer than
/// what is recorded, and the next sync walks again. Favourites and playlists
/// are a request or two each, and are reconciled every time.
pub fn sync_remote(
    db: &Database,
    client: &SubsonicClient,
    walk: Walk,
    url: &str,
    username: &str,
    progress: &(dyn Fn(crate::remote::sync::SyncProgress) + Sync),
) -> Result<Synced, crate::remote::sync::SyncError> {
    use crate::remote::sync;
    // One at a time. An automatic sync still reconciling favourites when a
    // sync was asked for wrote under it, and each fought the other for the
    // write lock. The second waits for the first, and then has little left
    // to do.
    static SYNCING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _one_at_a_time = SYNCING.lock();

    let walked = sync::library_version(db, url);
    let modified = client.library_modified(walked);
    crate::remote::refusal::observe(client.auth(), &modified);
    let version = modified
        .inspect_err(|e| log::debug!("library version unavailable: {e}"))
        .ok()
        .flatten();
    let library = if walk == Walk::IfChanged && version.is_some() && version == walked {
        log::info!("library unchanged on the server; not walked");
        sync::SyncResult::default()
    } else {
        let library = sync::sync_library(db, client, url, username, progress).inspect_err(|e| {
            if let sync::SyncError::Subsonic(e) = e {
                crate::remote::refusal::observe_error(client.auth(), e);
            }
        })?;
        if library.is_complete()
            && let Some(version) = version
        {
            sync::set_library_version(db, url, username, version)?;
        }
        library
    };
    Ok(Synced {
        library,
        favourites: reconcile_favourites(db, client),
        playlists: crate::playlists::reconcile_playlists(db, client, url, username),
        history: crate::remote::history::reconcile(db, client, url, username),
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
/// Stars every local favourite the server knows about but has not starred,
/// then imports everything the server has starred. Union rather than mirror:
/// neither side records an unstar, so treating one as authoritative would
/// silently delete favourites made on the other. Reading the server's stars
/// first keeps a sync from re-sending every favourite, one request each.
///
/// Covers albums and artists as well as tracks — `getStarred2` returns all
/// three from one request, and reading only songs would leave a starred album
/// invisible to koan.
pub fn reconcile_favourites(db: &Database, client: &SubsonicClient) -> FavouriteSync {
    let mut out = FavouriteSync::default();

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

    let unstarred = |ids: Vec<String>, starred: &[String]| {
        let starred: std::collections::HashSet<&String> = starred.iter().collect();
        ids.into_iter()
            .filter(|id| !starred.contains(id))
            .collect::<Vec<_>>()
    };
    let tracks = queries::favourites_with_remote_id(&db.conn, queries::LOCAL_USER)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    for remote_id in unstarred(tracks, &songs) {
        if client.star(&remote_id).is_ok() {
            out.pushed += 1;
        }
    }
    let local_albums = queries::favourite_albums_with_remote_id(&db.conn, queries::LOCAL_USER)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    for remote_id in unstarred(local_albums, &albums) {
        if client.star_album(&remote_id).is_ok() {
            out.pushed += 1;
        }
    }
    let local_artists = queries::favourite_artists_with_remote_id(&db.conn, queries::LOCAL_USER)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    for remote_id in unstarred(local_artists, &artists) {
        if client.star_artist(&remote_id).is_ok() {
            out.pushed += 1;
        }
    }

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
    /// Subsonic's error 41: the server cannot check the token a client signs
    /// with over plain HTTP against the account's password, and wants an app
    /// password or an API key instead.
    #[error(
        "this server needs an app password or API key: make one in the server's web UI and sign in with it"
    )]
    NeedsKey,
    #[error("could not write the configuration: {0}")]
    Config(#[from] crate::config::ConfigError),
}

/// Sign in to a Subsonic server with a password and remember it.
///
/// A koan server that offers `profile::SIGN_IN` is sent the password once and
/// answers with an API key for this device, which is what is kept, as joining
/// with an invite keeps one: the password never goes over the wire again, and
/// the key can be revoked on its own. An app password or the shared secret is
/// refused that trade, and is kept as typed. Any other server keeps the password in
/// `config.local.toml`, gitignored and written `0600`; Subsonic signs every
/// request with it or a salted MD5 of it, so what is kept is
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
    use crate::remote::client::{koan_sign_in, offers_unsigned};
    let url = url.trim_end_matches('/');
    // Asked first, without credentials: the password goes as `p=enc:` only to
    // a server that will trade it for a key.
    if offers_unsigned(url, crate::remote::profile::SIGN_IN).unwrap_or(false) {
        let device = crate::remote::link::LinkIdentity::this_device(None).name;
        match koan_sign_in(url, username, password, &device) {
            Ok(joined) => return adopt_api_key(url, &joined.username, &joined.api_key),
            // Error 50: what was typed is a credential of its own, an app
            // password or the shared secret, which the server will not trade
            // for a key. It is kept as a password is anywhere else.
            Err(SubsonicError::Api { code: 50, .. }) => {}
            Err(e) => return Err(rejected(e)),
        }
    }
    SubsonicClient::new(url, username, password)
        .ping()
        .map_err(rejected)?;

    remember_remote(url, username, Credential::Password(password.to_string()))
}

/// Sign in with an API key made elsewhere — the server's web UI, or another
/// device — for a device where typing a password is the harder thing. Checked
/// against the server before anything is written, as a password is.
pub fn set_remote_api_key(url: &str, username: &str, api_key: &str) -> Result<(), SignInError> {
    let url = url.trim_end_matches('/');
    let credential = Credential::ApiKey(api_key.to_string());
    SubsonicClient::from_auth(SubsonicAuth::with(url, username, credential.clone())).ping()?;
    remember_remote(url, username, credential)
}

/// A server's refusal, with error 41 told apart: see `SignInError::NeedsKey`.
fn rejected(e: SubsonicError) -> SignInError {
    match e {
        SubsonicError::Api { code: 41, .. } => SignInError::NeedsKey,
        e => SignInError::Rejected(e),
    }
}

/// Join a server with an invite. A token is traded for an API key named after
/// this device; an address with the account in it carries the password, which
/// signs in as `set_remote_credentials` does.
pub fn join_with_invite(invite: &crate::invite::Invite) -> Result<(), SignInError> {
    let url = invite.server.trim_end_matches('/');
    match (&invite.token, &invite.password) {
        (Some(token), _) => {
            let device = crate::remote::link::LinkIdentity::this_device(None).name;
            let joined = crate::remote::client::redeem_invite(url, token, &device)?;
            adopt_api_key(url, &joined.username, &joined.api_key)
        }
        (None, Some(password)) => set_remote_credentials(url, &invite.username, password),
        (None, None) => Err(SignInError::Rejected(SubsonicError::BadResponse)),
    }
}

/// Sign in with an API key the server just made for this device: by an
/// invite, or by pairing. The key this device held on the same account is
/// revoked, since left valid it would sit in the key list unused; so is the new
/// one if it cannot be stored.
pub(crate) fn adopt_api_key(url: &str, username: &str, api_key: &str) -> Result<(), SignInError> {
    let url = url.trim_end_matches('/');
    let replaced = Config::load()
        .ok()
        .filter(|c| c.remote.url.trim_end_matches('/') == url)
        .filter(|c| c.remote.username == username)
        .map(|c| c.remote.api_key)
        .filter(|k| !k.is_empty() && k != api_key);
    // Revoked best-effort: a key signs in to give itself up.
    let revoke = |key: &str| {
        let credential = Credential::ApiKey(key.to_string());
        let client = SubsonicClient::from_auth(SubsonicAuth::with(url, username, credential));
        if let Err(e) = client.koan_revoke_own_key() {
            log::warn!("could not revoke an unused API key: {e}");
        }
    };
    if let Err(e) = remember_remote(url, username, Credential::ApiKey(api_key.to_string())) {
        revoke(api_key);
        return Err(e);
    }
    if let Some(old) = replaced {
        revoke(&old);
    }
    Ok(())
}

/// Store a credential already checked against the server, replacing whichever
/// kind was there.
fn remember_remote(url: &str, username: &str, credential: Credential) -> Result<(), SignInError> {
    Config::persist(|cfg| {
        cfg.remote.enabled = true;
        cfg.remote.url = url.to_string();
        cfg.remote.username = username.to_string();
        (cfg.remote.password, cfg.remote.api_key) = match &credential {
            Credential::Password(p) => (p.clone(), String::new()),
            Credential::ApiKey(k) => (String::new(), k.clone()),
        };
        // A new keypair with each sign-in, registered against the new API
        // key; a password has no key row to register it on.
        cfg.remote.device_key = match &credential {
            Credential::ApiKey(_) => crate::remote::proof::new_device_key().unwrap_or_default(),
            Credential::Password(_) => String::new(),
        };
    })?;
    // Whatever account was here before, its devices are not this one's.
    crate::remote::proof::forget();
    // The link rests for up to a minute while signed out; the profile Settings
    // shows is probed when it wakes.
    crate::remote::link::nudge();
    // This device's announcement names the server it is signed in to.
    crate::remote::nearby::readvertise();
    Ok(())
}

/// Shared secret for koan's own Subsonic API.
///
/// Deliberately not the same secret as `remote_credential` — see `SubsonicConfig`.
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
    Some(SubsonicAuth::with(
        &cfg.remote.url,
        &cfg.remote.username,
        remote_credential(cfg)?,
    ))
}

/// One `SubsonicClient` per set of credentials, shared process-wide.
///
/// Constructing one builds two blocking `reqwest` clients, each carrying its
/// own runtime on its own thread, and each starting with a cold connection
/// pool — so a client per call means a fresh TLS handshake for every cover art
/// request.
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
/// act on.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
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
    // it. Only when the selection is the whole album.
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
///
/// What a server's own surfaces make whatever `[remote]` says: a link made
/// upstream would belong to the upstream's account, not the koan user who
/// asked, and could not be listed or revoked here.
pub fn create_native_share(
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

/// An album's id as `createShare` should be given it.
///
/// koan numbers albums and songs separately, publishes album ids bare, and
/// reads a bare id in `createShare` as a song, so album 5 would share song 5.
/// Its `al-` prefix says which is meant. Other servers get the id as they
/// issued it: some also number albums, and would not know the prefix.
fn album_share_id(client: &crate::remote::client::SubsonicClient, remote_id: String) -> String {
    let koan = crate::remote::profile::is_koan(client.auth());
    album_share_id_for(koan, remote_id)
}

fn album_share_id_for(koan: bool, remote_id: String) -> String {
    if koan && remote_id.parse::<i64>().is_ok() {
        format!("al-{remote_id}")
    } else {
        remote_id
    }
}

/// The album's own remote ID, but only when `selected` covers every track on
/// it. Sharing an album link for half an album would hand out more than the
/// user picked.
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
/// `.` and `..` become `_`: tags and server metadata are untrusted, and either
/// would move the path out of the directory it is joined onto.
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

    let cleaned = truncate_bytes(&cleaned, 240).trim_end().to_string();
    match cleaned.as_str() {
        "." | ".." => "_".into(),
        _ => cleaned,
    }
}

/// A file extension from a codec name, which for remote tracks is whatever
/// the server sent as `suffix`. ASCII alphanumerics only, so it can never
/// carry a separator or a `..`; `None` when nothing usable is left.
pub fn sanitise_extension(codec: &str) -> Option<String> {
    let ext: String = codec
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(16)
        .collect::<String>()
        .to_lowercase();
    (!ext.is_empty()).then_some(ext)
}

/// Whether `path` lies inside `dir` without leaving it on the way — every
/// component after the prefix is a plain name.
pub fn path_within(dir: &Path, path: &Path) -> bool {
    path.strip_prefix(dir).is_ok_and(|rest| {
        rest.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    })
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
        .and_then(sanitise_extension)
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
fn resolve_item_path(
    cfg: &Config,
    track: &queries::TrackRow,
    remote_url: Option<&str>,
    album_date: Option<&str>,
) -> (PathBuf, ItemState) {
    match queries::choose_playback_source(
        track.path.as_deref(),
        track.cached_path.as_deref(),
        remote_url,
    ) {
        Some(queries::PlaybackSource::Local(p)) => (p, ItemState::Ready),
        // A cache entry is only as good as its contents. Older builds could
        // store a Subsonic error body here, which reports Ready and then fails
        // to decode forever; treating it as Pending sends it back through the
        // download path, which discards it and re-fetches.
        Some(queries::PlaybackSource::Cached(p)) => {
            let state = if is_cached_audio(&p) {
                ItemState::Ready
            } else {
                ItemState::Pending
            };
            (p, state)
        }
        Some(queries::PlaybackSource::Remote(_)) => {
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
        pre_shuffle: None,
    }
}

/// Build playlist items for many tracks at once.
///
/// One config read and one query for the whole batch, whatever its size: the
/// rows already say where each track's file and download are, and what they
/// leave out — the stream URL and the album's date — is read for all of them
/// together.
pub fn playlist_items_for_tracks(db: &Database, tracks: &[queries::TrackRow]) -> Vec<PlaylistItem> {
    let cfg = Config::load().unwrap_or_default();
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let extras = queries::queue_item_extras(&db.conn, &ids).unwrap_or_default();

    tracks
        .iter()
        .map(|track| {
            let extra = extras.get(&track.id);
            let remote_url = extra.and_then(|e| e.remote_url.as_deref());
            let album_date = extra.and_then(|e| e.album_date.as_deref());
            let (path, state) = resolve_item_path(&cfg, track, remote_url, album_date);
            playlist_item_from_track(track, album_date, path, state)
        })
        .collect()
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
///
/// Runs the transfer the download queue claimed for this track in the
/// player's store. `cancelled` says when nothing wants it any more; `None` is
/// returned then. Says nothing to the queue entries waiting on it: the caller
/// settles them, all at once, with [`crate::remote::downloads::settle`].
pub(crate) fn download_track(
    db_id: i64,
    cancelled: &dyn Fn() -> bool,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
    state: &SharedPlayerState,
    cfg: &Config,
    client: &SubsonicClient,
) -> Option<Result<PathBuf, String>> {
    // From the pool. This runs once per track fetched, and opening a
    // connection runs the schema DDL and a WAL checkpoint — with several
    // transfers going, several init cycles would contend with each other and
    // with library reads.
    let db = match crate::db::pool::shared().get() {
        Ok(db) => db,
        Err(e) => return Some(Err(format!("db error: {e}"))),
    };
    let Ok(Some(track)) = queries::get_track_row(&db.conn, db_id) else {
        return Some(Err("track not found".into()));
    };

    // 1. The library's own file, when there is one.
    if let Some(p) = track.path.as_deref().map(PathBuf::from)
        && p.exists()
    {
        log::info!("download_track: local file exists, using {}", p.display());
        return Some(Ok(p));
    }
    let Some(remote_id) = track.remote_id.clone() else {
        return Some(Err(
            "not in the library folder, and no remote copy to fetch".into(),
        ));
    };

    let album_date: Option<String> = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let cache_dir = cfg.cache_dir();
    let dest = cache_path_for_track(&cache_dir, &track, album_date.as_deref());
    if !path_within(&cache_dir, &dest) {
        return Some(Err(format!(
            "cache path escapes the cache: {}",
            dest.display()
        )));
    }

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
        return Some(Ok(dest));
    }

    // 3. Download from remote, into the `.part` file the decoder streams from
    // while the bytes land.
    let store = state.downloads();
    let bytes_written = store.announce(
        db_id,
        track.title.clone(),
        track.artist_name.clone(),
        crate::remote::download::part_path(&dest),
        dest.clone(),
    );

    let progress_tx = tx.clone();
    let stream_ready_flag = std::sync::atomic::AtomicBool::new(false);
    // A retry restarts the byte count from zero, so a changed total re-announces.
    let announced_total = AtomicU64::new(u64::MAX);
    let result =
        client.download_with_progress(&remote_id, &dest, cancelled, |downloaded, total| {
            bytes_written.set(downloaded);
            // What knows a transfer moved is the code moving it. Held to a reading
            // every 250ms inside, so a chunk landing costs an atomic and a compare.
            store.progressed();
            if announced_total.swap(total, Ordering::Relaxed) != total {
                store.started(db_id, total);
            }
            if !stream_ready_flag.load(Ordering::Relaxed)
                && downloaded >= crate::player::state::STREAM_THRESHOLD
            {
                stream_ready_flag.store(true, Ordering::Relaxed);
                // Every entry waiting on it: whichever is under the cursor is the
                // one the player starts streaming.
                for id in store.waiters(db_id) {
                    progress_tx.send(PlayerCommand::TrackStreamReady(id)).ok();
                }
            }
        });

    match result {
        Err(SubsonicError::Download(DownloadError::Cancelled)) => None,
        Err(e) => {
            log::warn!("x {} — {}", track.title, e);
            Some(Err(e.to_string()))
        }
        Ok(()) => {
            // Without this row the file is invisible to cache eviction and never reclaimed.
            if let Err(e) = queries::set_cached_path(&db.conn, db_id, &dest.to_string_lossy()) {
                log::warn!(
                    "cached {} but failed to record it ({}) — it will not be evicted",
                    dest.display(),
                    e
                );
            }
            log::info!("+ {} — {}", track.title, track.artist_name);
            Some(Ok(dest))
        }
    }
}

/// Why there is no remote client, in words worth showing someone.
///
/// Every caller of `subsonic_client` gets `None` for three different reasons,
/// and reporting one for all of them makes "koan has no password", which sends
/// you to sign in, look like a server that is merely down.
pub fn remote_unavailable(cfg: &Config) -> String {
    if !cfg.remote.enabled {
        return "no remote server is configured".into();
    }
    if cfg.remote.url.is_empty() {
        return "the remote server has no address".into();
    }
    if remote_credential(cfg).is_none() {
        return "no password or API key is stored for the remote server".into();
    }
    // A credential resolved, so the client should have built. Nothing else
    // returns `None`, but saying so beats claiming a cause that is wrong.
    "the remote server could not be reached".into()
}

/// What every front end says when the server refused the stored credential.
pub const SIGN_IN_REFUSED: &str = "the remote server refused the stored sign-in; sign in again";

/// Why the configured server cannot be used, if it cannot: no credential, or
/// one the server has refused since (a revoked API key, a changed password).
/// `None` when no server is configured, or the one that is works as far as
/// anything has heard.
pub fn remote_problem(cfg: &Config) -> Option<String> {
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        return None;
    }
    match subsonic_auth(cfg) {
        None => Some(remote_unavailable(cfg)),
        Some(auth) if crate::remote::refusal::refused(&auth) => Some(SIGN_IN_REFUSED.into()),
        Some(_) => None,
    }
}

/// Whether the server refused the configured credential when it was last used.
pub fn sign_in_refused(cfg: &Config) -> bool {
    subsonic_auth(cfg).is_some_and(|auth| crate::remote::refusal::refused(&auth))
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
    fn cached_paths_follow_a_moved_cache_directory() {
        let old = tempfile::tempdir().unwrap();
        let new = tempfile::tempdir().unwrap();
        let db = test_db();

        let mut rows = Vec::new();
        for name in ["moved", "gone", "current"] {
            let mut meta = sample_meta(name, "Artist", "Album");
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(name.into());
            let id = queries::upsert_track(&db.conn, &meta).unwrap();
            let tail = format!("Artist/Album/{name}.flac");
            // Every copy now lives under the new directory except "gone".
            if name != "gone" {
                let file = new.path().join(&tail);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, b"audio").unwrap();
            }
            let stored = if name == "current" {
                new.path()
            } else {
                old.path()
            }
            .join(&tail);
            queries::set_cached_path(&db.conn, id, &stored.to_string_lossy()).unwrap();
            rows.push((id, tail));
        }

        assert_eq!(relocate_cached_paths(&db, new.path()).unwrap(), 1);

        let cached = |id: i64| -> String {
            db.conn
                .query_row("SELECT cached_path FROM tracks WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let expect = |root: &Path, tail: &str| root.join(tail).to_string_lossy().into_owned();
        assert_eq!(
            cached(rows[0].0),
            expect(new.path(), &rows[0].1),
            "re-rooted"
        );
        assert_eq!(
            cached(rows[1].0),
            expect(old.path(), &rows[1].1),
            "no file, left alone"
        );
        assert_eq!(
            cached(rows[2].0),
            expect(new.path(), &rows[2].1),
            "already current"
        );
        assert_eq!(
            relocate_cached_paths(&db, new.path()).unwrap(),
            0,
            "idempotent"
        );
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
    fn a_rebuild_re_reads_the_library_into_the_rows_it_has() {
        let db = test_db();
        let mut meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        meta.path = Some("/music/windowlicker.flac".into());
        let track_id = queries::upsert_track(&db.conn, &meta).unwrap();

        queries::toggle_favourite(&db.conn, crate::db::queries::LOCAL_USER, track_id).unwrap();
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
        let count = |sql: &str| -> i64 { db.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count("SELECT COUNT(*) FROM local_files"),
            0,
            "every file is read again"
        );

        // The scan reads the file again and takes its row back.
        assert_eq!(queries::upsert_track(&db.conn, &meta).unwrap(), track_id);
        assert_eq!(count("SELECT COUNT(*) FROM tracks"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM favourites"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM lyrics_cache"), 1);
    }

    #[test]
    fn a_rebuilt_file_that_is_gone_goes_with_its_folder_scan() {
        let db = test_db();
        let tmp = tempfile::tempdir().unwrap();
        let mut meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker");
        meta.path = Some(tmp.path().join("gone.flac").to_string_lossy().into_owned());
        queries::upsert_track(&db.conn, &meta).unwrap();
        rebuild_index(&db).unwrap();

        queries::remove_stale_tracks(&db.conn, tmp.path(), false).unwrap();
        let tracks: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tracks, 0);
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

    #[test]
    fn a_server_with_an_upstream_still_shares_natively() {
        // Local-only tracks, which the upstream path refuses as NothingRemote.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let db = Database { conn };
        let a = upsert_track(&db.conn, &sample_meta("A", "X", "Y")).unwrap();
        let mut cfg = Config::default();
        cfg.remote.enabled = true;
        cfg.remote.url = "https://upstream.invalid".into();
        cfg.remote.username = "someone".into();
        cfg.remote.password = "secret".into();
        cfg.sharing.public_url = Some("https://koan.example".into());
        let out = create_native_share(
            &db,
            queries::LOCAL_USER,
            &cfg,
            &ShareTarget::Tracks(vec![a]),
            None,
        )
        .unwrap();
        assert_eq!(out.url, format!("https://koan.example/share/{}", out.id));
        assert!(
            queries::shares::get_share(&db.conn, &out.id)
                .unwrap()
                .is_some()
        );
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

#[cfg(test)]
mod cache_path_tests {
    use super::*;

    fn track(artist: &str, album: &str, codec: &str) -> queries::TrackRow {
        queries::TrackRow {
            id: 1,
            album_id: None,
            artist_id: None,
            artist_name: artist.into(),
            album_artist_name: artist.into(),
            album_title: album.into(),
            disc: None,
            track_number: Some(1),
            title: "Song".into(),
            duration_ms: None,
            path: None,
            codec: Some(codec.into()),
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "remote".into(),
            remote_id: Some("r1".into()),
            cached_path: None,
        }
    }

    #[test]
    fn a_server_suffix_cannot_leave_the_cache() {
        let cache = Path::new("/cache");
        for codec in [
            "flac/../../../../x",
            "..",
            "../..",
            "/etc/passwd",
            "\\..\\..",
        ] {
            let path = cache_path_for_track(cache, &track("A", "B", codec), None);
            assert!(path_within(cache, &path), "{codec}: {}", path.display());
        }
        let path = cache_path_for_track(cache, &track("A", "B", "flac/../../../../x"), None);
        assert_eq!(path.extension().unwrap(), "flacx");
    }

    #[test]
    fn dot_names_cannot_climb_out() {
        let cache = Path::new("/cache");
        let path = cache_path_for_track(cache, &track("..", ".", ".."), None);
        assert!(path_within(cache, &path), "{}", path.display());
        assert_eq!(sanitise_filename(".."), "_");
        assert_eq!(sanitise_filename(" . "), "_");
        assert_eq!(sanitise_filename("..."), "...");
    }

    #[test]
    fn an_empty_suffix_falls_back_to_flac() {
        assert_eq!(sanitise_extension("../"), None);
        assert_eq!(sanitise_extension("FLAC"), Some("flac".into()));
        let path = cache_path_for_track(Path::new("/c"), &track("A", "B", "./"), None);
        assert_eq!(path.extension().unwrap(), "flac");
    }

    #[test]
    fn path_within_rejects_parent_components() {
        let dir = Path::new("/cache");
        assert!(path_within(dir, Path::new("/cache/a/b.flac")));
        assert!(!path_within(dir, Path::new("/cache/a/../../x")));
        assert!(!path_within(dir, Path::new("/elsewhere/x")));
    }
}

#[cfg(test)]
mod sign_in_tests {
    use super::*;

    /// A koan server as `set_remote_credentials` meets it over plain HTTP:
    /// `mate`'s account password is traded for a key, while `mate`'s app
    /// password and `testuser`'s shared secret are refused that trade with
    /// error 50 and accepted as tokens.
    fn serve() -> String {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let target = request.split_whitespace().nth(1).unwrap_or("");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let param = |name: &str| {
                    query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(k, _)| *k == name)
                        .map(|(_, v)| v.replace("%3A", ":"))
                        .unwrap_or_default()
                };
                let secret = match param("u").as_str() {
                    "mate" => "app-secret",
                    "testuser" => "shared-secret",
                    _ => "",
                };
                let ok = r#"{"subsonic-response":{"status":"ok"}}"#.to_owned();
                let refused = |code: i32| {
                    format!(
                        r#"{{"subsonic-response":{{"status":"failed","error":{{"code":{code},"message":"refused"}}}}}}"#
                    )
                };
                let body = match path.rsplit('/').next().unwrap() {
                    "getOpenSubsonicExtensions" => r#"{"subsonic-response":{"status":"ok","openSubsonicExtensions":[{"name":"koanSignIn","versions":[1]}]}}"#.to_owned(),
                    "koanSignIn" => {
                        let hex = param("p").trim_start_matches("enc:").to_owned();
                        let typed: String = (0..hex.len())
                            .step_by(2)
                            .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                            .map(char::from)
                            .collect();
                        match (param("u").as_str(), typed.as_str()) {
                            ("mate", "hunter22") => r#"{"subsonic-response":{"status":"ok","join":{"username":"mate","apiKey":"minted"}}}"#.to_owned(),
                            (_, typed) if typed == secret => refused(50),
                            _ => refused(40),
                        }
                    }
                    "ping" if param("apiKey") == "minted" => ok,
                    "ping" => {
                        let expected =
                            format!("{:x}", md5::compute(format!("{secret}{}", param("s"))));
                        if !secret.is_empty() && param("t") == expected {
                            ok
                        } else {
                            refused(40)
                        }
                    }
                    _ => ok,
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn a_password_ends_in_a_key_and_other_credentials_are_kept_as_typed() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::config::set_config_dir(dir.path());
        let url = serve();
        let kept = || {
            let remote = Config::load().unwrap().remote;
            (remote.username, remote.password, remote.api_key)
        };

        set_remote_credentials(&url, "mate", "hunter22").unwrap();
        assert_eq!(kept(), ("mate".into(), String::new(), "minted".into()));

        set_remote_credentials(&url, "mate", "app-secret").unwrap();
        assert_eq!(
            kept(),
            ("mate".into(), "app-secret".into(), String::new()),
            "an app password is refused a key and kept"
        );

        set_remote_credentials(&url, "testuser", "shared-secret").unwrap();
        assert_eq!(
            kept(),
            ("testuser".into(), "shared-secret".into(), String::new()),
            "the shared secret is refused a key and kept"
        );

        assert!(matches!(
            set_remote_credentials(&url, "mate", "wrong"),
            Err(SignInError::Rejected(SubsonicError::Api { code: 40, .. }))
        ));
        assert_eq!(
            kept().1,
            "shared-secret",
            "a refused sign-in writes nothing"
        );
    }
}

#[cfg(test)]
mod favourite_sync_tests {
    use super::*;
    use crate::db::queries::sample_meta;
    use std::sync::Mutex;

    /// A server with song `s1` starred, recording every id it is asked to star.
    fn serve(stars: Arc<Mutex<Vec<String>>>) -> String {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let target = request.split_whitespace().nth(1).unwrap_or("");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let body = match path.rsplit('/').next().unwrap() {
                    "getStarred2" => {
                        r#"{"subsonic-response":{"status":"ok","starred2":{"song":[{"id":"s1","title":"One"}]}}}"#
                    }
                    "star" => {
                        if let Some((_, id)) = query
                            .split('&')
                            .filter_map(|kv| kv.split_once('='))
                            .find(|(k, _)| *k == "id")
                        {
                            stars.lock().unwrap().push(id.to_string());
                        }
                        r#"{"subsonic-response":{"status":"ok"}}"#
                    }
                    _ => r#"{"subsonic-response":{"status":"ok"}}"#,
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn only_favourites_the_server_lacks_are_starred() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        for (title, remote_id) in [("One", "s1"), ("Two", "s2")] {
            let mut meta = sample_meta(title, "Artist", "Album");
            meta.path = Some(format!("/music/{title}.flac"));
            meta.remote_id = Some(remote_id.into());
            let id = queries::upsert_track(&db.conn, &meta).unwrap();
            queries::add_favourite(&db.conn, queries::LOCAL_USER, id).unwrap();
        }

        let stars = Arc::new(Mutex::new(Vec::new()));
        let url = serve(stars.clone());
        let sync = reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw"));
        assert_eq!(sync.pushed, 1);
        assert_eq!(*stars.lock().unwrap(), ["s2"]);
    }
}

#[cfg(test)]
mod refusal_tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// A koan server holding API keys that can be revoked. `koanSignIn` with
    /// `mate`'s password mints `second`.
    fn serve(keys: Arc<Mutex<HashSet<String>>>) -> String {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let target = request.split_whitespace().nth(1).unwrap_or("");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let param = |name: &str| {
                    query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(k, _)| *k == name)
                        .map(|(_, v)| v.to_owned())
                        .unwrap_or_default()
                };
                let endpoint = path.rsplit('/').next().unwrap();
                let body = if endpoint == "koanSignIn" {
                    keys.lock().unwrap().insert("second".into());
                    r#"{"subsonic-response":{"status":"ok","join":{"username":"mate","apiKey":"second"}}}"#.to_owned()
                } else if endpoint == "getOpenSubsonicExtensions" {
                    r#"{"subsonic-response":{"status":"ok","openSubsonicExtensions":[{"name":"koanSignIn","versions":[1]}]}}"#.to_owned()
                } else if !keys.lock().unwrap().contains(&param("apiKey")) {
                    r#"{"subsonic-response":{"status":"failed","error":{"code":44,"message":"invalid API key"}}}"#.to_owned()
                } else {
                    r#"{"subsonic-response":{"status":"ok","indexes":{"lastModified":1}}}"#
                        .to_owned()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn a_revoked_key_is_reported_until_signing_in_again() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::config::set_config_dir(dir.path());
        let keys = Arc::new(Mutex::new(HashSet::from(["first".to_owned()])));
        let url = serve(keys.clone());
        Config::persist(|c| {
            c.remote.enabled = true;
            c.remote.url = url.clone();
            c.remote.username = "mate".into();
            c.remote.api_key = "first".into();
        })
        .unwrap();

        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let sync = || {
            let cfg = Config::load().unwrap();
            let client = SubsonicClient::from_auth(subsonic_auth(&cfg).unwrap());
            let _ = sync_remote(&db, &client, Walk::IfChanged, &url, "mate", &|_| {});
        };

        sync();
        assert_eq!(remote_problem(&Config::load().unwrap()), None);

        keys.lock().unwrap().remove("first");
        sync();
        let cfg = Config::load().unwrap();
        assert_eq!(remote_problem(&cfg).as_deref(), Some(SIGN_IN_REFUSED));
        assert!(sign_in_refused(&cfg));

        set_remote_credentials(&url, "mate", "hunter22").unwrap();
        let cfg = Config::load().unwrap();
        assert_eq!(cfg.remote.api_key, "second");
        assert_eq!(remote_problem(&cfg), None, "a new credential starts clean");
        sync();
        assert_eq!(remote_problem(&Config::load().unwrap()), None);
    }
}
