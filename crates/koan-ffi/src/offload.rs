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
//!
//! Beside the pool are three lanes, each one thread running its work in the
//! order it was handed over: player commands, commands from other devices, and
//! settings writes.

use std::sync::LazyLock;
use std::time::Duration;

use crossbeam_channel::Sender;
use tokio::runtime::Runtime;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Owns the blocking pool.
///
/// One worker thread, and it matters that there is one at all. On a
/// current-thread runtime nobody ever calls `block_on`, so the scheduler is
/// never driven — and `spawn_blocking`'s completion has to travel through it to
/// wake the caller. The work still ran in microseconds; the *answer* took tens
/// of milliseconds to come back, on every call the app made. A worker thread is
/// a driver, and the answer arrives when the work finishes.
static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        // A tripwire, not a target. Threads are spawned on demand and parked
        // when idle, so a GUI sits at a handful; reaching this many blocked at
        // once is a runaway, and failing there beats failing at the OS thread
        // limit, where `spawn` starts erroring somewhere unrelated.
        .max_blocking_threads(512)
        .build()
        .expect("a runtime to start")
});

/// Run `f` off the calling thread.
///
/// Order between concurrent calls is undefined — use [`sequenced`] where it
/// matters. A panic in `f` is re-raised here, so uniffi reports it as it would
/// a panic in a synchronous call.
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
///
/// A panic in `f` is caught on the lane and re-raised here, as [`offload`]
/// does: the call that panicked fails, and the lane goes on running every
/// command after it.
pub async fn sequenced<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    LANE.send(Box::new(move || {
        // The receiver is gone when the caller's Task was cancelled. The work
        // is done either way; only the answer had nowhere to go.
        let _ = tx.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)));
    }))
    .expect("the command lane outlives the engine");

    match rx.await {
        Ok(Ok(value)) => value,
        Ok(Err(payload)) => std::panic::resume_unwind(payload),
        Err(_) => panic!("the command lane stopped running"),
    }
}

static LANE: LazyLock<Sender<Job>> = LazyLock::new(|| lane("koan-commands"));

/// Run `f` on the lane for commands from other devices, in arrival order,
/// without waiting for it. The link's and the network's socket threads hand
/// their commands here and go back to their sockets: one that syncs the
/// library first would otherwise leave its socket unread and unpinged for as
/// long as the sync took, and the server drops a link silent for 100 s. Its
/// own lane, apart from [`sequenced`], so the app's own commands do not wait
/// behind such a sync either. A panic in `f` is logged and the lane goes on.
pub fn from_another_device<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    fire(&REMOTE_LANE, "a command from another device", f);
}

static REMOTE_LANE: LazyLock<Sender<Job>> = LazyLock::new(|| lane("koan-remote-commands"));

/// Changes saved off the caller's thread, in batches. Each write reads and
/// rewrites the config files, which on a slow disk is long enough to stall the
/// UI thread that made the change; the change itself is already drawn from the
/// app's own copy. Changes made while a write is waiting join it, so a run of
/// them (View → Bigger held down) costs one write, applied in the order they
/// were made.
pub struct Batches<T> {
    pending: parking_lot::Mutex<Vec<T>>,
    /// Held while a batch is taken and written, so batches land in order.
    writing: parking_lot::Mutex<()>,
    write: fn(Vec<T>),
}

