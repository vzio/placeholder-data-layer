use serde::Serialize;

use crate::dataset::Dataset;
use crate::profile::DatasetProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Enforced. No output value of a high-cardinality direct-identifier
    /// column appears in that column's source value set.
    Leakage,
    /// Enforced. The output orphan set is identical to the input's, not empty:
    /// dangling foreign keys are a production bug that must survive.
    Referential,
    /// Reported only. Forcing k would distort the distribution the engine
    /// exists to preserve.
    Linkage,
}

/// Serialises to the `checks` entries of the report.
#[derive(Debug, Clone, Serialize)]
pub struct CheckOutcome {
    pub check: Check,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphans_in: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphans_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k_observed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Enforced checks only. A failure here drives a retry at the next
/// [`crate::transform::Strength`].
pub fn enforced(
    _before: &Dataset,
    _after: &Dataset,
    _profile: &DatasetProfile,
) -> Vec<CheckOutcome> {
    todo!("P5: leakage and referential checks")
}

/// Observed minimum equivalence-class size over the quasi-identifier set.
pub fn observe_linkage(_after: &Dataset, _profile: &DatasetProfile) -> CheckOutcome {
    todo!("P5: k_observed")
}

/// How close an attempt came to passing, used to pick the best attempt when no
/// attempt passes outright.
pub fn score(_outcomes: &[CheckOutcome]) -> u64 {
    todo!("P5: attempt scoring")
}
