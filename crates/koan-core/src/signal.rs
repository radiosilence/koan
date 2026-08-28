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

/// A generation counter that can be waited on.
pub struct Wake {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl Wake {
    pub fn new() -> Self {
        Self {
            generation: Mutex::new(0),
            changed: Condvar::new(),
        }
    }

    /// Something moved. Cheap, and cheapest of all when nobody is waiting.
    pub fn bump(&self) {
        let mut generation = self.generation.lock();
        *generation = generation.wrapping_add(1);
        self.changed.notify_all();
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
