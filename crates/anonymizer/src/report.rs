use std::collections::BTreeMap;

use serde::Serialize;

use crate::anomaly::AnomalyKind;
use crate::classify::SemanticType;
use crate::compliance::{CheckOutcome, Verdict};
use crate::pipeline::Stage;
use crate::preserve::PreserveReason;
use crate::transform::Strategy;

/// The compliance artifact returned alongside the anonymized dataset.
///
/// Every map here is a [`BTreeMap`]: output ordering is part of the
/// determinism contract. The report never contains raw source values, and
/// never the run seed.
#[derive(Debug, Clone, Serialize)]
pub struct FidelityReport {
    pub verdict: Verdict,
    pub columns: Vec<ColumnReport>,
    pub checks: Vec<CheckOutcome>,
    /// Every column [`Strategy::Keep`] ran under because of
    /// [`crate::policy::Policy::preserve`], marked as an explicit request or
    /// as class propagation — the first thing an audit of this report has
    /// to answer is what was *not* anonymized.
    pub preserved_columns: BTreeMap<String, PreserveReason>,
    pub timings_ms: BTreeMap<Stage, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnReport {
    /// `table.column`.
    pub column: String,
    pub semantic_type: SemanticType,
    /// See [`crate::classify::Classification::confidence`]: a match rate,
    /// not a calibrated probability.
    pub confidence: f64,
    pub strategy: Strategy,
    /// Domain shares before and after. Expected to be identical: value-local
    /// transforms make fidelity an invariant, so a non-zero distance is a bug
    /// signal rather than a tolerance to tune.
    pub distribution_before: BTreeMap<String, f64>,
    pub distribution_after: BTreeMap<String, f64>,
    pub total_variation_distance: f64,
    pub anomalies: BTreeMap<AnomalyKind, AnomalyRates>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AnomalyRates {
    pub before: f64,
    pub after: f64,
}
