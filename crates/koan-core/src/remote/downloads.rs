//! The download store: every transfer koan is running or has just run, and
//! what each one is for.
//!
//! This is the one table of transfers. A transfer is keyed by the track it
//! fetches, because the track decides which file it writes; it records every
//! queue entry waiting on it, so a track queued twice is fetched once and both
//! entries hear how it ended. The download queue decides *when* to fetch;
//! everything about a transfer once started lives here.
//!
//! Progress and structure are deliberately separate. The byte counter is a
//! [`ByteFeed`] the downloader writes without taking any lock, because it
//! moves hundreds of times a second; `version` moves only when an entry is
//! listed, finishes or fails. A client polls the counter and watches the
//! version, and neither costs the download anything.
//!
//! Who may change which entries wait on what is the download queue's to
//! serialise: it does so under its own lock, which it may hold while it takes
//! this store's. This store never takes another lock while holding its own.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::player::commands::PlayerCommand;
use crate::player::state::{ItemState, QueueItemId, SharedPlayerState};

/// Where a transfer has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadState {
    /// Accepted, not yet started. A queue of six shows five of these.
    Queued,
    /// Bytes are arriving.
    Running,
    /// Every byte landed and the file is at its final path.
    Done,
    /// Gave up. The reason is worth keeping — it is the only account of why a
    /// track will not play.
    Failed(String),
}

impl DownloadState {
    pub fn is_settled(&self) -> bool {
        matches!(self, Self::Done | Self::Failed(_))
    }
}

/// One transfer, as readers see it.
#[derive(Debug, Clone)]
pub struct Download {
    /// The library track it fetches, and so what identifies it: every queue
    /// entry for the track shares the one transfer.
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    /// Where the bytes are being written — the `.part` file.
    pub source: PathBuf,
    /// Where they end up.
    pub dest: PathBuf,
    /// Total expected, or 0 when the server sent no Content-Length.
    pub total: u64,
    /// Live byte count, shared with the downloader. Read it, do not store it.
    pub written: Arc<ByteFeed>,
    pub state: DownloadState,
    /// Bytes per second, smoothed. Zero until there are two samples to take a
    /// rate from — and zero is the honest answer for a transfer that has
    /// stopped moving, which is the one worth noticing.
    pub bytes_per_second: u64,
}

impl Download {
    /// 0–1, or `None` when the server never said how big this is.
    pub fn fraction(&self) -> Option<f64> {
        (self.total > 0)
            .then(|| self.written.load(Ordering::Relaxed) as f64 / self.total as f64)
            .map(|f| f.clamp(0.0, 1.0))
    }

    pub fn bytes_written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }
}

/// A transfer in progress, as a player reading its bytes needs it.
#[derive(Debug, Clone)]
pub struct Live {
    pub source: PathBuf,
    pub total: u64,
    pub written: Arc<ByteFeed>,
}

/// One transfer's figures at the moment they were read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub track_id: i64,
    pub written: u64,
    /// 0 when the server sent no Content-Length.
    pub total: u64,
    /// As of the last rate sample, which is taken less often than this is read.
    pub bytes_per_second: u64,
}

impl Reading {
    /// 0–1, or `None` when there is no total to measure against.
    pub fn fraction(&self) -> Option<f64> {
        (self.total > 0).then(|| (self.written as f64 / self.total as f64).clamp(0.0, 1.0))
    }
}

/// A byte count that can be waited on.
///
/// The downloader publishes it as chunks land. A read is a plain atomic load;
/// beside the atomic is somewhere to wait, so a decode thread reading a file
/// that is still arriving sleeps until there is more instead of polling.
///
/// Writing is an atomic store per chunk. The lock and the wake are paid only
/// while a reader is actually parked, which for most of a transfer nobody is.
#[derive(Debug, Default)]
pub struct ByteFeed {
    written: AtomicU64,
    /// Readers waiting, or about to. A writer that sees none skips the lock.
    parked: AtomicUsize,
    /// Held by a reader from its last look until it sleeps, and taken by a
    /// writer that saw it parked, so a store cannot land between the two
    /// unseen — which for a stream is a stall the length of the whole timeout.
    at: parking_lot::Mutex<()>,
    more: parking_lot::Condvar,
}

impl ByteFeed {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// What has been written so far. Reads take nothing.
    pub fn load(&self, order: Ordering) -> u64 {
        self.written.load(order)
    }

