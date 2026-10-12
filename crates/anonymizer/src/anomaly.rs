use std::collections::BTreeMap;

use rand_chacha::ChaCha8Rng;
use serde::Serialize;

use crate::classify::SemanticType;
use crate::dataset::{ColumnData, Value};

/// Classes of production corruption the engine can both detect and synthesise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    MalformedEmail,
    MalformedDocument,
    MalformedPhone,
    OutOfRange,
    ImpossibleDate,
    WhitespacePadding,
    EncodingArtifact,
    TruncatedText,
    UnexpectedNull,
}

impl AnomalyKind {
    /// Manufactures this class of corruption on an already-anonymized value.
    /// Detected exemplars are production PII and are never replayed.
    pub fn corrupt(self, _value: Value, _rng: &mut ChaCha8Rng) -> Value {
        todo!("P3: synthesis per kind")
    }
}

/// Positions, per kind, of the anomalies found in one column.
#[derive(Debug, Clone, Default)]
pub struct AnomalyScan {
    pub positions: BTreeMap<AnomalyKind, Vec<usize>>,
}

impl AnomalyScan {
    pub fn counts(&self) -> BTreeMap<AnomalyKind, usize> {
        self.positions.iter().map(|(k, v)| (*k, v.len())).collect()
    }
}

pub fn detect(_column: &ColumnData, _semantic_type: SemanticType) -> AnomalyScan {
    todo!("P3: detection")
}

/// Re-injects each kind at exactly `floor(rate * n)` positions, chosen by a
/// seeded shuffle, so input and output rates match exactly.
pub fn reinject(
    _values: &mut [Value],
    _scan: &AnomalyScan,
    _rng: &mut ChaCha8Rng,
) -> BTreeMap<AnomalyKind, usize> {
    todo!("P3: exact-count injection")
}
