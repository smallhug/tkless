//! In-process registry for long-running MCP jobs.
//!
//! Jobs are deliberately process-local: they survive neither restarts nor sharing between
//! server processes. Retention is bounded by a per-job TTL, and only a fixed number of
//! queued/running jobs may exist at once.
use crate::TklessError;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use tokio::task::AbortHandle;

/// Maximum number of queued or running jobs per server instance.
pub const MAX_ACTIVE_JOBS: usize = 4;
/// Maximum number of finished jobs retained regardless of TTL, so a client cannot
/// grow the registry indefinitely by completing jobs quickly.
pub const MAX_RETAINED_JOBS: usize = 64;
/// Retention after a job reaches a terminal state.
pub const DEFAULT_JOB_TTL_SECS: u64 = 3600;
pub const MIN_JOB_TTL_SECS: u64 = 60;
pub const MAX_JOB_TTL_SECS: u64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    #[default]
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JobProgress {
    pub completed: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSnapshot {
    pub job_id: String,
    pub tool: String,
    pub state: JobState,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub elapsed_ms: u64,
    pub progress: JobProgress,
    pub result: Option<Value>,
    pub error: Option<String>,
}

struct RecordState {
    state: JobState,
    started: Option<SystemTime>,
    finished: Option<SystemTime>,
    progress: JobProgress,
    result: Option<Value>,
    error: Option<String>,
}

impl Default for RecordState {
    fn default() -> Self {
        Self {
            state: JobState::Queued,
            started: None,
            finished: None,
            progress: JobProgress::default(),
            result: None,
            error: None,
        }
    }
}

pub struct JobHandle {
    id: String,
    seq: u64,
    tool: String,
    created: SystemTime,
    ttl: Duration,
    state: Mutex<RecordState>,
    abort: Mutex<Option<AbortHandle>>,
}

impl JobHandle {
    fn new(id: String, seq: u64, tool: String, ttl: Duration) -> Self {
        Self {
            id,
            seq,
            tool,
            created: SystemTime::now(),
            ttl,
            state: Mutex::new(RecordState::default()),
            abort: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, RecordState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn mark_running(&self) {
        let mut state = self.lock();
        if state.state == JobState::Queued {
            state.state = JobState::Running;
            state.started = Some(SystemTime::now());
            state.progress.message = Some("running".into());
        }
    }

    /// Records a terminal result. Jobs that already reached a terminal state,
    /// including cancelled ones, keep it and ignore late results.
    pub fn finish(&self, outcome: Result<Value, String>) {
        let mut state = self.lock();
        if !state.state.is_active() {
            return;
        }
        state.finished = Some(SystemTime::now());
        match outcome {
            Ok(value) => {
                state.state = JobState::Completed;
                state.result = Some(value);
                state.progress.message = Some("completed".into());
            }
            Err(message) => {
                state.state = JobState::Failed;
                state.error = Some(message);
                state.progress.message = Some("failed".into());
            }
        }
    }

    /// Marks a job that ended without reporting a result (for example a panic).
    fn abandon(&self) {
        let mut state = self.lock();
        if state.state.is_active() {
            state.state = JobState::Failed;
            state.finished = Some(SystemTime::now());
            state.error = Some("job task terminated before reporting a result".into());
            state.progress.message = Some("failed".into());
        }
    }

    pub fn set_abort_handle(&self, handle: AbortHandle) {
        if !self.lock().state.is_active() {
            return;
        }
        let mut slot = self
            .abort
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(handle);
    }

    pub fn cancel(&self) {
        {
            let mut state = self.lock();
            if !state.state.is_active() {
                return;
            }
            state.state = JobState::Cancelled;
            state.finished = Some(SystemTime::now());
            state.progress.message = Some("cancelled".into());
        }
        let handle = self
            .abort
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    fn state(&self) -> JobState {
        self.lock().state
    }

    fn expired(&self, now: SystemTime) -> bool {
        let state = self.lock();
        match state.finished {
            Some(finished) if !state.state.is_active() => now
                .duration_since(finished)
                .map(|age| age > self.ttl)
                .unwrap_or(false),
            _ => false,
        }
    }

    pub fn snapshot(&self) -> JobSnapshot {
        self.snapshot_inner(true)
    }

    pub fn summary(&self) -> JobSnapshot {
        self.snapshot_inner(false)
    }

    fn snapshot_inner(&self, include_result: bool) -> JobSnapshot {
        let state = self.lock();
        let end = state.finished.unwrap_or_else(SystemTime::now);
        let begin = state.started.unwrap_or(self.created);
        let elapsed_ms = end
            .duration_since(begin)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        JobSnapshot {
            job_id: self.id.clone(),
            tool: self.tool.clone(),
            state: state.state,
            created_at: timestamp(self.created),
            started_at: state.started.map(timestamp),
            finished_at: state.finished.map(timestamp),
            elapsed_ms,
            progress: state.progress.clone(),
            result: if include_result {
                state.result.clone()
            } else {
                None
            },
            error: state.error.clone(),
        }
    }
}

fn timestamp(time: SystemTime) -> String {
    let datetime: DateTime<Utc> = time.into();
    datetime.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[derive(Default)]
struct RegistryInner {
    jobs: HashMap<String, Arc<JobHandle>>,
    next_seq: u64,
}

/// Shared job registry; cheap to clone and safe across server clones.
#[derive(Clone, Default)]
pub struct JobRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Reserves a job slot, failing when too many jobs are still queued or running.
    pub fn reserve(&self, tool: &str, ttl: Duration) -> Result<Arc<JobHandle>, TklessError> {
        let mut inner = self.lock();
        inner.prune();
        let active = inner
            .jobs
            .values()
            .filter(|job| job.state().is_active())
            .count();
        if active >= MAX_ACTIVE_JOBS {
            return Err(TklessError::InvalidArgument(format!(
                "Too many active MCP jobs (limit {MAX_ACTIVE_JOBS}); wait for or cancel a running job"
            )));
        }
        inner.next_seq += 1;
        let id = format!("job-{}", inner.next_seq);
        let handle = Arc::new(JobHandle::new(
            id.clone(),
            inner.next_seq,
            tool.to_string(),
            ttl,
        ));
        inner.jobs.insert(id, handle.clone());
        Ok(handle)
    }

    pub fn snapshot(&self, id: &str) -> Option<JobSnapshot> {
        let mut inner = self.lock();
        inner.prune();
        inner.jobs.get(id).map(|job| job.snapshot())
    }

    pub fn cancel(&self, id: &str) -> Option<JobSnapshot> {
        let job = {
            let mut inner = self.lock();
            inner.prune();
            inner.jobs.get(id).cloned()
        };
        let job = job?;
        job.cancel();
        Some(job.snapshot())
    }

    pub fn list(&self, state: Option<JobState>, limit: usize) -> (Vec<JobSnapshot>, usize) {
        let mut inner = self.lock();
        inner.prune();
        let active = inner
            .jobs
            .values()
            .filter(|job| job.state().is_active())
            .count();
        let mut jobs: Vec<_> = inner
            .jobs
            .values()
            .filter(|job| state.is_none_or(|wanted| job.state() == wanted))
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.seq));
        let jobs = jobs
            .into_iter()
            .take(limit)
            .map(|job| job.summary())
            .collect();
        (jobs, active)
    }
}

impl RegistryInner {
    fn prune(&mut self) {
        let now = SystemTime::now();
        self.jobs.retain(|_, job| !job.expired(now));
        while self.jobs.len() > MAX_RETAINED_JOBS {
            let oldest = self
                .jobs
                .iter()
                .filter(|(_, job)| !job.state().is_active())
                .min_by_key(|(_, job)| job.seq)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    self.jobs.remove(&id);
                }
                None => break,
            }
        }
    }
}

