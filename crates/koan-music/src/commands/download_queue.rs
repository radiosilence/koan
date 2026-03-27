use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use koan_core::config;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{LoadState, QueueItemId, SharedPlayerState};

use super::enqueue::download_track;

/// Persistent download queue — lives for the app's lifetime.
///
/// Items are submitted via `enqueue()`, downloaded by a pool of worker threads.
/// Cursor changes trigger priority reordering so the current track downloads
/// first, followed by same-album tracks for gapless playback.
#[derive(Clone)]
pub struct DownloadQueue {
    inner: Arc<Inner>,
}

struct Inner {
    work: Mutex<VecDeque<(i64, QueueItemId)>>,
    has_work: Condvar,
    state: Arc<SharedPlayerState>,
    cmd_tx: crossbeam_channel::Sender<PlayerCommand>,
    log_buf: Arc<Mutex<Vec<String>>>,
}

impl DownloadQueue {
    /// Spawn the download queue with persistent worker threads.
    pub fn spawn(
        cmd_tx: crossbeam_channel::Sender<PlayerCommand>,
        state: Arc<SharedPlayerState>,
        log_buf: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        let cfg = config::Config::load().unwrap_or_default();
        let num_workers = cfg.remote.download_workers.max(1);

        let inner = Arc::new(Inner {
            work: Mutex::new(VecDeque::new()),
            has_work: Condvar::new(),
            state,
            cmd_tx,
            log_buf,
        });

        // Spawn worker threads that drain the queue.
        for i in 0..num_workers {
            let inner = inner.clone();
            std::thread::Builder::new()
                .name(format!("koan-dl-{}", i))
                .spawn(move || worker_loop(inner))
                .ok();
        }

        // Spawn watcher thread: monitors cursor changes, reprioritizes.
        let watcher_inner = inner.clone();
        std::thread::Builder::new()
            .name("koan-dl-watch".into())
            .spawn(move || cursor_watcher(watcher_inner))
            .ok();

        Self { inner }
    }

    /// Add items to the download queue.
    pub fn enqueue(&self, items: Vec<(i64, QueueItemId)>) {
        if items.is_empty() {
            return;
        }
        let mut q = self.inner.work.lock().unwrap();
        q.extend(items);
        // Wake all workers — there's new work.
        self.inner.has_work.notify_all();
    }

    /// Submit a single item for priority download (e.g. user clicked a Pending track).
    /// Also enqueues same-album pending tracks for gapless playback.
    pub fn prioritize(&self, db_id: i64, queue_id: QueueItemId) {
        // Yank this item from the queue if it's already there (avoid duplicate download).
        {
            let mut q = self.inner.work.lock().unwrap();
            q.retain(|(_, qid)| *qid != queue_id);
        }

        // Spawn a dedicated priority download thread for immediate start.
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("koan-dl-prio".into())
            .spawn(move || {
                let cfg = config::Config::load().unwrap_or_default();
                download_track(
                    db_id,
                    queue_id,
                    &inner.cmd_tx,
                    &inner.log_buf,
                    &inner.state,
                    &cfg,
                );
            })
            .ok();

        // Bump same-album pending tracks to front of the queue.
        let album_mates = self.inner.state.same_album_item_ids(queue_id);
        if !album_mates.is_empty() {
            let mate_set: std::collections::HashSet<QueueItemId> =
                album_mates.into_iter().collect();
            let mut q = self.inner.work.lock().unwrap();
            let mut front = VecDeque::new();
            let mut rest = VecDeque::new();
            for item in q.drain(..) {
                if mate_set.contains(&item.1) {
                    front.push_back(item);
                } else {
                    rest.push_back(item);
                }
            }
            front.extend(rest);
            *q = front;
            self.inner.has_work.notify_all();
        }
    }
}

/// Worker loop: wait for work, download, repeat.
fn worker_loop(inner: Arc<Inner>) {
    let cfg = config::Config::load().unwrap_or_default();
    loop {
        let item = {
            let mut q = inner.work.lock().unwrap();
            loop {
                if let Some(item) = q.pop_front() {
                    break item;
                }
                // Wait for new work — condvar releases lock while sleeping.
                q = inner.has_work.wait(q).unwrap();
            }
        };
        let (db_id, queue_id) = item;
        download_track(
            db_id,
            queue_id,
            &inner.cmd_tx,
            &inner.log_buf,
            &inner.state,
            &cfg,
        );
    }
}

/// Cursor watcher: when the cursor moves to a pending track, yank it from the
/// work queue and spawn a priority download thread. Also bumps same-album
/// tracks to front for gapless playback.
fn cursor_watcher(inner: Arc<Inner>) {
    let mut last_cursor: Option<QueueItemId> = None;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(30));

        let current = inner.state.cursor();
        if current == last_cursor {
            continue;
        }
        last_cursor = current;

        let Some(cursor_id) = current else {
            continue;
        };

        // Only act if the cursor track is pending and in our queue.
        let is_pending = inner
            .state
            .item_load_state(cursor_id)
            .is_some_and(|s| matches!(s, LoadState::Pending));
        if !is_pending {
            continue;
        }

        // Build set of same-album QueueItemIds for gapless prioritization.
        let album_mate_ids: std::collections::HashSet<QueueItemId> = inner
            .state
            .same_album_item_ids(cursor_id)
            .into_iter()
            .collect();

        // Pull cursor track from the queue and spawn priority download.
        let mut priority_items = Vec::new();
        {
            let mut q = inner.work.lock().unwrap();
            if let Some(pos) = q.iter().position(|(_, qid)| *qid == cursor_id) {
                priority_items.push(q.remove(pos).unwrap());

                // Bump same-album tracks to front.
                if !album_mate_ids.is_empty() {
                    let mut album_items = VecDeque::new();
                    let mut other_items = VecDeque::new();
                    for item in q.drain(..) {
                        if album_mate_ids.contains(&item.1) {
                            album_items.push_back(item);
                        } else {
                            other_items.push_back(item);
                        }
                    }
                    album_items.extend(other_items);
                    *q = album_items;
                }

                // Also grab the next track for gapless lookahead.
                if q.front().is_some() {
                    priority_items.push(q.pop_front().unwrap());
                }
            }
        }

        // Fire off immediate download threads for priority items.
        for (db_id, queue_id) in priority_items {
            log::info!("priority: spawning immediate download for {:?}", queue_id);
            let inner = inner.clone();
            std::thread::Builder::new()
                .name("koan-dl-prio".into())
                .spawn(move || {
                    let cfg = config::Config::load().unwrap_or_default();
                    download_track(
                        db_id,
                        queue_id,
                        &inner.cmd_tx,
                        &inner.log_buf,
                        &inner.state,
                        &cfg,
                    );
                })
                .ok();
        }
    }
}
