//! Artwork for every page: bytes kept on disk once per record, bitmaps kept in
//! memory per record and draw size.
//!
//! Two lanes of worker threads do the work, neither of them the main thread
//! nor GPUI's background pool. The fetch lane asks the engine for a record's
//! art (a file read, or an HTTP round trip on a remote library) and writes it
//! to disk; the decode lane turns the file into a bitmap at the size it is
//! drawn. Each picks the most urgent job there is every time it takes one, so
//! what is on screen always goes first, then where a flick will land, then
//! the rows the scroll is heading for. A job nobody wants any more is never
//! started.
//!
//! Row ids are reused once a row is deleted, so whatever is held under an id
//! is dropped when the engine says the record went (`art_evictions`), and a
//! fetch or decode that was running then is not kept: each record carries a
//! generation, bumped by an eviction, which a result must still match.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::*;
use koan_ffi::KoanEngine;
use parking_lot::{Condvar, Mutex};
use smallvec::smallvec;

/// What is asked of the server, and what lands on disk: large enough for a
/// record's own page on a 2× display, small enough that a library's worth is
/// a cache rather than a second copy of the artwork.
const SOURCE_PIXELS: u32 = 1000;

/// Roughly 360 grid tiles at 2×. A ceiling: entries drawn recently, or still
/// held by an element, are never evicted to meet it.
const MEMORY_BUDGET: usize = 256 * 1024 * 1024;

/// Draw sizes are rounded up to a multiple of this, so a window being resized
/// does not decode every tile again at each pixel.
const BUCKET: u32 = 32;

/// How long a failed fetch (a server out of reach) waits before it may be
/// tried again. A record with no art is remembered for good instead.
const RETRY_AFTER: Duration = Duration::from_secs(20);

/// Bumped when what a cache file means changes; any other scheme's directory
/// is deleted.
const SCHEME: &str = "v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Art {
    Album(i64),
    Artist(i64),
}

impl Art {
    fn filename(self) -> String {
        match self {
            Art::Album(id) => format!("album-{id}"),
            Art::Artist(id) => format!("artist-{id}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub art: Art,
    /// Device pixels along the sleeve's side, rounded up to a bucket.
    pub px: u32,
}

impl Key {
    pub fn new(art: Art, side: Pixels, scale: f32) -> Self {
        let px = (f32::from(side) * scale).ceil().max(1.0) as u32;
        Self {
            art,
            px: px.div_ceil(BUCKET) * BUCKET,
        }
    }
}

/// How much a request matters. Ordered most urgent first: everything on
/// screen, then where a flick will come to rest, then the rows ahead, each
/// tier nearest first.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub enum Priority {
    Visible,
    Landing(f32),
    Ahead(f32),
}

/// What a page draws for a record: the bitmap at its size, or a smaller or
/// larger one of the same record while that decodes.
pub enum Sleeve {
    Ready(Arc<RenderImage>),
    /// Asked for and not here yet.
    Waiting,
    /// The record has no art.
    None,
}

struct Held {
    image: Arc<RenderImage>,
    bytes: usize,
    used: Instant,
}

/// What both lanes and the main thread share, behind one lock that is held
/// only across map reads and writes.
#[derive(Default)]
struct Queue {
    /// Every request, by key, at the most urgent priority anyone asked for.
    wanted: HashMap<Key, Priority>,
    on_disk: HashSet<Art>,
    fetching: HashSet<Art>,
    decoding: HashSet<Key>,
    /// Decoded, and handed to the main thread.
    decoded: HashSet<Key>,
    /// Looked at, and the answer was that there is no art.
    absent: HashSet<Art>,
    failed: HashMap<Art, Instant>,
    generation: HashMap<Art, u64>,
    closed: bool,
}

impl Queue {
    fn generation(&self, art: Art) -> u64 {
        self.generation.get(&art).copied().unwrap_or(0)
    }

    /// The most urgent record that still needs fetching.
    fn next_fetch(&self) -> Option<Art> {
        let now = Instant::now();
        self.wanted
            .iter()
            .filter(|(k, _)| {
                !self.on_disk.contains(&k.art)
                    && !self.fetching.contains(&k.art)
                    && !self.absent.contains(&k.art)
                    && self
                        .failed
                        .get(&k.art)
                        .is_none_or(|at| now.duration_since(*at) > RETRY_AFTER)
            })
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| k.art)
    }

    /// The most urgent bitmap whose bytes are on disk.
    fn next_decode(&self) -> Option<Key> {
        self.wanted
            .iter()
            .filter(|(k, _)| {
                self.on_disk.contains(&k.art)
                    && !self.decoding.contains(k)
                    && !self.decoded.contains(k)
            })
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| *k)
    }
}

