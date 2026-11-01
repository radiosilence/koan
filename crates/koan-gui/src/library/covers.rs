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
//! generation, bumped by an eviction, which a result must still match. Until
//! the first such sweep has landed, files on disk are not trusted at all: ids
//! may have been reused while the app was closed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::*;
use koan_ffi::KoanEngine;
use parking_lot::{Condvar, Mutex};
use smallvec::{SmallVec, smallvec};

/// What is asked of the engine, and what lands on disk: large enough for a
/// record's own page on a 2× display, small enough that a library's worth is
/// a cache rather than a second copy of the artwork.
const SOURCE_PIXELS: u32 = 1000;

/// Roughly 360 grid tiles at 2×. A ceiling: entries drawn in the last moment,
/// or still held by an element, are never evicted to meet it.
const MEMORY_BUDGET: usize = 256 * 1024 * 1024;

/// Draw sizes are rounded up to a multiple of this, so a window being resized
/// does not decode every tile again at each pixel.
const BUCKET: u32 = 32;

/// How many bytes of new sleeves a frame takes into the GPU's atlas: about
/// three tiles at 2×. Each is copied into a texture on the main thread inside
/// that frame's paint, so a flick that lands forty at once spreads them over
/// frames, each tile showing its sleeve's colour until its turn. At least one
/// is taken a frame, whatever its size.
pub const UPLOAD_BYTES_PER_FRAME: usize = 2 * 1024 * 1024;

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

/// What a page draws for a record.
#[derive(Clone)]
pub enum Sleeve {
    /// The bitmap at its size, or the nearest other size of the same record
    /// while that decodes.
    Ready(Arc<RenderImage>),
    /// Decoded and waiting its turn for the atlas, or decoded once and since
    /// evicted: drawn as a flat field of the sleeve's colour.
    Colour(Hsla),
    /// Not here yet.
    Waiting,
    /// The record has no art.
    None,
}

impl Sleeve {
    /// The same thing to draw: the same bitmap, colour or absence.
    pub fn same(&self, other: &Sleeve) -> bool {
        match (self, other) {
            (Sleeve::Ready(a), Sleeve::Ready(b)) => Arc::ptr_eq(a, b),
            (Sleeve::Colour(a), Sleeve::Colour(b)) => a == b,
            (Sleeve::Waiting, Sleeve::Waiting) | (Sleeve::None, Sleeve::None) => true,
            _ => false,
        }
    }
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
    /// Asked for, and the engine failed (a server out of reach). Not tried
    /// again until the connection changes or the record is asked for afresh.
    failed: HashSet<Art>,
    generation: HashMap<Art, u64>,
    /// Evicted, with the file still being unlinked: not fetched, and its file
    /// not trusted, until that is done. Counted: a second eviction of the
    /// same id can land before the first one's unlink has run.
    evicting: HashMap<Art, u32>,
    /// The first eviction sweep has been tried, so fetches may start.
    fetches_allowed: bool,
    /// The first eviction sweep has landed, so a file found on disk belongs to
    /// the record now holding its id. Until then every record is fetched
    /// afresh, over whatever file it had.
    disk_trusted: bool,
    closed: bool,
}

impl Queue {
    fn generation(&self, art: Art) -> u64 {
        self.generation.get(&art).copied().unwrap_or(0)
    }

    fn fetchable(&self, key: &Key) -> bool {
        self.fetches_allowed
            && !self.on_disk.contains(&key.art)
            && !self.fetching.contains(&key.art)
            && !self.absent.contains(&key.art)
            && !self.failed.contains(&key.art)
            && !self.evicting.contains_key(&key.art)
    }

    fn decodable(&self, key: &Key) -> bool {
        self.on_disk.contains(&key.art)
            && !self.decoding.contains(key)
            && !self.decoded.contains(key)
    }

    fn most_urgent(&self, ok: impl Fn(&Key) -> bool) -> Option<Key> {
        self.wanted
            .iter()
            .filter(|(k, _)| ok(k))
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| *k)
    }

    fn next_fetch(&self) -> Option<Art> {
        self.most_urgent(|k| self.fetchable(k)).map(|k| k.art)
    }

    fn next_decode(&self) -> Option<Key> {
        self.most_urgent(|k| self.decodable(k))
    }
}

