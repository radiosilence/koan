//! The server's covers kept on this device, and fetching them ahead.
//!
//! A remote library has no files to read art from, so each cover is an HTTP
//! round trip. Fetched covers are kept in the download cache's `artwork`
//! directory, one file per cover and size bucket, named by the server's own
//! id for it, which never names another record. Every app reads through it
//! (`cover`), so a cover fetched once, by any page or by the preload, is a
//! file read from then on. The files are counted with downloads against the
//! cache limit and trimmed with them (`trim`).
//!
//! The preload walks every record whose cover is on the server only and
//! fetches it at the size grids draw, one at a time and only while no cover a
//! page asked for is in flight, so it never holds up what is on screen. It
//! sleeps on the engine's change signal while there is nothing to do, and
//! starts again when the library, the settings or the connection move.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::config::Config;
use crate::remote::client::{SubsonicClient, SubsonicError, cover_bucket};

/// The size the preload fetches: what every app's grid and lists draw at.
pub const PRELOAD_SIZE: u32 = 600;

/// The directory covers are kept in, inside the download cache.
pub fn directory(cfg: &Config) -> PathBuf {
    cfg.cache_dir().join("artwork")
}

/// A cover's file for `id` at `bucket`. The id is the server's, which may
/// hold any character, so the file is named by its digest.
fn file(dir: &Path, id: &str, bucket: u32) -> PathBuf {
    dir.join(format!("{:x}-{bucket}", md5::compute(id.as_bytes())))
}

/// A kept cover for `id` at `size` or larger, smallest first: a client draws
/// a larger one scaled down at no cost but the bytes, and never waits for a
/// fetch it does not need. `None` for the original, which is not kept.
pub fn kept(cfg: &Config, id: &str, size: Option<u32>) -> Option<Vec<u8>> {
    let wanted = cover_bucket(size?);
    let dir = directory(cfg);
    crate::remote::client::COVER_SIZES
        .into_iter()
        .filter(|b| *b >= wanted)
        .find_map(|b| std::fs::read(file(&dir, id, b)).ok())
        .filter(|data| !data.is_empty())
}

/// Keeps `data` as `id`'s cover at `size`. Written beside its name and
/// renamed, so a reader never sees half a file.
pub fn keep(cfg: &Config, id: &str, size: Option<u32>, data: &[u8]) {
    let Some(size) = size else { return };
    if data.is_empty() {
        return;
    }
    let dir = directory(cfg);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("artwork directory {}: {e}", dir.display());
        return;
    }
    let path = file(&dir, id, cover_bucket(size));
    let tmp = path.with_extension("part");
    let written = std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, &path));
    match written {
        Ok(()) => crate::helpers::cache_grew(data.len() as u64),
        Err(e) => {
            std::fs::remove_file(&tmp).ok();
            log::warn!("artwork {}: {e}", path.display());
        }
    }
}

/// `keep`, on the store's own writer thread, for a caller on an async runtime
/// or a page's lane that should not wait on the disk.
pub fn keep_later(id: String, size: Option<u32>, data: Vec<u8>) {
    static WRITER: std::sync::OnceLock<crossbeam_channel::Sender<(String, Option<u32>, Vec<u8>)>> =
        std::sync::OnceLock::new();
    let writer = WRITER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<(String, Option<u32>, Vec<u8>)>();
        std::thread::Builder::new()
            .name("koan-cover-store".into())
            .spawn(move || {
                for (id, size, data) in rx {
                    keep(&Config::cached(), &id, size, &data);
                }
            })
            .ok();
        tx
    });
    writer.send((id, size, data)).ok();
}

/// `id`'s cover at `size`: kept, or fetched through `client` and kept.
pub fn cover(
    client: &SubsonicClient,
    id: &str,
    size: Option<u32>,
) -> Result<Vec<u8>, SubsonicError> {
    let cfg = Config::cached();
    if let Some(data) = kept(&cfg, id, size) {
        return Ok(data);
    }
    let data = client.get_cover_art(id, size)?;
    keep(&cfg, id, size, &data);
    Ok(data)
}

/// The kept covers, oldest first by when they were last written, with their
/// sizes.
fn kept_files(cfg: &Config) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut files: Vec<_> = std::fs::read_dir(directory(cfg))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_none())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), meta.len(), meta.modified().ok()?))
        })
        .collect();
    files.sort_by_key(|(_, _, at)| *at);
    files
}

/// What the kept covers take on disk.
pub fn bytes(cfg: &Config) -> u64 {
    kept_files(cfg).iter().map(|(_, size, _)| size).sum()
}

