//! Background jobs for work that cannot fit in a request.
//!
//! A full library scan walks the filesystem and a remote sync paginates a
//! Subsonic server; both run for minutes. Returning a handle and doing the work
//! on a detached thread keeps them off the runtime entirely, so audio streaming
//! on the same process is unaffected.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use async_graphql::Enum;
use koan_core::helpers::WatchScan;
use koan_core::index::scanner::{ScanEvent, ScanResult};
use uuid::Uuid;

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
#[graphql(name = "JobState")]
pub(crate) enum JobState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug)]
pub(crate) struct Job {
    pub id: String,
    pub kind: String,
    pub state: JobState,
    pub message: String,
    /// How far a running job has got, where it counts.
    pub done: Option<u64>,
    pub total: Option<u64>,
    /// What started a scan the library watch ran: see [`WatchScan::Started`].
    pub reason: Option<&'static str>,
}

/// Jobs started by this process, newest last. Bounded so a long-lived server
/// does not accumulate history forever. Shared by GraphQL and the web UI, so
/// a scan started from either shows in both.
#[derive(Clone)]
pub(crate) struct JobRegistry {
    inner: Arc<Mutex<HashMap<String, Job>>>,
    /// Moves whenever a job starts, reports progress or finishes.
    changed: Arc<tokio::sync::watch::Sender<u64>>,
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            changed: Arc::new(tokio::sync::watch::Sender::new(0)),
        }
    }
}

const MAX_HISTORY: usize = 64;

impl JobRegistry {
    /// Register a new running job, or refuse if one of the same kind is live.
    ///
    /// Two concurrent scans of the same library fight over the same rows, so
    /// the second caller gets the first job's handle instead of a second scan.
    pub(crate) fn start(&self, kind: &str, reason: Option<&'static str>) -> Result<Job, Job> {
        let mut jobs = self.inner.lock();
        if let Some(running) = jobs
            .values()
            .find(|j| j.kind == kind && j.state == JobState::Running)
        {
            return Err(running.clone());
        }
        if jobs.len() >= MAX_HISTORY {
            let finished: Vec<String> = jobs
                .values()
                .filter(|j| j.state != JobState::Running)
                .map(|j| j.id.clone())
                .collect();
            for id in finished {
                jobs.remove(&id);
            }
        }
        let job = Job {
            id: Uuid::now_v7().to_string(),
            kind: kind.to_string(),
            state: JobState::Running,
            message: format!("{} started", kind),
            done: None,
            total: None,
            reason,
        };
        jobs.insert(job.id.clone(), job.clone());
        drop(jobs);
        self.ring();
        Ok(job)
    }

    pub(crate) fn finish(&self, id: &str, state: JobState, message: String) {
        let mut jobs = self.inner.lock();
        if let Some(job) = jobs.get_mut(id) {
            job.state = state;
            job.message = message;
        }
        drop(jobs);
        self.ring();
    }

