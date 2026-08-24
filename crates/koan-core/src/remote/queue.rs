use std::collections::{HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex as StdMutex};

use parking_lot::{Condvar, Mutex};

use crate::config;
use crate::player::commands::PlayerCommand;
use crate::player::state::{LoadState, QueueItemId, SharedPlayerState};
use crate::remote::client::SubsonicClient;

use crate::helpers::download_track;

/// Concurrent downloads the priority lane may run outside the worker pool.
/// Small on purpose: its job is to get the track under the cursor playing, and
/// every extra request competes with it for the same link.
const PRIORITY_PERMITS: usize = 2;

/// How often the cursor is sampled for priority reordering.
const CURSOR_POLL: std::time::Duration = std::time::Duration::from_millis(30);

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
    cfg: config::Config,
    /// `None` when remote is not configured — nothing is downloadable.
    client: Option<Arc<SubsonicClient>>,
}

/// Queue state and the in-flight bookkeeping that keeps a track from being
/// downloaded by two threads at once.
#[derive(Default)]
struct Queue {
    pending: VecDeque<(i64, QueueItemId)>,
    in_flight: HashSet<QueueItemId>,
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
    q.pending.retain(|(_, qid)| *qid != item.1);

    if q.in_flight.contains(&item.1) {
        return Dispatch::AlreadyRunning;
    }
    if q.priority_active >= PRIORITY_PERMITS {
        q.pending.push_front(item);
        return Dispatch::Requeued;
    }
    q.priority_active += 1;
    q.in_flight.insert(item.1);
    Dispatch::Spawn
}

fn release_priority(q: &mut Queue, id: QueueItemId) {
    q.in_flight.remove(&id);
    q.priority_active = q.priority_active.saturating_sub(1);
}

/// Releases an in-flight claim however the download ends — including a panic.
struct Claim {
    inner: Arc<Inner>,
    id: QueueItemId,
    priority: bool,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut q = self.inner.queue.lock();
        if self.priority {
            release_priority(&mut q, self.id);
        } else {
            q.in_flight.remove(&self.id);
        }
    }
}

impl DownloadQueue {
    /// Spawn the download queue with persistent worker threads.
    pub fn spawn(
        cmd_tx: crossbeam_channel::Sender<PlayerCommand>,
        state: Arc<SharedPlayerState>,
        log_buf: Arc<StdMutex<Vec<String>>>,
    ) -> Self {
        let cfg = config::Config::load().unwrap_or_default();
        let num_workers = cfg.remote.download_workers.max(1);
        let client = crate::helpers::subsonic_client(&cfg);
        if client.is_none() {
            log::info!("remote not configured — download queue will idle");
        }

        let inner = Arc::new(Inner {
            queue: Mutex::new(Queue::default()),
            has_work: Condvar::new(),
            state,
            cmd_tx,
            log_buf,
            cfg,
            client,
        });

        for i in 0..num_workers {
            let inner = inner.clone();
            if let Err(e) = std::thread::Builder::new()
                .name(format!("koan-dl-{}", i))
                .spawn(move || worker_loop(inner))
            {
                log::error!("failed to spawn download worker {}: {}", i, e);
            }
        }

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
        self.inner.queue.lock().pending.extend(items);
        self.inner.has_work.notify_all();
    }

    /// Submit a single item for priority download (e.g. user clicked a Pending
    /// track). Also bumps same-album pending tracks for gapless playback.
    pub fn prioritize(&self, db_id: i64, queue_id: QueueItemId) {
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
                        id: item.1,
                        priority: true,
                    };
                    run_download(&spawn_inner, item);
                });
            if let Err(e) = spawned {
                log::error!("failed to spawn priority download: {}", e);
                let mut q = inner.queue.lock();
                release_priority(&mut q, item.1);
                q.pending.push_front(item);
                drop(q);
                inner.has_work.notify_one();
            }
        }
    }
}

/// Run one download, containing any panic so the worker pool never shrinks.
fn run_download(inner: &Arc<Inner>, (db_id, queue_id): (i64, QueueItemId)) {
    let Some(client) = inner.client.as_ref() else {
        // Failed, not left Pending: the player waits for Ready, so a queue of
        // tracks that can never arrive would otherwise sit saying nothing.
        inner.state.update_load_state(
            queue_id,
            LoadState::Failed(crate::helpers::remote_unavailable(&inner.cfg)),
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
            &inner.cfg,
            client,
        );
    }));

    if outcome.is_err() {
        log::error!("download panicked for {:?}", queue_id);
        inner
            .state
            .update_load_state(queue_id, LoadState::Failed("download panicked".into()));
    }
}

/// Worker loop: wait for work, download, repeat.
fn worker_loop(inner: Arc<Inner>) {
    loop {
        let item = {
            let mut q = inner.queue.lock();
            loop {
                match q.pending.pop_front() {
                    Some(item) => {
                        // A duplicate entry for a track already downloading is dropped.
                        if q.in_flight.insert(item.1) {
                            break item;
                        }
                    }
                    None => inner.has_work.wait(&mut q),
                }
            }
        };
        let _claim = Claim {
            inner: inner.clone(),
            id: item.1,
            priority: false,
        };
        run_download(&inner, item);
    }
}

/// Cursor watcher: when the cursor moves to a pending track, hand it and the
/// next track to the priority lane and bump same-album tracks to the front.
fn cursor_watcher(inner: Arc<Inner>) {
    let mut last_cursor: Option<QueueItemId> = None;
    loop {
        std::thread::sleep(CURSOR_POLL);

        let current = inner.state.cursor();
        if current == last_cursor {
            continue;
        }
        last_cursor = current;

        let Some(cursor_id) = current else {
            continue;
        };

        let is_pending = inner
            .state
            .item_load_state(cursor_id)
            .is_some_and(|s| matches!(s, LoadState::Pending));
        if !is_pending {
            continue;
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
            dispatch_priority(&inner, item);
        }
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
        let a = qid();
        assert_eq!(claim_priority(&mut q, (1, a)), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, (2, qid())), Dispatch::Spawn);
        assert_eq!(claim_priority(&mut q, (3, qid())), Dispatch::Requeued);

        release_priority(&mut q, a);
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
}
