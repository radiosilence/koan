//! The server's covers kept on this device, and fetching them ahead.
//!
//! A remote library has no files to read art from, so each cover is an HTTP
//! round trip. Fetched covers are kept in the download cache's `artwork`
//! directory, one file per cover and size bucket, named by the server and its
//! id for the cover, so a sign-in elsewhere never shows another server's
//! sleeve; signing out removes them (`forget`). Every app reads through it
//! (`cover`), so a cover fetched once, by any page or by the preload, is a
//! file read from then on. The files are counted with downloads against the
//! cache limit and trimmed with them (`trim`).
//!
//! The preload walks every record whose cover is on the server only and
//! fetches it at the size grids draw, one at a time and only while no cover a
//! page asked for is in flight, so it never holds up what is on screen. It
//! sleeps on the engine's change signal while there is nothing to do, and
//! starts again when a sync finishes or the settings or the connection move.
//! It stops short of the cache limit, never pushing out a download, and
//! remembers the records the server has no cover for, so a pass asks the
//! server only about what it has not asked before. It
//! waits while the app is in the background, on a metered network, or while
//! the device saves power (`set_constrained`).

use std::path::PathBuf;
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

/// A cover's file for `id` at `bucket`, on the configured server. Ids are
/// the server's own and may be row numbers another server reuses, so the
/// file is named by a digest of the server and the id together. Bucket 0 is
/// the record's miss: the server had no cover for it.
fn file(cfg: &Config, id: &str, bucket: u32) -> PathBuf {
    let server = cfg.remote.url.trim_end_matches('/');
    let digest = md5::compute(format!("{server}\0{id}").as_bytes());
    directory(cfg).join(format!("{digest:x}-{bucket}"))
}

/// The bucket a miss is kept under.
const MISS: u32 = 0;

/// A kept cover for `id` at `size` or larger, smallest first: a client draws
/// a larger one scaled down at no cost but the bytes, and never waits for a
/// fetch it does not need. `None` for the original, which is not kept.
pub fn kept(cfg: &Config, id: &str, size: Option<u32>) -> Option<Vec<u8>> {
    let wanted = cover_bucket(size?);
    crate::remote::client::COVER_SIZES
        .into_iter()
        .filter(|b| *b >= wanted)
        .find_map(|b| {
            std::fs::read(file(cfg, id, b))
                .ok()
                .filter(|d| !d.is_empty())
        })
}

/// Whether the preload has nothing to ask about `id`: a cover is kept at
/// its size or larger, or the server said it has none. Reads no file.
fn settled(cfg: &Config, id: &str) -> bool {
    std::iter::once(MISS)
        .chain(
            crate::remote::client::COVER_SIZES
                .into_iter()
                .filter(|b| *b >= PRELOAD_SIZE),
        )
        .any(|b| file(cfg, id, b).exists())
}

/// Remembers that the server has no cover for `id`, for the preload alone:
/// a page asking still asks the server, and what it gets back replaces this.
fn keep_miss(cfg: &Config, id: &str) {
    let path = file(cfg, id, MISS);
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        std::fs::write(path, []).ok();
    }
}

/// Removes every kept cover and miss: the server they came from is no
/// longer this device's.
pub fn forget(cfg: &Config) {
    let dir = directory(cfg);
    if !dir.exists() {
        return;
    }
    let held = bytes(cfg);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => crate::helpers::cache_shrank(held),
        Err(e) => {
            log::warn!("artwork {}: {e}", dir.display());
            crate::helpers::measure_cache(cfg);
        }
    }
}

/// Keeps `data` as `id`'s cover at `size`. Written beside its name and
/// renamed, so a reader never sees half a file.
pub fn keep(cfg: &Config, id: &str, size: Option<u32>, data: &[u8]) {
    let Some(size) = size else { return };
    if data.is_empty() {
        return;
    }
    let path = file(cfg, id, cover_bucket(size));
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        log::warn!("artwork directory {}: {e}", dir.display());
        return;
    }
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
    /// A cover to keep: its id, the size asked for and its bytes.
    type Keeping = (String, Option<u32>, Vec<u8>);
    static WRITER: std::sync::OnceLock<crossbeam_channel::Sender<Keeping>> =
        std::sync::OnceLock::new();
    let writer = WRITER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<Keeping>();
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
/// Moves when a sync with the server finishes: the records may have changed.
static SYNCS: AtomicU64 = AtomicU64::new(0);

/// A sync with the server finished: look for records without a cover.
pub fn synced() {
    SYNCS.fetch_add(1, Ordering::Relaxed);
    crate::signal::engine_changed().bump();
}

/// Room left before the cache limit when the preload stops, so the next
/// download is not the one that crosses it.
const HEADROOM: u64 = 64 * 1024 * 1024;
/// The network is metered, or the device is saving power, as the app says.
static CONSTRAINED: AtomicBool = AtomicBool::new(false);

/// Whether the device is on a metered network or saving power: the preload
/// waits while it is. Told by the app, which knows; a change wakes the
/// preload.
pub fn set_constrained(constrained: bool) {
    if CONSTRAINED.swap(constrained, Ordering::Relaxed) != constrained {
        crate::signal::engine_changed().bump();
    }
}

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
    syncs: u64,
    enabled: bool,
    url: String,
    reachable: bool,
    constrained: bool,
    nudges: u64,
}

