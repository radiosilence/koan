//! Album covers for the web UI, share pages and Subsonic: an image beside the
//! tracks or the art embedded in them, resized to one of a few sizes, encoded
//! as JPEG, and kept.
//!
//! Reading embedded art means opening the audio file and parsing its tags, and
//! the art itself is often megabytes; doing that per tile made an albums grid
//! pull tens of megabytes and seconds of server time. Resized covers are kept
//! on disk under the config directory, keyed by the source file's path, size
//! and mtime so a re-tag or a replaced image is a new key. An album with no
//! art is remembered too, so a grid of them does not reopen every file on
//! every visit. The directory is held to `BUDGET`: every so many writes the
//! covers read least recently are removed, and so are the stale keys a re-tag
//! leaves and temporary files an interrupted write left behind.
//!
//! Nothing is held in memory: the kernel's page cache keeps hot files for
//! free and gives the memory back under pressure, where a cache of our own
//! would count against the process for as long as it runs. Decoding is the
//! expensive part — a full-size cover is tens of megabytes of pixels — so it
//! runs on a few threads of its own. A grid opening asks for hundreds of covers
//! at once; on the blocking pool each request would decode on its own thread,
//! and the allocator keeps what every one of those threads used.
//!
//! What the cache holds, what the warm is doing and what the last sweep
//! removed are reported (`usage`, `report`) for the web UI's Library page and
//! GraphQL's `serverCaches`, with `subscribe` ringing as they move.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use axum::body::Bytes;

use koan_core::db::queries::TrackRow;

/// The sizes a cover is served at, the ones every koan client asks for
/// (`koan_core::remote::client::COVER_SIZES`). A request is rounded up to
/// one of these, which bounds what the cache can hold per album.
pub(crate) const SIZES: [u32; 4] = koan_core::remote::client::COVER_SIZES;
/// Grid tiles, at 2x: the web UI's and the desktop app's.
pub(crate) const GRID: u32 = 600;
/// Headers, the player, and link previews.
pub(crate) const LARGE: u32 = 1200;
/// What the warm makes for every album: the desktop's and the web UI's grid,
/// and the phones' thumbnails. The Apple apps' 1200 is made as asked: a
/// library's worth of them would outgrow `BUDGET`.
pub(crate) const WARMED: [u32; 2] = [300, GRID];

/// How many of an album's tracks are opened looking for art before giving up.
const TRACKS_TRIED: usize = 3;
const JPEG_QUALITY: u8 = 85;

pub(crate) fn snap(size: Option<u32>) -> u32 {
    let size = size.unwrap_or(LARGE);
    SIZES
        .into_iter()
        .find(|s| *s >= size)
        .unwrap_or(SIZES[SIZES.len() - 1])
}

/// The threads covers are decoded and resized on.
static DECODE: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 4));
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("koan-covers-{i}"))
        .build()
        .expect("cover threads")
});

/// What the cover directory may hold before the least recently read go.
pub(crate) const BUDGET: u64 = 4 * 1024 * 1024 * 1024;
/// Writes between looks at the directory's size.
const SWEEP_EVERY: u64 = 256;
/// A hit moves a cover's mtime, which is its place in line for eviction, no
/// more than once in this long.
const TOUCH_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
/// Where the newest eviction applied is kept, beside the covers. A dot file,
/// which the sweep leaves alone.
const EVICTED: &str = ".evicted";
/// How often the warm looks for albums without a cover ready, besides when
/// the library changes: albums added by a scan evict nothing.
pub(crate) const WARM_EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// A temporary file older than this was left by a write that never finished.
const TMP_LEFT: std::time::Duration = std::time::Duration::from_secs(60 * 60);

pub struct Covers {
    dir: PathBuf,
    budget: u64,
    writes: std::sync::atomic::AtomicU64,
    sweeping: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The newest `art_evictions` entry already applied to the album
    /// entries; `None` until the first look, which only reads where the
    /// library has got to.
    evicted: parking_lot::Mutex<Option<i64>>,
    /// Wakes the warm when the library has changed.
    warm: (parking_lot::Mutex<bool>, parking_lot::Condvar),
    report: std::sync::Arc<parking_lot::Mutex<Report>>,
    /// Moves whenever the report does, or covers are cleared.
    changed: std::sync::Arc<tokio::sync::watch::Sender<u64>>,
}

/// What the warm is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarmState {
    /// Not started: an app sharing its own library makes covers as asked.
    Off,
    Running,
    /// Stopped with the directory at its budget; requests still write.
    AtBudget,
    Idle,
}

/// The warm's pass in progress, or its last.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Warm {
    pub state: WarmState,
    /// Records looked at so far, of how many there are with files.
    pub done: usize,
    pub total: usize,
    /// Records the pass has made covers for, the rest being ready already.
    pub made: usize,
    pub started: Option<std::time::SystemTime>,
    pub finished: Option<std::time::SystemTime>,
}

