use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use parking_lot::{Condvar, Mutex};

use crate::config;
use crate::player::commands::PlayerCommand;
use crate::player::state::{LoadState, QueueItemId, SharedPlayerState};

use crate::helpers::{Fetched, download_track, settle_transfer};

/// Concurrent downloads the priority lane may run outside the worker pool.
/// Small on purpose: its job is to get the track under the cursor playing, and
/// every extra request competes with it for the same link.
const PRIORITY_PERMITS: usize = 2;

/// Persistent download queue — lives for the app's lifetime.
///
/// Follows the playlist rather than being told about it: whenever the
/// playlist changes, every entry still waiting for its file is queued and
/// anything no longer in the playlist is dropped. A front end adds tracks to
/// the player and nothing else, so there is no second request to race the
/// first and no queue of entries the player has already discarded.
///
/// Downloads run on a pool of worker threads. Cursor changes reorder the queue
/// so the current track downloads first, followed by same-album tracks for
/// gapless playback; those jump the queue through a permit-limited priority
/// lane rather than by spawning unbounded threads.
#[derive(Clone)]
pub struct DownloadQueue {
    inner: Arc<Inner>,
}

struct Inner {
    /// May be held while the player's state is read or written, never the
    /// other way round.
    queue: Mutex<Queue>,
    has_work: Condvar,
    state: Arc<SharedPlayerState>,
    cmd_tx: crossbeam_channel::Sender<PlayerCommand>,
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
            .spawn(move || {
                // The cache is trimmed once at the start, by the first worker
                // rather than when the queue is made: a player that never
                // fetches anything has no business reading the cache's size.
                if have == 0 {
                    trim_cache(&worker);
                }
                worker_loop(worker, have)
            })
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
///
/// Tracks wanted only in the cache come after everything in the playlist.
fn next_item(q: &mut Queue, cursor: Option<(i64, QueueItemId)>) -> Option<Job> {
    let entry = match cursor {
        Some((db_id, _)) if q.in_flight.contains_key(&db_id) => return None,
        Some((_, queue_id)) => q
            .pending
            .iter()
            .position(|(_, qid)| *qid == queue_id)
            .and_then(|ix| q.pending.remove(ix))
            .or_else(|| q.pending.pop_front()),
        None => q.pending.pop_front(),
    };
    entry
        .map(|(db_id, id)| (db_id, Some(id)))
        .or_else(|| q.cache.pop_front().map(|db_id| (db_id, None)))
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

/// A track to fetch, and the queue entry it is for — `None` for a track
/// wanted only in the cache.
type Job = (i64, Option<QueueItemId>);

/// Queue state and the in-flight bookkeeping that keeps a track from being
/// downloaded by two threads at once.
#[derive(Default)]
struct Queue {
    /// Queue entries whose files are still to be fetched, in the order they
    /// will be.
    pending: VecDeque<(i64, QueueItemId)>,
    /// Tracks wanted in the cache with no queue entry behind them.
    cache: VecDeque<i64>,
    /// Tracks being fetched, and what is waiting on each.
    ///
    /// Keyed by track, because the track decides which file the download
    /// writes. Keyed by queue entry it would dedupe nothing that matters:
    /// playing something a second time before it has arrived makes a new entry
    /// with a new id, so nothing would match and a second transfer would start
    /// over the first — two threads truncating and writing one `.part`, and
    /// whichever finishes first renaming it out from under the other.
    in_flight: HashMap<i64, Transfer>,
    priority_active: usize,
}

/// What one track's transfer is for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Transfer {
    /// Every queue entry waiting on it, including the one that started it.
    waiters: HashSet<QueueItemId>,
    /// Wanted in the cache for its own sake, whatever the playlist does.
    keep: bool,
}

impl Transfer {
    fn for_entry(id: QueueItemId) -> Self {
        Self {
            waiters: HashSet::from([id]),
            keep: false,
        }
    }

