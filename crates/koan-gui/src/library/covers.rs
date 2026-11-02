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
use koan_ffi::KoanEngine;
use koan_kit::app::*;
// decoded images handed to the renderer.
use koan_kit::raw::RenderImage;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{Hsla, Rgba};
use parking_lot::{Condvar, Mutex};
use smallvec::{SmallVec, smallvec};

/// What is asked of the engine, and what lands on disk: large enough for a
/// record's own page on a 2× display, small enough that a library's worth is
/// a cache rather than a second copy of the artwork.
const SOURCE_PIXELS: u32 = 600;

/// The most the bitmaps may hold, roughly 360 grid tiles at 2×, however much
/// the lists ask for. Each bitmap is held twice on unified memory, here and
/// as its copy in the GPU's atlas, so the ceiling is rarely the bound: the
/// budget is what the lists on screen ask for plus `RETAIN`.
const MEMORY_CEILING: usize = 256 * 1024 * 1024;

/// Kept beyond what the lists ask for: about a screen of grid tiles at 2×,
/// so scrolling back a little finds its sleeves still in memory.
const RETAIN: usize = 32 * 1024 * 1024;

/// How long after a list goes away its sleeves are let go: past the moment
/// in which a bitmap drawn is never dropped (see `trim`).
const RELEASE_AFTER: Duration = Duration::from_millis(600);

/// Draw sizes are rounded up to a multiple of this, so a window being resized
/// does not decode every tile again at each pixel.
const BUCKET: u32 = 32;

/// How many bytes of new sleeves a frame takes into the GPU's atlas: about a
/// dozen tiles at 2×. Each is copied into a texture on the main thread inside
/// that frame's paint, so a flick that lands forty at once spreads them over
/// frames, each tile showing its sleeve's colour until its turn. At least one
/// is taken a frame, whatever its size.
pub const UPLOAD_BYTES_PER_FRAME: usize = 8 * 1024 * 1024;

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
    /// while that decodes, and when it entered memory: a tile that had
    /// nothing to show fades it in from then.
    Ready(Arc<RenderImage>, Instant),
    /// Decoded and waiting its turn for the atlas, or decoded once and since
    /// evicted: drawn as a flat field of the sleeve's colour.
    Colour(Hsla),
    /// Not here yet.
    Waiting,
    /// The record has no art.
    None,
}

impl From<Sleeve> for crate::components::CoverArt {
    fn from(sleeve: Sleeve) -> Self {
        use crate::components::CoverArt;
        match sleeve {
            Sleeve::Ready(image, _) => CoverArt::Image(image),
            Sleeve::Colour(colour) => CoverArt::Colour(colour),
            Sleeve::Waiting | Sleeve::None => CoverArt::Missing,
        }
    }
}

