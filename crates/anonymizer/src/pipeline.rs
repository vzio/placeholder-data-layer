use std::collections::BTreeMap;
use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;

use crate::Outcome;
use crate::classify::{self, Classification, SemanticType};
use crate::compliance::Verdict;
use crate::context::RunContext;
use crate::dataset::{ColumnData, Dataset, schema};
use crate::error::Error;
use crate::policy::Policy;
use crate::profile::{self, ColumnProfile};
use crate::report::{ColumnReport, FidelityReport};
use crate::transform::Strategy;
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
// TODO(P4): apply transform strategies through the ledger.
// TODO(P5): run compliance checks and the retry loop.
pub fn run(mut dataset: Dataset, policy: Policy) -> Result<Outcome, Error> {
    let ((_classes, _context), validate_ms) = timed_stage(Stage::Validate, || {
        schema::validate(&mut dataset)?;
        policy.validate(&dataset)?;
        let classes = EquivalenceClasses::build(&dataset)?;
        Ok((classes, RunContext::new(policy)))
    })?;

    let flat = dataset.qualified_columns();

    let (classifications, classify_ms) = timed_stage(Stage::Classify, || {
        Ok::<_, Error>(
            flat.par_iter()
                .map(|(name, column)| {
                    (
                        name.clone(),
                        classify::classify(&column.spec.name, &column.values),
                    )
                })
                .collect::<BTreeMap<String, Classification>>(),
        )
    })?;

    let (profiles, profile_ms) = timed_stage(Stage::Profile, || {
        Ok::<_, Error>(
            flat.par_iter()
                .map(|(name, column)| {
                    let classification = classification_of(&classifications, name);
                    (
                        name.clone(),
                        profile::profile_column(name, column, classification),
                    )
                })
                .collect::<BTreeMap<String, ColumnProfile>>(),
        )
    })?;

    let ((), transform_ms) = timed_stage(Stage::Transform, || Ok::<_, Error>(()))?;
    let ((), compliance_ms) = timed_stage(Stage::Compliance, || Ok::<_, Error>(()))?;

    let columns = build_column_reports(&flat, &classifications, &profiles);

    let mut timings_ms = BTreeMap::new();
    timings_ms.insert(Stage::Validate, validate_ms);
    timings_ms.insert(Stage::Classify, classify_ms);
    timings_ms.insert(Stage::Profile, profile_ms);
    timings_ms.insert(Stage::Transform, transform_ms);
    timings_ms.insert(Stage::Compliance, compliance_ms);

    let report = FidelityReport {
        verdict: Verdict::Pass,
        columns,
        checks: Vec::new(),
        timings_ms,
    };

    Ok(Outcome { dataset, report })
}

fn classification_of(
    classifications: &BTreeMap<String, Classification>,
    name: &str,
) -> Classification {
    classifications
        .get(name)
        .copied()
        .unwrap_or(Classification {
            semantic_type: SemanticType::Unknown,
            confidence: 0.0,
        })
}

/// No transform has run yet, so every column is reported as kept and the
/// before/after distributions are identical by construction.
fn build_column_reports(
    flat: &[(String, &ColumnData)],
    classifications: &BTreeMap<String, Classification>,
    profiles: &BTreeMap<String, ColumnProfile>,
) -> Vec<ColumnReport> {
    flat.iter()
        .map(|(name, _)| {
            let classification = classification_of(classifications, name);
            let shares = profiles
                .get(name)
                .map_or_else(BTreeMap::new, |profile| profile.distribution.shares());

            ColumnReport {
                column: name.clone(),
                semantic_type: classification.semantic_type,
                confidence: f64::from(classification.confidence),
                strategy: Strategy::Keep,
                distribution_before: shares.clone(),
                distribution_after: shares,
                total_variation_distance: 0.0,
                anomalies: BTreeMap::new(),
            }
        })
        .collect()
}

/// Runs `f` inside a span named after `stage` and returns its result
/// alongside the wall-clock time it took, in milliseconds.
fn timed_stage<T>(stage: Stage, f: impl FnOnce() -> Result<T, Error>) -> Result<(T, u64), Error> {
    let span = tracing::info_span!("pipeline_stage", stage = ?stage);
    let _entered = span.enter();
    let started = Instant::now();
    let value = f()?;
    let elapsed = elapsed_ms(started);
    tracing::debug!(elapsed_ms = elapsed, "stage finished");
    Ok((value, elapsed))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
