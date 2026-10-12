use std::collections::BTreeMap;

use serde::Serialize;

use crate::anomaly::AnomalyKind;
use crate::classify::SemanticType;
use crate::dataset::ColumnData;

/// Output of `POST /v1/profile`: steps 2-3 with no transformation.
#[derive(Debug, Clone, Serialize)]
pub struct DatasetProfile {
    /// Keyed by `table.column`.
    pub columns: BTreeMap<String, ColumnProfile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnProfile {
    pub column: String,
    pub semantic_type: SemanticType,
    pub confidence: f64,
    pub null_rate: f64,
    pub distinct: usize,
    pub distribution: Distribution,
    pub anomaly_rates: BTreeMap<AnomalyKind, f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Distribution {
    Categorical(CategoricalProfile),
    Numeric(NumericProfile),
    /// Too high-cardinality or too unstructured to summarise usefully.
    Unsummarised,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoricalProfile {
    /// Value share, summing to 1.0. Domain keys only; never raw PII rows.
    pub histogram: BTreeMap<String, f64>,
    pub cardinality: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct NumericProfile {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub stddev: f64,
}

pub fn profile_column(_column: &ColumnData, _semantic_type: SemanticType) -> ColumnProfile {
    todo!("P2: profiling")
}

/// Total variation distance between two shares-of-domain histograms.
pub fn total_variation_distance(
    _before: &BTreeMap<String, f64>,
    _after: &BTreeMap<String, f64>,
) -> f64 {
    todo!("P2: TVD")
}
