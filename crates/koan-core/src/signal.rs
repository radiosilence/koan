//! Saying that something changed, rather than being asked.
//!
//! koan's shared state is versions and atomics: the cheapest thing to read on
//! a hot path, and the cheapest thing to *miss* — a reader had to look again to
//! find out. This is the other half. A writer says so, a reader waits, and a
//! koan with nothing happening schedules nothing at all.
//!
//! The versions stay exactly as they were. They are what a reader consults on
//! waking to find out *what* moved, which is a handful of relaxed loads; this
//! only answers *whether* anything did.

use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use tokio::sync::watch;

/// A generation counter that can be waited on.
pub struct Wake {
    generation: Mutex<u64>,
    changed: Condvar,
    /// The same signal for a caller that cannot park a thread on it. A GraphQL
    /// subscription is a task on a runtime, and a task that blocks holds a
    /// worker rather than sleeping.
    published: watch::Sender<u64>,
}

impl Wake {
    pub fn new() -> Self {
        Self {
            generation: Mutex::new(0),
            changed: Condvar::new(),
            published: watch::Sender::new(0),
        }
    }

    /// Something moved. Cheap, and cheapest of all when nobody is waiting.
    pub fn bump(&self) {
        let mut generation = self.generation.lock();
        *generation = generation.wrapping_add(1);
        self.changed.notify_all();
        // Both kinds of waiter from one lock, so a thread and a task can never
        // disagree about which generation they are looking at.
        self.published.send_replace(*generation);
    }

    /// The generation now, to be handed back to `wait`.
    pub fn generation(&self) -> u64 {
        *self.generation.lock()
    }

    /// Wait until the generation leaves `seen`.
    ///
    /// Taken under the lock a bump takes, so one landing between a reader
    /// deciding to wait and waiting is not slept through.
    pub fn wait(&self, seen: u64) -> u64 {
        let mut generation = self.generation.lock();
        while *generation == seen {
            self.changed.wait(&mut generation);
        }
        *generation
    }

    /// A receiver that wakes on each bump, for an async caller. Its value is
    /// the generation, so a subscriber that slept through a burst sees one
    /// wake and reads the versions once — the same batching a thread gets.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.published.subscribe()
    }

    /// The same, giving up after `timeout`. For a caller that has something
    /// else to check on — an engine that may have been dropped, a flag it does
    /// not own — and cannot be woken for it.
    pub fn wait_until(&self, seen: u64, timeout: Duration) -> u64 {
        let mut generation = self.generation.lock();
        if *generation == seen {
            self.changed.wait_for(&mut generation, timeout);
        }
        *generation
    }
}

impl std::fmt::Debug for Wake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wake")
            .field("generation", &self.generation())
            .finish()
    }
}

impl Default for Wake {
    fn default() -> Self {
        Self::new()
    }
}

/// The one every front end waits on.
///
/// Process-wide, like the download store beside it, and for the same reason: a
/// writer deep in the player or in a download worker has to be able to say so
/// without having been handed anything. Two engines in one process would share
/// it, which costs a spurious wake and nothing else — what actually moved is
/// decided by the version counters afterwards, not by this.
pub fn engine_changed() -> &'static Wake {
    static WAKE: OnceLock<Wake> = OnceLock::new();
    WAKE.get_or_init(Wake::new)
}

/// Rung whenever koan writes its config (`Config::persist`, and anything else
/// that drops the cached copy). What a background thread that acts on a
/// setting waits on, so it neither re-reads the files on a timer nor misses a
/// change made while it slept. A file edited by hand does not ring it.
pub fn config_changed() -> &'static Wake {
    static WAKE: OnceLock<Wake> = OnceLock::new();
    WAKE.get_or_init(Wake::new)
}

/// Rung when the link to the server comes up or goes down. What auto-sync
/// waits on to know whether the server will say when its library moves.
pub fn link_changed() -> &'static Wake {
    static WAKE: OnceLock<Wake> = OnceLock::new();
    WAKE.get_or_init(Wake::new)
}

/// Send `message` down `tx` each time `wake` rings, from a thread parked on it,
/// for a waiter that has other things to hear on the same channel. The
/// generation is taken now, so a ring before the thread starts still arrives.
/// Ends at the first ring after the receiver is gone.
pub fn forward<T: Send + 'static>(
    wake: &'static Wake,
    tx: std::sync::mpsc::Sender<T>,
    message: impl Fn() -> T + Send + 'static,
) {
    let mut seen = wake.generation();
    let spawned = std::thread::Builder::new()
        .name("koan-signal".into())
        .spawn(move || {
            loop {
                seen = wake.wait(seen);
                if tx.send(message()).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not follow a signal: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn a_waiter_sleeps_until_something_moves() {
        let wake = Arc::new(Wake::new());
        let seen = wake.generation();
        let woken = Arc::new(AtomicBool::new(false));

        let waiter = {
            let (wake, woken) = (Arc::clone(&wake), Arc::clone(&woken));
            std::thread::spawn(move || {
                wake.wait(seen);
                woken.store(true, Ordering::Relaxed);
            })
        };

        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !woken.load(Ordering::Relaxed),
            "woke with nothing to wake for"
        );
        wake.bump();
        waiter.join().unwrap();
    }

    #[test]
    fn a_bump_before_the_wait_is_not_slept_through() {
        let wake = Wake::new();
        let seen = wake.generation();
        wake.bump();
        // Would block for ever if the generation were not what says so.
        assert_ne!(wake.wait(seen), seen);
    }
}
