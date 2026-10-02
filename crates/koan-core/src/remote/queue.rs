use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex as StdMutex};

use parking_lot::{Condvar, Mutex};

use crate::config;
use crate::player::commands::PlayerCommand;
use crate::player::state::{LoadState, QueueItemId, SharedPlayerState};

use crate::helpers::download_track;

/// Concurrent downloads the priority lane may run outside the worker pool.
/// Small on purpose: its job is to get the track under the cursor playing, and
/// every extra request competes with it for the same link.
const PRIORITY_PERMITS: usize = 2;

/// Persistent download queue — lives for the app's lifetime.
///
/// Items are submitted via `enqueue()` and downloaded by a fixed pool of worker
/// threads. Cursor changes reorder the queue so the current track downloads
/// first, followed by same-album tracks for gapless playback; those jump the
/// queue through a permit-limited priority lane rather than by spawning
/// unbounded threads.
#[derive(Clone)]
pub struct DownloadQueue {
    inner: Arc<Inner>,
}

struct Inner {
    queue: Mutex<Queue>,
    has_work: Condvar,
    state: Arc<SharedPlayerState>,
    cmd_tx: crossbeam_channel::Sender<PlayerCommand>,
    log_buf: Arc<StdMutex<Vec<String>>>,
    /// When the cache was last trimmed to its limit.
    last_evicted: Mutex<Option<std::time::Instant>>,
    /// Worker threads started so far. Grows to the configured count as work
    /// arrives; a worker past the current count parks rather than exits.
    spawned: std::sync::atomic::AtomicUsize,
}

/// The parallel-downloads setting as it stands now, not as it stood when the
/// queue was made: the queue lives as long as the process.
fn workers_allowed() -> usize {
    config::Config::cached()
        .remote
        .download_workers
        .clamp(1, 16)
}

/// Start workers until there are as many as the setting allows.
fn ensure_workers(inner: &Arc<Inner>) {
    use std::sync::atomic::Ordering;
    let want = workers_allowed();
    loop {
        let have = inner.spawned.load(Ordering::Relaxed);
        if have >= want {
            return;
        }
        if inner
            .spawned
            .compare_exchange(have, have + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            continue;
        }
        let worker = inner.clone();
        if let Err(e) = std::thread::Builder::new()
            .name(format!("koan-dl-{have}"))
            .spawn(move || worker_loop(worker, have))
        {
            log::error!("failed to spawn download worker {have}: {e}");
            inner.spawned.fetch_sub(1, Ordering::Relaxed);
            return;
        }
    }
}

/// What a worker should fetch next: the track under the cursor ahead of
/// everything, nothing at all while that track is already being fetched, and
/// otherwise the front of the queue.
fn next_item(q: &mut Queue, cursor: Option<(i64, QueueItemId)>) -> Option<(i64, QueueItemId)> {
    match cursor {
        Some((db_id, _)) if q.in_flight.contains_key(&db_id) => None,
        Some((_, queue_id)) => q
            .pending
            .iter()
            .position(|(_, qid)| *qid == queue_id)
            .and_then(|ix| q.pending.remove(ix))
            .or_else(|| q.pending.pop_front()),
        None => q.pending.pop_front(),
    }
}

/// Someone asked for music: try the server now rather than when the outage
/// backoff next says to. The backoff is for retries nobody is waiting on; a
/// minute of it, earned while the phone was in a pocket with no signal, is not
/// how long a tap on play should take.
fn retry_server_now() {
    if let Some(client) = crate::helpers::subsonic_client(&config::Config::cached()) {
        client.outage().retry_now();
    }
}

/// The track under the cursor, when it still has to be fetched.
fn cursor_download(state: &SharedPlayerState) -> Option<(i64, QueueItemId)> {
    let id = state.cursor()?;
    let item = state.get_item(id)?;
    if !matches!(item.state, crate::player::state::ItemState::Pending) {
        return None;
    }
    Some((item.db_id?, id))
}

