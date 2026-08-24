//! Somewhere for blocking work to happen that is not the caller's thread.
//!
//! koan-core is synchronous throughout — rusqlite has no async form, the audio
//! path is dedicated OS threads, and three of its four consumers are sync
//! loops. So the boundary is where the hop belongs, and this is the hop. It is
//! the same offload `koan-server` does at its own edge, for the same reason.
//!
//! Nothing here uses tokio's async I/O; `spawn_blocking` is a thread pool that
//! happens to hand back a future. The pool grows on demand and is not sized to
//! the core count — a thread waiting on a socket costs a kernel stack and
//! address space it never touches, not CPU, and capping blocking work at the
//! core count queues it behind nothing.

use std::sync::LazyLock;

use crossbeam_channel::Sender;
use tokio::runtime::Runtime;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Owns the blocking pool. Current-thread because the async half is never
/// used: uniffi polls exported futures from Swift, so there is no driver to
/// run and no worker threads beyond the ones jobs are handed to.
static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        // A tripwire, not a target. Threads are spawned on demand and parked
        // when idle, so a GUI sits at a handful; reaching this many blocked at
        // once is a runaway, and failing there beats failing at the OS thread
        // limit, where `spawn` starts erroring somewhere unrelated.
        .max_blocking_threads(512)
        .build()
        .expect("a runtime with no driver to start")
});

/// Run `f` off the calling thread.
///
/// Order between concurrent calls is undefined — use [`sequenced`] where it
/// matters. A panic in `f` is re-raised here, so uniffi reports it the same way
/// it did when these calls were synchronous.
pub async fn offload<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match RUNTIME.spawn_blocking(f).await {
        Ok(value) => value,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// Run `f` on the one lane every player command shares, in submission order.
///
/// Queue mutations resolve tracks against the database before sending a
/// `PlayerCommand`, so two running concurrently would reach the channel in
/// whichever order finished first: dropping in an album and then pressing undo
/// could undo the drop before it landed. A pool cannot promise that ordering,
/// so this is one thread and a queue.
pub async fn sequenced<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    LANE.send(Box::new(move || {
        // The receiver is gone when the caller's Task was cancelled. The work
        // is done either way; only the answer had nowhere to go.
        let _ = tx.send(f());
    }))
    .expect("the command lane outlives the engine");

    match rx.await {
        Ok(value) => value,
        Err(_) => panic!("the command lane stopped running"),
    }
}

static LANE: LazyLock<Sender<Job>> = LazyLock::new(|| {
    let (tx, rx) = crossbeam_channel::unbounded::<Job>();
    std::thread::Builder::new()
        .name("koan-commands".into())
        .spawn(move || {
            for job in rx {
                job();
            }
        })
        .expect("the command lane needs one thread");
    tx
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn offloaded_work_runs_off_the_caller() {
        let caller = std::thread::current().id();
        assert_ne!(caller, offload(|| std::thread::current().id()).await);
    }

    #[tokio::test]
    async fn the_lane_runs_commands_in_submission_order() {
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let started = Arc::new(AtomicUsize::new(0));

        // Submitted back to back without awaiting, which is how two queue
        // mutations from one gesture reach the engine.
        let jobs: Vec<_> = (0..16)
            .map(|i| {
                let (seen, started) = (seen.clone(), started.clone());
                sequenced(move || {
                    // The first job dawdles, so a pool would let later ones
                    // overtake it and the assertion below would catch it.
                    if started.fetch_add(1, Ordering::AcqRel) == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(30));
                    }
                    seen.lock().push(i);
                })
            })
            .collect();
        for job in jobs {
            job.await;
        }

        assert_eq!(*seen.lock(), (0..16).collect::<Vec<i32>>());
    }

    /// The case that matters: uniffi polls exported futures from Swift, with no
    /// tokio runtime anywhere on the stack. If awaiting these needed an ambient
    /// runtime the whole surface would deadlock in the app and nowhere else.
    #[test]
    fn both_work_with_no_runtime_on_the_stack() {
        assert_eq!(
            42,
            futures_lite::future::block_on(offload(|| 42)),
            "spawn_blocking's pool runs without its runtime being driven"
        );
        assert_eq!(7, futures_lite::future::block_on(sequenced(|| 7)));
    }

    #[tokio::test]
    async fn a_panic_crosses_back_to_the_caller() {
        let escaped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures_lite::future::block_on(offload(|| panic!("boom")))
        }));
        assert!(
            escaped.is_err(),
            "a panic must not be swallowed into a hang"
        );
    }
}
