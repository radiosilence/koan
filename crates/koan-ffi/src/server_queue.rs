//! This device's queue kept in the account's play queue on the server, where
//! Subsonic clients save theirs (`remote.play_queue`, off by default).
//!
//! While on, queue edits are saved a moment after they settle, and a change of
//! track or a pause is saved at once with the playhead; the app saves again
//! when it goes to the background. Nothing is polled: the saver sleeps on the
//! engine's change signal. At launch the server's queue replaces this one only
//! if another client saved it since this device did, which `changedBy` says:
//! each device saves under a client name of its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use koan_core::config::Config;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::PlaybackState;
use koan_core::remote::client::{SavedPlayQueue, SubsonicClient};

use crate::{KoanEngine, KoanError};

/// How long queue edits must settle before they are saved: a burst of edits
/// makes one request.
const SETTLE: Duration = Duration::from_secs(1);

/// Past this many tracks a queue is not sent: the server takes a form only as
/// long as a URI can be (#877), and a request it would refuse is not worth
/// making on every edit.
pub(crate) const MAX_SENT: usize = 1500;

/// How long a load or a restore may take to reach the player before the
/// saver looks anyway.
pub(crate) const LANDED: Duration = Duration::from_secs(5);

/// The saver running now, told to stop through its flag.
static RUNNING: parking_lot::Mutex<Option<Arc<AtomicBool>>> = parking_lot::Mutex::new(None);

/// The name this device saves under, which `changedBy` reports back.
fn client_name() -> String {
    match koan_core::remote::devices::this_id() {
        Some(id) => format!("koan {id}"),
        None => "koan".to_owned(),
    }
}

fn client() -> Option<Arc<SubsonicClient>> {
    koan_core::helpers::subsonic_client(&Config::load().unwrap_or_default())
}

fn offers(extension: &str) -> bool {
    koan_core::remote::profile::current().is_some_and(|p| p.offers(extension))
}

/// Whether the server takes the queue by index, which a track queued twice
/// needs.
fn by_index() -> bool {
    offers("indexBasedQueue")
}

fn remote(e: impl std::fmt::Display) -> KoanError {
    KoanError::Remote {
        message: e.to_string(),
    }
}

/// What the account has saved on the server, if anything.
pub(crate) fn saved() -> Result<Option<SavedPlayQueue>, KoanError> {
    let client = client().ok_or_else(|| remote("not signed in to a server"))?;
    client
        .get_play_queue(by_index())
        .map(|q| q.filter(|q| !q.entry.is_empty()))
        .map_err(remote)
}

impl KoanEngine {
    /// Replace this device's queue with `queue`, paused where it was, and
    /// return once the player has it, so whatever looks next sees the loaded
    /// queue and does not take it for an edit to save back. With `unless`,
    /// the queue as it stood when the load was decided: if the person has
    /// changed it or started something since, the load is skipped.
    pub(crate) fn load_server_queue(
        &self,
        queue: &SavedPlayQueue,
        unless: Option<&Seen>,
    ) -> Result<(), KoanError> {
        let db = self.db()?;
        let ids: Vec<String> = queue.entry.iter().map(|e| e.id.clone()).collect();
        // Fetches anything the library has not synced yet, then names each
        // entry on its own so the current place survives one that is missing.
        koan_core::remote::link::resolve_tracks(&db, &ids, true);
        let found: Vec<Option<i64>> = ids
            .iter()
            .map(|id| {
                koan_core::remote::link::resolve_tracks(&db, std::slice::from_ref(id), false)
                    .0
                    .first()
                    .copied()
            })
            .collect();
        let current = queue.current_at().unwrap_or(0);
        let start = found[..current.min(found.len())]
            .iter()
            .filter(|f| f.is_some())
            .count();
        let tracks: Vec<i64> = found.into_iter().flatten().collect();
        let items = self.build_items(&db, &tracks);
        if items.is_empty() {
            return Ok(());
        }
        if let Some(before) = unless
            && seen(self) != *before
        {
            log::info!("server queue: the queue changed here while the server's was read; kept");
            return Ok(());
        }
        log::info!(
            "server queue: loaded {} tracks saved by {}",
            items.len(),
            queue.changed_by
        );
        let content = self.state.content_version();
        self.send_local(PlayerCommand::ReplacePlaylist {
            start: start.min(items.len() - 1),
            items,
            position_ms: queue.position,
            play: false,
        })?;
        koan_core::remote::devices::await_until(
            || {
                self.state.content_version() != content
                    && self.state.playback_state() != PlaybackState::Playing
            },
            LANDED,
        );
        Ok(())
    }