    fn wanted(&self) -> bool {
        self.keep || !self.waiters.is_empty()
    }
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
/// every permit is taken. On `Spawn` the caller owns a permit and must hand it
/// back, through a `Permit`, when the download ends.
fn claim_priority(q: &mut Queue, item: (i64, QueueItemId)) -> Dispatch {
    let (db_id, queue_id) = item;
    q.pending.retain(|(_, qid)| *qid != queue_id);

    // Already being fetched: wait on it rather than fetch it again. The entry
    // is remembered so it gets the answer when the one transfer lands.
    if let Some(transfer) = q.in_flight.get_mut(&db_id) {
        transfer.waiters.insert(queue_id);
        return Dispatch::AlreadyRunning;
    }
    if q.priority_active >= PRIORITY_PERMITS {
        q.pending.push_front(item);
        return Dispatch::Requeued;
    }
    q.priority_active += 1;
    q.in_flight.insert(db_id, Transfer::for_entry(queue_id));
    Dispatch::Spawn
}

/// Hands back a priority permit however the download ends — including a panic.
///
/// The track's in-flight entry is not this guard's to remove: settling takes
/// it, and by the time this runs a new transfer for the same track may have
/// claimed it.
struct Permit {
    inner: Arc<Inner>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut q = self.inner.queue.lock();
        q.priority_active = q.priority_active.saturating_sub(1);
        drop(q);
        self.inner.has_work.notify_all();
    }
}

impl DownloadQueue {
    /// Spawn the download queue and the thread that keeps it following the
    /// playlist. Workers start with the first thing to fetch.
    pub fn spawn(
        cmd_tx: crossbeam_channel::Sender<PlayerCommand>,
        state: Arc<SharedPlayerState>,
    ) -> Self {
        let inner = Arc::new(Inner {
            queue: Mutex::new(Queue::default()),
            has_work: Condvar::new(),
            state,
            cmd_tx,
            last_evicted: Mutex::new(None),
            spawned: std::sync::atomic::AtomicUsize::new(0),
        });

        let watcher_inner = inner.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("koan-dl-watch".into())
            .spawn(move || follow_playlist(watcher_inner))
        {
            log::error!("failed to spawn download playlist watcher: {}", e);
        }

        Self { inner }
    }

    /// Fetch these tracks into the cache, with no queue entry to play them.
    /// They go behind everything the playlist is waiting on.
    pub fn cache(&self, track_ids: Vec<i64>) {
        if track_ids.is_empty() {
            return;
        }
        self.inner.queue.lock().cache.extend(track_ids);
        wake_workers(&self.inner);
    }
}

/// There is work: start what workers the setting allows, try the server now,
/// and wake them.
fn wake_workers(inner: &Arc<Inner>) {
    ensure_workers(inner);
    retry_server_now();
    inner.has_work.notify_all();
}

/// Bring the queue in line with the playlist.
///
/// Every entry still waiting for its file is either queued or waiting on the
/// transfer for its track; anything the playlist no longer holds is let go.
/// Order is kept for what was already queued — the cursor's promotions stand —
/// and new arrivals join the back in playlist order.
///
/// The playlist is read under the queue lock, as settling writes it: read
/// before, it could show an entry still pending whose transfer settled a
/// moment later, and the entry would be fetched a second time.
fn sync(inner: &Arc<Inner>) {
    let mut q = inner.queue.lock();
    let wanted = inner.state.pending_downloads();
    let added = sync_with(&mut q, &wanted);
    drop(q);
    if added {
        wake_workers(inner);
    }
}

/// [`sync`] against a list already read. Whether anything was queued.
fn sync_with(q: &mut Queue, wanted: &[(i64, QueueItemId)]) -> bool {
    let ids: HashSet<QueueItemId> = wanted.iter().map(|(_, id)| *id).collect();
    q.pending.retain(|(_, id)| ids.contains(id));
    for transfer in q.in_flight.values_mut() {
        transfer.waiters.retain(|id| ids.contains(id));
    }

    let queued: HashSet<QueueItemId> = q.pending.iter().map(|(_, id)| *id).collect();
    let mut added = false;
    for &(db_id, id) in wanted {
        if queued.contains(&id) {
            continue;
        }
        match q.in_flight.get_mut(&db_id) {
            Some(transfer) => {
                transfer.waiters.insert(id);
            }
            None => {
                q.pending.push_back((db_id, id));
                added = true;
            }
        }
    }
    added
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
                    let _permit = Permit {
                        inner: spawn_inner.clone(),
                    };
                    run_download(&spawn_inner, (item.0, Some(item.1)));
                });
            if let Err(e) = spawned {
                log::error!("failed to spawn priority download: {}", e);
                let mut q = inner.queue.lock();
                q.in_flight.remove(&item.0);
                q.priority_active = q.priority_active.saturating_sub(1);
                q.pending.push_front(item);
                drop(q);
                inner.has_work.notify_one();
            }
        }
    }
}

