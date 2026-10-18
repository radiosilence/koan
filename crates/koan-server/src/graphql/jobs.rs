//! Background jobs for work that cannot fit in a request.
//!
//! A full library scan walks the filesystem and a remote sync paginates a
//! Subsonic server; both run for minutes. Returning a handle and doing the work
//! on a detached thread keeps them off the runtime entirely, so audio streaming
//! on the same process is unaffected.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use async_graphql::Enum;
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
    pub(crate) fn start(&self, kind: &str) -> Result<Job, Job> {
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
        let job = match self.start(kind) {
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

    /// The most recently started job of `kind`.
    pub(crate) fn latest(&self, kind: &str) -> Option<Job> {
        // Ids are UUIDv7, so they sort by when the job started.
        self.inner
            .lock()
            .values()
            .filter(|j| j.kind == kind)
            .max_by(|a, b| a.id.cmp(&b.id))
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

/// Scan the library folders as a job. With `reread`, every file's tags are
/// read again, ignoring the scan cache: for when what koan takes from tags
/// changes and files that have not changed need reading again.
pub(crate) fn start_scan(
    registry: &JobRegistry,
    pool: Arc<koan_core::db::pool::Pool>,
    reread: bool,
) -> Result<Job, String> {
    registry.spawn(
        "scan",
        // Deliberately outside the pool's bound: this connection is held for
        // minutes and must not deny one to request-path resolvers.
        move || pool.take().map_err(|e| e.to_string()),
        move |db, _| {
            let cfg = koan_core::config::Config::load().unwrap_or_default();
            crate::clients::changed_if_library_moved(&db.conn);
            let result = koan_core::index::scanner::full_scan(
                &db,
                &cfg.library.folders,
                koan_core::index::scanner::ScanOptions {
                    force: reread,
                    ..Default::default()
                },
                None,
            );
            crate::clients::changed_if_library_moved(&db.conn);
            Ok(format!(
                "{} added, {} updated, {} unchanged",
                result.added, result.updated, result.skipped
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_job_per_kind_runs_at_a_time() {
        let reg = JobRegistry::default();
        let first = reg.start("scan").unwrap();
        let clash = reg.start("scan").unwrap_err();
        assert_eq!(first.id, clash.id);

        reg.finish(&first.id, JobState::Succeeded, "done".into());
        let second = reg.start("scan").unwrap();
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn a_panicking_job_ends_failed_and_the_kind_can_start_again() {
        let reg = JobRegistry::default();
        let job = reg.start("scan").unwrap();
        reg.run(&job.id, || panic!("boom"));

        let done = reg.get(&job.id).unwrap();
        assert_eq!(done.state, JobState::Failed);
        assert!(done.message.contains("boom"), "{}", done.message);

        let next = reg.start("scan").expect("a new scan can start");
        assert_ne!(next.id, job.id);
    }

    #[test]
    fn different_kinds_do_not_block_each_other() {
        let reg = JobRegistry::default();
        reg.start("scan").unwrap();
        assert!(reg.start("remoteSync").is_ok());
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn a_running_job_reports_progress_and_a_finished_one_keeps_its_outcome() {
        let reg = JobRegistry::default();
        let job = reg.start("remoteSync").unwrap();
        let handle = reg.handle(&job.id);

        handle.progress(12_400, Some(49_700), "tracks: 12400 of 49700".into());
        let running = reg.get(&job.id).unwrap();
        assert_eq!((running.done, running.total), (Some(12_400), Some(49_700)));

        reg.finish(&job.id, JobState::Succeeded, "done".into());
        handle.progress(1, None, "late".into());
        assert_eq!(reg.get(&job.id).unwrap().message, "done");
    }
}
