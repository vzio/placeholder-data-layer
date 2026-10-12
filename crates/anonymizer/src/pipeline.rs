use serde::Serialize;

use crate::Outcome;
use crate::dataset::Dataset;
use crate::error::Error;
use crate::policy::Policy;

/// Pipeline stages, in order. Also the key type of the report's timings map,
/// whose ordering follows this declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Classify,
    Profile,
    Transform,
    Compliance,
}

/// Spec steps 2 to 6: classify, profile, transform, check, retry, report.
/// Stages return their timings rather than mutating shared state, which is
/// what lets the context be `&mut` while building and `&` while transforming.
pub fn run(_dataset: Dataset, _policy: Policy) -> Result<Outcome, Error> {
    todo!("P1-P5: stage sequencing and the retry loop")
}