    /// Save this device's queue as it stands to the server.
    pub(crate) fn save_server_queue(&self) -> Result<(), KoanError> {
        let (items, cursor) = self.state.snapshot_playlist();
        let remote_ids = self.remote_ids(&items.iter().filter_map(|i| i.db_id).collect::<Vec<_>>());
        // Tracks only this device has stay out; the current place follows.
        let mut current = None;
        let mut ids = Vec::new();
        for item in &items {
            if Some(item.id) == cursor && current.is_none() {
                current = Some(ids.len());
            }
            if let Some(id) = item.db_id.and_then(|id| remote_ids.get(&id)) {
                ids.push(id.clone());
            }
        }
        if ids.len() > MAX_SENT {
            log::warn!(
                "server queue: {} tracks is more than the server takes; not saved",
                ids.len()
            );
            return Ok(());
        }
        let current = current.map(|at| at.min(ids.len().saturating_sub(1)));
        let client = client().ok_or_else(|| remote("not signed in to a server"))?;
        client
            .save_play_queue(
                &ids,
                current,
                self.state.position_ms(),
                by_index(),
                offers("formPost"),
                &client_name(),
            )
            .map_err(remote)
    }
}

/// Start following the server's queue: at launch, `reconcile` first takes the
/// server's if another client saved it since this device did.
pub(crate) fn start(engine: Weak<KoanEngine>, reconcile: bool) {
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(old) = RUNNING.lock().replace(stop.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    let _ = std::thread::Builder::new()
        .name("koan-server-queue".into())
        .spawn(move || {
            if reconcile && let Some(engine) = engine.upgrade() {
                // The queue as restored: the server's replaces it only if
                // nobody touches it while that is read and resolved.
                let restored = seen(&engine);
                match saved() {
                    Ok(Some(queue)) if queue.changed_by != client_name() => {
                        if let Err(e) = engine.load_server_queue(&queue, Some(&restored)) {
                            log::warn!("server queue: loading failed: {e}");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => log::info!("server queue: not read at launch: {e}"),
                }
            }
            follow(engine, &stop);
        });
}

/// Stop following the server's queue. Both copies stay as they are.
pub(crate) fn stop() {
    if let Some(running) = RUNNING.lock().take() {
        running.store(true, Ordering::Relaxed);
        koan_core::signal::engine_changed().bump();
    }
}

/// What the saver watches: what the queue holds (`content_version`, which a
/// download's progress does not move), the entry under the cursor, and
/// whether it is playing.
pub(crate) type Seen = (
    u64,
    Option<koan_core::player::state::QueueItemId>,
    PlaybackState,
);

pub(crate) fn seen(engine: &KoanEngine) -> Seen {
    (
        engine.state.content_version(),
        engine.state.cursor(),
        engine.state.playback_state(),
    )
}

/// Whether to save now, given what was seen before and now, and the last
/// queue edit not yet saved; and that edit afterwards. A change of track or a
/// pause saves at once, with the playhead; an edit waits for the burst it is
/// part of to settle.
fn decide(before: &Seen, now: &Seen, pending: Option<Instant>) -> (bool, Option<Instant>) {
    let pending = if now.0 != before.0 {
        Some(Instant::now())
    } else {
        pending
    };
    let moved = now.1 != before.1;
    let paused = now.2 == PlaybackState::Paused && before.2 != PlaybackState::Paused;
    let settled = pending.is_some_and(|at| at.elapsed() >= SETTLE);
    if moved || paused || settled {
        (true, None)
    } else {
        (false, pending)
    }
}

/// Save on each settled queue edit, each change of track and each pause.
fn follow(engine: Weak<KoanEngine>, stop: &AtomicBool) {
    let signal = koan_core::signal::engine_changed();
    let Some(mut before) = engine.upgrade().map(|e| seen(&e)) else {
        return;
    };
    let mut generation = signal.generation();
    let mut pending: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let wait = pending.map_or(Duration::from_secs(3600), |at| {
            SETTLE.saturating_sub(at.elapsed())
        });
        generation = signal.wait_until(generation, wait);
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Some(engine) = engine.upgrade() else {
            return;
        };
        let now = seen(&engine);
        let (save, next) = decide(&before, &now, pending);
        pending = next;
        before = now;
        if save && let Err(e) = engine.save_server_queue() {
            log::info!("server queue: not saved: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::player::state::QueueItemId;

    #[test]
    fn edits_wait_to_settle_and_a_move_or_a_pause_saves_at_once() {
        let a = Some(QueueItemId(uuid::Uuid::now_v7()));
        let b = Some(QueueItemId(uuid::Uuid::now_v7()));
        let playing = (1, a, PlaybackState::Playing);

        // An edit starts the wait; nothing else changing saves nothing yet.
        let edited = (2, a, PlaybackState::Playing);
        let (save, pending) = decide(&playing, &edited, None);
        assert!(!save && pending.is_some());
        let (save, still) = decide(&edited, &edited, pending);
        assert!(!save && still == pending, "not settled yet");

        // Settled: saved.
        let long_ago = Instant::now() - SETTLE;
        assert_eq!(decide(&edited, &edited, Some(long_ago)), (true, None));

        // Another track, or a pause, saves at once, edits and all.
        assert!(decide(&edited, &(3, b, PlaybackState::Playing), pending).0);
        assert_eq!(
            decide(&playing, &(1, a, PlaybackState::Paused), None),
            (true, None)
        );

        // The playhead moving is not news.
        assert_eq!(decide(&playing, &playing, None), (false, None));
    }
}