    /// Say how much has been written, and wake whoever is waiting for it.
    pub fn set(&self, bytes: u64) {
        self.written.store(bytes, Ordering::SeqCst);
        self.wake_parked();
    }

    /// Add to the count, for a writer that knows only how much it just wrote.
    pub fn advance(&self, bytes: u64) {
        self.written.fetch_add(bytes, Ordering::SeqCst);
        self.wake_parked();
    }

    /// The count is stored before `parked` is read, and a reader counts itself
    /// parked before it reads the count — both sequentially consistent — so at
    /// least one of the two sees the other.
    fn wake_parked(&self) {
        if self.parked.load(Ordering::SeqCst) > 0 {
            let _at = self.at.lock();
            self.more.notify_all();
        }
    }

    /// The transfer is over, however it ended. Whoever is waiting wants to
    /// look at the status now rather than at the byte count.
    pub fn done(&self) {
        let _at = self.at.lock();
        self.more.notify_all();
    }

    /// Wait for something to happen past `seen` bytes, or until `deadline`.
    /// Returns what has been written either way.
    ///
    /// Returns on any wake, not only on a byte: a transfer that failed has no
    /// more bytes to offer and the caller has a status to re-read, which is
    /// the answer it is really waiting for. Deciding that here would be
    /// deciding it twice.
    pub fn wait_past(&self, seen: u64, deadline: Instant) -> u64 {
        let mut at = self.at.lock();
        self.parked.fetch_add(1, Ordering::SeqCst);
        let written = self.written.load(Ordering::SeqCst);
        if written <= seen
            && let Some(left) = deadline.checked_duration_since(Instant::now())
        {
            self.more.wait_for(&mut at, left);
        }
        self.parked.fetch_sub(1, Ordering::SeqCst);
        self.written.load(Ordering::Acquire)
    }
}

/// A transfer and what it is for.
#[derive(Debug)]
struct Entry {
    download: Download,
    /// Shown to readers. Not while the transfer is only claimed: its track is
    /// still being looked up, and may turn out to be on disk already, which is
    /// no download at all.
    listed: bool,
    /// Every queue entry waiting on it.
    waiters: HashSet<QueueItemId>,
    /// Wanted in the cache for its own sake, whatever the playlist does.
    keep: bool,
}

impl Entry {
    fn is_live(&self) -> bool {
        !self.download.state.is_settled()
    }

    fn wanted(&self) -> bool {
        self.keep || !self.waiters.is_empty()
    }
}

fn join_in(entries: &mut [Entry], track_id: i64, waiter: Option<QueueItemId>) -> bool {
    let Some(entry) = entries
        .iter_mut()
        .find(|e| e.download.track_id == track_id && e.is_live())
    else {
        return false;
    };
    match waiter {
        Some(id) => {
            entry.waiters.insert(id);
        }
        None => entry.keep = true,
    }
    true
}

/// How a transfer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Failed(String),
    /// Stopped because nothing wanted it. Leaves no row behind.
    Withdrawn,
}

/// Every transfer koan knows about.
#[derive(Debug, Default)]
pub struct DownloadStore {
    /// Live transfers first, then the settled tail, most recent first.
    entries: parking_lot::RwLock<Vec<Entry>>,
    version: AtomicU64,
    /// The last reading taken of each transfer, for working out a rate.
    /// Separate from the entries so taking a sample does not touch the list
    /// every client is reading.
    samples: parking_lot::Mutex<HashMap<i64, Sample>>,
    /// When the last reading was taken, in nanoseconds on [`clock_ns`]; 0
    /// for never.
    ///
    /// Readings are taken as bytes land rather than on a timer — the thing
    /// that knows a transfer moved is the code moving it — and a chunk lands
    /// far more often than a figure needs redrawing, so this is what holds
    /// them to `MIN_SAMPLE_GAP`. An atomic, because it is asked per chunk.
    last_sample: AtomicU64,
    figures: AtomicU64,
    /// Rung when the figures move. Its own signal rather than the engine's:
    /// a transfer's progress is news only to what draws it, and the engine's
    /// wakes every link, subscription and watcher in the process.
    moved: crate::signal::Wake,
}

/// Nanoseconds since the first time this was asked, never 0.
fn clock_ns() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

/// How many settled entries to keep. This is a view of now, not an archive,
/// and old rows would push the live ones off the end of it.
const SETTLED_LIMIT: usize = 50;

