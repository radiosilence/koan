//! The library watch and auto-sync threads, and the lock that lets one process at a time run them.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::index::scanner::ScanResult;

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

/// What the library watch reports about a scan it runs.
pub enum WatchScan<'a> {
    /// A scan is starting, and why: `start-up`, `folder change`, `periodic`
    /// or `new folder`. It may yet wait for another scan to leave the lane.
    Started(&'static str),
    /// Files read, of those the scan will read: first `done: 0`, once the
    /// count is known, then after each batch stored. See
    /// [`ScanOptions::on_read`](crate::index::scanner::ScanOptions::on_read).
    Read {
        done: u64,
        total: u64,
    },
    Finished(&'a ScanResult),
}

/// What wakes the library watch.
enum Heard {
    Fs(notify::Result<notify::Event>),
    /// The config was written, and the folder list with it may have changed.
    Settings,
    /// A filesystem was mounted or unmounted somewhere on the machine.
    Mounts,
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
/// The folder list is read again whenever the config changes — written by
/// koan, or edited by hand, which the watch on the config files hears — so a
/// folder added is watched without a restart. A configured folder that is not
/// there yet is awaited through its nearest existing ancestor, and watched
/// once it appears. Each folder's identity
/// is checked whenever a filesystem is mounted or unmounted, so a volume
/// unplugged and plugged back in is watched afresh and rescanned. A folder on
/// a network share is rescanned every quarter of an hour as well, since a
/// file another machine writes there raises no event on this one; a local
/// folder is never scanned on a timer.
///
/// `on_scan` is told when each scan starts, how far it has got and how it
/// ended, so a UI can show it.
pub fn spawn_library_watch(
    db_path: std::path::PathBuf,
    on_scan: impl Fn(WatchScan) + Send + Sync + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    use std::collections::BTreeSet;
    use std::time::{Duration, Instant};

    use notify::{RecursiveMode, Watcher};

    use crate::index::scanner::{self, ScanOptions};
    use crate::index::watch::{WatchedRoot, nearest_existing, on_mount_change, scan_target};

    // Copying an album in is a burst of events. Wait for it to stop before
    // scanning, rather than scanning per file.
    const SETTLE: Duration = Duration::from_secs(5);
    // Past this many directories one walk of the library is cheaper than many.
    const MAX_DIRS: usize = 200;
    // How often a folder on a network share is scanned whole. Unchanged files
    // are skipped on mtime and size, so an idle one is a second's work.
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
            // One connection, opened once and kept between scans.
            let pool = crate::db::pool::Pool::new(db_path.clone());
            let scan = |reason: &'static str, folders: &[PathBuf], dirs: Option<&[PathBuf]>| {
                if folders.is_empty() {
                    return;
                }
                // Refused once a newer koan sharing the database has upgraded
                // it: what this build would write is no longer its shape.
                let Ok(db) = pool.get() else {
                    return;
                };
                on_scan(WatchScan::Started(reason));
                let on_read = |done, total| on_scan(WatchScan::Read { done, total });
                let opts = ScanOptions {
                    on_read: Some(&on_read),
                    ..Default::default()
                };
                // A scan that panics is reported as stopped part way, and the
                // watch carries on: the next change or rescan tries again.
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match dirs {
                        Some(dirs) => scanner::scan_dirs(&db, folders, dirs, opts, None),
                        None => scanner::full_scan(&db, folders, opts, None),
                    }))
                    .unwrap_or_else(|_| {
                        log::error!("{reason} scan panicked");
                        let mut stopped = ScanResult::default();
                        stopped.incomplete = true;
                        stopped
                    });
                on_scan(WatchScan::Finished(&result));
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
            crate::signal::forward(crate::signal::config_changed(), tx.clone(), || {
                Heard::Settings
            });
            {
                let tx = tx.clone();
                on_mount_change(move || tx.send(Heard::Mounts).is_ok());
            }
            let Ok(mut watcher) = notify::recommended_watcher(move |event| {
                let _ = tx.send(Heard::Fs(event));
            }) else {
                log::warn!("could not watch the library folders");
                return;
            };

            // The config files, watched themselves, so a hand edit is heard as
            // koan's own writes are.
            let config_files = [
                crate::config::config_file_path(),
                crate::config::config_local_file_path(),
            ];

            // Brings the watches in line with the configured folders as they
            // are now, returning the ones newly watched — added in settings, or
            // back from being unmounted — whose changes nobody heard.
            //
            // Beside the folders it keeps `followed`, watched one level deep:
            // each config file, and for a file or configured folder that is not
            // there yet, its nearest existing ancestor. `awaited` is those
            // paths as the kernel spells them; an event on one, or on a
            // directory on the way to one, is a reason to look again.
            // `refresh` watches the followed paths afresh, as a file replaced
            // by a rename needs on inotify, whose watch was on the old inode.
            let mut roots: Vec<WatchedRoot> = Vec::new();
            let mut followed: Vec<PathBuf> = Vec::new();
            let mut awaited: Vec<PathBuf> = Vec::new();
            let mut rewatch = |roots: &mut Vec<WatchedRoot>,
                               followed: &mut Vec<PathBuf>,
                               awaited: &mut Vec<PathBuf>,
                               refresh: bool| {
                let configured = folders();
                let wanted: Vec<WatchedRoot> = configured
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

                awaited.clear();
                let mut follow = Vec::new();
                let missing = configured
                    .iter()
                    .filter(|f| !roots.iter().any(|r| r.path == **f));
                for path in config_files.iter().chain(missing) {
                    let Some((existing, spelled)) = nearest_existing(path) else {
                        continue;
                    };
                    awaited.push(spelled);
                    // A folder watched whole already hears it.
                    let inside = roots
                        .iter()
                        .any(|r| existing.starts_with(&r.real) || existing.starts_with(&r.path));
                    if !inside && !follow.contains(&existing) {
                        follow.push(existing);
                    }
                }
                followed.retain(|p| {
                    let keep = !refresh && follow.contains(p);
                    if !keep {
                        let _ = watcher.unwatch(p);
                    }
                    keep
                });
                for path in follow {
                    if followed.contains(&path) {
                        continue;
                    }
                    match watcher.watch(&path, RecursiveMode::NonRecursive) {
                        Ok(()) => followed.push(path),
                        Err(e) => log::debug!("could not follow {}: {e}", path.display()),
                    }
                }
                fresh
            };
            // After the first frame and the first track, not competing with them.
            std::thread::sleep(Duration::from_secs(3));
            rewatch(&mut roots, &mut followed, &mut awaited, false);
            scan("start-up", &folders(), None);

            let mut dirs = BTreeSet::new();
            let mut everything = false;
            let mut recheck = false;
            let mut refresh = false;
            let mut settle_at: Option<Instant> = None;
            let mut rescan_at = Instant::now() + RESCAN;
            loop {
                // Changes heard meanwhile wait in the channel; a rescan that
                // fell due runs as soon as this returns.
                crate::quiet::wait_until_awake();
                // With no folder on a share there is nothing to scan on a
                // timer, and the first share added waits its full interval.
                let any_shared = roots.iter().any(|r| !r.local);
                if !any_shared {
                    rescan_at = Instant::now() + RESCAN;
                }
                let rescan_due = any_shared.then_some(rescan_at);
                let heard = match settle_at.into_iter().chain(rescan_due).min() {
                    Some(wake) => rx.recv_timeout(wake.saturating_duration_since(Instant::now())),
                    None => rx
                        .recv()
                        .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected),
                };
                match heard {
                    Ok(Heard::Settings | Heard::Mounts) => recheck = true,
                    Ok(Heard::Fs(Ok(event))) if event.need_rescan() => {
                        everything = true;
                        settle_at = Some(Instant::now() + SETTLE);
                    }
                    Ok(Heard::Fs(Ok(event))) => {
                        let mut changed = false;
                        for path in &event.paths {
                            if config_files
                                .iter()
                                .any(|f| path.file_name() == f.file_name())
                                && awaited.contains(path)
                            {
                                // Edited by hand, or written by koan: either
                                // way whatever acts on a setting looks again.
                                Config::invalidate_cache();
                                refresh = true;
                            }
                            // The folder itself coming or going: unmounted, or
                            // renamed away. Its identity says which. Or a
                            // path awaited, or a directory on the way to one,
                            // appearing.
                            if roots.iter().any(|r| *path == r.path || *path == r.real)
                                || awaited.iter().any(|a| a.starts_with(path))
                            {
                                recheck = true;
                            } else if let Some(dir) = scan_target(&event.kind, path, &roots) {
                                dirs.insert(dir);
                                changed = true;
                            }
                        }
                        if changed {
                            settle_at = Some(Instant::now() + SETTLE);
                        }
                    }
                    Ok(Heard::Fs(Err(e))) => log::debug!("library watch: {e}"),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }

                if std::mem::take(&mut recheck) {
                    let refresh = std::mem::take(&mut refresh);
                    let fresh = rewatch(&mut roots, &mut followed, &mut awaited, refresh);
                    if !fresh.is_empty() {
                        scan("new folder", &fresh, None);
                    }
                }
                let now = Instant::now();
                if settle_at.is_some_and(|at| now >= at) {
                    let changed =
                        scanner::minimal_dirs(std::mem::take(&mut dirs).into_iter().collect());
                    if everything || changed.len() > MAX_DIRS {
                        scan("folder change", &folders(), None);
                        rescan_at = Instant::now() + RESCAN;
                    } else {
                        scan("folder change", &folders(), Some(&changed));
                    }
                    everything = false;
                    settle_at = None;
                }
                if now >= rescan_at {
                    let shared: Vec<PathBuf> = roots
                        .iter()
                        .filter(|r| !r.local)
                        .map(|r| r.path.clone())
                        .collect();
                    if !shared.is_empty() {
                        scan("periodic", &shared, None);
                    }
                    rescan_at = Instant::now() + RESCAN;
                }
            }
        })
        .ok()
}