impl<T: Send + 'static> Batches<T> {
    pub const fn new(write: fn(Vec<T>)) -> Self {
        Self {
            pending: parking_lot::Mutex::new(Vec::new()),
            writing: parking_lot::Mutex::new(()),
            write,
        }
    }

    /// Queue `change` and return at once. The first change of a batch sends
    /// its write to the settings lane; a panic there is logged.
    pub fn add(&'static self, change: T) {
        let first = {
            let mut pending = self.pending.lock();
            pending.push(change);
            pending.len() == 1
        };
        if first {
            fire(&SETTINGS_LANE, "a settings write", || self.settle());
        }
    }

    /// Write whatever is queued, here, after any write in progress: for a read
    /// that must see every change made. Nothing queued and nothing writing
    /// costs two uncontended locks.
    pub fn settle(&self) {
        let _writing = self.writing.lock();
        let batch = std::mem::take(&mut *self.pending.lock());
        if !batch.is_empty() {
            (self.write)(batch);
        }
    }

    /// Wait up to `within` for everything queued to be written, as the process
    /// exits; false if it was not. The write runs on the lane, so a disk that
    /// has gone away holds the lane and not the caller.
    pub fn settle_within(&'static self, within: Duration) -> bool {
        if self.pending.lock().is_empty() && !self.writing.is_locked() {
            return true;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        fire(&SETTINGS_LANE, "a settings write", move || {
            self.settle();
            let _ = tx.send(());
        });
        rx.recv_timeout(within).is_ok()
    }
}

static SETTINGS_LANE: LazyLock<Sender<Job>> = LazyLock::new(|| lane("koan-settings"));

/// Send `f` down `lane` without waiting for it. A panic in `f` is logged as
/// `what` and the lane goes on.
fn fire<F>(lane: &Sender<Job>, what: &'static str, f: F)
where
    F: FnOnce() + Send + 'static,
{
    lane.send(Box::new(move || {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
            log::error!("{what} panicked");
        }
    }))
    .expect("a lane outlives the engine");
}

/// One thread running whatever is sent to it, in the order it was sent.
fn lane(name: &str) -> Sender<Job> {
    let (tx, rx) = crossbeam_channel::unbounded::<Job>();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            for job in rx {
                job();
            }
        })
        .unwrap_or_else(|e| panic!("{name} needs a thread: {e}"));
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn commands_from_another_device_return_at_once_and_run_in_order() {
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let (done, finished) = crossbeam_channel::bounded(1);
        let started = std::time::Instant::now();
        for i in 0..8 {
            let seen = seen.clone();
            let done = done.clone();
            from_another_device(move || {
                // The first is a long sync: the caller must not wait for it,
                // and the rest must not overtake it.
                if i == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                seen.lock().push(i);
                if i == 7 {
                    let _ = done.send(());
                }
            });
        }
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(*seen.lock(), (0..8).collect::<Vec<_>>());
    }

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

    #[tokio::test]
    async fn the_lane_outlives_a_panicking_command() {
        let escaped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures_lite::future::block_on(sequenced(|| panic!("boom")))
        }));
        assert!(escaped.is_err(), "the panicking call fails to its caller");
        assert_eq!(7, sequenced(|| 7).await, "and the next command still runs");
    }

    type Signal = (Sender<()>, crossbeam_channel::Receiver<()>);
    static WRITTEN: parking_lot::Mutex<Vec<Vec<u32>>> = parking_lot::Mutex::new(Vec::new());
    static ENTERED: LazyLock<Signal> = LazyLock::new(|| crossbeam_channel::bounded(1));
    static GATE: LazyLock<Signal> = LazyLock::new(|| crossbeam_channel::bounded(1));
    static BATCHES: Batches<u32> = Batches::new(|batch| {
        // The first write is a slow disk, held until the test lets it go.
        if batch == [1] {
            ENTERED.0.send(()).unwrap();
            GATE.1.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        WRITTEN.lock().push(batch);
    });

    #[test]
    fn changes_return_at_once_join_a_waiting_write_and_land_in_order() {
        BATCHES.add(1);
        ENTERED.1.recv_timeout(Duration::from_secs(5)).unwrap();
        // The lane is writing [1]: these return at once and make one batch.
        BATCHES.add(2);
        BATCHES.add(3);
        assert!(!BATCHES.settle_within(Duration::ZERO));
        GATE.0.send(()).unwrap();
        BATCHES.settle();
        assert_eq!(*WRITTEN.lock(), vec![vec![1], vec![2, 3]]);
        assert!(BATCHES.settle_within(Duration::from_secs(5)));
    }
}
