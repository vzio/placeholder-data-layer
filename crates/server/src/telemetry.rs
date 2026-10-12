use tracing_subscriber::EnvFilter;

use crate::config::Config;

/// Installs the process-wide subscriber. Idempotent: a second call is a no-op,
/// which keeps integration tests from fighting over the global.
pub fn init(config: &Config) {
    let filter = EnvFilter::builder().parse_lossy(&config.log_level);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
