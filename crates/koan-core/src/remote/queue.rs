use std::collections::{HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use crate::config;
use crate::db::queries;
use crate::helpers::download_track;
use crate::player::commands::PlayerCommand;
use crate::player::state::{ItemState, QueueItemId, SharedPlayerState};
use crate::remote::downloads::{self, DownloadStore};

/// Concurrent downloads the priority lane may run outside the worker pool.
/// Small on purpose: its job is to get the track under the cursor playing, and
/// every extra request competes with it for the same link.
const PRIORITY_PERMITS: usize = 2;

/// How often a running transfer asks whether it is still wanted.
const CANCEL_CHECK: Duration = Duration::from_millis(250);

/// Persistent download queue — lives for the app's lifetime.
///
/// Follows the playlist rather than being told about it: whenever the set of
/// entries waiting for a file changes, each is queued or joins the transfer
/// already running for its track, and anything no longer in the playlist is
/// let go. A front end adds tracks to the player and nothing else, so there is
/// no second request to race the first and no queue of entries the player has
/// already discarded.
///
/// What is in flight, and who waits on it, is the player's download store.
/// This decides only what is fetched when.
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
    /// Held across every change to which entries wait on which transfer —
    /// joining, letting go, settling — so none can land between another's
    /// look and its act. May be held while the player's state or its download
    /// store is locked; neither is ever held while taking this.
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
                    trim_cache(&worker, false);
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
fn next_item(
    q: &mut Queue,
    store: &DownloadStore,
    cursor: Option<(i64, QueueItemId)>,
) -> Option<Job> {
    let entry = match cursor {
        Some((db_id, _)) if store.in_flight(db_id) => return None,
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
    if item.state != ItemState::Pending {
        return None;
    }
    Some((item.db_id?, id))
}

/// A track to fetch, and the queue entry it is for — `None` for a track
/// wanted only in the cache.
type Job = (i64, Option<QueueItemId>);

/// What is to be fetched, in the order it will be. What is being fetched is
/// the download store's.
#[derive(Default)]
struct Queue {
    /// Queue entries whose files are still to be fetched.
    pending: VecDeque<(i64, QueueItemId)>,
    /// Tracks wanted in the cache with no queue entry behind them.
    cache: VecDeque<i64>,
    priority_active: usize,
    /// The tracks in the playback window as last worked out, which eviction
    /// leaves alone. `None` when the whole playlist is wanted: no cache limit,
    /// or a window that could not be worked out.
    window: Option<HashSet<i64>>,
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
fn claim_priority(q: &mut Queue, store: &DownloadStore, item: (i64, QueueItemId)) -> Dispatch {
    let (db_id, queue_id) = item;
    q.pending.retain(|(_, qid)| *qid != queue_id);

    // Already being fetched: wait on it rather than fetch it again. The entry
    // is remembered so it gets the answer when the one transfer lands.
    if store.join(db_id, Some(queue_id)) {
        return Dispatch::AlreadyRunning;
    }
    if q.priority_active >= PRIORITY_PERMITS {
        q.pending.push_front(item);
        return Dispatch::Requeued;
    }
    q.priority_active += 1;
    store.claim(db_id, Some(queue_id));
    Dispatch::Spawn
}

/// Hands back a priority permit however the download ends — including a panic.
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
/// Every entry still waiting for its file is either waiting on the transfer
/// for its track or queued, in the order the player will reach it: from the
/// cursor to the end, then from the top. Anything the playlist no longer
/// holds is let go.
///
/// With a cache limit, only the entries in the playback window are wanted.
/// One past it is let go like one removed, and a later sync brings it in when
/// the cursor or the cache makes room. One inside it is wanted on every sync
/// that finds it there, so its transfer is never abandoned by a window
/// recomputed around it.
///
/// The playlist is read under the queue lock, as settling writes it: read
/// before, it could show an entry still pending whose transfer settled a
/// moment later, and the entry would be fetched a second time. The window is
/// worked out before, from the database; an entry added in between is
/// outside it, and the sync its arrival causes brings it in.
fn sync(inner: &Arc<Inner>) {
    let window = playback_window(&inner.state);
    let mut q = inner.queue.lock();
    let wanted = inner.state.pending_downloads();
    let entries = window
        .as_ref()
        .map(|w| w.iter().map(|(_, id)| *id).collect::<HashSet<_>>());
    let added = sync_with(&mut q, inner.state.downloads(), &wanted, entries.as_ref());
    q.window = window.map(|w| w.into_iter().map(|(track, _)| track).collect());
    drop(q);
    if added {
        wake_workers(inner);
    }
}

/// The entries the cache has room for, from the cursor on
/// (`helpers::playback_window`). `None` with no cache limit, and when the
/// database cannot say: then the whole playlist is fetched, as without one.
fn playback_window(state: &SharedPlayerState) -> Option<Vec<(i64, QueueItemId)>> {
    let limit = config::Config::cached().cache_limit_bytes()?;
    let mut order = state.playback_order();
    let tracks: Vec<i64> = order.iter().map(|(track, _)| *track).collect();
    let fits = crate::db::pool::shared()
        .get()
        .map_err(|e| e.to_string())
        .and_then(|db| {
            crate::helpers::playback_window(&db, limit, &tracks).map_err(|e| e.to_string())
        });
    match fits {
        Ok(fits) => {
            order.truncate(fits);
            Some(order)
        }
        Err(e) => {
            log::warn!("cache window: {e}");
            None
        }
    }
}

/// [`sync`] against a list already read, in the order to fetch it, cut to
/// `window` when there is one. Whether anything was queued that was not
/// before.
fn sync_with(
    q: &mut Queue,
    store: &DownloadStore,
    wanted: &[(i64, QueueItemId)],
    window: Option<&HashSet<QueueItemId>>,
) -> bool {
    let wanted: Vec<_> = match window {
        Some(window) => wanted
            .iter()
            .filter(|(_, id)| window.contains(id))
            .copied()
            .collect(),
        None => wanted.to_vec(),
    };
    let unfetched = store.resync(&wanted);
    let before: HashSet<QueueItemId> = q.pending.iter().map(|(_, id)| *id).collect();
    let added = unfetched.iter().any(|(_, id)| !before.contains(id));
    q.pending = unfetched.into();
    added
}

/// Start a priority download, or queue it at the front when the lane is full.
fn dispatch_priority(inner: &Arc<Inner>, item: (i64, QueueItemId)) {
    let dispatch = claim_priority(&mut inner.queue.lock(), inner.state.downloads(), item);
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
                    run_download(&spawn_inner, item.0);
                });
            if let Err(e) = spawned {
                log::error!("failed to spawn priority download: {}", e);
                let mut q = inner.queue.lock();
                let store = inner.state.downloads();
                for id in downloads::withdraw(store, item.0) {
                    q.pending.push_front((item.0, id));
                }
                q.priority_active = q.priority_active.saturating_sub(1);
                drop(q);
                inner.has_work.notify_one();
            }
        }
    }
}