/// Queue state and the in-flight bookkeeping that keeps a track from being
/// downloaded by two threads at once.
#[derive(Default)]
struct Queue {
    pending: VecDeque<(i64, QueueItemId)>,
    /// Tracks being fetched, and every queue entry waiting on each.
    ///
    /// Keyed by track, because the track decides which file the download
    /// writes. Keyed by queue entry it would dedupe nothing that matters:
    /// playing something a second time before it has arrived makes a new entry
    /// with a new id, so nothing would match and a second transfer would start
    /// over the first — two threads truncating and writing one `.part`, and
    /// whichever finishes first renaming it out from under the other.
    in_flight: HashMap<i64, HashSet<QueueItemId>>,
    priority_active: usize,
}

/// What a priority request should do, given the state of the lane.
#[derive(Debug, PartialEq, Eq)]
enum Dispatch {
    /// A permit was taken and the item claimed — spawn a thread for it.
    Spawn,
    /// No permit free; the item now sits at the head of the work queue.
    Requeued,
    /// Some thread is already downloading it.
    AlreadyRunning,
}

/// Claim `item` for the priority lane, or push it to the front of the queue if
/// every permit is taken. On `Spawn` the caller owns the claim and must release
/// it via `release_priority` when the download ends.
fn claim_priority(q: &mut Queue, item: (i64, QueueItemId)) -> Dispatch {
    let (db_id, queue_id) = item;
    q.pending.retain(|(_, qid)| *qid != queue_id);

    // Already being fetched: wait on it rather than fetch it again. The entry
    // is remembered so it gets the answer when the one transfer lands.
    if let Some(waiting) = q.in_flight.get_mut(&db_id) {
        waiting.insert(queue_id);
        return Dispatch::AlreadyRunning;
    }
    if q.priority_active >= PRIORITY_PERMITS {
        q.pending.push_front(item);
        return Dispatch::Requeued;
    }
    q.priority_active += 1;
    q.in_flight.insert(db_id, HashSet::from([queue_id]));
    Dispatch::Spawn
}

fn release_priority(q: &mut Queue, db_id: i64) {
    q.in_flight.remove(&db_id);
    q.priority_active = q.priority_active.saturating_sub(1);
}

/// Releases an in-flight claim however the download ends — including a panic.
struct Claim {
    inner: Arc<Inner>,
    db_id: i64,
    priority: bool,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut q = self.inner.queue.lock();
        if self.priority {
            release_priority(&mut q, self.db_id);
        } else {
            q.in_flight.remove(&self.db_id);
        }
        drop(q);
        // Workers held back for the track under the cursor go on from here.
        self.inner.has_work.notify_all();
    }
}

impl DownloadQueue {
    /// Spawn the download queue with persistent worker threads.
    pub fn spawn(
        cmd_tx: crossbeam_channel::Sender<PlayerCommand>,
        state: Arc<SharedPlayerState>,
        log_buf: Arc<StdMutex<Vec<String>>>,
    ) -> Self {
        let inner = Arc::new(Inner {
            queue: Mutex::new(Queue::default()),
            has_work: Condvar::new(),
            state,
            cmd_tx,
            log_buf,
            last_evicted: Mutex::new(None),
            spawned: std::sync::atomic::AtomicUsize::new(0),
        });
        ensure_workers(&inner);

        let trimmer = inner.clone();
        let _ = std::thread::Builder::new()
            .name("koan-dl-trim".into())
            .spawn(move || trim_cache(&trimmer));

        let watcher_inner = inner.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("koan-dl-watch".into())
            .spawn(move || cursor_watcher(watcher_inner))
        {
            log::error!("failed to spawn download cursor watcher: {}", e);
        }

        Self { inner }
    }