struct Shared {
    queue: Mutex<Queue>,
    fetch_ready: Condvar,
    decode_ready: Condvar,
}

enum Done {
    Decoded {
        key: Key,
        generation: u64,
        image: Arc<RenderImage>,
    },
    /// The lanes learned something a page draws differently: a record has
    /// no art.
    Changed,
}

/// Something the library lost, whose id may come back as something else.
pub struct Evicted {
    pub albums: Vec<i64>,
    pub artists: Vec<i64>,
    pub tracks: Vec<i64>,
}

impl EventEmitter<Evicted> for Covers {}

pub struct Covers {
    engine: Arc<KoanEngine>,
    dir: Option<PathBuf>,
    shared: Arc<Shared>,
    memory: HashMap<Key, Held>,
    memory_bytes: usize,
    /// Each list's requests, replaced whole as it scrolls.
    aims: HashMap<EntityId, HashMap<Key, Priority>>,
    /// Requests from pages that draw a few sleeves: kept until they land.
    asks: HashMap<Key, Priority>,
    evictions_seq: i64,
    _results: Task<()>,
}

impl Covers {
    pub fn new(engine: Arc<KoanEngine>, cx: &mut Context<Self>) -> Self {
        let dir = directory();
        let evictions_seq = dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("evictions-seq")).ok())
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            fetch_ready: Condvar::new(),
            decode_ready: Condvar::new(),
        });
        let (tx, mut rx) = mpsc::unbounded();
        let decoders = std::thread::available_parallelism()
            .map_or(2, |n| n.get() / 2)
            .clamp(2, 6);
        for n in 0..decoders {
            let (shared, dir, tx) = (shared.clone(), dir.clone(), tx.clone());
            std::thread::Builder::new()
                .name(format!("koan-art-decode-{n}"))
                .spawn(move || decode_lane(shared, dir, tx))
                .expect("spawn art decoder");
        }
        // Fetches mostly wait on a disk or a server, so there are more of
        // them than there are cores to spare.
        for n in 0..6 {
            let (shared, dir, tx, engine) =
                (shared.clone(), dir.clone(), tx.clone(), engine.clone());
            std::thread::Builder::new()
                .name(format!("koan-art-fetch-{n}"))
                .spawn(move || fetch_lane(shared, dir, engine, tx))
                .expect("spawn art fetcher");
        }
        drop(tx);
        let results = cx.spawn(async move |this, cx| {
            while let Some(done) = rx.next().await {
                if this.update(cx, |covers, cx| covers.land(done, cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            engine,
            dir,
            shared,
            memory: HashMap::new(),
            memory_bytes: 0,
            aims: HashMap::new(),
            asks: HashMap::new(),
            evictions_seq,
            _results: results,
        }
    }

    /// What to draw for `key` now, without starting anything. A stand-in of
    /// another size is drawn while the asked-for one decodes, so a resize or
    /// a move to a display of another scale never blanks a sleeve.
    pub fn sleeve(&mut self, key: Key) -> Sleeve {
        let now = Instant::now();
        if let Some(held) = self.memory.get_mut(&key) {
            held.used = now;
            return Sleeve::Ready(held.image.clone());
        }
        let standin = self
            .memory
            .iter_mut()
            .filter(|(k, _)| k.art == key.art)
            .min_by_key(|(k, _)| k.px.abs_diff(key.px))
            .map(|(_, held)| {
                held.used = now;
                held.image.clone()
            });
        if let Some(image) = standin {
            return Sleeve::Ready(image);
        }
        if self.shared.queue.lock().absent.contains(&key.art) {
            Sleeve::None
        } else {
            Sleeve::Waiting
        }
    }

    pub fn has(&self, key: Key) -> bool {
        self.memory.contains_key(&key)
    }

    pub fn is_absent(&self, art: Art) -> bool {
        self.shared.queue.lock().absent.contains(&art)
    }

    /// A page's request for a sleeve it is drawing. Kept until it lands, so
    /// it suits a page with a few sleeves; a list that scrolls aims instead.
    /// Never notifies, so it can be called while drawing.
    pub fn ask(&mut self, key: Key) {
        if self.memory.contains_key(&key) || self.asks.contains_key(&key) {
            return;
        }
        self.asks.insert(key, Priority::Visible);
        self.publish();
    }

    /// A scrolling list's requests, replacing its last ones. What it no
    /// longer asks for is not started. Never notifies, so the list can aim
    /// from its own layout.
    pub fn aim(&mut self, owner: EntityId, ranked: impl IntoIterator<Item = (Key, Priority)>) {
        let wanted: HashMap<Key, Priority> = ranked
            .into_iter()
            .filter(|(k, _)| !self.memory.contains_key(k))
            .collect();
        if self.aims.get(&owner) == Some(&wanted) {
            return;
        }
        self.aims.insert(owner, wanted);
        self.publish();
    }

    /// A list going away: nothing it asked for is wanted on its account.
    pub fn release(&mut self, owner: EntityId) {
        if self.aims.remove(&owner).is_some() {
            self.publish();
        }
    }

    fn publish(&mut self) {
        let mut wanted = self.asks.clone();
        for aim in self.aims.values() {
            for (key, priority) in aim {
                wanted
                    .entry(*key)
                    .and_modify(|p| {
                        if priority < p {
                            *p = *priority
                        }
                    })
                    .or_insert(*priority);
            }
        }
        let mut queue = self.shared.queue.lock();
        // A bitmap decoded and then evicted from memory is wanted again.
        let memory = &self.memory;
        queue.decoded.retain(|k| memory.contains_key(k));
        queue.wanted = wanted;
        drop(queue);
        self.shared.fetch_ready.notify_all();
        self.shared.decode_ready.notify_all();
    }

    fn land(&mut self, done: Done, cx: &mut Context<Self>) {
        match done {
            Done::Decoded {
                key,
                generation,
                image,
            } => {
                let current = self.shared.queue.lock().generation(key.art);
                if current != generation {
                    cx.drop_image(image, None);
                    return;
                }
                let size = image.size(0);
                let bytes = (i32::from(size.width) * i32::from(size.height) * 4) as usize;
                if let Some(old) = self.memory.insert(
                    key,
                    Held {
                        image,
                        bytes,
                        used: Instant::now(),
                    },
                ) {
                    self.memory_bytes -= old.bytes;
                    cx.drop_image(old.image, None);
                }
                self.memory_bytes += bytes;
                if self.asks.remove(&key).is_some() {
                    self.publish();
                }
                self.trim(cx);
            }
            Done::Changed => {}
        }
        cx.notify();
    }

    /// Back under budget, oldest first, never touching a bitmap drawn in the
    /// last moment or still held by an element: dropping one of those from
    /// the atlas would only have the next paint upload it again, and nothing
    /// would ever drop it after that.
    fn trim(&mut self, cx: &mut Context<Self>) {
        if self.memory_bytes <= MEMORY_BUDGET {
            return;
        }
        let target = MEMORY_BUDGET / 10 * 9;
        let recent = Instant::now() - Duration::from_millis(500);
        let mut old: Vec<(Key, Instant)> = self
            .memory
            .iter()
            .filter(|(_, h)| h.used < recent && Arc::strong_count(&h.image) == 1)
            .map(|(k, h)| (*k, h.used))
            .collect();
        old.sort_by_key(|(_, used)| *used);
        let mut queue = self.shared.queue.lock();
        for (key, _) in old {
            if self.memory_bytes <= target {
                break;
            }
            if let Some(held) = self.memory.remove(&key) {
                self.memory_bytes -= held.bytes;
                queue.decoded.remove(&key);
                cx.drop_image(held.image, None);
            }
        }
    }

    /// Forget what the library has deleted since last asked: its files, its
    /// bitmaps, and any fetch or decode still running for it. Called on each
    /// library change; the seq is kept beside the files it describes.
    pub fn sweep(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let after = self.evictions_seq;
        cx.spawn(async move |this, cx| {
            let gone = match engine.art_evictions(after).await {
                Ok(gone) => gone,
                Err(e) => {
                    log::warn!("art evictions: {e}");
                    return;
                }
            };
            this.update(cx, |covers, cx| covers.evict(gone, cx)).ok();
        })
        .detach();
    }

    fn evict(&mut self, gone: koan_ffi::ArtEvictions, cx: &mut Context<Self>) {
        if gone.seq <= self.evictions_seq {
            return;
        }
        let arts: Vec<Art> = gone
            .albums
            .iter()
            .map(|id| Art::Album(*id))
            .chain(gone.artists.iter().map(|id| Art::Artist(*id)))
            .collect();
        {
            let mut queue = self.shared.queue.lock();
            for art in &arts {
                *queue.generation.entry(*art).or_default() += 1;
                queue.on_disk.remove(art);
                queue.absent.remove(art);
                queue.failed.remove(art);
                queue.decoded.retain(|k| k.art != *art);
            }
        }
        let drop: Vec<Key> = self
            .memory
            .keys()
            .filter(|k| arts.contains(&k.art))
            .copied()
            .collect();
        for key in drop {
            if let Some(held) = self.memory.remove(&key) {
                self.memory_bytes -= held.bytes;
                cx.drop_image(held.image, None);
            }
        }
        self.evictions_seq = gone.seq;
        if let Some(dir) = self.dir.clone() {
            let seq = gone.seq;
            cx.background_executor()
                .spawn(async move {
                    for art in arts {
                        std::fs::remove_file(dir.join(art.filename())).ok();
                    }
                    write_atomically(&dir.join("evictions-seq"), seq.to_string().as_bytes()).ok();
                })
                .detach();
        }
        cx.emit(Evicted {
            albums: gone.albums,
            artists: gone.artists,
            tracks: gone.tracks,
        });
        cx.notify();
    }
}

impl Drop for Covers {
    fn drop(&mut self) {
        self.shared.queue.lock().closed = true;
        self.shared.fetch_ready.notify_all();
        self.shared.decode_ready.notify_all();
    }
}

/// `$KOAN_CONFIG_DIR/artwork` for an instance on a configuration of its own
/// (a test library), since files are named by row id and another library's
/// ids would show its covers; the platform's cache directory otherwise.
fn directory() -> Option<PathBuf> {
    let root = match std::env::var_os("KOAN_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join("artwork"),
        None => dirs::cache_dir()?.join("koan-desktop").join("artwork"),
    };
    let dir = root.join(SCHEME);
    std::fs::create_dir_all(&dir).ok()?;
    // Nothing here is authoritative, so an older scheme is deleted rather
    // than migrated; it refills as pages are drawn.
    for entry in std::fs::read_dir(&root).ok()?.flatten() {
        if entry.file_name() != SCHEME {
            std::fs::remove_dir_all(entry.path()).ok();
        }
    }
    Some(dir)
}

fn write_atomically(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        std::fs::remove_file(&tmp).ok();
    })
}