/// Run one download, containing any panic so the worker pool never shrinks,
/// and settle every entry waiting on it. The transfer was claimed in the
/// player's store before this was called.
///
/// The client is looked up per download, not once for the queue's lifetime:
/// the queue lives as long as the process, and signing in, out or elsewhere
/// has to reach it.
fn run_download(inner: &Arc<Inner>, db_id: i64) {
    let store = inner.state.downloads();
    let cfg = config::Config::cached();
    let result = match crate::helpers::subsonic_client(&cfg) {
        // Failed, not left Pending: the player waits for Ready, so a queue of
        // tracks that can never arrive would otherwise sit saying nothing.
        None => Some(Err(crate::helpers::remote_unavailable(&cfg))),
        Some(client) => {
            // Asked per chunk, answered from the store at most every
            // `CANCEL_CHECK`. Replacing the queue is one command and one
            // playlist change (`SharedPlayerState::replace_playlist`), so the
            // playlist is never momentarily without a track still wanted.
            let checked = std::cell::Cell::new(std::time::Instant::now());
            let cancelled = || {
                if checked.get().elapsed() < CANCEL_CHECK {
                    return false;
                }
                checked.set(std::time::Instant::now());
                store.abandoned(db_id)
            };
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                download_track(
                    db_id,
                    &cancelled,
                    &inner.cmd_tx,
                    &inner.state,
                    &cfg,
                    &client,
                )
            }))
            .unwrap_or_else(|_| {
                log::error!("download panicked for track {db_id}");
                Some(Err("download panicked".into()))
            })
        }
    };

    // Under the queue lock, so no entry can join the transfer between being
    // told and the transfer ending: one arriving after this finds no
    // transfer and is queued afresh.
    if let Some(Ok(_)) = &result
        && store.kept(db_id)
    {
        pin(db_id);
    }
    let mut q = inner.queue.lock();
    let settled = match result {
        Some(result) => Some(downloads::settle(&inner.state, db_id, &result)),
        // Withdrawn because nothing wanted it, as far as it last looked. An
        // entry that has asked since goes back to the front.
        None => {
            for id in downloads::withdraw(store, db_id) {
                q.pending.push_front((db_id, id));
            }
            None
        }
    };
    drop(q);
    if let Some(settled) = settled {
        settled.announce(&inner.cmd_tx);
    }
    // Workers held back for the track under the cursor go on from here.
    inner.has_work.notify_all();
    // The file's real size replaces its estimate, which may move the window.
    if config::Config::cached().cache_limit_bytes().is_some() {
        sync(inner);
    }
    trim_cache(inner, false);
}

