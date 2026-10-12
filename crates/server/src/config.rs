use std::env;
use std::fmt::Display;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::str::FromStr;
use std::thread;

/// Every variable this service reads is prefixed with this.
pub const ENV_PREFIX: &str = "DL_";

const DEFAULT_BIND: &str = "0.0.0.0:8080";
const DEFAULT_BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_LOG_LEVEL: &str = "info";

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub body_limit_bytes: usize,
    /// `None` leaves rayon to size its own global pool.
    pub rayon_threads: Option<NonZeroUsize>,
    pub worker_pool_size: NonZeroUsize,
    pub log_level: String,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            bind: parse_or("BIND_ADDR", || {
                SocketAddr::from_str(DEFAULT_BIND).unwrap_or(SocketAddr::from(([0, 0, 0, 0], 8080)))
            })?,
            body_limit_bytes: parse_or("BODY_LIMIT_BYTES", || DEFAULT_BODY_LIMIT_BYTES)?,
            rayon_threads: parse_opt("RAYON_THREADS")?,
            worker_pool_size: parse_or("WORKER_THREADS", default_worker_pool_size)?,
            log_level: read("LOG_LEVEL").unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_owned()),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{ENV_PREFIX}{key}=`{value}` is not valid: {reason}")]
    Invalid {
        key: &'static str,
        value: String,
        reason: String,
    },
}

fn default_worker_pool_size() -> NonZeroUsize {
    thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

fn read(key: &str) -> Option<String> {
    env::var(format!("{ENV_PREFIX}{key}"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_opt<T>(key: &'static str) -> Result<Option<T>, ConfigError>
where
    T: FromStr,
    T::Err: Display,
{
    read(key)
        .map(|value| {
            value
                .parse()
                .map_err(|reason: T::Err| ConfigError::Invalid {
                    key,
                    value,
                    reason: reason.to_string(),
                })
        })
        .transpose()
}

fn parse_or<T>(key: &'static str, default: impl FnOnce() -> T) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: Display,
{
    Ok(parse_opt(key)?.unwrap_or_else(default))
}
