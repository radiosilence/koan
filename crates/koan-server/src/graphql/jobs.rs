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
pub(super) enum JobState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug)]
pub(super) struct Job {
    pub id: String,
    pub kind: String,
    pub state: JobState,
    pub message: String,
}

/// Jobs started by this process, newest last. Bounded so a long-lived server
/// does not accumulate history forever.
#[derive(Clone, Default)]
pub(super) struct JobRegistry {
    inner: Arc<Mutex<HashMap<String, Job>>>,
}

const MAX_HISTORY: usize = 64;

impl JobRegistry {
    /// Register a new running job, or refuse if one of the same kind is live.
    ///
    /// Two concurrent scans of the same library fight over the same rows, so
    /// the second caller gets the first job's handle instead of a second scan.
    pub(super) fn start(&self, kind: &str) -> Result<Job, Job> {
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
        };
        jobs.insert(job.id.clone(), job.clone());
        Ok(job)
    }

    pub(super) fn finish(&self, id: &str, state: JobState, message: String) {
        let mut jobs = self.inner.lock();
        if let Some(job) = jobs.get_mut(id) {
            job.state = state;
            job.message = message;
        }
    }

    pub(super) fn get(&self, id: &str) -> Option<Job> {
        self.inner.lock().get(id).cloned()
    }

    pub(super) fn list(&self) -> Vec<Job> {
        let mut jobs: Vec<Job> = self.inner.lock().values().cloned().collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }
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
    fn different_kinds_do_not_block_each_other() {
        let reg = JobRegistry::default();
        reg.start("scan").unwrap();
        assert!(reg.start("remoteSync").is_ok());
        assert_eq!(reg.list().len(), 2);
    }
}
