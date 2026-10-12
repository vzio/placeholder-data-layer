use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tokio::sync::Semaphore;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct JobId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Profile,
    Anonymize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

/// What `GET /v1/jobs` exposes. Never carries dataset contents.
#[derive(Debug, Clone, Serialize)]
pub struct JobSummary {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
    pub rows: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("the worker pool is saturated")]
    Saturated,
    #[error("the worker terminated unexpectedly")]
    WorkerLost,
}

/// Bounded admission control. Work runs on `spawn_blocking` so the HTTP
/// reactor stays free while rayon owns the CPU.
#[derive(Debug)]
pub struct WorkerPool {
    permits: Arc<Semaphore>,
    capacity: NonZeroUsize,
}

impl WorkerPool {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(capacity.get())),
            capacity,
        }
    }

    pub fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    pub fn available(&self) -> usize {
        self.permits.available_permits()
    }

    /// Non-blocking admission: a full pool is a `429`, not a queue.
    pub async fn run<F, T>(&self, _work: F) -> Result<T, JobError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        todo!("P1: try_acquire_owned, then spawn_blocking")
    }
}

/// In-memory, request-lifetime job bookkeeping. Jobs are evicted on
/// completion; nothing is retained past the response.
#[derive(Debug)]
pub struct JobRegistry {
    jobs: Mutex<BTreeMap<JobId, JobSummary>>,
    pool: WorkerPool,
}

impl JobRegistry {
    pub fn new(worker_pool_size: NonZeroUsize) -> Self {
        Self {
            jobs: Mutex::new(BTreeMap::new()),
            pool: WorkerPool::new(worker_pool_size),
        }
    }

    pub fn pool(&self) -> &WorkerPool {
        &self.pool
    }

    pub fn list(&self) -> Vec<JobSummary> {
        self.jobs().values().cloned().collect()
    }

    /// Admits a job, runs it to completion, and evicts it.
    pub async fn submit<F, T>(&self, _kind: JobKind, _rows: usize, _work: F) -> Result<T, JobError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        todo!("P1: register, run on the pool, evict")
    }

    /// Poisoning carries no invariant here: the map is plain bookkeeping.
    fn jobs(&self) -> MutexGuard<'_, BTreeMap<JobId, JobSummary>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
