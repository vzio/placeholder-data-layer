use std::any::Any;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use serde::Serialize;
use tokio::sync::oneshot;
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
    #[error("failed to start a worker thread: {0}")]
    SpawnFailed(#[from] std::io::Error),
}

type Job = Box<dyn FnOnce() + Send + 'static>;
type Jobs = Arc<Mutex<BTreeMap<JobId, JobSummary>>>;

/// A fixed set of OS threads pulling jobs from a shared queue. Each job runs
/// to completion on the thread that picks it up, so rayon work started from
/// inside a job runs on that same thread.
#[derive(Debug)]
pub struct WorkerPool {
    sender: Sender<Job>,
    threads: NonZeroUsize,
}

impl WorkerPool {
    /// The channel holds at most `threads` jobs: exactly the count a caller
    /// is ever allowed to have outstanding, so the queue can never grow past
    /// what the pool can eventually run.
    pub fn new(threads: NonZeroUsize) -> Result<Self, JobError> {
        let (sender, receiver) = crossbeam_channel::bounded::<Job>(threads.get());
        for index in 0..threads.get() {
            let receiver: Receiver<Job> = receiver.clone();
            thread::Builder::new()
                .name(format!("dl-worker-{index}"))
                .spawn(move || worker_loop(receiver))?;
        }
        Ok(Self { sender, threads })
    }

    pub fn threads(&self) -> NonZeroUsize {
        self.threads
    }

    /// Submits work and awaits its result. A panic inside `work` is caught on
    /// the worker thread, so it neither poisons the pool nor hangs the
    /// caller: the awaiting side observes [`JobError::WorkerLost`].
    pub async fn run<F, T>(&self, work: F) -> Result<T, JobError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel::<T>();
        let job: Job = Box::new(move || match panic::catch_unwind(AssertUnwindSafe(work)) {
            Ok(value) => {
                let _ = tx.send(value);
            }
            Err(payload) => {
                tracing::error!(panic = %panic_message(&payload), "worker job panicked");
            }
        });
        self.sender.send(job).map_err(|_| JobError::WorkerLost)?;
        rx.await.map_err(|_| JobError::WorkerLost)
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message.to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn worker_loop(receiver: Receiver<Job>) {
    for job in receiver {
        job();
    }
}

/// In-memory, request-lifetime job bookkeeping. Jobs are evicted on
/// completion; nothing is retained past the response.
#[derive(Debug)]
pub struct JobRegistry {
    jobs: Jobs,
    pool: WorkerPool,
    capacity: NonZeroUsize,
}

impl JobRegistry {
    pub fn new(worker_pool_size: NonZeroUsize) -> Result<Self, JobError> {
        Ok(Self {
            jobs: Arc::new(Mutex::new(BTreeMap::new())),
            pool: WorkerPool::new(worker_pool_size)?,
            capacity: worker_pool_size,
        })
    }

    pub fn pool(&self) -> &WorkerPool {
        &self.pool
    }

    pub fn list(&self) -> Vec<JobSummary> {
        lock(&self.jobs).values().cloned().collect()
    }

    /// Admits a job and runs it on the worker pool. The registry entry
    /// belongs to the work itself and is removed when the work finishes or
    /// panics: dropping the returned future — a client disconnect, a
    /// timeout — never evicts an entry whose job is still queued or running.
    pub async fn submit<F, T>(&self, kind: JobKind, rows: usize, work: F) -> Result<T, JobError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let id = JobId(Uuid::new_v4());
        {
            let mut jobs = lock(&self.jobs);
            if jobs.len() >= self.capacity.get() {
                return Err(JobError::Saturated);
            }
            jobs.insert(
                id,
                JobSummary {
                    id,
                    kind,
                    state: JobState::Queued,
                    rows,
                },
            );
        }

        let jobs = Arc::clone(&self.jobs);
        self.pool
            .run(move || {
                set_state(&jobs, id, JobState::Running);
                let _eviction = EvictOnDrop {
                    jobs: Arc::clone(&jobs),
                    id,
                };
                let value = work();
                set_state(&jobs, id, JobState::Succeeded);
                value
            })
            .await
    }
}

fn lock(jobs: &Mutex<BTreeMap<JobId, JobSummary>>) -> MutexGuard<'_, BTreeMap<JobId, JobSummary>> {
    jobs.lock().unwrap_or_else(PoisonError::into_inner)
}

fn set_state(jobs: &Mutex<BTreeMap<JobId, JobSummary>>, id: JobId, state: JobState) {
    if let Some(summary) = lock(jobs).get_mut(&id) {
        summary.state = state;
    }
}

struct EvictOnDrop {
    jobs: Jobs,
    id: JobId,
}

impl Drop for EvictOnDrop {
    fn drop(&mut self) {
        let mut jobs = lock(&self.jobs);
        if thread::panicking() {
            if let Some(summary) = jobs.get_mut(&self.id) {
                summary.state = JobState::Failed;
            }
        }
        jobs.remove(&self.id);
    }
}