/// A track downloaded because someone asked for the file: eviction takes it
/// only once everything fetched for playback has gone.
fn pin(db_id: i64) {
    let pinned = crate::db::pool::shared()
        .get()
        .map_err(|e| e.to_string())
        .and_then(|db| queries::pin_cached(&db.conn, &[db_id]).map_err(|e| e.to_string()));
    if let Err(e) = pinned {
        log::warn!("could not pin the download of track {db_id}: {e}");
    }
}

/// How often the cache is checked against its limit, at most: each download
/// adds to it, and a check reads the whole cache's size from the database.
const EVICT_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Trim the cache to its configured limit, keeping the playback window: the
/// player may be reading those files, and they are what is wanted next.
/// Without a window, everything in the playlist is kept. `now` skips the
/// throttle, for a limit just changed. The limit is read afresh, so one set
/// in Settings applies without a restart.
fn trim_cache(inner: &Inner, now: bool) {
    {
        let mut last = inner.last_evicted.lock();
        if !now && last.is_some_and(|t| t.elapsed() < EVICT_EVERY) {
            return;
        }
        *last = Some(std::time::Instant::now());
    }
    let cfg = config::Config::cached();
    if cfg.cache_limit_bytes().is_none() {
        return;
    }
    let keep = inner.queue.lock().window.clone().unwrap_or_else(|| {
        inner
            .state
            .playback_order()
            .into_iter()
            .map(|(track, _)| track)
            .collect()
    });
    match crate::db::pool::shared().get() {
        Ok(db) => {
            if crate::helpers::evict_cache(&db, &cfg, &keep, false) > 0 {
                // Played entries pointed at the files just removed. Pending
                // again, they are fetched when the window comes back to them.
                inner.state.reset_items_with_missing_files();
            }
        }
        Err(e) => log::warn!("cache eviction: could not open the database: {e}"),
    }
}

/// Moved whenever the cache limit is changed.
static LIMIT_CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The cache limit was changed: work the window out again and evict down to
/// the new limit now, rather than at the next download.
pub fn cache_limit_changed() {
    LIMIT_CHANGES.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    crate::signal::engine_changed().bump();
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
            let store = inner.state.downloads();
            match next_item(&mut q, store, cursor) {
                // Already being fetched: the entry waits on that transfer
                // rather than starting a second over it.
                Some((db_id, entry)) => {
                    if store.claim(db_id, entry) {
                        break db_id;
                    }
                }
                None => inner.has_work.wait(&mut q),
            }
        };
        run_download(&inner, item);
    }
}

