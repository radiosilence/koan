//! The library watch and auto-sync threads, and the lock that lets one process at a time run them.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::db::connection::Database;

use super::*;

/// The right to scan and watch the library at `db_path`, held by one process
/// at a time: an exclusive `flock` on `watch.lock` beside the database. Two
/// servers share a database while one replaces the other, and two scanners
/// would only contend for its write lock. `None` while another process holds
/// it. The kernel drops the lock when the process ends, however it ends, so a
/// killed server cannot keep it.
pub fn try_watch_lock(db_path: &Path) -> std::io::Result<Option<std::fs::File>> {
    let file = watch_lock_file(db_path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// [`try_watch_lock`], waiting as long as another process holds it. The
/// kernel wakes the waiter when the holder lets go; nothing polls.
pub fn watch_lock(db_path: &Path) -> std::io::Result<std::fs::File> {
    let file = watch_lock_file(db_path)?;
    file.lock()?;
    Ok(file)
}

fn watch_lock_file(db_path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(db_path.with_file_name("watch.lock"))
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
            // Another process watching this library, as an outgoing server
            // does while its replacement starts, keeps the watch until it
            // exits; this thread sleeps in the kernel until then. Serving
            // does not wait for it. A directory that cannot hold the lock
            // file is one koan is not sharing, so it watches as before.
            let _lock = match try_watch_lock(&db_path) {
                Ok(Some(lock)) => Some(lock),
                Ok(None) => {
                    log::info!(
                        "another koan is watching this library; serving, and waiting to take over"
                    );
                    let lock = watch_lock(&db_path);
                    if lock.is_ok() {
                        log::info!("library watch: taken over");
                    }
                    lock.inspect_err(|e| log::warn!("library watch: lock: {e}"))
                        .ok()
                }
                Err(e) => {
                    log::warn!("library watch: no lock beside the database: {e}");
                    None
                }
            };
            let scan = |reason: &str, folders: &[PathBuf], dirs: Option<&[PathBuf]>| {
                if folders.is_empty() {
                    return;
                }
                let Ok(db) = Database::open_existing(&db_path) else {
                    return;
                };
                // A newer koan sharing the database has upgraded it: what
                // this build would write is no longer its shape.
                if crate::db::pool::understood(&db.conn).is_err() {
                    return;
                }
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

#[cfg(test)]
mod watch_lock_tests {
    use super::*;

    /// Two processes on one database: whoever holds the lock watches, the
    /// other waits, and takes over once the first lets go. `flock` locks
    /// belong to the open file, so two opens in one process stand in for two
    /// processes.
    #[test]
    fn one_watcher_per_database_and_the_next_takes_over() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("koan.db");
        let first = try_watch_lock(&db).unwrap().expect("the first takes it");
        assert!(try_watch_lock(&db).unwrap().is_none(), "the second waits");
        assert!(dir.path().join("watch.lock").exists());
        drop(first);
        assert!(try_watch_lock(&db).unwrap().is_some(), "and then takes it");
    }

    /// The waiting side blocks in the kernel and wakes when the holder lets
    /// go, with no retry interval to sit out.
    #[test]
    fn a_waiting_watcher_wakes_when_the_lock_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("koan.db");
        let first = try_watch_lock(&db).unwrap().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiting = db.clone();
        std::thread::spawn(move || {
            let lock = watch_lock(&waiting);
            let _ = tx.send(lock.is_ok());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "it waits while the lock is held"
        );
        drop(first);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
    }

    #[test]
    fn databases_in_different_directories_do_not_share_a_lock() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let _a = try_watch_lock(&a.path().join("koan.db")).unwrap().unwrap();
        assert!(try_watch_lock(&b.path().join("koan.db")).unwrap().is_some());
    }
}