fn seen() -> Seen {
    let cfg = Config::cached();
    Seen {
        syncs: SYNCS.load(Ordering::Relaxed),
        enabled: cfg.remote.enabled && cfg.remote.preload_covers,
        url: cfg.remote.url.clone(),
        reachable: crate::helpers::server_unreachable().is_none(),
        constrained: CONSTRAINED.load(Ordering::Relaxed),
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
    seen.enabled
        && seen.reachable
        && !seen.constrained
        && !STOPPED.load(Ordering::Relaxed)
        && crate::quiet::awake()
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
    let limit = cfg.cache_limit_bytes();
    // A cache kept to nothing keeps no covers ahead either.
    if limit == Some(0) {
        return;
    }
    let missing: Vec<String> = ids.into_iter().filter(|id| !settled(&cfg, id)).collect();
    if missing.is_empty() {
        return;
    }
    let total = missing.len() as u64;
    log::info!("cover preload: {total} records to fetch");
    set_progress(Some(Preload { done: 0, total }));
    for (done, id) in (1..).zip(missing) {
        let cfg = Config::cached();
        if !may_run(&seen()) || !cfg.remote.preload_covers {
            break;
        }
        // Never into the room downloads need: past it, keeping one more
        // cover would push out music.
        if cfg
            .cache_limit_bytes()
            .is_some_and(|limit| crate::helpers::cache_bytes() + HEADROOM >= limit)
        {
            log::info!("cover preload: stopped short of the cache limit");
            break;
        }
        crate::remote::client::wait_for_idle_covers();
        match client.get_cover_art(&id, Some(PRELOAD_SIZE)) {
            Ok(data) if data.is_empty() => keep_miss(&cfg, &id),
            Ok(data) => keep(&cfg, &id, Some(PRELOAD_SIZE), &data),
            // The server says it has no such cover: remembered, so the next
            // pass does not ask again. Any other refusal (a restart, a
            // proxy's 502, a lapsed sign-in) says nothing about the record.
            Err(SubsonicError::Api { code: 70, .. }) => keep_miss(&cfg, &id),
            Err(e) => {
                log::info!("cover preload paused: {e}");
                break;
            }
        }
        set_progress(Some(Preload { done, total }));
    }
    set_progress(None);
    // Held to the cache limit with the downloads, the playback window kept.
    crate::remote::queue::cache_limit_changed();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dir: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.remote.cache_dir = Some(dir.to_path_buf());
        cfg
    }

    #[test]
    fn a_kept_cover_serves_its_size_and_every_smaller_one() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        assert_eq!(kept(&cfg, "al-1", Some(600)), None);
        keep(&cfg, "al-1", Some(600), b"six hundred");
        assert_eq!(
            kept(&cfg, "al-1", Some(600)).as_deref(),
            Some(&b"six hundred"[..])
        );
        assert_eq!(
            kept(&cfg, "al-1", Some(300)).as_deref(),
            Some(&b"six hundred"[..])
        );
        assert_eq!(kept(&cfg, "al-1", Some(1200)), None, "never scaled up");
        assert_eq!(kept(&cfg, "al-1", None), None, "the original is not kept");
        assert_eq!(kept(&cfg, "al-2", Some(600)), None, "another record's");
        keep(&cfg, "al-1", Some(300), b"three");
        assert_eq!(
            kept(&cfg, "al-1", Some(150)).as_deref(),
            Some(&b"three"[..]),
            "the smallest that fits"
        );
    }

    #[test]
    fn covers_are_kept_per_server() {
        let dir = tempfile::tempdir().unwrap();
        let mut one = config(dir.path());
        one.remote.url = "https://one.example".into();
        let mut two = config(dir.path());
        two.remote.url = "https://two.example".into();
        keep(&one, "al-12", Some(600), b"one's");
        assert_eq!(
            kept(&two, "al-12", Some(600)),
            None,
            "another server's al-12"
        );
        forget(&one);
        assert_eq!(
            kept(&one, "al-12", Some(600)),
            None,
            "forgotten at sign-out"
        );
    }

    #[test]
    fn a_miss_settles_the_preload_and_serves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        assert!(!settled(&cfg, "al-1"));
        keep_miss(&cfg, "al-1");
        assert!(settled(&cfg, "al-1"), "not asked again");
        assert_eq!(kept(&cfg, "al-1", Some(300)), None, "a page still asks");
        keep(&cfg, "al-2", Some(1200), b"large");
        assert!(settled(&cfg, "al-2"));
        keep(&cfg, "al-3", Some(300), b"small");
        assert!(!settled(&cfg, "al-3"), "smaller than the preload's");
    }

    #[test]
    fn nothing_is_kept_of_an_empty_answer_or_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        keep(&cfg, "al-1", Some(600), b"");
        keep(&cfg, "al-1", None, b"original");
        assert_eq!(bytes(&cfg), 0);
    }

    #[test]
    fn trimming_removes_the_oldest_until_enough_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        keep(&cfg, "old", Some(600), &[0; 100]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        keep(&cfg, "new", Some(600), &[0; 100]);
        assert_eq!(bytes(&cfg), 200);
        assert_eq!(trim(&cfg, 50), 100);
        assert!(kept(&cfg, "old", Some(600)).is_none());
        assert!(kept(&cfg, "new", Some(600)).is_some());
    }
}