impl Warm {
    /// When the next pass runs unless the library changes first.
    pub(crate) fn next(&self) -> Option<std::time::SystemTime> {
        match self.state {
            WarmState::Idle | WarmState::AtBudget => self.finished.map(|t| t + WARM_EVERY),
            WarmState::Off | WarmState::Running => None,
        }
    }
}

/// What a sweep held the directory to its budget by removing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sweep {
    pub at: std::time::SystemTime,
    pub removed: usize,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Report {
    pub warm: Warm,
    /// The last sweep that found the directory over its budget, since start.
    pub sweep: Option<Sweep>,
}

/// The covers kept at one size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SizeUsage {
    pub size: u32,
    /// Covers kept, remembered misses included.
    pub covers: usize,
    pub bytes: u64,
    /// Records or files remembered to have no art.
    pub misses: usize,
}

/// What the cover directory holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    pub bytes: u64,
    pub budget: u64,
    pub sizes: Vec<SizeUsage>,
    /// Records with files that lack a cover ready at one of `WARMED`.
    pub unwarmed: usize,
}

impl Usage {
    pub(crate) fn covers(&self) -> usize {
        self.sizes.iter().map(|s| s.covers).sum()
    }

    pub(crate) fn misses(&self) -> usize {
        self.sizes.iter().map(|s| s.misses).sum()
    }
}

impl Covers {
    pub fn new(dir: PathBuf) -> Self {
        let evicted = std::fs::read_to_string(dir.join(EVICTED))
            .ok()
            .and_then(|s| s.trim().parse().ok());
        Self {
            dir,
            budget: BUDGET,
            writes: Default::default(),
            sweeping: Default::default(),
            evicted: parking_lot::Mutex::new(evicted),
            warm: Default::default(),
            report: std::sync::Arc::new(parking_lot::Mutex::new(Report {
                warm: Warm {
                    state: WarmState::Off,
                    done: 0,
                    total: 0,
                    made: 0,
                    started: None,
                    finished: None,
                },
                sweep: None,
            })),
            changed: std::sync::Arc::new(tokio::sync::watch::Sender::new(0)),
        }
    }

    pub(crate) fn report(&self) -> Report {
        *self.report.lock()
    }

    /// Wakes whenever the report moves or the covers are cleared.
    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn ring(&self) {
        ring(&self.changed);
    }

    fn set_warm(&self, change: impl FnOnce(&mut Warm)) {
        change(&mut self.report.lock().warm);
        self.ring();
    }