fn fetch_lane(
    shared: Arc<Shared>,
    dir: Option<PathBuf>,
    engine: Arc<KoanEngine>,
    tx: mpsc::UnboundedSender<Done>,
) {
    loop {
        let (art, generation) = {
            let mut queue = shared.queue.lock();
            loop {
                if queue.closed {
                    return;
                }
                if let Some(art) = queue.next_fetch() {
                    queue.fetching.insert(art);
                    break (art, queue.generation(art));
                }
                shared.fetch_ready.wait(&mut queue);
            }
        };
        let file = dir.as_ref().map(|d| d.join(art.filename()));
        if file.as_ref().is_some_and(|f| f.exists()) {
            let mut queue = shared.queue.lock();
            queue.fetching.remove(&art);
            if queue.generation(art) == generation {
                queue.on_disk.insert(art);
            }
            drop(queue);
            shared.decode_ready.notify_all();
            continue;
        }
        let fetched = futures::executor::block_on(async {
            match art {
                Art::Album(id) => engine.clone().album_cover_art(id, Some(SOURCE_PIXELS)).await,
                Art::Artist(id) => engine.clone().artist_image(id).await,
            }
        });
        // Written beside its final name and renamed under the lock, so an
        // eviction of the record in between leaves nothing under its id.
        let staged = match (&fetched, &file) {
            (Ok(Some(cover)), Some(file)) => {
                let tmp = file.with_extension("part");
                std::fs::write(&tmp, &cover.data).ok().map(|()| tmp)
            }
            _ => None,
        };
        let mut queue = shared.queue.lock();
        queue.fetching.remove(&art);
        let current = queue.generation(art) == generation;
        let mut changed = false;
        match fetched {
            Ok(Some(_)) => match (staged, &file) {
                (Some(tmp), Some(file)) if current && std::fs::rename(&tmp, file).is_ok() => {
                    queue.on_disk.insert(art);
                }
                (Some(tmp), _) => {
                    std::fs::remove_file(tmp).ok();
                }
                // No directory to keep it in: nothing can be shown.
                _ => {
                    queue.failed.insert(art, Instant::now());
                }
            },
            Ok(None) if current => {
                queue.absent.insert(art);
                changed = true;
            }
            Ok(None) => {}
            Err(e) => {
                log::debug!("art for {art:?}: {e}");
                queue.failed.insert(art, Instant::now());
            }
        }
        drop(queue);
        shared.decode_ready.notify_all();
        if changed && tx.unbounded_send(Done::Changed).is_err() {
            return;
        }
    }
}

