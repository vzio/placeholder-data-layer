//! Deterministic, I/O-free anonymization engine.
//!
//! The supported surface is [`anonymize`] and [`profile`]; everything reachable
//! below them is an implementation detail that may change without notice.
//!
//! Determinism is a contract: a pinned [`Policy::seed`] must reproduce
//! byte-identical output under any thread count.

#![forbid(unsafe_code)]

pub mod anomaly;
pub mod classify;
pub mod compliance;
pub mod context;
pub mod dataset;
pub mod error;
pub mod pipeline;
pub mod policy;
pub mod profile;
pub mod report;
pub mod transform;

pub use dataset::{ColumnData, Dataset, Table, Value};
pub use error::Error;
pub use policy::Policy;
pub use profile::DatasetProfile;
pub use report::FidelityReport;

/// An anonymized dataset and the compliance report describing how it was produced.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub dataset: Dataset,
    pub report: FidelityReport,
}

/// Runs the full pipeline: validate, classify, profile, transform, check, report.
pub fn anonymize(_dataset: Dataset, _policy: Policy) -> Result<Outcome, Error> {
    todo!("P1-P5: see pipeline::run")
}

/// Runs classification, profiling and anomaly detection without transforming.
pub fn profile(_dataset: &Dataset, _policy: &Policy) -> Result<DatasetProfile, Error> {
    todo!("P2: classification and profiling")
}