/// Every queue entry the transfer for `db_id` is for now, or `None` once
/// nothing wants it — what a download asks while it runs.
fn waiting(inner: &Inner, db_id: i64) -> Option<Vec<QueueItemId>> {
    let q = inner.queue.lock();
    q.in_flight
        .get(&db_id)
        .filter(|t| t.wanted())
        .map(|t| t.waiters.iter().copied().collect())
}

/// Run one download, containing any panic so the worker pool never shrinks,
/// and settle every entry waiting on it.
///
/// The client is looked up per download, not once for the queue's lifetime:
/// the queue lives as long as the process, and signing in, out or elsewhere
/// has to reach it.
fn run_download(inner: &Arc<Inner>, (db_id, entry): Job) {
    // A track fetched only to cache has no entry to name its transfer by.
    let lead = entry.unwrap_or_else(QueueItemId::new);
    let cfg = config::Config::cached();
    let fetched = match crate::helpers::subsonic_client(&cfg) {
        // Failed, not left Pending: the player waits for Ready, so a queue of
        // tracks that can never arrive would otherwise sit saying nothing.
        None => Some(Fetched {
            result: Err(crate::helpers::remote_unavailable(&cfg)),
            transfer: None,
        }),
        Some(client) => {
            let still_waiting = || waiting(inner, db_id);
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                download_track(
                    db_id,
                    lead,
                    &still_waiting,
                    &inner.cmd_tx,
                    &inner.state,
                    &cfg,
                    &client,
                )
            }))
            .unwrap_or_else(|_| {
                log::error!("download panicked for track {db_id}");
                Some(Fetched {
                    result: Err("download panicked".into()),
                    transfer: None,
                })
            })
        }
    };

    // Under the queue lock, so no entry can join the transfer between being
    // told and the transfer being forgotten: one arriving after this finds no
    // transfer and is queued afresh.
    let mut q = inner.queue.lock();
    let transfer = q.in_flight.remove(&db_id).unwrap_or_default();
    match fetched {
        Some(fetched) => {
            let waiters: Vec<QueueItemId> = transfer.waiters.into_iter().collect();
            settle_transfer(
                &inner.state,
                &inner.cmd_tx,
                &waiters,
                fetched.transfer.as_ref().map(|(id, feed)| (*id, &**feed)),
                &fetched.result,
            );
        }
        // Withdrawn because nothing wanted it, as far as it last looked. An
        // entry that has asked since goes back to the front.
        None => {
            for id in transfer.waiters {
                q.pending.push_front((db_id, id));
            }
        }
    }
    drop(q);
    // Workers held back for the track under the cursor go on from here.
    inner.has_work.notify_all();
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
            let cursor = cursor_download(&inner.state);
            let mut q = inner.queue.lock();
            if index >= workers_allowed() {
                inner.has_work.wait(&mut q);
                continue;
            }
            match next_item(&mut q, cursor) {
                Some(job) => {
                    if claim(&mut q, job) {
                        break job;
                    }
                }
                None => inner.has_work.wait(&mut q),
            }
        };
        run_download(&inner, item);
    }
}

/// Claim a worker's job: true when this worker is to run it. A track already
/// being fetched is not fetched again; the entry waits on that transfer.
fn claim(q: &mut Queue, (db_id, entry): Job) -> bool {
    match (q.in_flight.get_mut(&db_id), entry) {
        (Some(transfer), Some(id)) => {
            transfer.waiters.insert(id);
            false
        }
        (Some(transfer), None) => {
            transfer.keep = true;
            false
        }
        (None, Some(id)) => {
            q.in_flight.insert(db_id, Transfer::for_entry(id));
            true
        }
        (None, None) => {
            q.in_flight.insert(
                db_id,
                Transfer {
                    waiters: HashSet::new(),
                    keep: true,
                },
            );
            true
        }
    }
}