impl Sleeve {
    /// The same thing to draw: the same bitmap, colour or absence.
    pub fn same(&self, other: &Sleeve) -> bool {
        match (self, other) {
            (Sleeve::Ready(a, _), Sleeve::Ready(b, _)) => Arc::ptr_eq(a, b),
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
    /// When it entered memory.
    arrived: Instant,
}

/// What both lanes and the main thread share, behind one lock that is held
/// only across map reads and writes.
#[derive(Default)]
struct Queue {
    /// Every request, by key, at the most urgent priority anyone asked for.
    wanted: HashMap<Key, Priority>,
    on_disk: HashSet<Art>,
    fetching: HashSet<Art>,
    /// Each running fetch's way to be called off: sent, or dropped, it ends
    /// the fetch at once and cancels its request.
    cancels: HashMap<Art, (u64, futures_channel::oneshot::Sender<()>)>,
    /// Names each fetch started, so one called off and finishing anyway
    /// does not end another started since for the same record.
    tickets: u64,
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
    /// How many fetches may run at once: twice the covers on screen, so
    /// what is shown and where the scroll is heading are fetched together,
    /// up to `MAX_FETCHES`. Falls as covers land and the screen fills.
    limit: usize,
    /// The congestion window: how many fetches the server is answering
    /// promptly, grown and cut by `observe` as TCP's is. Zero until the first
    /// answer, read as `WINDOW_START`.
    window: f64,
    /// The last `HISTORY` answers' times, newest last. A server's answers are
    /// bimodal (a cover it keeps comes back in tens of milliseconds, one it
    /// reads off a disk in hundreds), so it is judged by medians, never by
    /// its quickest.
    answers: std::collections::VecDeque<Duration>,
    /// Answers since the window was last judged: one judgement per round
    /// trip, a window's worth of answers.
    since_judged: usize,
    /// Round trips in a row whose recent median stood well above the
    /// history's.
    slow_rounds: u32,
    /// When the window was last cut, so a burst of failures cuts it once.
    last_cut: Option<Instant>,
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

    /// Fetches running for records no longer asked for: scrolled away.
    fn abandoned(&self) -> Vec<Art> {
        let wanted: HashSet<Art> = self.wanted.keys().map(|k| k.art).collect();
        self.fetching
            .iter()
            .filter(|art| !wanted.contains(art))
            .copied()
            .collect()
    }

    /// Calls off what must give way, and returns how many places that frees:
    /// every fetch for a record no longer asked for, and, while covers on
    /// screen wait for a place, fetches for covers off screen, the least
    /// urgent first. Nothing in flight for a cover off screen holds back one
    /// on it.
    fn give_way(&mut self) -> usize {
        let mut called_off = self.abandoned();
        let visible_waiting = self
            .wanted
            .iter()
            .filter(|(k, p)| **p == Priority::Visible && self.fetchable(k))
            .count();
        let free = self
            .allowed()
            .saturating_sub(self.fetching.len() - called_off.len());
        if visible_waiting > free {
            let urgency = |art: Art| {
                self.wanted
                    .iter()
                    .filter(|(k, _)| k.art == art)
                    .map(|(_, p)| *p)
                    .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            };
            let mut off_screen: Vec<(Art, Priority)> = self
                .fetching
                .iter()
                .filter(|art| !called_off.contains(art))
                .filter_map(|art| urgency(*art).map(|p| (*art, p)))
                .filter(|(_, p)| *p != Priority::Visible)
                .collect();
            // Least urgent first.
            off_screen.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            called_off.extend(
                off_screen
                    .into_iter()
                    .take(visible_waiting - free)
                    .map(|(art, _)| art),
            );
        }
        for art in &called_off {
            if let Some((_, cancel)) = self.cancels.remove(art) {
                cancel.send(()).ok();
            }
            self.fetching.remove(art);
        }
        called_off.len()
    }

    /// How many fetches may run now: the window, under the limit.
    fn allowed(&self) -> usize {
        let window = if self.window == 0.0 {
            WINDOW_START
        } else {
            self.window as usize
        };
        window.min(self.limit).max(1)
    }

    /// One fetch answered after `took`, or failed. Every answer grows the
    /// window by 1/window, about one place per round trip. A failure halves
    /// it at once (once per round trip). Otherwise it halves only when the
    /// median of the recent answers has stood more than `SLOWER` times the
    /// median of the longer history for `SLOW_ROUNDS` round trips running:
    /// a server whose disk is seeking under load, not one answering a mix of
    /// kept and unkept covers. Returns whether the window grew by a whole
    /// place, which wakes one more fetcher.
    fn observe(&mut self, took: Duration, ok: bool) -> bool {
        let before = self.allowed();
        if self.window == 0.0 {
            self.window = WINDOW_START as f64;
        }
        if !ok {
            let typical = median(self.answers.iter().copied()).unwrap_or(Duration::ZERO);
            if self
                .last_cut
                .is_none_or(|at| at.elapsed() > typical.max(SETTLE))
            {
                self.cut(format_args!("a fetch failed after {} ms", took.as_millis()));
            }
            return false;
        }
        if self.answers.len() == HISTORY {
            self.answers.pop_front();
        }
        self.answers.push_back(took);
        self.window = (self.window + 1.0 / self.window).min(MAX_FETCHES as f64);
        self.since_judged += 1;
        if self.since_judged >= self.window as usize {
            self.since_judged = 0;
            let recent = median(self.answers.iter().rev().take(RECENT).copied());
            let typical = median(self.answers.iter().copied());
            match (recent, typical) {
                (Some(recent), Some(typical))
                    if self.answers.len() >= 2 * RECENT && recent > typical * SLOWER =>
                {
                    self.slow_rounds += 1;
                    if self.slow_rounds >= SLOW_ROUNDS {
                        self.slow_rounds = 0;
                        self.cut(format_args!(
                            "answers slowed: median {} ms lately against {} ms",
                            recent.as_millis(),
                            typical.as_millis()
                        ));
                    }
                }
                _ => self.slow_rounds = 0,
            }
        }
        self.allowed() > before
    }

    fn cut(&mut self, why: std::fmt::Arguments) {
        self.window = (self.window / 2.0).max(WINDOW_MIN as f64);
        self.last_cut = Some(Instant::now());
        log::debug!("covers: {why}; window now {}", self.window as usize);
    }

    /// A fetch done: its place is freed, unless it was called off and the
    /// record has been started again since.
    fn finish(&mut self, art: Art, ticket: u64) {
        if self.cancels.get(&art).is_some_and(|(t, _)| *t == ticket) {
            self.cancels.remove(&art);
            self.fetching.remove(&art);
        }
    }

    fn next_fetch(&self) -> Option<Art> {
        if self.fetching.len() >= self.allowed() {
            return None;
        }
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
        for _ in 0..fetches.min(MAX_FETCHES) {
            self.fetch_ready.notify_one();
        }
        for _ in 0..decodes {
            self.decode_ready.notify_one();
        }
    }
}

/// The most covers fetched at once, however many are on screen: a fast
/// scroll through a large library cannot flood the uplink, sockets or disk.
pub const MAX_FETCHES: usize = 512;

/// Where the congestion window starts, and the least it is cut to. A server
/// reading covers off a spinning disk answers 16 at once well and 512 badly;
/// the window finds what this one takes.
const WINDOW_START: usize = 16;
const WINDOW_MIN: usize = 8;
/// How many answers the history holds, and how many of the newest are
/// "recent".
const HISTORY: usize = 256;
const RECENT: usize = 16;
/// How many times the history's median the recent median must stand, and
/// for how many round trips running, before the window is cut.
const SLOWER: u32 = 3;
const SLOW_ROUNDS: u32 = 3;
/// The least time between two cuts for failures.
const SETTLE: Duration = Duration::from_millis(500);

/// The middle of `times`, or `None` when there are none.
fn median(times: impl Iterator<Item = Duration>) -> Option<Duration> {
    let mut times: Vec<Duration> = times.collect();
    if times.is_empty() {
        return None;
    }
    let mid = times.len() / 2;
    Some(*times.select_nth_unstable(mid).1)
}

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
    /// Fetch threads started so far: grown to the limit as it rises, kept
    /// once started, waiting on the queue while there is nothing to fetch.
    fetchers: usize,
    fetch_tx: mpsc::UnboundedSender<Done>,
    _results: Task<()>,
}

impl Covers {
    pub fn new(engine: Arc<KoanEngine>, cx: &mut Context<Self>) -> Self {
        // The cache bounds its own fetches by what is on screen.
        engine.set_cover_fetch_limit(MAX_FETCHES as u32);
        let dir = directory();
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            disk: Mutex::new(()),
            fetch_ready: Condvar::new(),
            decode_ready: Condvar::new(),
        });
        let (tx, mut rx) = mpsc::unbounded();
        // Half the cores: the engine reads and resizes local covers on the
        // other half (`CoverCore`), and the two together fill the machine
        // without either thrashing it.
        let decoders = std::thread::available_parallelism()
            .map_or(2, |n| n.get() / 2)
            .max(2);
        for n in 0..decoders {
            let (shared, dir, tx) = (shared.clone(), dir.clone(), tx.clone());
            std::thread::Builder::new()
                .name(format!("koan-art-decode-{n}"))
                .spawn(move || decode_lane(shared, dir, tx))
                .expect("spawn art decoder");
        }
        let fetch_tx = tx.clone();
        drop(tx);
        let results = cx.spawn(async move |this, cx| {
            // Everything that has landed since the last pass lands together,
            // with one notify: covers ready in the same frame appear together
            // rather than one at a time.
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let landed = this.update(cx, |covers, cx| {
                    for done in batch {
                        covers.land(done);
                    }
                    cx.notify();
                });
                if landed.is_err() {
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
            fetchers: 0,
            fetch_tx,
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
            return Sleeve::Ready(held.image.clone(), held.arrived);
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
    /// Its sleeves are trimmed to the smaller budget once nothing can be
    /// drawing them, outside any window's update, so each one dropped leaves
    /// every window's atlas too.
    pub fn release(&mut self, owner: EntityId, cx: &mut Context<Self>) {
        if self.requests.remove(&owner).is_some() {
            self.publish();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RELEASE_AFTER).await;
                this.update(cx, |covers, cx| covers.trim(None, cx)).ok();
            })
            .detach();
        }
    }

