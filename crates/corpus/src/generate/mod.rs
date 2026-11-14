//! Deterministic synthetic dataset generation, independent of the engine.
//!
//! A generator is a pure function of its seed and parameters. It draws from
//! one [`rng::Rng`] in a fixed order on one thread, so the same inputs yield
//! the same bytes on every machine, and a committed digest pins them.

pub mod company;
pub mod docs;
pub mod mess;
pub mod person;
pub mod rng;
pub mod table;
pub mod vocab;

use serde::de::DeserializeOwned;
use server::http::dto::DatasetDto;

/// Why a generator could not produce its dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenError(pub String);

impl std::fmt::Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GenError {}

pub type GeneratorFn = fn(seed: u64, params: &serde_json::Value) -> Result<DatasetDto, GenError>;

/// Reads a generator's own parameter struct, which rejects unknown fields.
pub fn params<T: DeserializeOwned>(value: &serde_json::Value) -> Result<T, GenError> {
    serde_json::from_value(value.clone()).map_err(|e| GenError(format!("parameters: {e}")))
}

/// The generator registered under `name` by any pack.
pub fn generator(name: &str) -> Option<GeneratorFn> {
    crate::packs::seed::generator(name)
        .or_else(|| crate::packs::commerce::generator(name))
        .or_else(|| crate::packs::people::generator(name))
        .or_else(|| crate::packs::legacy::generator(name))
}