    /// Add items to the download queue.
    pub fn enqueue(&self, items: Vec<(i64, QueueItemId)>) {
        if items.is_empty() {
            return;
        }
        ensure_workers(&self.inner);
        retry_server_now();
        self.inner.queue.lock().pending.extend(items);
        self.inner.has_work.notify_all();
        if let Some(cursor) = self.inner.state.cursor() {
            promote_cursor(&self.inner, cursor);
        }
    }

    /// Submit a single item for priority download (e.g. user clicked a Pending
    /// track). Also bumps same-album pending tracks for gapless playback.
    pub fn prioritize(&self, db_id: i64, queue_id: QueueItemId) {
        retry_server_now();
        dispatch_priority(&self.inner, (db_id, queue_id));

        let album_mates = self.inner.state.same_album_item_ids(queue_id);
        if !album_mates.is_empty() {
            let mate_set: HashSet<QueueItemId> = album_mates.into_iter().collect();
            bump_to_front(&mut self.inner.queue.lock().pending, &mate_set);
            self.inner.has_work.notify_all();
        }
    }
}

/// Move every item whose id is in `ids` ahead of the rest, preserving order.
fn bump_to_front(pending: &mut VecDeque<(i64, QueueItemId)>, ids: &HashSet<QueueItemId>) {
    let (front, rest): (VecDeque<_>, VecDeque<_>) =
        pending.drain(..).partition(|(_, qid)| ids.contains(qid));
    *pending = front;
    pending.extend(rest);
}

/// Start a priority download, or queue it at the front when the lane is full.
fn dispatch_priority(inner: &Arc<Inner>, item: (i64, QueueItemId)) {
    let dispatch = claim_priority(&mut inner.queue.lock(), item);
    match dispatch {
        Dispatch::AlreadyRunning => {}
        Dispatch::Requeued => {
            inner.has_work.notify_one();
        }
        Dispatch::Spawn => {
            let spawn_inner = inner.clone();
            let spawned = std::thread::Builder::new()
                .name("koan-dl-prio".into())
                .spawn(move || {
                    let _claim = Claim {
                        inner: spawn_inner.clone(),
                        db_id: item.0,
                        priority: true,
                    };
                    run_download(&spawn_inner, item);
                });
            if let Err(e) = spawned {
                log::error!("failed to spawn priority download: {}", e);
                let mut q = inner.queue.lock();
                release_priority(&mut q, item.0);
                q.pending.push_front(item);
                drop(q);
                inner.has_work.notify_one();
            }
        }
    }
}

/// Hand every other entry waiting on this track the answer the transfer got.
///
/// Two entries for one track are one download and two things to tell. Without
/// this the second sits `Pending` forever, waiting for a transfer that already
/// finished and will not run again.
fn settle_waiters(inner: &Arc<Inner>, db_id: i64, downloaded: QueueItemId) {
    let waiting: Vec<QueueItemId> = {
        let q = inner.queue.lock();
        q.in_flight
            .get(&db_id)
            .map(|ids| ids.iter().copied().filter(|id| *id != downloaded).collect())
            .unwrap_or_default()
    };
    if waiting.is_empty() {
        return;
    }
    let Some(item) = inner.state.get_item(downloaded) else {
        return;
    };
    for id in waiting {
        inner.state.update_paths(&[(id, item.path.clone())]);
        inner.state.update_item_state(id, item.state.clone());
        // The player only wakes for this, so an entry the cursor is sitting on
        // would otherwise wait on a download that has already happened.
        if inner.state.is_cursor(id) {
            inner.cmd_tx.send(PlayerCommand::TrackReady(id)).ok();
        }
    }
}

