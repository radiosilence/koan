//! The download store — what koan is fetching, and what it just fetched.
//!
//! One place every front end reads, rather than each deriving its own answer
//! from the queue. The queue only knows about transfers for tracks that are in
//! it, and it forgets a download the instant it lands — which is exactly when
//! somebody wants to see that it did.
//!
//! Progress and structure are deliberately separate. The byte counter is an
//! `Arc<AtomicU64>` the downloader writes without taking any lock, because it
//! moves hundreds of times a second; `version` moves only when an entry is
//! added, finishes or fails. A client polls the counter and watches the
//! version, and neither costs the download anything.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::player::state::QueueItemId;

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

/// One transfer.
#[derive(Debug, Clone)]
pub struct Download {
    /// The queue item this is being fetched for. Also the identity of the
    /// transfer, because a track wanted twice is wanted by two queue entries.
    pub id: QueueItemId,
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
    pub written: Arc<AtomicU64>,
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

/// What a transfer is doing, in a form cheap enough to ask about per row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phase {
    pub state: PhaseKind,
    pub written: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseKind {
    Queued,
    Running,
    Done,
    Failed,
}

impl Phase {
    pub fn is_running(&self) -> bool {
        matches!(self.state, PhaseKind::Queued | PhaseKind::Running)
    }
}

/// Every transfer koan knows about.
#[derive(Debug, Default)]
pub struct DownloadStore {
    entries: parking_lot::RwLock<Vec<Download>>,
    version: AtomicU64,
    /// The last reading taken of each transfer, for working out a rate.
    /// Separate from the entries so taking a sample does not touch the list
    /// every client is reading.
    samples: parking_lot::Mutex<HashMap<QueueItemId, Sample>>,
    /// When the last reading was taken, and the count that goes with it.
    ///
    /// Readings are taken as bytes land rather than on a timer — the thing
    /// that knows a transfer moved is the code moving it — and a chunk lands
    /// far more often than a figure needs redrawing, so this is what holds
    /// them to `MIN_SAMPLE_GAP`.
    last_sample: parking_lot::Mutex<Option<Instant>>,
    figures: AtomicU64,
    /// How many settled entries to keep. This is a view of now, not an archive,
    /// and old rows would push the live ones off the end of it.
    settled_limit: usize,
}

impl DownloadStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: parking_lot::RwLock::new(Vec::new()),
            version: AtomicU64::new(0),
            samples: parking_lot::Mutex::new(HashMap::new()),
            last_sample: parking_lot::Mutex::new(None),
            figures: AtomicU64::new(0),
            settled_limit: 50,
        })
    }

    /// Bumped when an entry appears, settles or is forgotten — not when its
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

    /// Everything, running first, then whatever settled most recently.
    pub fn all(&self) -> Vec<Download> {
        self.entries.read().clone()
    }

    /// The transfer for one queue item, if there is one.
    ///
    /// A scan, deliberately: entries are bounded by the number of download
    /// workers plus the settled tail, because a transfer only appears here
    /// when a worker picks it up. Indexing tens of rows would cost more to
    /// maintain than it saves.
    pub fn get(&self, id: QueueItemId) -> Option<Download> {
        self.entries.read().iter().find(|d| d.id == id).cloned()
    }

    /// Whether a transfer exists for this item and what it is doing, without
    /// cloning its paths and titles — what deriving a queue row needs, per row
    /// per frame.
    pub fn phase_of(&self, id: QueueItemId) -> Option<Phase> {
        self.entries
            .read()
            .iter()
            .find(|d| d.id == id)
            .map(|d| Phase {
                state: match &d.state {
                    DownloadState::Queued => PhaseKind::Queued,
                    DownloadState::Running => PhaseKind::Running,
                    DownloadState::Done => PhaseKind::Done,
                    DownloadState::Failed(_) => PhaseKind::Failed,
                },
                written: d.bytes_written(),
                total: d.total,
            })
    }

    /// How many transfers are actually moving.
    pub fn active(&self) -> usize {
        self.entries
            .read()
            .iter()
            .filter(|d| !d.state.is_settled())
            .count()
    }

    /// Note that a transfer is wanted. Replaces any earlier entry for the same
    /// queue item — a track cleared and fetched again is the same row starting
    /// over, not a second one.
    pub fn queued(&self, download: Download) {
        let mut entries = self.entries.write();
        entries.retain(|d| d.id != download.id);
        entries.insert(0, download);
        drop(entries);
        self.settle();
    }

    /// Bytes have started arriving, and this is how many there are in total.
    pub fn started(&self, id: QueueItemId, total: u64, written: Arc<AtomicU64>) {
        let mut entries = self.entries.write();
        if let Some(entry) = entries.iter_mut().find(|d| d.id == id) {
            entry.total = total;
            entry.written = written;
            entry.state = DownloadState::Running;
        }
        drop(entries);
        self.bump();
    }

    /// It landed.
    pub fn finished(&self, id: QueueItemId) {
        self.settle_one(id, DownloadState::Done);
    }

    /// It did not.
    pub fn failed(&self, id: QueueItemId, reason: String) {
        self.settle_one(id, DownloadState::Failed(reason));
    }

    /// Drop everything that has already settled. The running ones are not this
    /// call's business — stopping a transfer is a different verb.
    pub fn clear_settled(&self) {
        let mut entries = self.entries.write();
        let before = entries.len();
        entries.retain(|d| !d.state.is_settled());
        let changed = entries.len() != before;
        drop(entries);
        if changed {
            self.bump();
        }
    }

    fn settle_one(&self, id: QueueItemId, state: DownloadState) {
        let mut entries = self.entries.write();
        if let Some(entry) = entries.iter_mut().find(|d| d.id == id) {
            entry.state = state;
            // Said here rather than at the next reading: a transfer that has
            // finished takes no more readings, and a row left showing the rate
            // it managed on its last chunk is a row that never stops.
            entry.bytes_per_second = 0;
        }
        drop(entries);
        self.samples.lock().remove(&id);
        self.figures.fetch_add(1, Ordering::Release);
        // `settle` bumps the version, which says so for both.
        self.settle();
    }

    /// Keep running transfers at the top and the settled tail bounded.
    fn settle(&self) {
        let mut entries = self.entries.write();
        // Stable, so a list being watched does not shuffle under the pointer.
        let (mut running, settled): (Vec<_>, Vec<_>) =
            entries.drain(..).partition(|d| !d.state.is_settled());
        running.extend(settled.into_iter().take(self.settled_limit));
        *entries = running;
        drop(entries);
        self.bump();
    }

    fn bump(&self) {
        self.version.fetch_add(1, Ordering::Release);
        crate::signal::engine_changed().bump();
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
        let now = Instant::now();
        {
            let mut last = self.last_sample.lock();
            if last.is_some_and(|at| now.saturating_duration_since(at) < MIN_SAMPLE_GAP) {
                return;
            }
            *last = Some(now);
        }
        self.sample_rates_at(now);
    }

    fn sample_rates_at(&self, now: Instant) {
        let mut entries = self.entries.write();
        let mut samples = self.samples.lock();

        for entry in entries.iter_mut() {
            if entry.state.is_settled() {
                entry.bytes_per_second = 0;
                samples.remove(&entry.id);
                continue;
            }
            let bytes = entry.written.load(Ordering::Relaxed);
            match samples.get_mut(&entry.id) {
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
                        entry.id,
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
        let live: std::collections::HashSet<QueueItemId> = entries.iter().map(|e| e.id).collect();
        samples.retain(|id, _| live.contains(id));

        // Said once for the whole reading, so a client redraws every figure
        // from one moment rather than a row at a time.
        self.figures.fetch_add(1, Ordering::Release);
        crate::signal::engine_changed().bump();
    }
}

/// The process's download store.
///
/// A singleton for the same reason the download queue is one: there is one set
/// of transfers happening, and everything that reports on them — the queue, the
/// front ends, the downloader itself — has to be looking at the same set.
pub fn store() -> &'static Arc<DownloadStore> {
    static STORE: std::sync::OnceLock<Arc<DownloadStore>> = std::sync::OnceLock::new();
    STORE.get_or_init(DownloadStore::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(title: &str) -> Download {
        Download {
            id: QueueItemId::new(),
            track_id: 1,
            title: title.into(),
            artist: "Artist".into(),
            source: PathBuf::from(format!("/cache/{title}.opus.part")),
            dest: PathBuf::from(format!("/cache/{title}.opus")),
            total: 0,
            written: Arc::new(AtomicU64::new(0)),
            state: DownloadState::Queued,
            bytes_per_second: 0,
        }
    }

    #[test]
    fn a_transfer_runs_then_settles() {
        let store = DownloadStore::new();
        let entry = download("train");
        let id = entry.id;
        store.queued(entry);
        assert_eq!(store.active(), 1);

        let written = Arc::new(AtomicU64::new(0));
        store.started(id, 400, written.clone());
        written.store(100, Ordering::Relaxed);
        assert_eq!(store.all()[0].fraction(), Some(0.25));

        store.finished(id);
        assert_eq!(store.active(), 0);
        assert_eq!(store.all()[0].state, DownloadState::Done);
    }

    #[test]
    fn progress_does_not_move_the_version() {
        // The counter is read every frame and the list is rebuilt on the
        // version; if bytes bumped it, every client would rebuild at the rate
        // the download writes.
        let store = DownloadStore::new();
        let entry = download("train");
        let id = entry.id;
        store.queued(entry);
        let written = Arc::new(AtomicU64::new(0));
        store.started(id, 1000, written.clone());

        let before = store.version();
        written.store(500, Ordering::Relaxed);
        assert_eq!(store.version(), before);
        assert_eq!(store.all()[0].bytes_written(), 500);
    }

    #[test]
    fn no_content_length_means_no_fraction() {
        // A bar drawn at zero for a transfer that is going fine reads as stuck.
        let store = DownloadStore::new();
        let entry = download("chunked");
        let id = entry.id;
        store.queued(entry);
        store.started(id, 0, Arc::new(AtomicU64::new(9000)));
        assert_eq!(store.all()[0].fraction(), None);
        assert_eq!(store.all()[0].bytes_written(), 9000);
    }

    #[test]
    fn fetching_the_same_item_again_restarts_its_row() {
        // Clearing a download and playing the track again is the same transfer
        // starting over, not a second one to scroll past.
        let store = DownloadStore::new();
        let first = download("train");
        let id = first.id;
        store.queued(first);
        store.finished(id);

        let mut again = download("train");
        again.id = id;
        store.queued(again);

        assert_eq!(store.all().len(), 1);
        assert_eq!(store.all()[0].state, DownloadState::Queued);
    }

    #[test]
    fn running_transfers_sort_above_settled_ones() {
        let store = DownloadStore::new();
        let done = download("done");
        let done_id = done.id;
        store.queued(done);
        let running = download("running");
        store.queued(running);
        store.finished(done_id);

        let all = store.all();
        assert_eq!(all[0].title, "running");
        assert_eq!(all[1].title, "done");
    }

    #[test]
    fn a_failure_keeps_its_reason() {
        let store = DownloadStore::new();
        let entry = download("gone");
        let id = entry.id;
        store.queued(entry);
        store.failed(id, "server returned 404".into());
        assert_eq!(
            store.all()[0].state,
            DownloadState::Failed("server returned 404".into())
        );
    }

    #[test]
    fn a_rate_needs_two_readings_and_a_gap_between_them() {
        let store = DownloadStore::new();
        let entry = download("train");
        let (id, written) = (entry.id, entry.written.clone());
        store.queued(entry);
        store.started(id, 1_000_000, written.clone());

        let start = Instant::now();
        store.sample_rates_at(start);
        assert_eq!(
            store.all()[0].bytes_per_second,
            0,
            "one reading is not a rate"
        );

        // A second too close to the first says nothing.
        written.store(100_000, Ordering::Relaxed);
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
        let entry = download("train");
        let (id, written) = (entry.id, entry.written.clone());
        store.queued(entry);
        store.started(id, 1000, written.clone());
        let start = Instant::now();
        store.sample_rates_at(start);
        written.store(500, Ordering::Relaxed);
        store.sample_rates_at(start + Duration::from_secs(1));
        assert!(store.all()[0].bytes_per_second > 0);

        store.finished(id);
        store.sample_rates_at(start + Duration::from_secs(2));
        assert_eq!(store.all()[0].bytes_per_second, 0);
    }

    #[test]
    fn a_transfer_can_be_found_by_its_queue_item() {
        let store = DownloadStore::new();
        let entry = download("train");
        let (id, written) = (entry.id, entry.written.clone());
        store.queued(entry);
        store.started(id, 400, written.clone());
        written.store(100, Ordering::Relaxed);

        let phase = store.phase_of(id).expect("the transfer is there");
        assert_eq!(phase.state, PhaseKind::Running);
        assert_eq!(phase.written, 100);
        assert_eq!(phase.total, 400);
        assert!(phase.is_running());

        assert!(
            store.phase_of(QueueItemId::new()).is_none(),
            "and only that one"
        );
    }

    #[test]
    fn clearing_settled_leaves_the_running_alone() {
        let store = DownloadStore::new();
        let done = download("done");
        let done_id = done.id;
        store.queued(done);
        store.queued(download("running"));
        store.finished(done_id);

        store.clear_settled();
        let all = store.all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "running");
    }
}