/// Removes kept covers, oldest first, until `over` bytes are gone or none is
/// left. Returns the bytes freed.
pub fn trim(cfg: &Config, over: u64) -> u64 {
    let mut freed = 0;
    for (path, size, _) in kept_files(cfg) {
        if freed >= over {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            freed += size;
        }
    }
    freed
}

// --- Preload -------------------------------------------------------------

/// How far the preload has got through the records it found missing a
/// cover, while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preload {
    pub done: u64,
    pub total: u64,
}

static PROGRESS: Mutex<Option<Preload>> = Mutex::new(None);
/// Stopped by the person, until the setting is turned on again or the app
/// restarts.
static STOPPED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
/// Moves when the preload should look again: the person turned it back on.
static NUDGES: AtomicU64 = AtomicU64::new(0);

/// The preload's progress while it runs; `None` otherwise.
pub fn progress() -> Option<Preload> {
    *PROGRESS.lock()
}

/// Stops the running preload, keeping what it fetched.
pub fn stop() {
    STOPPED.store(true, Ordering::Relaxed);
    crate::signal::engine_changed().bump();
}

/// The setting was turned on, or the person asked for the covers again:
/// look for missing ones now.
pub fn resume() {
    STOPPED.store(false, Ordering::Relaxed);
    NUDGES.fetch_add(1, Ordering::Relaxed);
    crate::signal::engine_changed().bump();
}

fn set_progress(progress: Option<Preload>) {
    let mut held = PROGRESS.lock();
    if *held != progress {
        *held = progress;
        drop(held);
        crate::signal::engine_changed().bump();
    }
}

/// Starts the preload's thread, once per process. It waits on the engine's
/// change signal, so it costs nothing while there is nothing to fetch.
pub fn start() {
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::Builder::new()
        .name("koan-cover-preload".into())
        .spawn(run)
        .ok();
}

/// What a pass looks at to decide whether to look again: the library, the
/// settings that matter, the connection and a nudge.
#[derive(PartialEq, Eq)]
struct Seen {
    library: u64,
    enabled: bool,
    url: String,
    reachable: bool,
    nudges: u64,
}

fn seen() -> Seen {
    let cfg = Config::cached();
    Seen {
        library: crate::db::queries::library_version(),
        enabled: cfg.remote.enabled && cfg.remote.preload_covers,
        url: cfg.remote.url.clone(),
        reachable: crate::helpers::server_unreachable().is_none(),
        nudges: NUDGES.load(Ordering::Relaxed),
    }
}

fn run() {
    let signal = crate::signal::engine_changed();
    let mut last: Option<Seen> = None;
    loop {
        // Work nobody asked for waits while the app is in the background
        // (`quiet`), however the signal moves.
        crate::quiet::wait_until_awake();
        let generation = signal.generation();
        let now = seen();
        if last.as_ref() != Some(&now) && may_run(&now) {
            pass();
        }
        last = Some(now);
        signal.wait(generation);
    }
}

fn may_run(seen: &Seen) -> bool {
    seen.enabled && seen.reachable && !STOPPED.load(Ordering::Relaxed) && crate::quiet::awake()
}

/// Fetches every missing cover once. Gives up early when the setting goes
/// off, the person stops it, the server cannot be reached or the app goes
/// quiet; the next pass carries on from what is kept.
fn pass() {
    let cfg = Config::cached();
    let Some(client) = crate::helpers::subsonic_client(&cfg) else {
        return;
    };
    let ids = match crate::db::pool::shared().get() {
        Ok(db) => crate::db::queries::remote_cover_ids(&db.conn).unwrap_or_default(),
        Err(_) => return,
    };
    let missing: Vec<String> = ids
        .into_iter()
        .filter(|id| kept(&cfg, id, Some(PRELOAD_SIZE)).is_none())
        .collect();
    if missing.is_empty() {
        return;
    }
    let total = missing.len() as u64;
    log::info!("cover preload: {total} records to fetch");
    let mut done = 0;
    set_progress(Some(Preload { done, total }));
    for id in missing {
        let cfg = Config::cached();
        if !may_run(&seen()) || !cfg.remote.preload_covers {
            break;
        }
        crate::remote::client::wait_for_idle_covers();
        match client.get_cover_art(&id, Some(PRELOAD_SIZE)) {
            Ok(data) => keep(&cfg, &id, Some(PRELOAD_SIZE), &data),
            // The server has none for this record: nothing to keep.
            Err(SubsonicError::Api { .. } | SubsonicError::BadResponse) => {}
            Err(e) => {
                log::info!("cover preload paused: {e}");
                break;
            }
        }
        done += 1;
        set_progress(Some(Preload { done, total }));
    }
    set_progress(None);
    // Held to the cache limit with the downloads, the playback window kept.
    crate::remote::queue::cache_limit_changed();
}
