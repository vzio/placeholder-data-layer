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
pub fn anonymize(dataset: Dataset, policy: Policy) -> Result<Outcome, Error> {
    pipeline::run(dataset, policy)
}

/// Runs classification and profiling without transforming. Anomaly rates are
/// left empty until anomaly detection lands.
///
/// Takes `dataset` by reference per the public contract and never mutates
/// it: profiling only ever reads values back out, so it checks the dataset
/// in place with [`dataset::schema::check`] rather than validating a clone.
pub fn profile(dataset: &Dataset, policy: &Policy) -> Result<DatasetProfile, Error> {
    dataset::schema::check(dataset)?;
    policy.validate(dataset)?;
    Ok(profile::profile_dataset(dataset))
}