    fn ring(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Wakes whenever any job moves.
    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Run `work` on a detached thread with a connection from `open`,
    /// returning its job. A job of the same kind already running is returned
    /// as it is rather than started twice.
    pub(crate) fn spawn<O, F>(&self, kind: &'static str, open: O, work: F) -> Result<Job, String>
    where
        O: FnOnce() -> Result<koan_core::db::connection::Database, String> + Send + 'static,
        F: FnOnce(koan_core::db::connection::Database, JobHandle) -> Result<String, String>
            + Send
            + 'static,
    {
        let job = match self.start(kind, None) {
            Ok(job) => job,
            Err(running) => return Ok(running),
        };
        let id = job.id.clone();
        let registry = self.clone();
        let reporter = self.handle(&id);
        let spawned = std::thread::Builder::new()
            .name(format!("koan-job-{kind}"))
            .spawn(move || {
                registry.run(&id, || work(open()?, reporter));
            });
        if spawned.is_err() {
            self.finish(
                &job.id,
                JobState::Failed,
                "failed to spawn worker thread".into(),
            );
            return Err("failed to start job".into());
        }
        Ok(job)
    }

    /// Run `work` and record its outcome against job `id`. A panic finishes the
    /// job as Failed: left Running it would be handed back by `start` for
    /// every later call, and the kind could never be triggered again.
    pub(super) fn run(&self, id: &str, work: impl FnOnce() -> Result<String, String>) {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|panic| {
                let what = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                Err(format!("job panicked: {what}"))
            });
        match outcome {
            Ok(message) => self.finish(id, JobState::Succeeded, message),
            Err(message) => {
                log::error!("job {id} failed: {message}");
                self.finish(id, JobState::Failed, message)
            }
        }
    }

    /// Something the job's own thread can report progress through.
    pub(super) fn handle(&self, id: &str) -> JobHandle {
        JobHandle {
            registry: self.clone(),
            id: id.to_string(),
        }
    }

    pub(crate) fn get(&self, id: &str) -> Option<Job> {
        self.inner.lock().get(id).cloned()
    }

    /// The job of any of `kinds` worth showing: the earliest still running,
    /// which is the one holding the lane while later ones wait, or else the
    /// one started last.
    pub(crate) fn current(&self, kinds: &[&str]) -> Option<Job> {
        // Ids are UUIDv7, so they sort by when the job started.
        let jobs = self.inner.lock();
        let of_kind = || jobs.values().filter(|j| kinds.contains(&j.kind.as_str()));
        of_kind()
            .filter(|j| j.state == JobState::Running)
            .min_by(|a, b| a.id.cmp(&b.id))
            .or_else(|| of_kind().max_by(|a, b| a.id.cmp(&b.id)))
            .cloned()
    }

    pub(super) fn list(&self) -> Vec<Job> {
        let mut jobs: Vec<Job> = self.inner.lock().values().cloned().collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }
}

/// A running job's line back to the registry.
pub(crate) struct JobHandle {
    registry: JobRegistry,
    id: String,
}

impl JobHandle {
    pub(super) fn progress(&self, done: u64, total: Option<u64>, message: String) {
        let mut jobs = self.registry.inner.lock();
        if let Some(job) = jobs.get_mut(&self.id)
            && job.state == JobState::Running
        {
            job.done = Some(done);
            job.total = total;
            job.message = message;
        }
        drop(jobs);
        self.registry.ring();
    }
}

/// What a scan's job is called: a re-read is a job of its own, so asking for
/// one while a plain scan runs waits its turn in the scan lane rather than
/// joining a scan that will not read every file.
pub(crate) const SCAN: &str = "scan";
pub(crate) const REREAD: &str = "reread";
/// A scan the library watch started by itself: at start-up, on a change in
/// the folders, every so often, or for a folder newly added.
pub(crate) const WATCH: &str = "watch";

/// How many files a scan reads between progress reports.
const REPORT_EVERY: u64 = 50;

/// Scan the library folders as a job. With `reread`, every file's tags are
/// read again, ignoring the scan cache: for when what koan takes from tags
/// changes and files that have not changed need reading again. Progress
/// counts the files whose tags were read, which in a plain scan are only the
/// changed ones.
pub(crate) fn start_scan(
    registry: &JobRegistry,
    pool: Arc<koan_core::db::pool::Pool>,
    reread: bool,
) -> Result<Job, String> {
    registry.spawn(
        if reread { REREAD } else { SCAN },
        // Taken outside the pool's bound: this connection is held for minutes
        // and must not deny one to request-path resolvers.
        move || pool.take().map_err(|e| e.to_string()),
        move |db, progress| {
            let cfg = koan_core::config::Config::load().unwrap_or_default();
            crate::clients::changed_if_library_moved(&db.conn);
            let to_read = Arc::new(AtomicU64::new(0));
            let read = AtomicU64::new(0);
            let on_track: &dyn Fn(ScanEvent) = &|_| {
                let done = read.fetch_add(1, Ordering::Relaxed) + 1;
                if done.is_multiple_of(REPORT_EVERY) {
                    progress.read(done, to_read.load(Ordering::Relaxed));
                }
            };
            let result = koan_core::index::scanner::full_scan(
                &db,
                &cfg.library.folders,
                koan_core::index::scanner::ScanOptions {
                    force: reread,
                    to_read: Some(to_read.clone()),
                    ..Default::default()
                },
                Some(on_track),
            );
            crate::clients::changed_if_library_moved(&db.conn);
            outcome(&result)
        },
    )
}

impl JobHandle {
    fn read(&self, done: u64, total: u64) {
        self.progress(done, Some(total), format!("{done} of {total} files read"));
    }
}

/// How a scan ended, as a job's message.
fn outcome(result: &ScanResult) -> Result<String, String> {
    let mut summary = format!(
        "{} added, {} updated, {} removed, {} unchanged",
        result.added, result.updated, result.removed, result.skipped
    );
    if !result.errors.is_empty() {
        summary.push_str(&format!(", {} unreadable", result.errors.len()));
    }
    if result.cancelled {
        Err(format!("cancelled after {summary}"))
    } else if result.incomplete {
        Err(format!("stopped part way, after {summary}"))
    } else {
        Ok(summary)
    }
}

/// Records the library watch's scans as [`WATCH`] jobs, so `job(id:)` and the
/// web UI report them as they do a scan someone started.
pub(crate) fn record_watch(registry: &JobRegistry) -> impl Fn(&WatchScan) + Send + Sync + 'static {
    let registry = registry.clone();
    let running: Mutex<Option<JobHandle>> = Mutex::new(None);
    move |scan| match scan {
        WatchScan::Started(reason) => {
            // A scan every quarter hour would push people's own jobs out of
            // the history; only the last watch scan is worth showing.
            registry
                .inner
                .lock()
                .retain(|_, j| j.kind != WATCH || j.state == JobState::Running);
            // The watch runs one scan at a time, so none of its kind is live.
            let (Ok(job) | Err(job)) = registry.start(WATCH, Some(reason));
            *running.lock() = Some(registry.handle(&job.id));
        }
        WatchScan::Read { done, total } => {
            if let Some(job) = running.lock().as_ref() {
                job.read(*done, *total);
            }
        }
        WatchScan::Finished(result) => {
            if let Some(job) = running.lock().take() {
                let (state, message) = match outcome(result) {
                    Ok(message) => (JobState::Succeeded, message),
                    Err(message) => (JobState::Failed, message),
                };
                registry.finish(&job.id, state, message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_job_per_kind_runs_at_a_time() {
        let reg = JobRegistry::default();
        let first = reg.start("scan", None).unwrap();
        let clash = reg.start("scan", None).unwrap_err();
        assert_eq!(first.id, clash.id);

        reg.finish(&first.id, JobState::Succeeded, "done".into());
        let second = reg.start("scan", None).unwrap();
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn a_panicking_job_ends_failed_and_the_kind_can_start_again() {
        let reg = JobRegistry::default();
        let job = reg.start("scan", None).unwrap();
        reg.run(&job.id, || panic!("boom"));

        let done = reg.get(&job.id).unwrap();
        assert_eq!(done.state, JobState::Failed);
        assert!(done.message.contains("boom"), "{}", done.message);

        let next = reg.start("scan", None).expect("a new scan can start");
        assert_ne!(next.id, job.id);
    }

    #[test]
    fn the_library_watch_s_scans_are_jobs() {
        let reg = JobRegistry::default();
        let record = record_watch(&reg);
        record(&WatchScan::Started("start-up"));
        record(&WatchScan::Read {
            done: 50,
            total: 1200,
        });
        let running = reg.current(&[WATCH]).unwrap();
        assert_eq!(running.state, JobState::Running);
        assert_eq!(running.reason, Some("start-up"));
        assert_eq!((running.done, running.total), (Some(50), Some(1200)));
        assert_eq!(running.message, "50 of 1200 files read");

        let mut result = ScanResult::default();
        result.added = 3;
        record(&WatchScan::Finished(&result));
        let done = reg.get(&running.id).unwrap();
        assert_eq!(done.state, JobState::Succeeded);
        assert_eq!(done.message, "3 added, 0 updated, 0 removed, 0 unchanged");

        record(&WatchScan::Started("folder change"));
        assert_ne!(reg.current(&[WATCH]).unwrap().id, running.id);
        assert!(reg.get(&running.id).is_none(), "only the last is kept");
    }

    #[test]
    fn different_kinds_do_not_block_each_other() {
        let reg = JobRegistry::default();
        reg.start("scan", None).unwrap();
        assert!(reg.start("remoteSync", None).is_ok());
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn a_running_job_reports_progress_and_a_finished_one_keeps_its_outcome() {
        let reg = JobRegistry::default();
        let job = reg.start("remoteSync", None).unwrap();
        let handle = reg.handle(&job.id);

        handle.progress(12_400, Some(49_700), "tracks: 12400 of 49700".into());
        let running = reg.get(&job.id).unwrap();
        assert_eq!((running.done, running.total), (Some(12_400), Some(49_700)));

        reg.finish(&job.id, JobState::Succeeded, "done".into());
        handle.progress(1, None, "late".into());
        assert_eq!(reg.get(&job.id).unwrap().message, "done");
    }
}