fn decode_lane(shared: Arc<Shared>, dir: Option<PathBuf>, tx: mpsc::UnboundedSender<Done>) {
    let Some(dir) = dir else { return };
    loop {
        let (key, generation) = {
            let mut queue = shared.queue.lock();
            loop {
                if queue.closed {
                    return;
                }
                if let Some(key) = queue.next_decode() {
                    queue.decoding.insert(key);
                    break (key, queue.generation(key.art));
                }
                shared.decode_ready.wait(&mut queue);
            }
        };
        let file = dir.join(key.art.filename());
        let decoded = std::fs::read(&file)
            .map_err(|_| Missing)
            .and_then(|data| decode(&data, key.px).ok_or(Missing).map_err(|_| Broken));
        let mut queue = shared.queue.lock();
        queue.decoding.remove(&key);
        let current = queue.generation(key.art) == generation;
        match decoded {
            Ok(image) => {
                queue.decoded.insert(key);
                drop(queue);
                let done = Done::Decoded {
                    key,
                    generation,
                    image: Arc::new(image),
                };
                if tx.unbounded_send(done).is_err() {
                    return;
                }
            }
            // Gone from disk (a cache cleared under it): fetched again.
            Err(Failure::Missing) => {
                if current {
                    queue.on_disk.remove(&key.art);
                }
                drop(queue);
                shared.fetch_ready.notify_all();
            }
            // Bytes that will not make a bitmap will not make one next time:
            // the record is taken to have no art, and the file goes so a later
            // launch asks again.
            Err(Failure::Broken) => {
                if current {
                    queue.on_disk.remove(&key.art);
                    queue.absent.insert(key.art);
                    std::fs::remove_file(&file).ok();
                }
                drop(queue);
                if tx.unbounded_send(Done::Changed).is_err() {
                    return;
                }
            }
        }
    }
}