/// Keep the queue following the player: when the set of entries waiting for a
/// file may have changed, bring the queue in line with it; when the cursor moves to a pending track, hand it
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
    let mut last_limit = LIMIT_CHANGES.load(std::sync::atomic::Ordering::Acquire);
    loop {
        // The queue runs from the cursor, so a cursor move reorders it as
        // well. Synced before promoting, so the cursor's track is queued by
        // the time it is looked for.
        let version = inner.state.pending_version();
        let current = inner.state.cursor();
        let limit = LIMIT_CHANGES.load(std::sync::atomic::Ordering::Acquire);
        if last_version != Some(version) || current != last_cursor || limit != last_limit {
            last_version = Some(version);
            sync(&inner);
        }
        if limit != last_limit {
            last_limit = limit;
            trim_cache(&inner, true);
        }
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
/// the cursor's track is waiting in the queue. The queue is already in the
/// order the player will reach it, so the one after is its front.
fn promote_cursor(inner: &Arc<Inner>, cursor_id: QueueItemId) {
    if inner.state.item_state(cursor_id) != Some(ItemState::Pending) {
        return;
    }
    let mut priority_items = Vec::new();
    {
        let mut q = inner.queue.lock();
        if let Some(pos) = q.pending.iter().position(|(_, qid)| *qid == cursor_id) {
            priority_items.push(q.pending.remove(pos).expect("position just found"));
            // The next track too, for gapless lookahead.
            if let Some(next) = q.pending.pop_front() {
                priority_items.push(next);
            }
        }
    }
    for item in priority_items {
        dispatch_priority(inner, item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::state::PlaylistItem;

    fn qid() -> QueueItemId {
        QueueItemId::new()
    }

    fn waiters(store: &DownloadStore, db_id: i64) -> HashSet<QueueItemId> {
        store.waiters(db_id).into_iter().collect()
    }

    #[test]
    fn priority_lane_never_exceeds_its_permits() {
        let (mut q, store) = (Queue::default(), DownloadStore::new());

        // Rapid cursor movement: a fresh track lands on the lane every poll.
        let mut spawned = 0;
        for i in 0..500 {
            if claim_priority(&mut q, &store, (i, qid())) == Dispatch::Spawn {
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
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        assert_eq!(claim_priority(&mut q, &store, (1, qid())), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, &store, (2, qid())), Dispatch::Spawn);
        assert_eq!(
            claim_priority(&mut q, &store, (3, qid())),
            Dispatch::Requeued
        );

        let _ = downloads::withdraw(&store, 1);
        q.priority_active -= 1;
        assert_eq!(claim_priority(&mut q, &store, (4, qid())), Dispatch::Spawn);
        assert!(q.priority_active <= PRIORITY_PERMITS);
    }

    #[test]
    fn an_in_flight_track_is_never_claimed_twice() {
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        let id = qid();
        assert_eq!(claim_priority(&mut q, &store, (1, id)), Dispatch::Spawn);
        assert_eq!(
            claim_priority(&mut q, &store, (1, id)),
            Dispatch::AlreadyRunning
        );
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
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        let (first, again) = (qid(), qid());
        assert_eq!(claim_priority(&mut q, &store, (7, first)), Dispatch::Spawn);
        assert_eq!(
            claim_priority(&mut q, &store, (7, again)),
            Dispatch::AlreadyRunning
        );

        assert_eq!(q.priority_active, 1, "one transfer, not two");
        assert!(q.pending.is_empty());
        assert_eq!(
            waiters(&store, 7),
            HashSet::from([first, again]),
            "both entries wait on the one transfer"
        );
    }

    #[test]
    fn different_tracks_still_run_side_by_side() {
        // Keying on the track must not serialise unrelated downloads.
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        assert_eq!(claim_priority(&mut q, &store, (1, qid())), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, &store, (2, qid())), Dispatch::Spawn);
        assert_eq!(q.priority_active, 2);
    }

    #[test]
    fn requeued_priority_item_goes_to_the_head_of_the_queue() {
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        q.pending.push_back((9, qid()));
        for i in 0..PRIORITY_PERMITS {
            claim_priority(&mut q, &store, (i as i64, qid()));
        }

        let wanted = qid();
        assert_eq!(
            claim_priority(&mut q, &store, (7, wanted)),
            Dispatch::Requeued
        );
        assert_eq!(q.pending.front().map(|(_, id)| *id), Some(wanted));
    }

    #[test]
    fn claiming_removes_a_duplicate_queue_entry() {
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        let id = qid();
        q.pending.push_back((1, id));
        q.pending.push_back((2, qid()));

        assert_eq!(claim_priority(&mut q, &store, (1, id)), Dispatch::Spawn);
        assert_eq!(
            q.pending.len(),
            1,
            "the pool must not also pick up the claimed track"
        );
    }

    #[test]
    fn the_track_under_the_cursor_goes_first_and_goes_alone() {
        let (a, b, c) = (qid(), qid(), qid());
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        q.pending.extend([(1, a), (2, b), (3, c)]);

        // Pressed play on the third: it jumps the queue.
        assert_eq!(next_item(&mut q, &store, Some((3, c))), Some((3, Some(c))));
        store.claim(3, Some(c));

        // While it downloads, nothing else starts.
        assert_eq!(next_item(&mut q, &store, Some((3, c))), None);
        assert_eq!(q.pending.len(), 2, "the rest wait their turn");

        // Once it has landed the queue runs in order again.
        let _ = downloads::withdraw(&store, 3);
        assert_eq!(next_item(&mut q, &store, None), Some((1, Some(a))));
        assert_eq!(next_item(&mut q, &store, None), Some((2, Some(b))));
    }

    #[test]
    fn a_cursor_with_nothing_queued_for_it_holds_nothing_up() {
        let (a, elsewhere) = (qid(), qid());
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        q.pending.push_back((1, a));
        assert_eq!(
            next_item(&mut q, &store, Some((9, elsewhere))),
            Some((1, Some(a)))
        );
    }

    #[test]
    fn a_track_wanted_only_in_the_cache_waits_behind_the_playlist() {
        let a = qid();
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        q.cache.push_back(5);
        q.pending.push_back((1, a));
        assert_eq!(next_item(&mut q, &store, None), Some((1, Some(a))));
        assert_eq!(next_item(&mut q, &store, None), Some((5, None)));
    }

    #[test]
    fn the_queue_lets_go_of_what_the_playlist_no_longer_holds() {
        // Replacing a large queue used to leave every old entry queued, and
        // each cost a worker a wait before it gave up.
        let (old, kept, waiter) = (qid(), qid(), qid());
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        q.pending.extend([(1, old), (2, kept)]);
        store.claim(3, Some(waiter));

        sync_with(&mut q, &store, &[(2, kept)], None);

        assert_eq!(q.pending, VecDeque::from([(2, kept)]));
        assert!(waiters(&store, 3).is_empty());
        assert!(
            store.abandoned(3),
            "a transfer nothing waits on any more is on its way to being stopped"
        );
    }

    #[test]
    fn the_queue_is_in_the_order_it_is_given() {
        // The playlist's order from the cursor on, which `pending_downloads`
        // gives: a queue that kept its old order fetched the tracks before a
        // cursor that had jumped ahead first.
        let (a, b, c) = (qid(), qid(), qid());
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        q.pending.extend([(1, a), (2, b), (3, c)]);

        assert!(
            !sync_with(&mut q, &store, &[(3, c), (1, a), (2, b)], None),
            "nothing new"
        );
        assert_eq!(q.pending, VecDeque::from([(3, c), (1, a), (2, b)]));

        let d = qid();
        assert!(sync_with(
            &mut q,
            &store,
            &[(3, c), (4, d), (1, a), (2, b)],
            None
        ));
        assert_eq!(q.pending, VecDeque::from([(3, c), (4, d), (1, a), (2, b)]));
    }

    #[test]
    fn playing_from_the_middle_fetches_from_there_first() {
        // Pressed play on track three of five: three, four and five come
        // before one and two.
        let (inner, ids, _) = queue_over(&[1, 2, 3, 4, 5]);
        inner.state.set_cursor(Some(ids[2].1));
        let wanted = inner.state.pending_downloads();
        sync_with(
            &mut inner.queue.lock(),
            inner.state.downloads(),
            &wanted,
            None,
        );

        let order: Vec<i64> = inner.queue.lock().pending.iter().map(|(t, _)| *t).collect();
        assert_eq!(order, vec![3, 4, 5, 1, 2]);
    }

    #[test]
    fn a_new_entry_for_a_track_in_flight_waits_on_that_transfer() {
        let (running, again) = (qid(), qid());
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        store.claim(7, Some(running));

        assert!(!sync_with(
            &mut q,
            &store,
            &[(7, running), (7, again)],
            None
        ));

        assert!(q.pending.is_empty());
        assert_eq!(waiters(&store, 7), HashSet::from([running, again]));
    }

    #[test]
    fn a_queue_replaced_with_the_same_track_keeps_its_transfer() {
        // A new queue holding the same track is new entries for it; the one
        // transfer serves them and is never wanted by nothing in between.
        let (old, new) = (qid(), qid());
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        store.claim(7, Some(old));

        sync_with(&mut q, &store, &[(7, new)], None);

        assert_eq!(waiters(&store, 7), HashSet::from([new]));
        assert!(!store.abandoned(7));
    }

    #[test]
    fn entries_past_the_window_wait_until_it_reaches_them() {
        let (played, next, beyond) = (qid(), qid(), qid());
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        let wanted = [(2, next), (3, beyond), (1, played)];

        sync_with(&mut q, &store, &wanted, Some(&HashSet::from([next])));
        assert_eq!(q.pending, VecDeque::from([(2, next)]));

        // The cursor moves on and the window with it.
        sync_with(&mut q, &store, &wanted, Some(&HashSet::from([beyond])));
        assert_eq!(q.pending, VecDeque::from([(3, beyond)]));
    }

    #[test]
    fn a_track_inside_the_window_keeps_its_transfer_when_the_window_is_recomputed() {
        let (playing, next, beyond) = (qid(), qid(), qid());
        let (mut q, store) = (Queue::default(), DownloadStore::new());
        store.claim(2, Some(next));
        store.claim(3, Some(beyond));
        let wanted = [(1, playing), (2, next), (3, beyond)];

        // The download of the next track lands, the window is worked out
        // again from the cache's new size, and comes out smaller.
        sync_with(
            &mut q,
            &store,
            &wanted,
            Some(&HashSet::from([playing, next])),
        );

        assert!(!store.abandoned(2), "inside the window: still wanted");
        assert_eq!(waiters(&store, 2), HashSet::from([next]));
        assert!(store.abandoned(3), "past the window: let go");
        assert_eq!(q.pending, VecDeque::from([(1, playing)]));
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
        let item = |db_id: i64| PlaylistItem {
            playlist_entry_id: None,
            id: qid(),
            db_id: Some(db_id),
            path: std::path::PathBuf::from(format!("/cache/track-{db_id}.flac")),
            title: format!("track-{db_id}"),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            state: ItemState::Pending,
        };
        let items: Vec<_> = tracks.iter().map(|&db_id| item(db_id)).collect();
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

    /// The first play after launch: the player has the new queue and its
    /// cursor, and the download queue holds its tracks.
    fn first_play() -> (Arc<Inner>, Vec<(i64, QueueItemId)>) {
        let (inner, ids, _) = queue_over(&[1, 2, 3]);
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

    fn failed(inner: &Inner, id: QueueItemId) -> bool {
        matches!(inner.state.item_state(id), Some(ItemState::Failed(_)))
    }

    /// The watcher starts after the player already has its queue and cursor,
    /// with nothing further to wake it: it finds the tracks by itself and
    /// fetches every one. With no server, each fails rather than waiting.
    #[test]
    fn a_watcher_started_over_a_playlist_fetches_it_unprompted() {
        let (inner, ids, _) = queue_over(&[1, 2, 3]);
        let watched = inner.clone();
        std::thread::spawn(move || follow_playlist(watched));

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ids.iter().all(|(_, id)| failed(&inner, *id)) && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ids.iter().all(|(_, id)| failed(&inner, *id)));
    }

    /// A track queued twice, both entries waiting on its one transfer, the
    /// second under the cursor. The transfer cannot happen: no server.
    fn duplicate_that_fails() -> (
        Arc<Inner>,
        crossbeam_channel::Receiver<PlayerCommand>,
        QueueItemId,
        QueueItemId,
    ) {
        let (inner, ids, cmd_rx) = queue_over(&[1, 1]);
        let (first, again) = (ids[0].1, ids[1].1);
        inner.state.set_cursor(Some(again));
        let store = inner.state.downloads();
        store.claim(1, Some(first));
        store.join(1, Some(again));
        (inner, cmd_rx, first, again)
    }

    #[test]
    fn a_failed_transfer_fails_every_entry_waiting_on_it() {
        let (inner, cmd_rx, first, again) = duplicate_that_fails();

        run_download(&inner, 1);

        for id in [first, again] {
            assert!(failed(&inner, id), "every waiter hears the one answer");
        }
        let sent: Vec<_> = cmd_rx.try_iter().collect();
        assert!(
            matches!(sent.as_slice(), [PlayerCommand::TrackFailed(id)] if *id == again),
            "the cursor's entry is told it failed, not that it is ready: {sent:?}"
        );
        assert!(!inner.state.downloads().in_flight(1));
    }

    #[test]
    fn a_transfer_whose_first_entry_was_removed_still_answers_the_rest() {
        let (inner, _cmd_rx, first, again) = duplicate_that_fails();
        inner.state.remove_item(first);
        sync(&inner);

        run_download(&inner, 1);

        assert!(failed(&inner, again));
    }

    #[test]
    fn a_transfer_withdrawn_with_an_entry_waiting_queues_it_again() {
        // The entry joined after the download decided nothing wanted it.
        let (inner, ids, _) = queue_over(&[1]);
        let store = inner.state.downloads();
        store.claim(1, Some(ids[0].1));

        let mut q = inner.queue.lock();
        for id in downloads::withdraw(store, 1) {
            q.pending.push_front((1, id));
        }
        assert_eq!(q.pending, VecDeque::from([ids[0]]));
    }
}