    /// Every key some list or page asks for, at its most urgent priority.
    fn asked(&self) -> HashMap<Key, Priority> {
        let mut wanted: HashMap<Key, Priority> = HashMap::new();
        for request in self.requests.values() {
            for (key, priority) in request {
                let p = wanted.entry(*key).or_insert(*priority);
                if priority < p {
                    *p = *priority;
                }
            }
        }
        wanted
    }

    /// What memory may hold now: what is asked for, and `RETAIN` beyond it,
    /// under `MEMORY_CEILING`.
    fn budget(asked: &HashMap<Key, Priority>) -> usize {
        let asked: usize = asked.keys().map(|k| (k.px * k.px * 4) as usize).sum();
        (asked + RETAIN).min(MEMORY_CEILING)
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

    /// Starts fetch threads until there are as many as `limit`. A fetch
    /// waits on a disk or a server, so each holds a thread while it does.
    fn grow_fetchers(&mut self, limit: usize) {
        while self.fetchers < limit {
            let (shared, dir, tx, engine) = (
                self.shared.clone(),
                self.dir.clone(),
                self.fetch_tx.clone(),
                self.engine.clone(),
            );
            let n = self.fetchers;
            let started = std::thread::Builder::new()
                .name(format!("koan-art-fetch-{n}"))
                // A fetch needs little stack: it waits on a socket.
                .stack_size(256 * 1024)
                .spawn(move || fetch_lane(shared, dir, engine, tx));
            if started.is_err() {
                break;
            }
            self.fetchers += 1;
        }
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
        let shown = wanted
            .iter()
            .filter(|(k, p)| **p == Priority::Visible && !queue.on_disk.contains(&k.art))
            .count();
        queue.limit = (shown * 2).clamp(1, MAX_FETCHES);
        queue.wanted = wanted;
        // The abandoned still hold threads while they finish: enough more
        // that the limit's worth run beside them, under the ceiling.
        let called_off = queue.give_way();
        let threads = queue.limit;
        let (live, limit) = (queue.fetching.len(), queue.allowed());
        let waiting = queue.wanted.keys().filter(|k| queue.fetchable(k)).count();
        drop(queue);
        log::debug!(
            "covers: {live} fetching, {called_off} called off, {waiting} waiting, {limit} allowed"
        );
        self.grow_fetchers(threads);
        // Every free place, not only the new requests: what was waiting
        // behind fetches called off can start now.
        self.shared.wake(
            fetches.max(waiting.min(limit.saturating_sub(live))),
            decodes,
        );
    }

    fn land(&mut self, done: Done) {
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
        // Decoded for a list that has since scrolled past or gone: dropped
        // rather than held, and decoded again if it is asked for again.
        let asked = self.asked();
        let stale: Vec<Key> = self
            .staged
            .keys()
            .filter(|k| !asked.contains_key(k))
            .copied()
            .collect();
        if !stale.is_empty() {
            let mut queue = self.shared.queue.lock();
            for key in stale {
                self.staged.remove(&key);
                queue.decoded.remove(&key);
            }
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
            let now = Instant::now();
            let held = Held {
                image,
                bytes,
                used: now,
                arrived: now,
            };
            if let Some(old) = self.memory.insert(key, held) {
                self.memory_bytes -= old.bytes;
                cx.drop_image(old.image, window.as_deref_mut());
            } else {
                self.sizes.entry(key.art).or_default().push(key.px);
            }
            self.memory_bytes += bytes;
            // Its other sizes stood in while this one decoded; those nobody
            // asks for and nothing draws go now.
            let stand_ins: SmallVec<[u32; 4]> = self.sizes[&key.art]
                .iter()
                .copied()
                .filter(|px| *px != key.px)
                .filter(|px| {
                    let other = Key {
                        art: key.art,
                        px: *px,
                    };
                    !asked.contains_key(&other)
                        && self
                            .memory
                            .get(&other)
                            .is_some_and(|h| Arc::strong_count(&h.image) == 1)
                })
                .collect();
            for px in stand_ins {
                let other = Key { art: key.art, px };
                self.forget(other, window.as_deref_mut(), cx);
                self.shared.queue.lock().decoded.remove(&other);
            }
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
        let wanted = self.asked();
        let budget = Self::budget(&wanted);
        if self.memory_bytes <= budget {
            return;
        }
        let target = budget / 10 * 9;
        let recent = Instant::now() - Duration::from_millis(500);
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

    /// What the artwork directory holds: how many covers and their bytes.
    /// Reads the directory: call it off the main thread.
    pub fn on_disk(dir: &Path) -> (usize, u64) {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| is_cover(&e.file_name().to_string_lossy()))
            .filter_map(|e| e.metadata().ok())
            .fold((0, 0), |(n, bytes), m| (n + 1, bytes + m.len()))
    }

    pub fn dir(&self) -> PathBuf {
        self.dir.clone()
    }

    /// Forgets every cover: the files, the bitmaps and their GPU images,
    /// the colours, what was found to have no art and what failed. Fetches
    /// in flight land on a moved generation and are dropped. The disk is not
    /// trusted until the files are gone, then what is on screen is fetched
    /// again.
    pub fn clear(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        {
            let mut queue = self.shared.queue.lock();
            let known: HashSet<Art> = queue
                .on_disk
                .iter()
                .chain(&queue.fetching)
                .chain(&queue.absent)
                .chain(&queue.failed)
                .copied()
                .chain(queue.decoded.iter().map(|k| k.art))
                .chain(self.memory.keys().map(|k| k.art))
                .chain(self.colours.keys().copied())
                .collect();
            for art in known {
                *queue.generation.entry(art).or_default() += 1;
            }
            queue.on_disk.clear();
            queue.absent.clear();
            queue.failed.clear();
            queue.decoded.clear();
            queue.disk_trusted = false;
        }
        self.staged.clear();
        self.colours.clear();
        let keys: Vec<Key> = self.memory.keys().copied().collect();
        for key in keys {
            self.forget(key, window.as_deref_mut(), cx);
        }
        let (dir, shared) = (self.dir.clone(), self.shared.clone());
        cx.background_executor()
            .spawn(async move {
                let disk = shared.disk.lock();
                for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    if is_cover(&entry.file_name().to_string_lossy()) {
                        std::fs::remove_file(entry.path()).ok();
                    }
                }
                drop(disk);
                let mut queue = shared.queue.lock();
                queue.disk_trusted = true;
                let fetches = queue.wanted.keys().filter(|k| queue.fetchable(k)).count();
                drop(queue);
                shared.wake(fetches, 0);
            })
            .detach();
        cx.notify();
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

/// A record's cover file, as `Art::filename` names them.
fn is_cover(name: &str) -> bool {
    name.starts_with("album-") || name.starts_with("artist-")
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
        let (art, generation, called_off, ticket) = {
            let mut queue = shared.queue.lock();
            loop {
                if queue.closed {
                    return;
                }
                if let Some(art) = queue.next_fetch() {
                    queue.fetching.insert(art);
                    let (cancel, called_off) = futures_channel::oneshot::channel();
                    queue.tickets += 1;
                    let ticket = queue.tickets;
                    queue.cancels.insert(art, (ticket, cancel));
                    break (art, queue.generation(art), called_off, ticket);
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
                queue.finish(art, ticket);
                queue.on_disk.insert(art);
                let decodes = queue.wanted.keys().filter(|k| k.art == art).count();
                drop(queue);
                drop(disk);
                shared.wake(0, decodes);
                continue;
            }
        }
        drop(disk);
        let started = Instant::now();
        let fetch = async {
            match art {
                Art::Album(id) => {
                    engine
                        .clone()
                        .album_cover_art_cancellable(id, Some(SOURCE_PIXELS))
                        .await
                }
                Art::Artist(id) => engine.clone().artist_image(id).await,
            }
        };
        // Called off (scrolled away, or giving way to a cover on screen):
        // the request is dropped, which cancels it, and the place is free.
        let fetched =
            match futures::executor::block_on(futures::future::select(Box::pin(fetch), called_off))
            {
                futures::future::Either::Left((fetched, _)) => fetched,
                futures::future::Either::Right(_) => {
                    let fetches = usize::from(shared.queue.lock().next_fetch().is_some());
                    shared.wake(fetches, 0);
                    continue;
                }
            };
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
        queue.finish(art, ticket);
        let grew = queue.observe(started.elapsed(), fetched.is_ok());
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
        shared.wake(usize::from(refetch) + usize::from(grew), decodes);
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
    // Sizes for the keys the tests make.
    use koan_kit::raw::px;

    #[test]
    fn the_budget_is_what_is_asked_for_and_a_screen_more() {
        let key = |id| Key {
            art: Art::Album(id),
            px: 416,
        };
        let tile = 416 * 416 * 4;
        assert_eq!(Covers::budget(&HashMap::new()), RETAIN);
        let screen: HashMap<Key, Priority> =
            (0..40).map(|id| (key(id), Priority::Visible)).collect();
        assert_eq!(Covers::budget(&screen), 40 * tile + RETAIN);
        let flood: HashMap<Key, Priority> = (0..10_000)
            .map(|id| (key(id), Priority::Ahead(1.0)))
            .collect();
        assert_eq!(Covers::budget(&flood), MEMORY_CEILING);
    }

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

    /// What clearing would delete: the covers, not the directory's own record
    /// of where the evictions got to.
    #[test]
    fn the_cache_is_measured_by_its_covers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("album-1"), [0; 100]).unwrap();
        std::fs::write(dir.path().join("artist-2"), [0; 50]).unwrap();
        std::fs::write(dir.path().join("evictions-seq"), b"9").unwrap();
        assert_eq!(Covers::on_disk(dir.path()), (2, 150));
    }

    /// Hits and misses together, as a server answers a mix of covers it
    /// keeps and covers it reads off disk, only widen the window.
    #[test]
    fn a_mix_of_quick_and_slow_answers_never_narrows_the_window() {
        let mut q = Queue {
            limit: 512,
            ..Queue::default()
        };
        assert_eq!(q.allowed(), WINDOW_START);
        for i in 0..2000 {
            let took = if i % 3 == 0 { 700 } else { 90 };
            q.observe(Duration::from_millis(took), true);
        }
        assert!(q.allowed() > WINDOW_START * 2, "{}", q.allowed());
    }

    /// A server that slows and stays slow is given fewer requests, after a
    /// few round trips; a failure halves the window at once; never below
    /// the floor.
    #[test]
    fn sustained_slowness_and_failures_narrow_the_window_to_its_floor() {
        let mut q = Queue {
            limit: 512,
            ..Queue::default()
        };
        for _ in 0..300 {
            q.observe(Duration::from_millis(100), true);
        }
        let grown = q.allowed();
        // One slow round trip is not enough.
        for _ in 0..grown {
            q.observe(Duration::from_secs(2), true);
        }
        assert!(q.allowed() >= grown, "{} < {grown}", q.allowed());
        for _ in 0..grown * 4 {
            q.observe(Duration::from_secs(2), true);
        }
        assert!(q.allowed() < grown, "{} not below {grown}", q.allowed());
        for _ in 0..20 {
            q.last_cut = None;
            q.observe(Duration::from_secs(30), false);
        }
        assert_eq!(q.allowed(), WINDOW_MIN);
        q.limit = 3;
        assert_eq!(q.allowed(), 3);
    }

    /// After a fling every place is held by fetches for covers off screen,
    /// some scrolled away, some ahead: every one of them is called off, and
    /// every cover on screen starts before any cover off it.
    #[test]
    fn after_a_fling_every_visible_cover_starts_before_any_other() {
        let mut q = Queue {
            fetches_allowed: true,
            limit: 512,
            ..Queue::default()
        };
        let key = |id| Key {
            art: Art::Album(id),
            px: 64,
        };
        let mut told = Vec::new();
        for id in 0..16 {
            let (cancel, called_off) = futures_channel::oneshot::channel();
            q.fetching.insert(Art::Album(id));
            q.cancels.insert(Art::Album(id), (id as u64, cancel));
            told.push(called_off);
        }
        // 0..10 scrolled away; 10..16 still wanted, ahead.
        for id in 10..16 {
            q.wanted.insert(key(id), Priority::Ahead(id as f32));
        }
        for id in 100..120 {
            q.wanted.insert(key(id), Priority::Visible);
        }
        assert_eq!(q.give_way(), 16);
        for mut called_off in told {
            assert!(matches!(called_off.try_recv(), Ok(Some(()))));
        }
        let mut started = Vec::new();
        while let Some(art) = q.next_fetch() {
            q.fetching.insert(art);
            started.push(art);
        }
        assert_eq!(started.len(), q.allowed());
        let visible = started
            .iter()
            .take_while(|art| matches!(art, Art::Album(id) if *id >= 100))
            .count();
        assert_eq!(visible, 16, "{started:?}");
        // With room, the rest of the screen, then what is ahead.
        q.limit = 512;
        q.window = 64.0;
        while let Some(art) = q.next_fetch() {
            q.fetching.insert(art);
            started.push(art);
        }
        let first_off_screen = started
            .iter()
            .position(|art| matches!(art, Art::Album(id) if *id < 100))
            .unwrap();
        assert_eq!(first_off_screen, 20, "{started:?}");
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