    /// What the directory holds, by size, and how many of the library's
    /// records lack a warmed cover. Lists the directory: blocking.
    pub(crate) fn usage(&self, conn: &rusqlite::Connection) -> Usage {
        let mut sizes: Vec<SizeUsage> = SIZES
            .into_iter()
            .map(|size| SizeUsage {
                size,
                ..Default::default()
            })
            .collect();
        let mut bytes = 0;
        // Records with an entry at each warmed size, by bit.
        let mut warmed: std::collections::HashMap<i64, u8> = Default::default();
        if let Ok(read) = std::fs::read_dir(&self.dir) {
            for entry in read.flatten() {
                let len = entry.metadata().map_or(0, |m| m.len());
                bytes += len;
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some((album, size)) = parse(&name) else {
                    continue;
                };
                if let Some(at) = sizes.iter_mut().find(|s| s.size == size) {
                    at.covers += 1;
                    at.bytes += len;
                    at.misses += usize::from(len == 0);
                }
                if let (Some(album), Some(bit)) = (album, WARMED.iter().position(|s| *s == size)) {
                    *warmed.entry(album).or_default() |= 1 << bit;
                }
            }
        }
        let all = (1u8 << WARMED.len()) - 1;
        let unwarmed = conn
            .prepare(
                "SELECT DISTINCT album_id FROM tracks
                  WHERE path IS NOT NULL AND album_id IS NOT NULL",
            )
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, i64>(0)).map(|ids| {
                    ids.flatten()
                        .filter(|id| warmed.get(id) != Some(&all))
                        .count()
                })
            })
            .unwrap_or(0);
        Usage {
            bytes,
            budget: self.budget,
            sizes,
            unwarmed,
        }
    }

    /// Removes every cover kept, misses included, and wakes the warm to make
    /// the grid sizes again. What was removed: files and bytes. Blocking.
    pub(crate) fn clear(&self) -> (usize, u64) {
        let (mut files, mut bytes) = (0, 0);
        if let Ok(read) = std::fs::read_dir(&self.dir) {
            for entry in read.flatten() {
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let len = entry.metadata().map_or(0, |m| m.len());
                if std::fs::remove_file(entry.path()).is_ok() {
                    files += 1;
                    bytes += len;
                }
            }
        }
        log::info!("covers: cleared {files} covers ({} MB)", bytes >> 20);
        self.ring();
        self.poke();
        (files, bytes)
    }

    /// Starts a warm pass now rather than at the next change or interval.
    /// Whether a warm runs to start.
    pub(crate) fn warm_now(&self) -> bool {
        if self.report().warm.state == WarmState::Off {
            return false;
        }
        self.poke();
        true
    }

    /// An album's cover at `size` from the album's own entry: one small file
    /// read, no music file opened, listed or even looked at. Without an entry,
    /// `make` finds it (the tracks' art, as `cover` does) and its answer is
    /// kept for the album, no art included; `None` from `make` says it could
    /// not tell, and nothing is kept. Blocking.
    pub(crate) fn album_cover(
        &self,
        album: i64,
        size: u32,
        make: impl FnOnce() -> Option<Option<Bytes>>,
    ) -> Option<Bytes> {
        let key = album_key(album, size);
        if let Some(hit) = self.read_disk(&key) {
            return hit;
        }
        let art = make()?;
        self.write_disk(&key, art.as_deref());
        art
    }

    /// Drops the entries of albums the library has named in `art_evictions`
    /// since the last look (deleted, or rescanned with perhaps another image
    /// beside them), so their next request reads the files again. One indexed
    /// read when nothing has changed.
    pub(crate) fn forget_evicted(&self, conn: &rusqlite::Connection) {
        let Ok(newest) =
            conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM art_evictions", [], |r| {
                r.get::<_, i64>(0)
            })
        else {
            return;
        };
        let mut seen = self.evicted.lock();
        let Some(since) = *seen else {
            // No entries kept yet: nothing to forget.
            *seen = Some(newest);
            self.mark(newest);
            return;
        };
        if newest <= since {
            return;
        }
        if let Ok(mut stmt) =
            conn.prepare("SELECT DISTINCT id FROM art_evictions WHERE kind = 'album' AND seq > ?1")
            && let Ok(ids) = stmt.query_map([since], |r| r.get::<_, i64>(0))
        {
            for id in ids.flatten() {
                for size in SIZES {
                    let _ = std::fs::remove_file(self.dir.join(album_key(id, size)));
                }
            }
        }
        *seen = Some(newest);
        self.mark(newest);
        drop(seen);
        self.poke();
    }

    fn mark(&self, seq: i64) {
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(self.dir.join(EVICTED), seq.to_string());
    }

    /// Wakes the warm for another pass.
    fn poke(&self) {
        *self.warm.0.lock() = true;
        self.warm.1.notify_one();
    }

    /// Keeps every album's grid-size cover ready, on a thread of its own:
    /// at start, after the library changes, and every so often, one album at
    /// a time in the order its files lie in the library, so the disk reads
    /// in order rather than seeking between requests' files.
    pub fn warm(self: &std::sync::Arc<Self>, pool: std::sync::Arc<koan_core::db::pool::Pool>) {
        let covers = self.clone();
        let started = std::thread::Builder::new()
            .name("koan-covers-warm".into())
            .spawn(move || {
                prune(&covers.dir);
                covers.set_warm(|w| w.state = WarmState::Idle);
                loop {
                    covers.warm_pass(&pool);
                    let mut poked = covers.warm.0.lock();
                    if !*poked {
                        covers.warm.1.wait_for(&mut poked, WARM_EVERY);
                    }
                    *poked = false;
                }
            });
        if let Err(e) = started {
            log::warn!("covers: no warm thread: {e}");
        }
    }

    fn warm_pass(&self, pool: &koan_core::db::pool::Pool) {
        // Warmed covers are the least recently read, so past the budget the
        // sweep would take them first and the next pass would make them
        // again: the warm stops at the budget, and requests still write.
        let mut used = dir_bytes(&self.dir);
        if used >= self.budget {
            self.set_warm(|w| {
                w.state = WarmState::AtBudget;
                w.finished = Some(std::time::SystemTime::now());
            });
            return;
        }
        let albums: Vec<i64> = {
            let Ok(db) = pool.get() else { return };
            self.forget_evicted(&db.conn);
            let Ok(mut stmt) = db.conn.prepare(
                "SELECT album_id FROM tracks
                  WHERE path IS NOT NULL AND album_id IS NOT NULL
                  GROUP BY album_id ORDER BY MIN(path)",
            ) else {
                return;
            };
            stmt.query_map([], |r| r.get(0))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
        };
        let total = albums.len();
        self.set_warm(|w| {
            *w = Warm {
                state: WarmState::Running,
                done: 0,
                total,
                made: 0,
                started: Some(std::time::SystemTime::now()),
                finished: w.finished,
            }
        });
        let mut made = 0usize;
        let mut state = WarmState::Idle;
        let mut finished = true;
        for (done, album) in albums.into_iter().enumerate() {
            let wanted: Vec<u32> = WARMED
                .into_iter()
                .filter(|size| !self.dir.join(album_key(album, *size)).exists())
                .collect();
            if wanted.is_empty() {
                // Ready already: counted without a ring per record, which
                // would wake every follower thousands of times a pass.
                self.report.lock().warm.done = done + 1;
                continue;
            }
            let tracks = match pool.get() {
                Ok(db) => {
                    koan_core::db::queries::tracks_for_album(&db.conn, album).unwrap_or_default()
                }
                Err(_) => {
                    finished = false;
                    break;
                }
            };
            // Files that cannot be read (a volume not mounted yet) say
            // nothing about whether the album has art: nothing is kept, and
            // the next pass tries again.
            if !readable(&tracks) {
                continue;
            }
            // The first source that makes a cover, read once for every size
            // wanted.
            let mut sources = self.source_arts(&tracks);
            let mut source = None;
            for size in wanted {
                let art = DECODE.install(|| match &source {
                    Some(bytes) => encode(bytes, size),
                    None => sources.find_map(|bytes| {
                        let art = encode(&bytes, size)?;
                        source = Some(bytes);
                        Some(art)
                    }),
                });
                used += art.as_ref().map_or(0, |a| a.len() as u64);
                self.write_disk(&album_key(album, size), art.as_deref());
            }
            made += 1;
            self.set_warm(|w| {
                w.done = done + 1;
                w.made = made;
            });
            if used >= self.budget {
                log::info!("covers: the cover directory has reached its budget; warming stops");
                state = WarmState::AtBudget;
                break;
            }
        }
        // A pass the database stopped keeps the count it reached.
        self.set_warm(|w| {
            w.state = state;
            if state == WarmState::Idle && finished {
                w.done = total;
            }
            w.finished = Some(std::time::SystemTime::now());
        });
        if made > 0 {
            log::info!("covers: {made} albums' covers made ready");
        }
    }

    /// Covers kept under koan's config directory.
    pub fn in_config_dir() -> Self {
        Self::new(koan_core::config::config_dir().join("covers"))
    }

    /// A cover from the first of `tracks` that has art, at `size` (one of
    /// `SIZES`), as JPEG. An image beside a track comes before the art
    /// embedded in it (see `koan_core::index::folder_art`). Blocking: call it
    /// off the async workers.
    pub(crate) fn cover(&self, tracks: &[TrackRow], size: u32) -> Option<Bytes> {
        let sources: Vec<(Source, String)> = tracks
            .iter()
            .filter_map(|t| crate::subsonic::track_file_path(t).map(PathBuf::from))
            .take(TRACKS_TRIED)
            .map(|p| {
                let source = match koan_core::index::folder_art::folder_cover(&p) {
                    Some(image) => Source::Image(image),
                    None => Source::Embedded(p),
                };
                let key = key(source.path(), size);
                (source, key)
            })
            .collect();
        // Keyed on the first candidate: that is the file whose art is shown
        // whenever it has any. An image added beside it is a new key, so a
        // remembered miss does not outlive it.
        let (_, first_key) = sources.first()?;
        if let Some(hit) = self.read_disk(first_key) {
            return hit;
        }
        // Files that cannot be read now say nothing: no miss is kept.
        if !readable(tracks) {
            return None;
        }
        let art = DECODE.install(|| {
            sources.iter().find_map(|(source, _)| {
                let bytes = match source {
                    Source::Image(p) => std::fs::read(p).ok(),
                    Source::Embedded(p) => koan_core::index::metadata::extract_cover_art(p),
                };
                bytes.and_then(|bytes| encode(&bytes, size))
            })
        });
        self.write_disk(first_key, art.as_deref());
        art.map(Bytes::from)
    }

    /// The art of each of the first of `tracks` that has any, as found, read
    /// as asked for: an image beside it, or the art embedded in it.
    /// Unresized.
    fn source_arts<'a>(&self, tracks: &'a [TrackRow]) -> impl Iterator<Item = Vec<u8>> + 'a {
        tracks
            .iter()
            .filter_map(|t| crate::subsonic::track_file_path(t).map(PathBuf::from))
            .take(TRACKS_TRIED)
            .filter_map(|p| match koan_core::index::folder_art::folder_cover(&p) {
                Some(image) => std::fs::read(image).ok(),
                None => koan_core::index::metadata::extract_cover_art(&p),
            })
    }

    /// `Some(None)` is a remembered miss. A hit is marked as read, so the
    /// covers in use are the last evicted.
    fn read_disk(&self, key: &str) -> Option<Option<Bytes>> {
        use std::io::Read as _;
        let mut file = std::fs::File::open(self.dir.join(key)).ok()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        let now = std::time::SystemTime::now();
        if file
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > TOUCH_AFTER)
        {
            let _ = file.set_modified(now);
        }
        Some((!bytes.is_empty()).then(|| Bytes::from(bytes)))
    }

    /// An empty file records that there is no art. Failure to write is only a
    /// lost cache entry. Requests that miss the same cover together each write
    /// a temporary file of their own, so the one renamed into place last is
    /// whole rather than another's half-written file.
    fn write_disk(&self, key: &str, art: Option<&[u8]>) {
        use std::sync::atomic::Ordering;
        let _ = std::fs::create_dir_all(&self.dir);
        let n = self.writes.fetch_add(1, Ordering::Relaxed);
        if n.is_multiple_of(SWEEP_EVERY) && !self.sweeping.swap(true, Ordering::AcqRel) {
            let (dir, budget, sweeping) = (self.dir.clone(), self.budget, self.sweeping.clone());
            let (report, changed) = (self.report.clone(), self.changed.clone());
            std::thread::spawn(move || {
                if let Some((removed, bytes)) = sweep(&dir, budget) {
                    report.lock().sweep = Some(Sweep {
                        at: std::time::SystemTime::now(),
                        removed,
                        bytes,
                    });
                    ring(&changed);
                }
                sweeping.store(false, Ordering::Release);
            });
        }
        let tmp = self
            .dir
            .join(format!("{key}.{}-{n}.tmp", std::process::id()));
        if std::fs::write(&tmp, art.unwrap_or_default()).is_ok()
            && std::fs::rename(&tmp, self.dir.join(key)).is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Removes what no request will read again: covers kept at sizes no longer
/// served, and files in no scheme of this version's. They count against the
/// budget like any other, and an older version's can fill it.
fn prune(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let (mut files, mut bytes) = (0usize, 0u64);
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !current(&name) {
            let len = entry.metadata().map_or(0, |m| m.len());
            if std::fs::remove_file(entry.path()).is_ok() {
                files += 1;
                bytes += len;
            }
        }
    }
    if files > 0 {
        log::info!(
            "covers: removed {files} covers no longer served ({} MB)",
            bytes >> 20
        );
    }
}