/// Run one download, containing any panic so the worker pool never shrinks.
///
/// The client is looked up per download, not once for the queue's lifetime:
/// the queue lives as long as the process, and signing in, out or elsewhere
/// has to reach it.
fn run_download(inner: &Arc<Inner>, (db_id, queue_id): (i64, QueueItemId)) {
    let cfg = config::Config::cached();
    let Some(client) = crate::helpers::subsonic_client(&cfg) else {
        // Failed, not left Pending: the player waits for Ready, so a queue of
        // tracks that can never arrive would otherwise sit saying nothing.
        crate::helpers::fail_track(
            &inner.state,
            &inner.cmd_tx,
            queue_id,
            crate::helpers::remote_unavailable(&cfg),
        );
        return;
    };

    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        download_track(
            db_id,
            queue_id,
            &inner.cmd_tx,
            &inner.log_buf,
            &inner.state,
            &cfg,
            &client,
        );
    }));

    if outcome.is_err() {
        log::error!("download panicked for {:?}", queue_id);
        crate::helpers::fail_track(
            &inner.state,
            &inner.cmd_tx,
            queue_id,
            "download panicked".into(),
        );
    }

    // Before the claim is released, while the waiting entries are still
    // recorded against this track.
    settle_waiters(inner, db_id, queue_id);
    trim_cache(inner);
}

/// How often the cache is checked against its limit, at most: each download
/// adds to it, and a check reads the whole cache's size from the database.
const EVICT_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Trim the cache to its configured limit, keeping everything in the queue:
/// the player may be reading those files, and they are what is wanted next.
/// The limit is read afresh, so one set in Settings applies without a
/// restart.
fn trim_cache(inner: &Inner) {
    {
        let mut last = inner.last_evicted.lock();
        if last.is_some_and(|t| t.elapsed() < EVICT_EVERY) {
            return;
        }
        *last = Some(std::time::Instant::now());
    }
    let cfg = config::Config::cached();
    if cfg.cache_limit_bytes().is_none() {
        return;
    }
    let keep = inner
        .state
        .snapshot_playlist()
        .0
        .iter()
        .filter_map(|i| i.db_id)
        .collect();
    match crate::db::pool::shared().get() {
        Ok(db) => {
            crate::helpers::evict_cache(&db, &cfg, &keep, false);
        }
        Err(e) => log::warn!("cache eviction: could not open the database: {e}"),
    }
}

/// Worker loop: wait for work, download, repeat.
///
/// While the server is down, workers take nothing new: the download that found
/// it down waits it out, and everything behind it stays `Pending` rather than
/// each piling onto a server that is not answering.
/// One download worker. `index` is its place in the pool: a worker past the
/// current parallel-downloads setting parks until the setting lets it work.
///
/// While the track under the cursor is still being fetched, no worker starts
/// another transfer. On a slow link every parallel download takes a share of
/// the bandwidth, and the one track someone is waiting to hear would arrive
/// last among equals; the rest of the album can follow it.
fn worker_loop(inner: Arc<Inner>, index: usize) {
    loop {
        let item = loop {
            if let Some(client) = crate::helpers::subsonic_client(&config::Config::cached()) {
                client.outage().hold();
            }
            // Read before the queue lock, which is never held across the
            // player state's.
            let cursor = cursor_download(&inner.state);
            let mut q = inner.queue.lock();
            if index >= workers_allowed() {
                inner.has_work.wait(&mut q);
                continue;
            }
            match next_item(&mut q, cursor) {
                Some(item) => {
                    // Already being fetched: this entry waits on the one
                    // transfer rather than starting a second over it.
                    match q.in_flight.get_mut(&item.0) {
                        Some(waiting) => {
                            waiting.insert(item.1);
                        }
                        None => {
                            q.in_flight.insert(item.0, HashSet::from([item.1]));
                            break item;
                        }
                    }
                }
                None => inner.has_work.wait(&mut q),
            }
        };
        let _claim = Claim {
            inner: inner.clone(),
            db_id: item.0,
            priority: false,
        };
        run_download(&inner, item);
    }
}

