use std::collections::BTreeMap;
use std::time::Instant;

use serde::Serialize;

use crate::Outcome;
use crate::compliance::Verdict;
use crate::context::RunContext;
use crate::dataset::{Dataset, schema};
use crate::error::Error;
use crate::policy::Policy;
use crate::report::FidelityReport;
use crate::transform::ledger::EquivalenceClasses;

/// Pipeline stages, in order. Also the key type of the report's timings map,
/// whose ordering follows this declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Validate,
    Classify,
    Profile,
    Transform,
    Compliance,
}

/// Validates the dataset and policy, then runs spec steps 2 to 6: classify,
/// profile, transform, check, retry, report. Stages return their timings
/// rather than mutating shared state, which is what lets the context be
/// `&mut` while building and `&` while transforming.
// TODO(P2): classify and profile the dataset.
// TODO(P4): apply transform strategies through the ledger.
// TODO(P5): run compliance checks and the retry loop.
pub fn run(mut dataset: Dataset, policy: Policy) -> Result<Outcome, Error> {
    let started = Instant::now();
    schema::validate(&mut dataset)?;
    policy.validate(&dataset)?;
    let _classes = EquivalenceClasses::build(&dataset)?;
    let _context = RunContext::new(policy);
    let validate_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let mut timings_ms = BTreeMap::new();
    timings_ms.insert(Stage::Validate, validate_ms);
    timings_ms.insert(Stage::Classify, 0);
    timings_ms.insert(Stage::Profile, 0);
    timings_ms.insert(Stage::Transform, 0);
    timings_ms.insert(Stage::Compliance, 0);

    let report = FidelityReport {
        verdict: Verdict::Pass,
        columns: Vec::new(),
        checks: Vec::new(),
        timings_ms,
    };

    Ok(Outcome { dataset, report })
}
