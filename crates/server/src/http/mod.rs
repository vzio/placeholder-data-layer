pub mod dto;
pub mod error;
pub mod router;
pub mod routes;

use std::sync::Arc;

use crate::config::Config;
use crate::job::JobRegistry;

pub use router::router;

/// Shared, read-only-ish handles. No request state lives here: a run's context
/// is owned by its request and dropped with the response.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub jobs: Arc<JobRegistry>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let jobs = Arc::new(JobRegistry::new(config.worker_pool_size));
        Self {
            config: Arc::new(config),
            jobs,
        }
    }
}
