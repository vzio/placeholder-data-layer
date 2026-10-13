use std::collections::BTreeMap;

use serde::Serialize;

use crate::anomaly::AnomalyKind;
use crate::classify::SemanticType;
use crate::compliance::{CheckOutcome, Verdict};
use crate::pipeline::Stage;
use crate::preserve::PreserveReason;
use crate::transform::{Strategy, Strength};

/// The compliance artifact returned alongside the anonymized dataset.
///
/// Every map here is a [`BTreeMap`]: output ordering is part of the
/// determinism contract. The report never contains raw source values, and
/// never the run seed.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
    /// The [`Strength`] of the attempt the returned dataset actually came
    /// from. "Anonymized at `maximum` after two failed attempts" and
    /// "passed on the first try at `default`" are materially different
    /// claims for anyone defending this artifact, and neither is
    /// recoverable from the rest of the report without this.
    pub accepted_strength: Strength,
    /// How many escalation attempts the retry loop made before accepting
    /// this outcome, including the first. `1` means the first attempt
    /// already passed every enforced check.
    pub attempts: usize,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct AnomalyRates {
    pub before: f64,
    pub after: f64,
}