struct Shared {
    queue: Mutex<Queue>,
    /// Held across a check of a record's file and what is done on it, and
    /// across an eviction's unlinks, so neither runs between the other's
    /// check and act. Taken before `queue`, never by the main thread.
    disk: Mutex<()>,
    fetch_ready: Condvar,
    decode_ready: Condvar,
}

impl Shared {
    /// Wake one worker per job that has become startable, and none when
    /// nothing has: a scroll re-aims every frame, and most frames add nothing
    /// a lane can start.
    fn wake(&self, fetches: usize, decodes: usize) {
        for _ in 0..fetches.min(FETCHERS) {
            self.fetch_ready.notify_one();
        }
        for _ in 0..decodes.min(MAX_DECODERS) {
            self.decode_ready.notify_one();
        }
    }
}

const FETCHERS: usize = 6;
const MAX_DECODERS: usize = 6;

enum Done {
    Decoded {
        key: Key,
        generation: u64,
        image: Arc<RenderImage>,
        colour: Hsla,
    },
    /// A record turned out to have no art.
    Absent,
}

/// Records the library lost, whose ids may come back as other records.
pub struct Evicted {
    pub albums: Vec<i64>,
    pub artists: Vec<i64>,
    pub tracks: Vec<i64>,
}

impl EventEmitter<Evicted> for Covers {}

pub struct Covers {
    engine: Arc<KoanEngine>,
    dir: PathBuf,
    shared: Arc<Shared>,
    memory: HashMap<Key, Held>,
    /// The sizes held for each record, so a miss finds a stand-in without a
    /// walk of every bitmap.
    sizes: HashMap<Art, SmallVec<[u32; 4]>>,
    memory_bytes: usize,
    /// Decoded, not yet in memory: admitted a few a frame (`admit`).
    staged: HashMap<Key, (Arc<RenderImage>, u64)>,
    /// Each record's colour, kept once known: what its tile shows while its
    /// sleeve waits for the atlas, or after it is evicted. A few bytes each.
    colours: HashMap<Art, Hsla>,
    /// Each list's and page's requests, replaced whole as it draws.
    requests: HashMap<EntityId, HashMap<Key, Priority>>,
    evictions_seq: i64,
    _results: Task<()>,
}