/// Cursor watcher: when the cursor moves to a pending track, hand it and the
/// next track to the priority lane and bump same-album tracks to the front.
///
/// Also wakes the workers, which hold back while the cursor's track is being
/// fetched: one that looked before the cursor moved is waiting on a track
/// nobody wants any more.
///
/// Looks before it first waits. The watcher is made with the queue, by the
/// first downloads queued — on the first play after launch, by that play — and
/// the player can move the cursor between `enqueue` looking at it and this
/// thread starting to listen. Waiting first, that move was missed by both, and
/// the track waited its turn behind whatever else was queued.
fn cursor_watcher(inner: Arc<Inner>) {
    let changed = crate::signal::engine_changed();
    let mut seen = changed.generation();
    let mut last_cursor: Option<QueueItemId> = None;
    loop {
        let current = inner.state.cursor();
        if current != last_cursor {
            last_cursor = current;
            inner.has_work.notify_all();
            if let Some(cursor_id) = current {
                promote_cursor(&inner, cursor_id);
            }
        }
        seen = changed.wait(seen);
    }
}

/// Send the cursor's track, and the one after it, down the priority lane, if
/// the cursor's track is waiting in the queue.
///
/// Called when the cursor moves and when tracks are queued, because either can
/// happen first: playing an album sends the new queue to the player and queues
/// its downloads at once, and the player may set the cursor before or after.
/// Waiting for the cursor alone missed the first play after launch, when the
/// queue (and this watcher) did not exist until those downloads made it.
fn promote_cursor(inner: &Arc<Inner>, cursor_id: QueueItemId) {
    let is_pending = inner
        .state
        .item_load_state(cursor_id)
        .is_some_and(|s| matches!(s, LoadState::Pending));
    if !is_pending {
        return;
    }

    let album_mate_ids: HashSet<QueueItemId> = inner
        .state
        .same_album_item_ids(cursor_id)
        .into_iter()
        .collect();

    let mut priority_items = Vec::new();
    {
        let mut q = inner.queue.lock();
        if let Some(pos) = q.pending.iter().position(|(_, qid)| *qid == cursor_id) {
            priority_items.push(q.pending.remove(pos).expect("position just found"));

            if !album_mate_ids.is_empty() {
                bump_to_front(&mut q.pending, &album_mate_ids);
            }

            // Grab the next track too, for gapless lookahead.
            if let Some(next) = q.pending.pop_front() {
                priority_items.push(next);
            }
        }
    }

    for item in priority_items {
        dispatch_priority(inner, item);
    }
}