enum Failure {
    Missing,
    Broken,
}
use Failure::{Broken, Missing};

/// The sleeve filling a square `px` on a side, centred, as BGRA.
fn decode(data: &[u8], px: u32) -> Option<RenderImage> {
    let image = image::load_from_memory(data).ok()?;
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return None;
    }
    // Scaled so the shorter side fills the square, then the middle taken. Not
    // enlarged: a small sleeve is drawn larger by the GPU instead.
    let scale = (px as f32 / w.min(h) as f32).min(1.0);
    let (sw, sh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let scaled = if scale < 1.0 {
        image.thumbnail_exact(sw, sh)
    } else {
        image
    };
    let side = sw.min(sh);
    let mut rgba = scaled
        .crop_imm((sw - side) / 2, (sh - side) / 2, side, side)
        .into_rgba8();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(RenderImage::new(smallvec![image::Frame::new(rgba)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_up_to_a_bucket() {
        let k = Key::new(Art::Album(1), px(200.0), 2.0);
        assert_eq!(k.px, 416);
        assert_eq!(Key::new(Art::Album(1), px(208.0), 2.0).px, 416);
        assert_eq!(Key::new(Art::Album(1), px(0.0), 1.0).px, BUCKET);
    }

    #[test]
    fn on_screen_comes_before_landing_before_ahead() {
        assert!(Priority::Visible < Priority::Landing(1000.0));
        assert!(Priority::Landing(1000.0) < Priority::Ahead(0.0));
        assert!(Priority::Ahead(10.0) < Priority::Ahead(20.0));
    }

    #[test]
    fn decode_fills_the_square_and_swaps_to_bgra() {
        let mut data = Vec::new();
        image::RgbImage::from_pixel(400, 200, image::Rgb([255, 0, 0]))
            .write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png)
            .unwrap();
        let image = decode(&data, 100).unwrap();
        let size = image.size(0);
        assert_eq!((i32::from(size.width), i32::from(size.height)), (100, 100));
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
        assert!(decode(b"junk", 100).is_none());
    }

    #[test]
    fn the_queue_takes_the_most_urgent_job_it_can_start() {
        let mut q = Queue::default();
        let (a, b) = (Art::Album(1), Art::Album(2));
        q.wanted.insert(Key { art: a, px: 64 }, Priority::Ahead(5.0));
        q.wanted.insert(Key { art: b, px: 64 }, Priority::Visible);
        assert_eq!(q.next_fetch(), Some(b));
        q.absent.insert(b);
        assert_eq!(q.next_fetch(), Some(a));
        q.failed.insert(a, Instant::now());
        assert_eq!(q.next_fetch(), None);
        q.on_disk.insert(a);
        assert_eq!(q.next_decode(), Some(Key { art: a, px: 64 }));
        q.decoded.insert(Key { art: a, px: 64 });
        assert_eq!(q.next_decode(), None);
    }
}