fn ring(changed: &tokio::sync::watch::Sender<u64>) {
    changed.send_modify(|n| *n = n.wrapping_add(1));
}

/// Whether `name` is something this version keeps: a dot file, a write in
/// progress, or a cover at a size it serves, kept by album or by source.
fn current(name: &str) -> bool {
    name.starts_with('.') || name.ends_with(".tmp") || parse(name).is_some()
}

/// A cover's record, when it is kept by record, and size; `None` for
/// anything that is not a cover at a size this version serves.
fn parse(name: &str) -> Option<(Option<i64>, u32)> {
    let (head, size) = name.strip_suffix(".jpg")?.rsplit_once('-')?;
    let size = size.parse().ok().filter(|size| SIZES.contains(size))?;
    match head.strip_prefix("album-") {
        Some(id) => Some((Some(id.parse().ok()?), size)),
        None => (head.len() == 32 && head.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some((None, size)),
    }
}

/// What the covers in `dir` take up.
fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|read| {
            read.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Whether any of `tracks`' files can be read: when none can (a volume not
/// mounted, a download not landed), not finding art says nothing about the
/// album, and is not remembered.
pub(crate) fn readable(tracks: &[TrackRow]) -> bool {
    tracks
        .iter()
        .filter_map(crate::subsonic::track_file_path)
        .take(TRACKS_TRIED)
        .any(|p| Path::new(&p).exists())
}

/// Hold `dir` to `budget` bytes, removing the covers read least recently
/// until it is a tenth under, and every temporary file older than `TMP_LEFT`.
/// Covers removed and their bytes, when it was over the budget.
fn sweep(dir: &Path, budget: u64) -> Option<(usize, u64)> {
    let now = std::time::SystemTime::now();
    let read = std::fs::read_dir(dir).ok()?;
    let mut kept: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().unwrap_or(now);
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.extension().is_some_and(|e| e == "tmp") {
            if now.duration_since(modified).unwrap_or_default() > TMP_LEFT {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        kept.push((modified, meta.len(), path));
    }
    let mut total: u64 = kept.iter().map(|(_, len, _)| len).sum();
    if total <= budget {
        return None;
    }
    kept.sort_by_key(|(modified, _, _)| *modified);
    let target = budget - budget / 10;
    let (mut removed, mut bytes) = (0, 0);
    for (_, len, path) in kept {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
            removed += 1;
            bytes += len;
        }
    }
    Some((removed, bytes))
}

/// Where a cover is read from.
enum Source {
    /// An image file beside the track.
    Image(PathBuf),
    /// The art embedded in the track itself.
    Embedded(PathBuf),
}

impl Source {
    fn path(&self) -> &Path {
        match self {
            Self::Image(p) | Self::Embedded(p) => p,
        }
    }
}

/// The source's path, size and mtime, and the size asked for: a re-tagged or
/// replaced file is a different key, and stale entries are simply never read.
fn key(path: &Path, size: u32) -> String {
    let meta = std::fs::metadata(path).ok();
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let len = meta.map_or(0, |m| m.len());
    let digest = md5::compute(format!("{}\0{len}\0{mtime}", path.display()));
    format!("{digest:x}-{size}.jpg")
}

/// An album's own entry: what `album_cover` reads before any file.
fn album_key(album: i64, size: u32) -> String {
    format!("album-{album}-{size}.jpg")
}

/// Fit within `size` pixels on a side and encode as JPEG. A JPEG already
/// within bounds is passed through untouched.
fn encode(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    use image::GenericImageView as _;
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = img.dimensions();
    if w.max(h) <= size && bytes.starts_with(&[0xFF, 0xD8]) {
        return Some(bytes.to_vec());
    }
    let img = if w.max(h) > size {
        img.thumbnail(size, size)
    } else {
        img
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&img.to_rgb8())
        .ok()?;
    Some(out)
}

/// The ensō on a `surface` square, as PNG: what a record without art is
/// served, so every client draws an image. The apps' placeholder geometry —
/// the mark fitted inside a 26% inset, stroked at 4.5% of the side — in the
/// dark appearance's `muted` on `surface`, which reads as a dark sleeve in a
/// light client too. Drawn once per size and kept.
pub(crate) fn placeholder(size: u32) -> Bytes {
    static DRAWN: [std::sync::OnceLock<Bytes>; SIZES.len()] =
        [const { std::sync::OnceLock::new() }; SIZES.len()];
    let size = snap(Some(size));
    let slot = SIZES
        .iter()
        .position(|s| *s == size)
        .expect("snap answers one of SIZES");
    DRAWN[slot].get_or_init(|| draw_placeholder(size)).clone()
}

fn draw_placeholder(size: u32) -> Bytes {
    // The dark appearance's `--surface` (#2a2a2a) and `--muted` (#919191),
    // from site/src/theme.css.
    const SURFACE: [f32; 3] = [42.0, 42.0, 42.0];
    const MUTED: [f32; 3] = [145.0, 145.0, 145.0];
    // `just icons` writes each glyph as absolute `M`/`L` polylines, which is
    // all this reads; the test below fails if that changes.
    // The glyph's 24-unit box, with the ring spanning 2.5–21.5, scaled so the
    // ring is as wide as the apps' (60% of the inset box).
    let side = size as f32;
    let scale = side * 0.48 * 0.6 / 19.0;
    let points: Vec<(f32, f32)> = crate::icon::ENSO
        .strokes
        .split(['M', 'L'])
        .filter_map(|p| {
            let (x, y) = p.split_once(' ')?;
            let (x, y): (f32, f32) = (x.trim().parse().ok()?, y.trim().parse().ok()?);
            Some((
                side / 2.0 + (x - 12.0) * scale,
                side / 2.0 + (y - 12.0) * scale,
            ))
        })
        .collect();
    let radius = side * 0.045 / 2.0;
    let mut img = image::RgbImage::from_pixel(size, size, image::Rgb(SURFACE.map(|c| c as u8)));
    // Only the pixels the stroke can reach are measured.
    let (mut lo, mut hi) = (side, 0.0f32);
    for &(x, y) in &points {
        lo = lo.min(x.min(y) - radius - 1.0);
        hi = hi.max(x.max(y) + radius + 1.0);
    }
    let span = (lo.max(0.0) as u32)..(hi.min(side) as u32);
    for y in span.clone() {
        for x in span.clone() {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let distance = points
                .windows(2)
                .map(|w| {
                    let ((ax, ay), (bx, by)) = (w[0], w[1]);
                    let (dx, dy) = (bx - ax, by - ay);
                    let t =
                        (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                    (px - ax - t * dx).hypot(py - ay - t * dy)
                })
                .fold(f32::INFINITY, f32::min);
            let cover = (radius + 0.5 - distance).clamp(0.0, 1.0);
            if cover > 0.0 {
                let mix = |i: usize| (SURFACE[i] + (MUTED[i] - SURFACE[i]) * cover).round() as u8;
                img.put_pixel(x, y, image::Rgb([mix(0), mix(1), mix(2)]));
            }
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .expect("PNG encodes in memory");
    Bytes::from(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Covers at sizes no longer served, and strangers, go; what this version
    /// reads stays.
    #[test]
    fn covers_no_longer_served_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "0123456789abcdef0123456789abcdef";
        let keep = [
            "album-12-600.jpg".to_string(),
            format!("{hash}-300.jpg"),
            ".evicted".to_string(),
            "album-1-600.jpg.42-7.tmp".to_string(),
        ];
        let go = [
            format!("{hash}-800.jpg"),
            "album-12-400.jpg".to_string(),
            "notes.txt".to_string(),
        ];
        for name in keep.iter().chain(&go) {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        prune(dir.path());
        for name in &keep {
            assert!(dir.path().join(name).exists(), "{name} kept");
        }
        for name in &go {
            assert!(!dir.path().join(name).exists(), "{name} removed");
        }
    }

    /// An album's entry answers without its files: `make` runs once, a miss
    /// is kept like a hit, and a `make` that could not tell keeps nothing.
    #[test]
    fn an_album_is_read_from_its_files_once() {
        let dir = tempfile::tempdir().unwrap();
        let covers = Covers::new(dir.path().to_path_buf());
        let made = std::cell::Cell::new(0);
        let make = |art: Option<Bytes>| {
            made.set(made.get() + 1);
            Some(art)
        };
        let jpeg = Bytes::from_static(b"jpeg");
        assert_eq!(
            covers.album_cover(1, GRID, || make(Some(jpeg.clone()))),
            Some(jpeg.clone())
        );
        assert_eq!(covers.album_cover(1, GRID, || make(None)), Some(jpeg));
        assert_eq!(covers.album_cover(2, GRID, || make(None)), None);
        assert_eq!(
            covers.album_cover(2, GRID, || make(Some(Bytes::new()))),
            None
        );
        assert_eq!(made.get(), 2);
        assert_eq!(covers.album_cover(3, GRID, || None), None);
        assert!(!dir.path().join(album_key(3, GRID)).exists());
    }

    /// Albums the library names after the first look lose their entries;
    /// others keep theirs.
    #[test]
    fn evicted_albums_are_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE art_evictions (seq INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT, id INTEGER);
             INSERT INTO art_evictions (kind, id) VALUES ('album', 1);",
        )
        .unwrap();
        let covers = Covers::new(dir.path().to_path_buf());
        let keep = |album| covers.album_cover(album, GRID, || Some(Some(Bytes::from_static(b"x"))));
        keep(1);
        keep(2);
        // The first look only notes where the library is.
        covers.forget_evicted(&conn);
        assert!(dir.path().join(album_key(1, GRID)).exists());
        conn.execute_batch(
            "INSERT INTO art_evictions (kind, id) VALUES ('album', 2), ('track', 1);",
        )
        .unwrap();
        covers.forget_evicted(&conn);
        assert!(dir.path().join(album_key(1, GRID)).exists());
        assert!(!dir.path().join(album_key(2, GRID)).exists());
        // Where it got to is kept across restarts.
        assert_eq!(
            Covers::new(dir.path().to_path_buf())
                .evicted
                .lock()
                .unwrap(),
            3
        );
    }

    /// The report counts each size, its misses, and the records still
    /// lacking a warmed size; a clear empties all but the eviction mark.
    #[test]
    fn usage_counts_sizes_and_records_left_to_warm_and_a_clear_empties_it() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tracks (id INTEGER PRIMARY KEY, album_id INTEGER, path TEXT);
             INSERT INTO tracks (album_id, path) VALUES (1, '/a'), (2, '/b'), (3, NULL);",
        )
        .unwrap();
        let covers = Covers::new(dir.path().to_path_buf());
        let hash = "0123456789abcdef0123456789abcdef";
        for (name, len) in [
            ("album-1-300.jpg", 100),
            ("album-1-600.jpg", 0),
            ("album-2-600.jpg", 400),
            (&format!("{hash}-1200.jpg") as &str, 1000),
            (".evicted", 2),
        ] {
            std::fs::write(dir.path().join(name), vec![0u8; len]).unwrap();
        }
        let u = covers.usage(&conn);
        assert_eq!(u.bytes, 1502);
        assert_eq!(u.budget, BUDGET);
        let at = |size| *u.sizes.iter().find(|s| s.size == size).unwrap();
        assert_eq!(at(150).covers, 0);
        assert_eq!((at(300).covers, at(300).bytes, at(300).misses), (1, 100, 0));
        assert_eq!((at(600).covers, at(600).bytes, at(600).misses), (2, 400, 1));
        assert_eq!(at(1200).covers, 1);
        assert_eq!((u.covers(), u.misses()), (4, 1));
        assert_eq!(u.unwarmed, 1, "record 2 lacks its 300; 3 has no files");

        let changed = covers.subscribe();
        assert_eq!(covers.clear(), (4, 1500));
        assert!(changed.has_changed().unwrap());
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, [".evicted"]);
        assert_eq!(covers.usage(&conn).unwarmed, 2);
    }

    /// A warm reports its pass as it goes and when it ends; one not
    /// started refuses to be woken.
    #[test]
    fn the_warm_reports_its_pass() {
        let dir = tempfile::tempdir().unwrap();
        let covers = Covers::new(dir.path().join("covers"));
        assert_eq!(covers.report().warm.state, WarmState::Off);
        assert!(!covers.warm_now());
        let db = dir.path().join("koan.db");
        koan_core::db::connection::Database::open(&db).unwrap();
        let pool = koan_core::db::pool::Pool::new(db);
        covers.warm_pass(&pool);
        let warm = covers.report().warm;
        assert_eq!(warm.state, WarmState::Idle);
        assert_eq!((warm.done, warm.total, warm.made), (0, 0, 0));
        assert!(warm.started.is_some() && warm.finished.is_some());
        assert_eq!(warm.next(), Some(warm.finished.unwrap() + WARM_EVERY));
    }

    #[test]
    fn requests_snap_up_to_a_served_size() {
        assert_eq!(snap(Some(1)), 150);
        assert_eq!(snap(Some(300)), 300);
        assert_eq!(snap(Some(301)), 600);
        assert_eq!(snap(Some(1000)), 1200);
        assert_eq!(snap(Some(99_999)), 1200);
        assert_eq!(snap(None), LARGE);
    }

    #[test]
    fn the_enso_is_one_absolute_polyline() {
        let strokes = crate::icon::ENSO.strokes;
        assert!(strokes.starts_with('M'));
        assert_eq!(strokes.matches('M').count(), 1);
        assert!(
            strokes
                .chars()
                .all(|c| matches!(c, 'M' | 'L' | ' ' | '.' | '-' | '0'..='9'))
        );
    }

    #[test]
    fn the_placeholder_is_the_size_asked_with_the_mark_inside_the_inset() {
        let png = placeholder(300);
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (300, 300));
        let surface = image::Rgb([42, 42, 42]);
        assert_eq!(*img.get_pixel(0, 0), surface);
        assert_eq!(*img.get_pixel(150, 150), surface, "the ring is open");
        let inked: Vec<_> = img
            .enumerate_pixels()
            .filter(|(_, _, p)| **p != surface)
            .collect();
        assert!(!inked.is_empty());
        let inset = (300.0 * 0.26) as u32;
        assert!(
            inked
                .iter()
                .all(|(x, y, _)| (inset..300 - inset).contains(x)
                    && (inset..300 - inset).contains(y))
        );
    }

    #[test]
    fn writers_racing_on_one_cover_leave_it_whole() {
        let dir = tempfile::tempdir().unwrap();
        let covers = Covers::new(dir.path().to_path_buf());
        let arts: Vec<Vec<u8>> = (0..8u8).map(|i| vec![i; 256 * 1024]).collect();
        for _ in 0..20 {
            std::thread::scope(|s| {
                for art in &arts {
                    s.spawn(|| covers.write_disk("k-400.jpg", Some(art)));
                }
            });
            let got = std::fs::read(dir.path().join("k-400.jpg")).unwrap();
            assert!(arts.contains(&got), "a torn write of {} bytes", got.len());
        }
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary files left behind");
    }

    #[test]
    fn the_directory_is_held_to_its_budget_oldest_read_first() {
        let dir = tempfile::tempdir().unwrap();
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        let now = std::time::SystemTime::now();
        let put = |name: &str, len: usize, age: u32| {
            let path = dir.path().join(name);
            std::fs::write(&path, vec![0u8; len]).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(now - day * age)
                .unwrap();
        };
        for i in 0..10u32 {
            put(&format!("c{i}-400.jpg"), 1000, 10 - i);
        }
        put("c0-400.jpg.1-2.tmp", 10, 2);
        put("c9-400.jpg.1-3.tmp", 10, 0);
        assert_eq!(sweep(dir.path(), 5600), Some((5, 5000)));
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "c5-400.jpg",
                "c6-400.jpg",
                "c7-400.jpg",
                "c8-400.jpg",
                "c9-400.jpg",
                "c9-400.jpg.1-3.tmp"
            ],
            "the five read longest ago go, and the abandoned temporary file"
        );
    }

    #[test]
    fn covers_are_bounded_jpegs() {
        use image::GenericImageView as _;
        let png = {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(2400, 1200)
                .write_to(&mut out, image::ImageFormat::Png)
                .unwrap();
            out.into_inner()
        };
        let out = encode(&png, 400).unwrap();
        assert!(out.starts_with(&[0xFF, 0xD8]));
        assert_eq!(
            image::load_from_memory(&out).unwrap().dimensions(),
            (400, 200)
        );

        let small = encode(&png, 400).unwrap();
        assert_eq!(
            encode(&small, 800).unwrap(),
            small,
            "a small JPEG passes through"
        );
    }
}