/// The process's download queue.
///
/// One player means one pool, one priority lane and one cursor watcher; a
/// second set would compete with the first for the same link and the same
/// cursor. Every front end reaches downloads through here — the TUI directly,
/// the FFI and the GraphQL server through `helpers::spawn_downloads`.
///
/// `log_buf` is only honoured by whoever initialises it, which is the TUI when
/// it is running, since it is the only front end that shows the buffer.
pub fn shared(
    cmd_tx: &crossbeam_channel::Sender<PlayerCommand>,
    state: &Arc<SharedPlayerState>,
    log_buf: Option<Arc<StdMutex<Vec<String>>>>,
) -> &'static DownloadQueue {
    static QUEUE: std::sync::OnceLock<DownloadQueue> = std::sync::OnceLock::new();
    QUEUE.get_or_init(|| {
        DownloadQueue::spawn(
            cmd_tx.clone(),
            state.clone(),
            log_buf.unwrap_or_else(|| Arc::new(StdMutex::new(Vec::new()))),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qid() -> QueueItemId {
        QueueItemId::new()
    }

    #[test]
    fn priority_lane_never_exceeds_its_permits() {
        let mut q = Queue::default();

        // Rapid cursor movement: a fresh track lands on the lane every poll.
        let mut spawned = 0;
        for i in 0..500 {
            if claim_priority(&mut q, (i, qid())) == Dispatch::Spawn {
                spawned += 1;
            }
            assert!(
                q.priority_active <= PRIORITY_PERMITS,
                "priority lane over its permit count at iteration {}",
                i
            );
        }

        assert_eq!(spawned, PRIORITY_PERMITS, "only permitted claims may spawn");
        assert_eq!(
            q.pending.len(),
            500 - PRIORITY_PERMITS,
            "everything else must be queued, not dropped"
        );
    }

    #[test]
    fn released_permits_are_reusable() {
        let mut q = Queue::default();
        assert_eq!(claim_priority(&mut q, (1, qid())), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, (2, qid())), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, (3, qid())), Dispatch::Requeued);

        release_priority(&mut q, 1);
        assert_eq!(claim_priority(&mut q, (4, qid())), Dispatch::Spawn);
        assert!(q.priority_active <= PRIORITY_PERMITS);
    }

    #[test]
    fn an_in_flight_track_is_never_claimed_twice() {
        let mut q = Queue::default();
        let id = qid();
        assert_eq!(claim_priority(&mut q, (1, id)), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, (1, id)), Dispatch::AlreadyRunning);
        assert_eq!(q.priority_active, 1);
        assert!(
            q.pending.is_empty(),
            "a duplicate request must not re-queue the track"
        );
    }

    #[test]
    fn playing_a_track_again_joins_the_transfer_already_running() {
        // Playing something twice before it has arrived makes a second queue
        // entry with an id of its own. The track is the same, and so is the
        // file a download would write — two of them would truncate and write
        // over one another, and whichever finished first would rename it away
        // from the other.
        let mut q = Queue::default();
        let (first, again) = (qid(), qid());
        assert_eq!(claim_priority(&mut q, (7, first)), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, (7, again)), Dispatch::AlreadyRunning);

        assert_eq!(q.priority_active, 1, "one transfer, not two");
        assert!(q.pending.is_empty());
        assert_eq!(
            q.in_flight.get(&7),
            Some(&HashSet::from([first, again])),
            "both entries wait on the one transfer"
        );
    }

    #[test]
    fn a_worker_picking_up_a_duplicate_waits_on_the_running_one() {
        // The same, arriving through the queue rather than the priority lane.
        let mut q = Queue::default();
        let (running, queued) = (qid(), qid());
        assert_eq!(claim_priority(&mut q, (7, running)), Dispatch::Spawn);

        // What `worker_loop` does with the next pending item.
        match q.in_flight.get_mut(&7) {
            Some(waiting) => {
                waiting.insert(queued);
            }
            None => panic!("the track should already be claimed"),
        }

        assert_eq!(
            q.in_flight.get(&7),
            Some(&HashSet::from([running, queued])),
            "the queued entry waits rather than starting a second transfer"
        );
    }

    #[test]
    fn different_tracks_still_run_side_by_side() {
        // Keying on the track must not serialise unrelated downloads.
        let mut q = Queue::default();
        assert_eq!(claim_priority(&mut q, (1, qid())), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, (2, qid())), Dispatch::Spawn);
        assert_eq!(q.priority_active, 2);
    }

    #[test]
    fn requeued_priority_item_goes_to_the_head_of_the_queue() {
        let mut q = Queue::default();
        q.pending.push_back((9, qid()));
        for i in 0..PRIORITY_PERMITS {
            claim_priority(&mut q, (i as i64, qid()));
        }

        let wanted = qid();
        assert_eq!(claim_priority(&mut q, (7, wanted)), Dispatch::Requeued);
        assert_eq!(q.pending.front().map(|(_, id)| *id), Some(wanted));
    }

    #[test]
    fn claiming_removes_a_duplicate_queue_entry() {
        let mut q = Queue::default();
        let id = qid();
        q.pending.push_back((1, id));
        q.pending.push_back((2, qid()));

        assert_eq!(claim_priority(&mut q, (1, id)), Dispatch::Spawn);
        assert_eq!(
            q.pending.len(),
            1,
            "the pool must not also pick up the claimed track"
        );
    }

    #[test]
    fn bump_to_front_preserves_relative_order() {
        let (a, b, c, d) = (qid(), qid(), qid(), qid());
        let mut pending: VecDeque<(i64, QueueItemId)> =
            [(1, a), (2, b), (3, c), (4, d)].into_iter().collect();
        let mates: HashSet<QueueItemId> = [b, d].into_iter().collect();

        bump_to_front(&mut pending, &mates);

        let order: Vec<QueueItemId> = pending.iter().map(|(_, id)| *id).collect();
        assert_eq!(order, vec![b, d, a, c]);
    }

    #[test]
    fn the_track_under_the_cursor_goes_first_and_goes_alone() {
        let (a, b, c) = (qid(), qid(), qid());
        let mut q = Queue::default();
        q.pending.extend([(1, a), (2, b), (3, c)]);

        // Pressed play on the third: it jumps the queue.
        assert_eq!(next_item(&mut q, Some((3, c))), Some((3, c)));
        q.in_flight.insert(3, HashSet::from([c]));

        // While it downloads, nothing else starts.
        assert_eq!(next_item(&mut q, Some((3, c))), None);
        assert_eq!(q.pending.len(), 2, "the rest wait their turn");

        // Once it has landed the queue runs in order again.
        q.in_flight.remove(&3);
        assert_eq!(next_item(&mut q, None), Some((1, a)));
        assert_eq!(next_item(&mut q, None), Some((2, b)));
    }

    #[test]
    fn a_cursor_with_nothing_queued_for_it_holds_nothing_up() {
        let (a, elsewhere) = (qid(), qid());
        let mut q = Queue::default();
        q.pending.push_back((1, a));
        assert_eq!(next_item(&mut q, Some((9, elsewhere))), Some((1, a)));
    }

    /// The first play after launch: the player has the new queue and its
    /// cursor, and the download queue holds its tracks.
    fn first_play() -> (Arc<Inner>, Vec<(i64, QueueItemId)>) {
        crate::config::isolate_config_for_tests();
        let item = |title: &str, db_id: i64| crate::player::state::PlaylistItem {
            playlist_entry_id: None,
            id: qid(),
            db_id: Some(db_id),
            path: std::path::PathBuf::from(format!("/cache/{title}.flac")),
            title: title.into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            state: crate::player::state::ItemState::Pending,
        };
        let items = vec![item("one", 1), item("two", 2), item("three", 3)];
        let ids: Vec<_> = items.iter().map(|i| (i.db_id.unwrap(), i.id)).collect();

        // The player has the new queue and its cursor before the download
        // queue hears of any of it — the first play after launch.
        let state = SharedPlayerState::new();
        state.add_items(items);
        state.set_cursor(Some(ids[0].1));

        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded();
        let inner = Arc::new(Inner {
            queue: Mutex::new(Queue::default()),
            has_work: Condvar::new(),
            state,
            cmd_tx,
            log_buf: Arc::new(StdMutex::new(Vec::new())),
            last_evicted: Mutex::new(None),
            spawned: std::sync::atomic::AtomicUsize::new(0),
        });
        inner.queue.lock().pending.extend(ids.iter().copied());
        (inner, ids)
    }

    #[test]
    fn a_cursor_set_before_its_tracks_were_queued_still_goes_first() {
        let (inner, ids) = first_play();
        promote_cursor(&inner, ids[0].1);

        let left: Vec<_> = inner.queue.lock().pending.iter().copied().collect();
        assert_eq!(
            left,
            vec![ids[2]],
            "the cursor's track and the next went to the priority lane"
        );
    }

    /// The watcher starts after the cursor moved, with nothing further to wake
    /// it: it still sends the cursor's track ahead.
    #[test]
    fn a_watcher_started_after_the_cursor_moved_still_promotes_it() {
        let (inner, ids) = first_play();
        let watched = inner.clone();
        std::thread::spawn(move || cursor_watcher(watched));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while inner.queue.lock().pending.len() > 1 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let left: Vec<_> = inner.queue.lock().pending.iter().copied().collect();
        assert_eq!(left, vec![ids[2]], "promoted without waiting for a change");
    }
}