impl Covers {
    pub fn new(engine: Arc<KoanEngine>, cx: &mut Context<Self>) -> Self {
        let dir = directory();
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            disk: Mutex::new(()),
            fetch_ready: Condvar::new(),
            decode_ready: Condvar::new(),
        });
        let (tx, mut rx) = mpsc::unbounded();
        let decoders = std::thread::available_parallelism()
            .map_or(2, |n| n.get() / 2)
            .clamp(2, MAX_DECODERS);
        for n in 0..decoders {
            let (shared, dir, tx) = (shared.clone(), dir.clone(), tx.clone());
            std::thread::Builder::new()
                .name(format!("koan-art-decode-{n}"))
                .spawn(move || decode_lane(shared, dir, tx))
                .expect("spawn art decoder");
        }
        // Fetches mostly wait on a disk or a server, so there are more of
        // them than there are cores to spare.
        for n in 0..FETCHERS {
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
        // The directory is made, and the last sweep's mark read, off the main
        // thread; nothing is fetched until the first sweep after it.
        let prepare = dir.clone();
        cx.spawn(async move |this, cx| {
            let seq = cx
                .background_executor()
                .spawn(async move { prepare_directory(&prepare) })
                .await;
            this.update(cx, |covers, cx| {
                covers.evictions_seq = seq;
                covers.sweep(cx);
            })
            .ok();
        })
        .detach();
        Self {
            engine,
            dir,
            shared,
            memory: HashMap::new(),
            sizes: HashMap::new(),
            memory_bytes: 0,
            staged: HashMap::new(),
            colours: HashMap::new(),
            requests: HashMap::new(),
            evictions_seq: 0,
            _results: results,
        }
    }

    /// What to draw for `key` now, without starting anything. Another size of
    /// the same record stands in while the asked-for one decodes, the nearest
    /// larger first, so a resize or a move to a display of another scale never
    /// blanks a sleeve, and a row's thumbnail is not blown up over a tile.
    pub fn sleeve(&mut self, key: Key) -> Sleeve {
        let now = Instant::now();
        let px = self.sizes.get(&key.art).and_then(|sizes| {
            if sizes.contains(&key.px) {
                return Some(key.px);
            }
            let larger = sizes.iter().filter(|p| **p > key.px).min();
            let smaller = sizes.iter().filter(|p| **p < key.px).max();
            larger.or(smaller).copied()
        });
        if let Some(held) = px.and_then(|px| self.memory.get_mut(&Key { art: key.art, px })) {
            held.used = now;
            return Sleeve::Ready(held.image.clone());
        }
        if let Some(colour) = self.colours.get(&key.art) {
            return Sleeve::Colour(*colour);
        }
        if self.shared.queue.lock().absent.contains(&key.art) {
            Sleeve::None
        } else {
            Sleeve::Waiting
        }
    }

    pub fn is_absent(&self, art: Art) -> bool {
        self.shared.queue.lock().absent.contains(&art)
    }

    /// Whether `key` itself is held, not a stand-in of another size.
    pub fn has(&self, key: Key) -> bool {
        self.memory.contains_key(&key)
    }

    /// `owner`'s requests, replacing its last ones: a list's visible and
    /// upcoming sleeves ranked as it scrolls, or a page's few. What it no
    /// longer asks for is not started on its account. A record that comes on
    /// screen after a failed fetch is tried again. Never notifies, so it can
    /// be called while drawing.
    pub fn request(&mut self, owner: EntityId, ranked: impl IntoIterator<Item = (Key, Priority)>) {
        // A key ranked twice keeps its most urgent ranking.
        let mut wanted: HashMap<Key, Priority> = HashMap::new();
        for (key, priority) in ranked {
            let held = wanted.entry(key).or_insert(priority);
            if priority < *held {
                *held = priority;
            }
        }
        let previous = self.requests.get(&owner);
        if previous == Some(&wanted) {
            return;
        }
        // Only what has just come on screen: the reach ahead moves with the
        // scroll's speed, and a key flickering at its edge would otherwise
        // send each flicker to a server that is not answering.
        let shown: Vec<Art> = wanted
            .iter()
            .filter(|(k, p)| {
                **p == Priority::Visible
                    && previous.and_then(|prev| prev.get(k)) != Some(&Priority::Visible)
            })
            .map(|(k, _)| k.art)
            .collect();
        if !shown.is_empty() {
            let mut queue = self.shared.queue.lock();
            for art in shown {
                queue.failed.remove(&art);
            }
        }
        self.requests.insert(owner, wanted);
        self.publish();
    }

    /// A list or page going away: nothing it asked for is wanted on its
    /// account.
    pub fn release(&mut self, owner: EntityId) {
        if self.requests.remove(&owner).is_some() {
            self.publish();
        }
    }

    /// The connection to the server changed: what failed may work now.
    pub fn retry_failed(&mut self) {
        let mut queue = self.shared.queue.lock();
        if queue.failed.is_empty() {
            return;
        }
        queue.failed.clear();
        let fetches = queue.wanted.keys().filter(|k| queue.fetchable(k)).count();
        drop(queue);
        self.shared.wake(fetches, 0);
    }

    fn publish(&mut self) {
        let mut wanted: HashMap<Key, Priority> = HashMap::new();
        for request in self.requests.values() {
            for (key, priority) in request.iter().filter(|(k, _)| !self.memory.contains_key(k)) {
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
        // A bitmap decoded and since evicted from memory is wanted again;
        // one staged for the atlas is not.
        let (memory, staged) = (&self.memory, &self.staged);
        queue
            .decoded
            .retain(|k| memory.contains_key(k) || staged.contains_key(k));
        let fetches = wanted
            .keys()
            .filter(|k| !queue.wanted.contains_key(k) && queue.fetchable(k))
            .count();
        let decodes = wanted
            .keys()
            .filter(|k| !queue.wanted.contains_key(k) && queue.decodable(k))
            .count();
        queue.wanted = wanted;
        drop(queue);
        self.shared.wake(fetches, decodes);
    }

    fn land(&mut self, done: Done, cx: &mut Context<Self>) {
        match done {
            Done::Decoded {
                key,
                generation,
                image,
                colour,
            } => {
                let mut queue = self.shared.queue.lock();
                if queue.generation(key.art) != generation {
                    // Made for the record that held this id before. Not
                    // marked decoded, so the new record's is made.
                    queue.decoded.remove(&key);
                    let decodes =
                        usize::from(queue.wanted.contains_key(&key) && queue.decodable(&key));
                    drop(queue);
                    self.shared.wake(0, decodes);
                    return;
                }
                drop(queue);
                self.colours.insert(key.art, colour);
                self.staged.insert(key, (image, generation));
            }
            Done::Absent => {}
        }
        cx.notify();
    }

    /// Take staged sleeves into memory up to `budget` bytes, and so into the
    /// atlas at this frame's paint: the most urgent first, by what the
    /// lists on screen asked for. Returns whether more are staged, in which
    /// case the caller asks for another frame. Never notifies, so a list can
    /// call it from its own layout.
    /// `window` is the window being drawn, when called while drawing it: an
    /// image dropped from the atlas from inside a window's update must name
    /// that window, which the app's list of windows does not hold just then.
    pub fn admit(
        &mut self,
        budget: usize,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.staged.is_empty() {
            return false;
        }
        let queue = self.shared.queue.lock();
        let mut ranked: Vec<(Key, Option<Priority>, u64)> = self
            .staged
            .keys()
            .map(|k| (*k, queue.wanted.get(k).copied(), queue.generation(k.art)))
            .collect();
        drop(queue);
        ranked.sort_by(|a, b| match (a.1, b.1) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        let mut spent = 0;
        for (key, _, current) in ranked {
            if spent > 0 && spent >= budget {
                break;
            }
            let Some((image, generation)) = self.staged.remove(&key) else {
                continue;
            };
            if current != generation {
                continue;
            }
            let size = image.size(0);
            let bytes = (i32::from(size.width) * i32::from(size.height) * 4) as usize;
            spent += bytes;
            let held = Held {
                image,
                bytes,
                used: Instant::now(),
            };
            if let Some(old) = self.memory.insert(key, held) {
                self.memory_bytes -= old.bytes;
                cx.drop_image(old.image, window.as_deref_mut());
            } else {
                self.sizes.entry(key.art).or_default().push(key.px);
            }
            self.memory_bytes += bytes;
        }
        self.trim(window, cx);
        !self.staged.is_empty()
    }

    fn forget(&mut self, key: Key, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if let Some(held) = self.memory.remove(&key) {
            self.memory_bytes -= held.bytes;
            if let Some(sizes) = self.sizes.get_mut(&key.art) {
                sizes.retain(|p| *p != key.px);
                if sizes.is_empty() {
                    self.sizes.remove(&key.art);
                }
            }
            cx.drop_image(held.image, window);
        }
    }

    /// Back under budget. What nobody is asking for goes first, oldest first;
    /// then what is wanted least, farthest ahead first. A bitmap drawn in the
    /// last moment, or still held by an element, is never dropped: the atlas
    /// would upload it again at the next paint and nothing would drop it
    /// after that. Evicting what is wanted only after the rest is what keeps
    /// a tight budget from evicting and re-decoding the rows ahead in a loop.
    fn trim(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.memory_bytes <= MEMORY_BUDGET {
            return;
        }
        let target = MEMORY_BUDGET / 10 * 9;
        let recent = Instant::now() - Duration::from_millis(500);
        let mut wanted: HashMap<Key, Priority> = HashMap::new();
        for request in self.requests.values() {
            for (key, priority) in request {
                let p = wanted.entry(*key).or_insert(*priority);
                if priority < p {
                    *p = *priority;
                }
            }
        }
        let rank = |key: &Key| match wanted.get(key) {
            None => 0.0,
            Some(Priority::Ahead(d)) => 1.0 / (1.0 + d),
            Some(Priority::Landing(_)) => 2.0,
            Some(Priority::Visible) => 3.0,
        };
        let mut candidates: Vec<(Key, f32, Instant)> = self
            .memory
            .iter()
            .filter(|(_, h)| h.used < recent && Arc::strong_count(&h.image) == 1)
            .map(|(k, h)| (*k, rank(k), h.used))
            .collect();
        candidates.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.2.cmp(&b.2)));
        let mut dropped = Vec::new();
        for (key, _, _) in candidates {
            if self.memory_bytes <= target {
                break;
            }
            self.forget(key, window.as_deref_mut(), cx);
            dropped.push(key);
        }
        let mut queue = self.shared.queue.lock();
        for key in dropped {
            queue.decoded.remove(&key);
        }
    }

    /// Forget what the library has deleted since last asked: its files, its
    /// bitmaps, and any fetch or decode still running for it. Called at launch
    /// and on each library change.
    pub fn sweep(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let after = self.evictions_seq;
        cx.spawn(async move |this, cx| {
            let gone = engine.art_evictions(after).await;
            this.update(cx, |covers, cx| match gone {
                Ok(gone) => covers.evict(gone, cx),
                // Not knowing what went is no reason to show nothing, but it
                // is reason not to trust the files: records are fetched
                // afresh until a sweep lands.
                Err(e) => {
                    log::warn!("art evictions: {e}");
                    covers.allow_fetches(false);
                }
            })
            .ok();
        })
        .detach();
    }

    fn allow_fetches(&mut self, trust_disk: bool) {
        let mut queue = self.shared.queue.lock();
        queue.disk_trusted |= trust_disk;
        if queue.fetches_allowed {
            return;
        }
        queue.fetches_allowed = true;
        let fetches = queue.wanted.keys().filter(|k| queue.fetchable(k)).count();
        drop(queue);
        self.shared.wake(fetches, 0);
    }

    fn evict(&mut self, gone: koan_ffi::ArtEvictions, cx: &mut Context<Self>) {
        let fresh = gone.seq > self.evictions_seq;
        let arts: Vec<Art> = if fresh {
            gone.albums
                .iter()
                .map(|id| Art::Album(*id))
                .chain(gone.artists.iter().map(|id| Art::Artist(*id)))
                .collect()
        } else {
            Vec::new()
        };
        {
            // Held back from the fetch lane until their files are gone, so it
            // cannot find one between the generation moving and the unlink.
            let mut queue = self.shared.queue.lock();
            for art in &arts {
                *queue.generation.entry(*art).or_default() += 1;
                queue.on_disk.remove(art);
                queue.absent.remove(art);
                queue.failed.remove(art);
                *queue.evicting.entry(*art).or_default() += 1;
                queue.decoded.retain(|k| k.art != *art);
            }
        }
        if !arts.is_empty() {
            let (dir, shared, arts) = (self.dir.clone(), self.shared.clone(), arts.clone());
            cx.background_executor()
                .spawn(async move {
                    let disk = shared.disk.lock();
                    for art in &arts {
                        std::fs::remove_file(dir.join(art.filename())).ok();
                    }
                    drop(disk);
                    let mut queue = shared.queue.lock();
                    for art in &arts {
                        if let Some(n) = queue.evicting.get_mut(art) {
                            *n -= 1;
                            if *n == 0 {
                                queue.evicting.remove(art);
                            }
                        }
                    }
                    // What is on screen under those ids is the new record's.
                    let fetches = queue
                        .wanted
                        .keys()
                        .filter(|k| arts.contains(&k.art) && queue.fetchable(k))
                        .count();
                    drop(queue);
                    shared.wake(fetches, 0);
                })
                .detach();
        }
        self.staged.retain(|k, _| !arts.contains(&k.art));
        for art in &arts {
            self.colours.remove(art);
        }
        let drop: Vec<Key> = self
            .memory
            .keys()
            .filter(|k| arts.contains(&k.art))
            .copied()
            .collect();
        for key in drop {
            // Outside any window's update: the app reaches every window.
            self.forget(key, None, cx);
        }
        self.allow_fetches(true);
        if !fresh {
            return;
        }
        self.evictions_seq = gone.seq;
        let (dir, seq) = (self.dir.clone(), gone.seq);
        cx.background_executor()
            .spawn(async move {
                write_atomically(&dir.join("evictions-seq"), seq.to_string().as_bytes()).ok();
            })
            .detach();
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
/// ids would show its covers; the platform's cache directory otherwise, and
/// the temporary directory where there is none. Names it only: see
/// `prepare_directory`.
fn directory() -> PathBuf {
    let root = match std::env::var_os("KOAN_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join("artwork"),
        None => dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("koan-gui")
            .join("artwork"),
    };
    root.join(SCHEME)
}

/// Makes the directory, deletes any other scheme's beside it, and reads the
/// eviction mark kept there. Off the main thread.
fn prepare_directory(dir: &Path) -> i64 {
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!("artwork directory {}: {e}", dir.display());
        return 0;
    }
    // Nothing here is authoritative, so an older scheme is deleted rather
    // than migrated; it refills as pages are drawn.
    if let Some(root) = dir.parent() {
        for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
            if entry.file_name() != SCHEME {
                std::fs::remove_dir_all(entry.path()).ok();
            }
        }
    }
    std::fs::read_to_string(dir.join("evictions-seq"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
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
    dir: PathBuf,
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
        let file = dir.join(art.filename());
        let usable = |queue: &Queue| {
            queue.disk_trusted
                && !queue.evicting.contains_key(&art)
                && queue.generation(art) == generation
        };
        let disk = shared.disk.lock();
        if usable(&shared.queue.lock()) && file.exists() {
            // Checked again: an eviction may have moved the generation
            // while the file was looked for, though not yet unlinked it.
            let mut queue = shared.queue.lock();
            if usable(&queue) {
                queue.fetching.remove(&art);
                queue.on_disk.insert(art);
                let decodes = queue.wanted.keys().filter(|k| k.art == art).count();
                drop(queue);
                drop(disk);
                shared.wake(0, decodes);
                continue;
            }
        }
        drop(disk);
        let fetched = futures::executor::block_on(async {
            match art {
                Art::Album(id) => {
                    engine
                        .clone()
                        .album_cover_art(id, Some(SOURCE_PIXELS))
                        .await
                }
                Art::Artist(id) => engine.clone().artist_image(id).await,
            }
        });
        // Written beside its final name and renamed only while the record
        // is still the one fetched. An eviction after the check unlinks the
        // file once the rename is done, and moves the generation, so the
        // file is not counted as on disk below.
        let staged = match &fetched {
            Ok(Some(cover)) => {
                let tmp = file.with_extension("part");
                std::fs::write(&tmp, &cover.data).ok().map(|()| tmp)
            }
            _ => None,
        };
        let renamed = staged.is_some_and(|tmp| {
            let disk = shared.disk.lock();
            let renamed = shared.queue.lock().generation(art) == generation
                && std::fs::rename(&tmp, &file).is_ok();
            drop(disk);
            if !renamed {
                std::fs::remove_file(tmp).ok();
            }
            renamed
        });
        let mut queue = shared.queue.lock();
        queue.fetching.remove(&art);
        let current = queue.generation(art) == generation;
        let mut absent = false;
        match fetched {
            Ok(Some(_)) if current && renamed => {
                queue.on_disk.insert(art);
            }
            // Could not be written: as good as a failed fetch.
            Ok(Some(_)) if current => {
                queue.failed.insert(art);
            }
            Ok(None) if current => {
                queue.absent.insert(art);
                absent = true;
            }
            Err(e) if current => {
                log::debug!("art for {art:?}: {e}");
                queue.failed.insert(art);
            }
            _ => {}
        }
        let decodes = queue
            .wanted
            .keys()
            .filter(|k| k.art == art && queue.decodable(k))
            .count();
        // Overtaken by an eviction: the record now holding the id is fetched.
        let refetch = !current
            && queue
                .wanted
                .keys()
                .any(|k| k.art == art && queue.fetchable(k));
        drop(queue);
        shared.wake(usize::from(refetch), decodes);
        if absent && tx.unbounded_send(Done::Absent).is_err() {
            return;
        }
    }
}

fn decode_lane(shared: Arc<Shared>, dir: PathBuf, tx: mpsc::UnboundedSender<Done>) {
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
        let decoded = match std::fs::read(&file) {
            Ok(data) => decode(&data, key.px).ok_or(Failure::Broken),
            Err(_) => Err(Failure::Missing),
        };
        let mut queue = shared.queue.lock();
        queue.decoding.remove(&key);
        let current = queue.generation(key.art) == generation;
        match decoded {
            Ok(image) => {
                // Marked only when it still belongs to the record holding the
                // id; otherwise the new record's is made in its place.
                if current {
                    queue.decoded.insert(key);
                }
                drop(queue);
                if !current {
                    continue;
                }
                let (image, colour) = image;
                let done = Done::Decoded {
                    key,
                    generation,
                    image: Arc::new(image),
                    colour,
                };
                if tx.unbounded_send(done).is_err() {
                    return;
                }
            }
            // Gone from disk (a cache cleared under it): fetched again.
            Err(Failure::Missing) => {
                let fetch = current && queue.on_disk.remove(&key.art);
                drop(queue);
                shared.wake(usize::from(fetch), 0);
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
                if current && tx.unbounded_send(Done::Absent).is_err() {
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

/// The sleeve filling a square `px` on a side, centred, as BGRA, and its
/// average colour for the field drawn while it waits for the atlas.
fn decode(data: &[u8], px: u32) -> Option<(RenderImage, Hsla)> {
    // A file small enough to be served as it is keeps its EXIF orientation.
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = image::ImageDecoder::orientation(&mut decoder)
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
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
    let (mut sum, mut n) = ([0u64; 3], 0u64);
    for pixel in rgba.pixels().step_by(7) {
        for (c, v) in sum.iter_mut().zip(pixel.0) {
            *c += u64::from(v);
        }
        n += 1;
    }
    let mean = |c: u64| c as f32 / n.max(1) as f32 / 255.0;
    let colour: Hsla = Rgba {
        r: mean(sum[0]),
        g: mean(sum[1]),
        b: mean(sum[2]),
        a: 1.0,
    }
    .into();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some((RenderImage::new(smallvec![image::Frame::new(rgba)]), colour))
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` brings GPUI's own `test` attribute in with it.
    use core::prelude::v1::test;

    #[test]
    fn keys_round_up_to_a_bucket() {
        assert_eq!(Key::new(Art::Album(1), px(200.0), 2.0).px, 416);
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
            .write_to(
                &mut std::io::Cursor::new(&mut data),
                image::ImageFormat::Png,
            )
            .unwrap();
        let (image, colour) = decode(&data, 100).unwrap();
        assert!((colour.h - 0.0).abs() < 0.01 && colour.s > 0.9);
        let size = image.size(0);
        assert_eq!((i32::from(size.width), i32::from(size.height)), (100, 100));
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
        assert!(decode(b"junk", 100).is_none());
    }

    #[test]
    fn nothing_is_fetched_before_the_first_sweep_and_failures_wait() {
        let mut q = Queue::default();
        let (a, b) = (Art::Album(1), Art::Album(2));
        q.wanted
            .insert(Key { art: a, px: 64 }, Priority::Ahead(5.0));
        q.wanted.insert(Key { art: b, px: 64 }, Priority::Visible);
        assert_eq!(q.next_fetch(), None);
        q.fetches_allowed = true;
        assert_eq!(q.next_fetch(), Some(b));
        q.absent.insert(b);
        assert_eq!(q.next_fetch(), Some(a));
        q.failed.insert(a);
        assert_eq!(q.next_fetch(), None);
        q.on_disk.insert(a);
        assert_eq!(q.next_decode(), Some(Key { art: a, px: 64 }));
        q.decoded.insert(Key { art: a, px: 64 });
        assert_eq!(q.next_decode(), None);
    }
}