/// What auto-sync acts on. Anything else written to the config wakes it and is
/// ignored.
#[derive(PartialEq)]
struct AutoSync {
    on: bool,
    interval_mins: u64,
    server: (String, String),
}

impl AutoSync {
    fn of(cfg: &Config) -> Self {
        Self {
            on: cfg.remote.enabled && cfg.remote.auto_sync,
            interval_mins: cfg.remote.auto_sync_interval_mins,
            server: (cfg.remote.url.clone(), cfg.remote.username.clone()),
        }
    }
}

/// What wakes auto-sync between runs.
enum Woke {
    Settings,
    Link,
}

/// Keep the library in step with the server, without being asked.
///
/// One sync shortly after startup, then every `auto_sync_interval_mins` for a
/// server that cannot say when it changes. A koan server can: it sends every
/// linked app a sync when its library or a playlist moves, and queues one for
/// an app that was away, so a timer would only ask a question already
/// answered, and none is set while the link is up. Each run walks the library only if the server
/// says it moved — see `Walk::IfChanged` — which is what makes it cheap enough
/// to run unattended.
///
/// Between runs this sleeps until the interval is up or koan writes a change
/// to the remote settings — auto-sync turned on, another server signed in to —
/// which runs it again at once.
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
    use std::time::{Duration, Instant};

    std::thread::Builder::new()
        .name("koan-auto-sync".into())
        .spawn(move || {
            // Before the first run, so a change while it runs is heard after.
            let (tx, rx) = std::sync::mpsc::channel();
            crate::signal::forward(crate::signal::config_changed(), tx.clone(), || {
                Woke::Settings
            });
            crate::signal::forward(crate::signal::link_changed(), tx, || Woke::Link);
            let mut sync = true;
            let pool = crate::db::pool::Pool::new(db_path);
            std::thread::sleep(Duration::from_secs(5));
            loop {
                // Due while the app was in the background: runs when it is not.
                crate::quiet::wait_until_awake();
                let cfg = Config::cached();
                let settings = AutoSync::of(&cfg);

                if std::mem::take(&mut sync)
                    && settings.on
                    && let Some(client) = subsonic_client(&cfg)
                    && let Ok(db) = pool.get()
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
                        Err(crate::remote::sync::SyncError::Cancelled) => {
                            log::info!("auto sync cancelled")
                        }
                        Err(e) => log::warn!("auto sync failed: {e}"),
                    }
                    on_state(false);
                }

                // The interval, while no link is up to say when the library
                // changed. A link coming up or going down plans it again.
                let linked = crate::remote::devices::linked();
                let due = (settings.on && settings.interval_mins > 0 && !linked)
                    .then(|| Instant::now() + Duration::from_secs(settings.interval_mins * 60));
                sync = loop {
                    let heard = match due {
                        Some(due) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
                        None => rx
                            .recv()
                            .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected),
                    };
                    match heard {
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break true,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                        Ok(Woke::Settings) if AutoSync::of(&Config::cached()) != settings => {
                            break true;
                        }
                        Ok(Woke::Link) if crate::remote::devices::linked() != linked => {
                            break false;
                        }
                        Ok(_) => {}
                    }
                };
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