/// Marks a job as failed if its task is dropped without reporting a result (panic or abort).
pub struct JobGuard(pub Arc<JobHandle>);

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_lifecycle_and_limits() {
        let registry = JobRegistry::new();
        let job = registry
            .reserve("crawl_site", Duration::from_secs(60))
            .unwrap();
        assert_eq!(registry.snapshot(&job.id).unwrap().state, JobState::Queued);
        job.mark_running();
        job.finish(Ok(serde_json::json!({"ok": true})));
        let snapshot = registry.snapshot(&job.id).unwrap();
        assert_eq!(snapshot.state, JobState::Completed);
        assert_eq!(snapshot.result.unwrap()["ok"], true);
        assert!(snapshot.finished_at.is_some());
        job.finish(Err("late".into()));
        assert_eq!(
            registry.snapshot(&job.id).unwrap().state,
            JobState::Completed
        );

        let (list, active) = registry.list(None, 10);
        assert_eq!(list.len(), 1);
        assert_eq!(active, 0);
        assert!(list[0].result.is_none());
    }

    #[test]
    fn cancel_is_idempotent_and_wins_over_late_results() {
        let registry = JobRegistry::new();
        let job = registry
            .reserve("batch_fetch_webpages", Duration::from_secs(60))
            .unwrap();
        job.mark_running();
        let cancelled = registry.cancel(&job.id).unwrap();
        assert_eq!(cancelled.state, JobState::Cancelled);
        job.finish(Ok(serde_json::json!({"late": true})));
        let snapshot = registry.snapshot(&job.id).unwrap();
        assert_eq!(snapshot.state, JobState::Cancelled);
        assert!(snapshot.result.is_none());
        assert_eq!(registry.cancel(&job.id).unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn active_jobs_are_capped_and_expired_jobs_pruned() {
        let registry = JobRegistry::new();
        let mut jobs = Vec::new();
        for _ in 0..MAX_ACTIVE_JOBS {
            jobs.push(
                registry
                    .reserve("crawl_site", Duration::from_secs(60))
                    .expect("active slot"),
            );
        }
        assert!(registry
            .reserve("crawl_site", Duration::from_secs(60))
            .is_err());

        for job in &jobs {
            job.finish(Ok(serde_json::json!({"done": true})));
        }
        assert!(registry.list(None, 10).0.len() == MAX_ACTIVE_JOBS);

        let expiring = registry.reserve("sync_collection", Duration::ZERO).unwrap();
        expiring.finish(Ok(serde_json::json!({"done": true})));
        let (list, _) = registry.list(None, 10);
        assert!(!list.iter().any(|job| job.job_id == expiring.id));
    }

    #[test]
    fn dropped_job_tasks_are_reported_as_failed() {
        let registry = JobRegistry::new();
        let job = registry
            .reserve("collect_pages", Duration::from_secs(60))
            .unwrap();
        job.mark_running();
        drop(JobGuard(job.clone()));
        assert_eq!(registry.snapshot(&job.id).unwrap().state, JobState::Failed);
    }
}