/// Keep the queue following the player: on any playlist change, bring the
/// queue in line with it; when the cursor moves to a pending track, hand it
/// and the next track to the priority lane and bump same-album tracks to the
/// front.
///
/// A cursor move also wakes the workers, which hold back while the cursor's
/// track is being fetched: one that looked before the cursor moved is waiting
/// on a track nobody wants any more.
///
/// Looks before it first waits, so a playlist the player already holds when
/// the queue is made — a restored session — is fetched without waiting for
/// something else to change.
fn follow_playlist(inner: Arc<Inner>) {
    let changed = crate::signal::engine_changed();
    let mut seen = changed.generation();
    let mut last_version: Option<u64> = None;
    let mut last_cursor: Option<QueueItemId> = None;
    loop {
        // Before the cursor, so the cursor's track is queued by the time it
        // is looked for.
        let version = inner.state.playlist_version();
        if last_version != Some(version) {
            last_version = Some(version);
            sync(&inner);
        }
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
/// One player means one pool, one priority lane and one playlist watcher; a
/// second set would compete with the first for the same link and the same
/// cursor. Made by `Player::spawn`, so whatever front end runs the player
/// gets downloads with it.
pub fn shared(
    cmd_tx: &crossbeam_channel::Sender<PlayerCommand>,
    state: &Arc<SharedPlayerState>,
) -> &'static DownloadQueue {
    static QUEUE: std::sync::OnceLock<DownloadQueue> = std::sync::OnceLock::new();
    QUEUE.get_or_init(|| DownloadQueue::spawn(cmd_tx.clone(), state.clone()))
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

        q.in_flight.remove(&1);
        q.priority_active -= 1;
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
            q.in_flight.get(&7).map(|t| &t.waiters),
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

        assert!(
            !claim(&mut q, (7, Some(queued))),
            "not fetched a second time"
        );

        assert_eq!(
            q.in_flight.get(&7).map(|t| &t.waiters),
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
        assert_eq!(next_item(&mut q, Some((3, c))), Some((3, Some(c))));
        q.in_flight.insert(3, Transfer::for_entry(c));

        // While it downloads, nothing else starts.
        assert_eq!(next_item(&mut q, Some((3, c))), None);
        assert_eq!(q.pending.len(), 2, "the rest wait their turn");

        // Once it has landed the queue runs in order again.
        q.in_flight.remove(&3);
        assert_eq!(next_item(&mut q, None), Some((1, Some(a))));
        assert_eq!(next_item(&mut q, None), Some((2, Some(b))));
    }

    #[test]
    fn a_cursor_with_nothing_queued_for_it_holds_nothing_up() {
        let (a, elsewhere) = (qid(), qid());
        let mut q = Queue::default();
        q.pending.push_back((1, a));
        assert_eq!(next_item(&mut q, Some((9, elsewhere))), Some((1, Some(a))));
    }

    #[test]
    fn a_track_wanted_only_in_the_cache_waits_behind_the_playlist() {
        let a = qid();
        let mut q = Queue::default();
        q.cache.push_back(5);
        q.pending.push_back((1, a));
        assert_eq!(next_item(&mut q, None), Some((1, Some(a))));
        assert_eq!(next_item(&mut q, None), Some((5, None)));
    }

    #[test]
    fn a_track_fetched_to_cache_is_wanted_with_nothing_waiting_on_it() {
        // `download_to_cache` has no queue entry; its transfer must not read
        // as abandoned for want of one.
        let mut q = Queue::default();
        assert!(claim(&mut q, (5, None)));
        assert!(q.in_flight[&5].wanted());

        // And a cache request for a track already on its way joins it.
        let mut q = Queue::default();
        assert!(claim(&mut q, (5, Some(qid()))));
        assert!(!claim(&mut q, (5, None)));
        assert!(q.in_flight[&5].keep);
    }

    #[test]
    fn the_queue_lets_go_of_what_the_playlist_no_longer_holds() {
        // Replacing a large queue used to leave every old entry queued, and
        // each cost a worker a wait before it gave up.
        let (old, kept, waiter) = (qid(), qid(), qid());
        let mut q = Queue::default();
        q.pending.extend([(1, old), (2, kept)]);
        q.in_flight.insert(3, Transfer::for_entry(waiter));

        sync_with(&mut q, &[(2, kept)]);

        assert_eq!(q.pending, VecDeque::from([(2, kept)]));
        assert!(
            !q.in_flight[&3].wanted(),
            "a transfer nothing waits on any more reads as abandoned"
        );
    }

    #[test]
    fn the_queue_takes_new_entries_in_playlist_order_behind_what_it_has() {
        let (promoted, a, b) = (qid(), qid(), qid());
        let mut q = Queue::default();
        q.pending.push_back((9, promoted));

        assert!(sync_with(&mut q, &[(1, a), (9, promoted), (2, b)]));

        assert_eq!(q.pending, VecDeque::from([(9, promoted), (1, a), (2, b)]));
        assert!(
            !sync_with(&mut q, &[(1, a), (9, promoted), (2, b)]),
            "idempotent"
        );
    }

    #[test]
    fn a_new_entry_for_a_track_in_flight_waits_on_that_transfer() {
        let (running, again) = (qid(), qid());
        let mut q = Queue::default();
        q.in_flight.insert(7, Transfer::for_entry(running));

        assert!(!sync_with(&mut q, &[(7, running), (7, again)]));

        assert!(q.pending.is_empty());
        assert_eq!(q.in_flight[&7].waiters, HashSet::from([running, again]));
    }

    /// The first play after launch: the player has the new queue and its
    /// cursor, and the download queue holds its tracks.
    fn first_play() -> (Arc<Inner>, Vec<(i64, QueueItemId)>) {
        let (inner, ids, _) = queue_over(&[9001, 9002, 9003]);
        inner.queue.lock().pending.extend(ids.iter().copied());
        (inner, ids)
    }

    /// A player holding one pending item per track id, the cursor on the
    /// first, and a download queue over it that has heard of none of them.
    fn queue_over(
        tracks: &[i64],
    ) -> (
        Arc<Inner>,
        Vec<(i64, QueueItemId)>,
        crossbeam_channel::Receiver<PlayerCommand>,
    ) {
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
        let items: Vec<_> = tracks
            .iter()
            .map(|&db_id| item(&format!("track-{db_id}"), db_id))
            .collect();
        let ids: Vec<_> = items.iter().map(|i| (i.db_id.unwrap(), i.id)).collect();

        let state = SharedPlayerState::new();
        state.add_items(items);
        state.set_cursor(Some(ids[0].1));

        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let inner = Arc::new(Inner {
            queue: Mutex::new(Queue::default()),
            has_work: Condvar::new(),
            state,
            cmd_tx,
            last_evicted: Mutex::new(None),
            spawned: std::sync::atomic::AtomicUsize::new(0),
        });
        (inner, ids, cmd_rx)
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

    /// The watcher starts after the player already has its queue and cursor,
    /// with nothing further to wake it: it finds the tracks by itself and
    /// fetches every one. With no server, each fails rather than waiting.
    #[test]
    fn a_watcher_started_over_a_playlist_fetches_it_unprompted() {
        let (inner, ids) = first_play();
        inner.queue.lock().pending.clear();
        let watched = inner.clone();
        std::thread::spawn(move || follow_playlist(watched));

        let failed = |id| matches!(inner.state.item_load_state(id), Some(LoadState::Failed(_)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ids.iter().all(|(_, id)| failed(*id)) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ids.iter().all(|(_, id)| failed(*id)));
    }

    /// A track queued twice, both entries waiting on its one transfer, the
    /// second under the cursor. The transfer cannot happen: no server.
    fn duplicate_that_fails() -> (
        Arc<Inner>,
        crossbeam_channel::Receiver<PlayerCommand>,
        QueueItemId,
        QueueItemId,
    ) {
        let (inner, ids, cmd_rx) = queue_over(&[9101, 9101]);
        let (first, again) = (ids[0].1, ids[1].1);
        inner.state.set_cursor(Some(again));
        inner.queue.lock().in_flight.insert(
            9101,
            Transfer {
                waiters: HashSet::from([first, again]),
                keep: false,
            },
        );
        (inner, cmd_rx, first, again)
    }

    #[test]
    fn a_failed_transfer_fails_every_entry_waiting_on_it() {
        let (inner, cmd_rx, first, again) = duplicate_that_fails();

        run_download(&inner, (9101, Some(first)));

        for id in [first, again] {
            assert!(
                matches!(inner.state.item_load_state(id), Some(LoadState::Failed(_))),
                "every waiter hears the one answer"
            );
        }
        let sent: Vec<_> = cmd_rx.try_iter().collect();
        assert!(
            matches!(sent.as_slice(), [PlayerCommand::TrackFailed(id)] if *id == again),
            "the cursor's entry is told it failed, not that it is ready: {sent:?}"
        );
        assert!(inner.queue.lock().in_flight.is_empty());
    }

    #[test]
    fn a_transfer_whose_first_entry_was_removed_still_answers_the_rest() {
        let (inner, _cmd_rx, first, again) = duplicate_that_fails();
        inner.state.remove_item(first);
        sync(&inner);

        run_download(&inner, (9101, Some(first)));

        assert!(matches!(
            inner.state.item_load_state(again),
            Some(LoadState::Failed(_))
        ));
    }
}
