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

/// The most ids one `koanCovers` request names, and how many the preload
/// asks for at a time: about ten megabytes of covers at 600 px.
pub const BULK_MAX: usize = 200;
/// `koanCovers`' content type: a stream of frames (`frame`), not Subsonic's
/// XML or JSON, which a refusal still is.
pub const BULK_TYPE: &str = "application/vnd.koan.covers";

/// One record's answer in a `koanCovers` stream.
#[derive(Debug, PartialEq, Eq)]
pub enum BulkCover<T> {
    /// The cover, as JPEG.
    Cover(T),
    /// The record has no cover.
    None,
    /// The server does not hold this cover ready: ask `getCoverArt`.
    NotHeld,
}

const FRAME_COVER: u8 = 0;
const FRAME_NONE: u8 = 1;
const FRAME_NOT_HELD: u8 = 2;

/// One frame of a `koanCovers` stream: the id's length (2 bytes, big-endian)
/// and the id as asked, what the server has (1 byte: 0 a cover, 1 none, 2
/// not held), and the cover's length (4 bytes, big-endian) and bytes, empty
/// unless it is a cover.
pub fn frame(id: &str, cover: BulkCover<&[u8]>) -> Vec<u8> {
    let (kind, data): (u8, &[u8]) = match cover {
        BulkCover::Cover(data) => (FRAME_COVER, data),
        BulkCover::None => (FRAME_NONE, &[]),
        BulkCover::NotHeld => (FRAME_NOT_HELD, &[]),
    };
    let id = &id.as_bytes()[..id.len().min(u16::MAX as usize)];
    let mut out = Vec::with_capacity(7 + id.len() + data.len());
    out.extend_from_slice(&(id.len() as u16).to_be_bytes());
    out.extend_from_slice(id);
    out.push(kind);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// The next frame from a `koanCovers` stream, or `None` at its end. A kind
/// this client does not know is read past and answered `NotHeld`, so a
/// server may add kinds without breaking older clients.
pub fn read_frame(
    from: &mut impl std::io::Read,
) -> std::io::Result<Option<(String, BulkCover<Vec<u8>>)>> {
    let mut len = [0u8; 2];
    match from.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut id = vec![0u8; u16::from_be_bytes(len) as usize];
    from.read_exact(&mut id)?;
    let mut head = [0u8; 5];
    from.read_exact(&mut head)?;
    let size = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    let mut data = vec![0u8; size];
    from.read_exact(&mut data)?;
    let id = String::from_utf8_lossy(&id).into_owned();
    let cover = match head[0] {
        FRAME_COVER => BulkCover::Cover(data),
        FRAME_NONE => BulkCover::None,
        _ => BulkCover::NotHeld,
    };
    Ok(Some((id, cover)))
}

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

/// How many covers the preload fetches at once, one by one, where the
/// server has no `koanCovers` or does not hold a cover ready: enough to keep
/// a connection busy across round trips, few enough to leave the server room.
const WORKERS: usize = 6;

/// Why a pass stopped early.
#[derive(Debug)]
enum Halt {
    /// The setting went off, the person stopped it, the app went quiet or
    /// the network is constrained.
    NotNow,
    /// Keeping more would crowd out downloads.
    Full,
    /// The server refused or could not be reached.
    Failed(SubsonicError),
}

/// Whether the pass may fetch one more cover now.
fn may_fetch() -> Result<(), Halt> {
    let cfg = Config::cached();
    if !may_run(&seen()) || !cfg.remote.preload_covers {
        return Err(Halt::NotNow);
    }
    // Never into the room downloads need: past it, keeping one more cover
    // would push out music.
    if cfg
        .cache_limit_bytes()
        .is_some_and(|limit| crate::helpers::cache_bytes() + HEADROOM >= limit)
    {
        return Err(Halt::Full);
    }
    Ok(())
}

/// Fetches every missing cover once: from a server with `koanCovers`, as
/// many as it holds ready in a few streamed answers, and the rest, or every
/// one from another server, `WORKERS` at a time. Each request waits while a
/// page's covers are in flight. Gives up early when the setting goes off,
/// the person stops it, the server cannot be reached or the app goes quiet;
/// the next pass carries on from what is kept.
fn pass() {
    let cfg = Config::cached();
    let Some(client) = crate::helpers::subsonic_client(&cfg) else {
        return;
    };
    let ids = match crate::db::pool::shared().get() {
        Ok(db) => crate::db::queries::remote_cover_ids(&db.conn).unwrap_or_default(),
        Err(_) => return,
    };
    // A cache kept to nothing keeps no covers ahead either.
    if cfg.cache_limit_bytes() == Some(0) {
        return;
    }
    let missing: Vec<String> = ids.into_iter().filter(|id| !settled(&cfg, id)).collect();
    if missing.is_empty() {
        return;
    }
    let total = missing.len() as u64;
    let started = std::time::Instant::now();
    log::info!("cover preload: {total} records to fetch");
    let done = AtomicU64::new(0);
    let tick = || {
        let done = done.fetch_add(1, Ordering::Relaxed) + 1;
        set_progress(Some(Preload { done, total }));
    };
    set_progress(Some(Preload { done: 0, total }));
    let bulk = crate::remote::profile::probed(client.auth())
        .is_some_and(|p| p.offers(crate::remote::profile::COVERS));
    let (one_by_one, halted) = if bulk {
        in_bulk(&client, &cfg, missing, &tick)
    } else {
        (missing, None)
    };
    let in_bulk_done = done.load(Ordering::Relaxed);
    let halted = halted.or_else(|| one_at_a_time(&client, &cfg, one_by_one, &tick));
    let fetched = done.load(Ordering::Relaxed);
    let took = started.elapsed().as_secs_f64();
    log::info!(
        "cover preload: {fetched} of {total} in {took:.1} s ({:.0} a second), {in_bulk_done} in bulk{}",
        fetched as f64 / took.max(0.001),
        match &halted {
            None => String::new(),
            Some(Halt::NotNow) => "; paused".into(),
            Some(Halt::Full) => "; stopped short of the cache limit".into(),
            Some(Halt::Failed(e)) => format!("; paused: {e}"),
        }
    );
    set_progress(None);
    // Held to the cache limit with the downloads, the playback window kept.
    crate::remote::queue::cache_limit_changed();
}

/// `koanCovers` for `ids`, `BULK_MAX` at a time: keeps what the server holds
/// and says it has none of, and returns the ids it does not hold ready.
fn in_bulk(
    client: &SubsonicClient,
    cfg: &Config,
    ids: Vec<String>,
    tick: &dyn Fn(),
) -> (Vec<String>, Option<Halt>) {
    let mut rest = Vec::new();
    let mut chunks = ids.chunks(BULK_MAX);
    for chunk in chunks.by_ref() {
        if let Err(halt) = may_fetch() {
            rest.extend(chunks.flatten().cloned());
            return (rest, Some(halt));
        }
        crate::remote::client::wait_for_idle_covers();
        let answered = client.get_covers(chunk, PRELOAD_SIZE, &mut |id, cover| match cover {
            BulkCover::Cover(data) => {
                keep(cfg, &id, Some(PRELOAD_SIZE), &data);
                tick();
            }
            BulkCover::None => {
                keep_miss(cfg, &id);
                tick();
            }
            BulkCover::NotHeld => rest.push(id),
        });
        if let Err(e) = answered {
            // What it did not answer is fetched one by one, which says for
            // itself whether the server is there.
            log::info!("cover preload: koanCovers failed ({e}); fetching one by one");
            let answered: std::collections::HashSet<&str> =
                rest.iter().map(String::as_str).collect();
            let unanswered: Vec<String> = chunk
                .iter()
                .filter(|id| !answered.contains(id.as_str()) && !settled(cfg, id))
                .cloned()
                .collect();
            rest.extend(unanswered);
            rest.extend(chunks.flatten().cloned());
            return (rest, None);
        }
    }
    (rest, None)
}

/// `getCoverArt` for each of `ids`, `WORKERS` at a time.
fn one_at_a_time(
    client: &SubsonicClient,
    cfg: &Config,
    ids: Vec<String>,
    tick: &(dyn Fn() + Sync),
) -> Option<Halt> {
    if ids.is_empty() {
        return None;
    }
    let next = Mutex::new(ids.into_iter());
    let halted: Mutex<Option<Halt>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                loop {
                    if halted.lock().is_some() {
                        return;
                    }
                    if let Err(halt) = may_fetch() {
                        halted.lock().get_or_insert(halt);
                        return;
                    }
                    let Some(id) = next.lock().next() else {
                        return;
                    };
                    crate::remote::client::wait_for_idle_covers();
                    match client.get_cover_art_background(&id, Some(PRELOAD_SIZE)) {
                        Ok(data) if data.is_empty() => keep_miss(cfg, &id),
                        Ok(data) => keep(cfg, &id, Some(PRELOAD_SIZE), &data),
                        // The server says it has no such cover: remembered,
                        // so the next pass does not ask again. Any other
                        // refusal (a restart, a proxy's 502, a lapsed
                        // sign-in) says nothing about the record.
                        Err(SubsonicError::Api { code: 70, .. }) => keep_miss(cfg, &id),
                        Err(e) => {
                            halted.lock().get_or_insert(Halt::Failed(e));
                            return;
                        }
                    }
                    tick();
                }
            });
        }
    });
    halted.into_inner()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Frames read back as written, in order; a kind this client does not
    /// know reads as not held, and the stream's end as `None`.
    #[test]
    fn bulk_frames_read_back_as_written() {
        let mut stream = frame("al-1", BulkCover::Cover(b"jpeg"));
        stream.extend(frame("al-2", BulkCover::None));
        stream.extend(frame("al-3", BulkCover::NotHeld));
        let mut later = frame("al-4", BulkCover::None);
        later[2 + 4] = 9;
        stream.extend(later);
        let mut read = &stream[..];
        let mut got = Vec::new();
        while let Some(f) = read_frame(&mut read).unwrap() {
            got.push(f);
        }
        assert_eq!(
            got,
            vec![
                ("al-1".to_string(), BulkCover::Cover(b"jpeg".to_vec())),
                ("al-2".to_string(), BulkCover::None),
                ("al-3".to_string(), BulkCover::NotHeld),
                ("al-4".to_string(), BulkCover::NotHeld),
            ]
        );
        let cut = &stream[..stream.len() - 3];
        let mut read = cut;
        let mut err = None;
        loop {
            match read_frame(&mut read) {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        assert!(
            err.is_some(),
            "a stream cut inside a frame is an error, not its end"
        );
    }

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