impl DownloadStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Bumped when an entry is listed, settles or is forgotten — not when its
    /// byte count moves. A client redraws its list on this and reads the
    /// counters every frame regardless.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Bumped whenever a byte count or a rate here moved. What a client
    /// redraws a figure on, as against `version`, which is the list itself
    /// changing shape.
    pub fn figures(&self) -> u64 {
        self.figures.load(Ordering::Acquire)
    }

    /// Rung whenever `figures` moves. What a reader of progress waits on; the
    /// engine's signal is not rung for it.
    pub fn moved(&self) -> &crate::signal::Wake {
        &self.moved
    }

    // --- What the download queue asks and decides ---

    /// Have `waiter` wait on the live transfer for `track_id`, if there is one —
    /// `None` for a fetch wanted only in the cache. Whether there was.
    pub fn join(&self, track_id: i64, waiter: Option<QueueItemId>) -> bool {
        join_in(&mut self.entries.write(), track_id, waiter)
    }

    /// Join the live transfer for `track_id`, or start one. `true` when this call
    /// started it, and the caller is to run it. One lock for both, so two
    /// callers can never both start it.
    ///
    /// A new transfer is not listed until [`announce`](Self::announce) says
    /// bytes are to be fetched; it replaces any settled row for the same track_id,
    /// because a track fetched again is the same row starting over.
    pub fn claim(&self, track_id: i64, waiter: Option<QueueItemId>) -> bool {
        let mut entries = self.entries.write();
        if join_in(&mut entries, track_id, waiter) {
            return false;
        }
        let had_row = entries
            .iter()
            .any(|e| e.download.track_id == track_id && e.listed);
        entries.retain(|e| e.download.track_id != track_id);
        entries.insert(
            0,
            Entry {
                download: Download {
                    track_id,
                    title: String::new(),
                    artist: String::new(),
                    source: PathBuf::new(),
                    dest: PathBuf::new(),
                    total: 0,
                    written: ByteFeed::new(),
                    state: DownloadState::Queued,
                    bytes_per_second: 0,
                },
                listed: false,
                waiters: waiter.into_iter().collect(),
                keep: waiter.is_none(),
            },
        );
        drop(entries);
        if had_row {
            self.bump();
        }
        true
    }

    /// Make every live transfer's waiters exactly the entries in `wanted` for
    /// its track. Returns the wanted entries with no live transfer, in the
    /// order given: those still to be fetched. A transfer left wanted by
    /// nothing is abandoned, and stops when it next asks.
    ///
    /// One write lock for all of it. `abandoned` is asked from each running
    /// transfer without the queue's lock, so a transfer must never be seen
    /// between losing its old waiters and gaining its new ones: a queue
    /// replaced with the same track would have that track's transfer stopped
    /// and started again, and a decoder streaming its `.part` left waiting on
    /// a feed nothing writes to.
    pub fn resync(&self, wanted: &[(i64, QueueItemId)]) -> Vec<(i64, QueueItemId)> {
        let mut entries = self.entries.write();
        let mut live: HashMap<i64, usize> = HashMap::new();
        for (ix, entry) in entries.iter_mut().enumerate().filter(|(_, e)| e.is_live()) {
            entry.waiters.clear();
            live.insert(entry.download.track_id, ix);
        }
        let mut unfetched = Vec::new();
        for &(track_id, id) in wanted {
            match live.get(&track_id) {
                Some(&ix) => {
                    entries[ix].waiters.insert(id);
                }
                None => unfetched.push((track_id, id)),
            }
        }
        unfetched
    }

    /// Whether a transfer for `track_id` is live.
    pub fn in_flight(&self, track_id: i64) -> bool {
        self.entries
            .read()
            .iter()
            .any(|e| e.download.track_id == track_id && e.is_live())
    }

    /// The queue entries waiting on the live transfer for `track_id`.
    pub fn waiters(&self, track_id: i64) -> Vec<QueueItemId> {
        self.entries
            .read()
            .iter()
            .find(|e| e.download.track_id == track_id && e.is_live())
            .map(|e| e.waiters.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Whether nothing wants the transfer for `track_id` any more, or there
    /// is no live transfer for it at all.
    pub fn abandoned(&self, track_id: i64) -> bool {
        self.entries
            .read()
            .iter()
            .find(|e| e.download.track_id == track_id && e.is_live())
            .is_none_or(|e| !e.wanted())
    }

    // --- What the downloader reports ---

    /// Bytes are to be fetched: list the transfer, so a queue of six shows six
    /// rows rather than one row and five tracks that look like nothing is
    /// happening to them. Returns the feed to write the byte count into.
    pub fn announce(
        &self,
        track_id: i64,
        title: String,
        artist: String,
        source: PathBuf,
        dest: PathBuf,
    ) -> Arc<ByteFeed> {
        let mut entries = self.entries.write();
        let feed = match entries
            .iter_mut()
            .find(|e| e.download.track_id == track_id && e.is_live())
        {
            Some(entry) => {
                let d = &mut entry.download;
                (d.title, d.artist, d.source, d.dest) = (title, artist, source, dest);
                entry.listed = true;
                d.written.clone()
            }
            None => ByteFeed::new(),
        };
        drop(entries);
        self.bump();
        feed
    }

    /// Bytes have started arriving, and this is how many there are in total.
    pub fn started(&self, track_id: i64, total: u64) {
        let mut entries = self.entries.write();
        if let Some(entry) = entries
            .iter_mut()
            .find(|e| e.download.track_id == track_id && e.is_live())
        {
            entry.download.total = total;
            entry.download.state = DownloadState::Running;
        }
        drop(entries);
        self.bump();
    }

    /// End the live transfer for `track_id`. Returns who was waiting on it, and the
    /// feed a decoder reading it may be parked on.
    ///
    /// A transfer that never listed — its file was found on disk, or it failed
    /// before fetching anything — leaves no row. Nor does a withdrawn one.
    fn end(&self, track_id: i64, outcome: Outcome) -> Option<(Vec<QueueItemId>, Arc<ByteFeed>)> {
        let mut entries = self.entries.write();
        let ix = entries
            .iter()
            .position(|e| e.download.track_id == track_id && e.is_live())?;
        let entry = &mut entries[ix];
        let waiters = std::mem::take(&mut entry.waiters).into_iter().collect();
        let feed = entry.download.written.clone();
        let listed = entry.listed;
        match outcome {
            Outcome::Done if listed => entry.download.state = DownloadState::Done,
            Outcome::Failed(reason) if listed => {
                entry.download.state = DownloadState::Failed(reason)
            }
            _ => {
                entries.remove(ix);
            }
        }
        if let Some(entry) = entries
            .get_mut(ix)
            .filter(|e| e.download.track_id == track_id)
        {
            // Said here rather than at the next reading: a transfer that has
            // finished takes no more readings, and a row left showing the rate
            // it managed on its last chunk is a row that never stops.
            entry.download.bytes_per_second = 0;
        }
        // Live first, then the settled tail, most recent first. Stable, so a
        // list being watched does not shuffle under the pointer.
        let (mut live, settled): (Vec<_>, Vec<_>) = entries.drain(..).partition(Entry::is_live);
        let (now, older): (Vec<_>, Vec<_>) = settled
            .into_iter()
            .partition(|e| e.download.track_id == track_id);
        live.extend(now);
        live.extend(older.into_iter().take(SETTLED_LIMIT.saturating_sub(1)));
        *entries = live;
        drop(entries);
        self.samples.lock().remove(&track_id);
        if listed {
            self.figures.fetch_add(1, Ordering::Release);
            self.moved.bump();
            self.bump();
        }
        Some((waiters, feed))
    }

    // --- What readers ask ---

    /// Every listed transfer, live first, then whatever settled most recently.
    pub fn all(&self) -> Vec<Download> {
        self.entries
            .read()
            .iter()
            .filter(|e| e.listed)
            .map(|e| e.download.clone())
            .collect()
    }

    /// The listed, live transfer for `track_id`, as a player reading its bytes
    /// needs it.
    pub fn live(&self, track_id: i64) -> Option<Live> {
        self.entries
            .read()
            .iter()
            .find(|e| e.download.track_id == track_id && e.listed && e.is_live())
            .map(|e| Live {
                source: e.download.source.clone(),
                total: e.download.total,
                written: e.download.written.clone(),
            })
    }

    /// The byte counts of every listed transfer still going, read now — for a
    /// client drawing progress at its display's rate, and for deriving a queue
    /// without cloning a path per row.
    ///
    /// Holds the list's read lock for one pass over it, which is bounded by
    /// the download workers.
    pub fn readings(&self) -> Vec<Reading> {
        self.entries
            .read()
            .iter()
            .filter(|e| e.listed && e.is_live())
            .map(|e| Reading {
                track_id: e.download.track_id,
                written: e.download.bytes_written(),
                total: e.download.total,
                bytes_per_second: e.download.bytes_per_second,
            })
            .collect()
    }

    /// How many transfers are actually moving.
    pub fn active(&self) -> usize {
        self.entries
            .read()
            .iter()
            .filter(|e| e.listed && e.is_live())
            .count()
    }

    /// Drop everything that has already settled. The running ones are not this
    /// call's business — stopping a transfer is a different verb.
    pub fn clear_settled(&self) {
        let mut entries = self.entries.write();
        let before = entries.len();
        entries.retain(Entry::is_live);
        let changed = entries.len() != before;
        drop(entries);
        if changed {
            self.bump();
        }
    }

    fn bump(&self) {
        self.version.fetch_add(1, Ordering::Release);
        crate::signal::engine_changed().bump();
    }
}

/// What is left to do once a transfer has settled, after whatever lock the
/// caller settled it under is released.
#[must_use = "announce wakes a decoder parked on the transfer and tells the player"]
pub struct Settled {
    feed: Option<Arc<ByteFeed>>,
    tell: Vec<PlayerCommand>,
}

impl Settled {
    /// Wake a decoder parked at the write head, then tell the player about any
    /// entry under the cursor. The rest it picks up when the cursor reaches
    /// them.
    pub fn announce(self, tx: &crossbeam_channel::Sender<PlayerCommand>) {
        if let Some(feed) = self.feed {
            feed.done();
        }
        for cmd in self.tell {
            tx.send(cmd).ok();
        }
    }
}

/// End the transfer for `track_id` and give every entry waiting on it the one
/// result.
///
/// The order is the point. Each entry's state is written first, then the
/// store's row, and only after both does [`Settled::announce`] wake the feed:
/// a decoder parked at the `.part` file's write head reads the entry's state
/// once woken, to learn whether the file is complete or has failed. Woken
/// before that state is written, it sees a download still running and parks
/// again with nothing left to wake it.
///
/// The download queue calls this under its own lock, so no entry can join the
/// transfer between being told and the transfer ending.
pub fn settle(
    state: &SharedPlayerState,
    track_id: i64,
    result: &Result<PathBuf, String>,
) -> Settled {
    let store = state.downloads();
    let waiters = store.waiters(track_id);
    for &id in &waiters {
        match result {
            Ok(path) => {
                state.update_paths(&[(id, path.clone())]);
                state.update_item_state(id, ItemState::Ready);
            }
            Err(reason) => state.update_item_state(id, ItemState::Failed(reason.clone())),
        }
    }
    let outcome = match result {
        Ok(_) => Outcome::Done,
        Err(reason) => Outcome::Failed(reason.clone()),
    };
    let feed = store.end(track_id, outcome).map(|(_, feed)| feed);
    let tell = waiters
        .into_iter()
        .filter(|id| state.is_cursor(*id))
        .map(|id| match result {
            Ok(_) => PlayerCommand::TrackReady(id),
            Err(_) => PlayerCommand::TrackFailed(id),
        })
        .collect();
    Settled { feed, tell }
}

/// Withdraw the transfer for `track_id`: nothing wanted it. Returns whoever asked
/// for it in the meantime, to be queued again, and wakes its feed.
pub fn withdraw(store: &DownloadStore, track_id: i64) -> Vec<QueueItemId> {
    match store.end(track_id, Outcome::Withdrawn) {
        Some((waiters, feed)) => {
            feed.done();
            waiters
        }
        None => Vec::new(),
    }
}

/// The last reading of one transfer.
#[derive(Debug)]
struct Sample {
    at: Instant,
    bytes: u64,
    /// Smoothed rate, so a figure on screen does not jump about between frames.
    bps: f64,
}

/// How much of a new reading to believe against the running average. Low
/// enough to be steady, high enough that a transfer stopping shows within a
/// second or so.
const RATE_SMOOTHING: f64 = 0.3;

/// Ignore samples closer together than this — over a short enough interval the
/// arithmetic is mostly noise.
const MIN_SAMPLE_GAP: Duration = Duration::from_millis(250);

impl DownloadStore {
    /// Take a rate reading, if one is due.
    ///
    /// Called by the downloader as bytes land, not by a timer: what knows a
    /// transfer moved is the code moving it, and what knows it stopped is the
    /// absence of the next call. Chunks arrive far faster than a figure needs
    /// redrawing, so this is gated to `MIN_SAMPLE_GAP` before it touches the
    /// list every client is reading.
    ///
    /// Every running transfer is sampled, not just the one that moved: a
    /// transfer that has stalled has nothing to report by definition, and its
    /// figure decaying to zero is the one worth noticing.
    ///
    /// Rates live here rather than in each front end because every one of them
    /// would otherwise keep its own last-reading map and get a different
    /// answer.
    pub fn progressed(&self) {
        let now = clock_ns();
        let last = self.last_sample.load(Ordering::Relaxed);
        if last != 0 && now.saturating_sub(last) < MIN_SAMPLE_GAP.as_nanos() as u64 {
            return;
        }
        // One chunk among the transfers running takes the reading; the rest
        // see it taken and go on.
        if self
            .last_sample
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.sample_rates_at(Instant::now());
        }
    }

    fn sample_rates_at(&self, now: Instant) {
        let mut entries = self.entries.write();
        let mut samples = self.samples.lock();

        for entry in entries.iter_mut() {
            let track_id = entry.download.track_id;
            let entry = &mut entry.download;
            if entry.state.is_settled() {
                entry.bytes_per_second = 0;
                samples.remove(&track_id);
                continue;
            }
            let bytes = entry.written.load(Ordering::Relaxed);
            match samples.get_mut(&track_id) {
                Some(previous) => {
                    let elapsed = now.saturating_duration_since(previous.at);
                    if elapsed < MIN_SAMPLE_GAP {
                        entry.bytes_per_second = previous.bps as u64;
                        continue;
                    }
                    let moved = bytes.saturating_sub(previous.bytes) as f64;
                    let instant = moved / elapsed.as_secs_f64();
                    previous.bps = previous.bps * (1.0 - RATE_SMOOTHING) + instant * RATE_SMOOTHING;
                    previous.at = now;
                    previous.bytes = bytes;
                    entry.bytes_per_second = previous.bps as u64;
                }
                None => {
                    samples.insert(
                        track_id,
                        Sample {
                            at: now,
                            bytes,
                            bps: 0.0,
                        },
                    );
                    entry.bytes_per_second = 0;
                }
            }
        }

        // A transfer that left the list leaves its reading behind with it.
        let live: HashSet<i64> = entries.iter().map(|e| e.download.track_id).collect();
        samples.retain(|track_id, _| live.contains(track_id));

        // Said once for the whole reading, so a client redraws every figure
        // from one moment rather than a row at a time.
        self.figures.fetch_add(1, Ordering::Release);
        self.moved.bump();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transfer for track `id`, claimed for one entry and announced.
    fn running(store: &DownloadStore, id: i64, title: &str) -> (i64, QueueItemId, Arc<ByteFeed>) {
        let track_id = id;
        let waiter = QueueItemId::new();
        assert!(store.claim(track_id, Some(waiter)));
        let feed = store.announce(
            track_id,
            title.into(),
            "Artist".into(),
            PathBuf::from(format!("/cache/{title}.opus.part")),
            PathBuf::from(format!("/cache/{title}.opus")),
        );
        (track_id, waiter, feed)
    }

    #[test]
    fn a_track_wanted_twice_is_one_transfer_with_two_waiters() {
        let store = DownloadStore::new();
        let track_id = 7;
        let (first, again) = (QueueItemId::new(), QueueItemId::new());
        assert!(
            store.claim(track_id, Some(first)),
            "the first claim starts it"
        );
        assert!(!store.claim(track_id, Some(again)), "the second joins it");

        let waiters: HashSet<_> = store.waiters(track_id).into_iter().collect();
        assert_eq!(waiters, HashSet::from([first, again]));
    }

    #[test]
    fn a_claim_is_not_listed_until_bytes_are_to_be_fetched() {
        // Its track may turn out to be on disk, which is no download at all.
        let store = DownloadStore::new();
        let track_id = 1;
        store.claim(track_id, Some(QueueItemId::new()));
        assert!(store.all().is_empty());
        assert!(store.live(track_id).is_none());
        assert!(
            store.in_flight(track_id),
            "but it is in flight: nothing fetches it twice"
        );

        let version = store.version();
        let _ = withdraw(&store, track_id);
        assert_eq!(store.version(), version, "and it leaves without a trace");
        assert!(!store.in_flight(track_id));
    }

    #[test]
    fn a_transfer_runs_then_settles() {
        let store = DownloadStore::new();
        let (track_id, _, feed) = running(&store, 1, "train");
        assert_eq!(store.active(), 1);

        store.started(track_id, 400);
        feed.set(100);
        assert_eq!(store.all()[0].fraction(), Some(0.25));
        assert_eq!(store.live(track_id).map(|l| l.total), Some(400));

        store.end(track_id, Outcome::Done);
        assert_eq!(store.active(), 0);
        assert_eq!(store.all()[0].state, DownloadState::Done);
        assert!(store.live(track_id).is_none());
    }

    #[test]
    fn settling_hands_back_every_waiter_and_the_feed() {
        let store = DownloadStore::new();
        let (track_id, first, feed) = running(&store, 1, "train");
        let again = QueueItemId::new();
        store.join(track_id, Some(again));

        let (waiters, settled_feed) = store.end(track_id, Outcome::Failed("404".into())).unwrap();
        assert_eq!(
            waiters.into_iter().collect::<HashSet<_>>(),
            HashSet::from([first, again])
        );
        assert!(Arc::ptr_eq(&feed, &settled_feed));
        assert!(
            !store.join(track_id, Some(QueueItemId::new())),
            "nothing to join once settled"
        );
    }

    #[test]
    fn readings_are_live_and_leave_settled_transfers_out() {
        let store = DownloadStore::new();
        let (going, _, feed) = running(&store, 1, "going");
        let (landed, _, _) = running(&store, 2, "landed");
        store.started(going, 400);
        store.end(landed, Outcome::Done);

        feed.set(100);
        let readings = store.readings();
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].track_id, going);
        assert_eq!(readings[0].fraction(), Some(0.25));

        // No sample taken in between: a reading sees the bytes as they land.
        feed.set(300);
        assert_eq!(store.readings()[0].fraction(), Some(0.75));
    }

    #[test]
    fn progress_does_not_move_the_version() {
        // The counter is read every frame and the list is rebuilt on the
        // version; if bytes bumped it, every client would rebuild at the rate
        // the download writes.
        let store = DownloadStore::new();
        let (track_id, _, feed) = running(&store, 1, "train");
        store.started(track_id, 1000);

        let before = store.version();
        feed.set(500);
        assert_eq!(store.version(), before);
        assert_eq!(store.all()[0].bytes_written(), 500);
    }

    #[test]
    fn no_content_length_means_no_fraction() {
        // A bar drawn at zero for a transfer that is going fine reads as stuck.
        let store = DownloadStore::new();
        let (track_id, _, feed) = running(&store, 1, "chunked");
        store.started(track_id, 0);
        feed.set(9000);
        assert_eq!(store.all()[0].fraction(), None);
        assert_eq!(store.all()[0].bytes_written(), 9000);
    }

    #[test]
    fn fetching_the_same_track_again_restarts_its_row() {
        // Clearing a download and playing the track again is the same transfer
        // starting over, not a second one to scroll past.
        let store = DownloadStore::new();
        let (track_id, _, _) = running(&store, 1, "train");
        store.end(track_id, Outcome::Done);

        running(&store, 1, "train");
        assert_eq!(store.all().len(), 1);
        assert_eq!(store.all()[0].state, DownloadState::Queued);
    }

    #[test]
    fn running_transfers_sort_above_settled_ones() {
        let store = DownloadStore::new();
        let (done, _, _) = running(&store, 1, "done");
        running(&store, 2, "running");
        store.end(done, Outcome::Done);

        let all = store.all();
        assert_eq!(all[0].title, "running");
        assert_eq!(all[1].title, "done");
    }

    #[test]
    fn a_failure_keeps_its_reason() {
        let store = DownloadStore::new();
        let (track_id, _, _) = running(&store, 1, "gone");
        store.end(track_id, Outcome::Failed("server returned 404".into()));
        assert_eq!(
            store.all()[0].state,
            DownloadState::Failed("server returned 404".into())
        );
    }

    #[test]
    fn a_withdrawn_transfer_leaves_no_row_and_returns_late_waiters() {
        let store = DownloadStore::new();
        let (track_id, first, _) = running(&store, 1, "train");
        assert_eq!(withdraw(&store, track_id), vec![first]);
        assert!(store.all().is_empty());
    }

    #[test]
    fn a_resync_hands_a_transfer_from_its_old_entry_to_its_new_in_one_step() {
        // A queue replaced with the same track: the old entry goes, a new one
        // comes, and nothing asking in between can see the transfer unwanted,
        // because there is no in between.
        let store = DownloadStore::new();
        let (track_id, old, _) = running(&store, 7, "train");
        let new = QueueItemId::new();
        let other = QueueItemId::new();

        let unfetched = store.resync(&[(7, new), (8, other)]);

        assert_eq!(store.waiters(track_id), vec![new]);
        assert!(!store.waiters(track_id).contains(&old));
        assert!(!store.abandoned(track_id));
        assert_eq!(
            unfetched,
            vec![(8, other)],
            "the rest, in order, to be fetched"
        );
    }

    #[test]
    fn a_transfer_nothing_waits_on_is_abandoned() {
        let store = DownloadStore::new();
        let (track_id, waiter, _) = running(&store, 1, "train");
        assert!(!store.abandoned(track_id));

        store.resync(&[]);
        assert!(store.abandoned(track_id));

        store.join(track_id, Some(waiter));
        assert!(!store.abandoned(track_id), "wanted again");
    }

    #[test]
    fn a_transfer_kept_for_the_cache_is_never_abandoned() {
        let store = DownloadStore::new();
        let track_id = 5;
        assert!(store.claim(track_id, None));
        store.resync(&[]);
        assert!(!store.abandoned(track_id));
    }

    #[test]
    fn a_rate_needs_two_readings_and_a_gap_between_them() {
        let store = DownloadStore::new();
        let (track_id, _, feed) = running(&store, 1, "train");
        store.started(track_id, 1_000_000);

        let start = Instant::now();
        store.sample_rates_at(start);
        assert_eq!(
            store.all()[0].bytes_per_second,
            0,
            "one reading is not a rate"
        );

        // A second too close to the first says nothing.
        feed.set(100_000);
        store.sample_rates_at(start + Duration::from_millis(50));
        assert_eq!(store.all()[0].bytes_per_second, 0);

        // A second far enough away does. Smoothed, so it reads low at first.
        store.sample_rates_at(start + Duration::from_secs(1));
        let bps = store.all()[0].bytes_per_second;
        assert!(bps > 0, "a rate should have been worked out, got {bps}");
        assert!(bps < 100_000, "and smoothed rather than taken whole: {bps}");
    }

    #[test]
    fn a_settled_transfer_has_no_rate() {
        // Zero, not the speed it happened to be going when it stopped.
        let store = DownloadStore::new();
        let (track_id, _, feed) = running(&store, 1, "train");
        store.started(track_id, 1000);
        let start = Instant::now();
        store.sample_rates_at(start);
        feed.set(500);
        store.sample_rates_at(start + Duration::from_secs(1));
        assert!(store.all()[0].bytes_per_second > 0);

        store.end(track_id, Outcome::Done);
        store.sample_rates_at(start + Duration::from_secs(2));
        assert_eq!(store.all()[0].bytes_per_second, 0);
    }

    #[test]
    fn clearing_settled_leaves_the_running_alone() {
        let store = DownloadStore::new();
        let (done, _, _) = running(&store, 1, "done");
        running(&store, 2, "running");
        store.end(done, Outcome::Done);

        store.clear_settled();
        let all = store.all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "running");
    }

    #[test]
    fn the_settled_tail_is_bounded() {
        let store = DownloadStore::new();
        for id in 0..(SETTLED_LIMIT as i64 + 10) {
            let (track_id, _, _) = running(&store, id, "t");
            store.end(track_id, Outcome::Done);
        }
        assert_eq!(store.all().len(), SETTLED_LIMIT);
    }

    #[test]
    fn a_parked_reader_is_woken_by_every_write_it_waits_for() {
        // The writer skips the lock when nobody is parked; a reader parking as
        // a write lands must still see it, every time.
        let feed = ByteFeed::new();
        let reader = {
            let feed = feed.clone();
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut seen = 0;
                while seen < 20_000 {
                    seen = feed.wait_past(seen, deadline);
                    assert!(
                        Instant::now() < deadline,
                        "a write was slept through at {seen}"
                    );
                }
            })
        };
        for n in 1..=20_000 {
            feed.set(n);
        }
        reader.join().unwrap();
    }

    #[test]
    fn a_reading_rings_the_store_and_not_the_engine() {
        // Progress is news to what draws it. The engine's signal wakes every
        // link, subscription and watcher in the process.
        let store = DownloadStore::new();
        let (track_id, _, feed) = running(&store, 1, "train");
        store.started(track_id, 1000);
        let before = store.moved().generation();
        feed.set(500);
        store.progressed();
        assert_ne!(store.moved().generation(), before);
    }
}
