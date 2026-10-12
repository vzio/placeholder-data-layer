use serde::{Deserialize, Serialize};

use crate::dataset::Value;

/// What a column means, as far as this build can tell. Enum dispatch: adding a
/// detector is a variant plus an arm, never a registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticType {
    Email,
    Phone,
    Cep,
    Cpf,
    Cnpj,
    FullName,
    Numeric,
    Date,
    FreeText,
    PrimaryKey,
    ForeignKey,
    Unknown,
}

impl SemanticType {
    /// Backs `GET /v1/detectors`.
    pub const ALL: &'static [SemanticType] = &[
        SemanticType::Email,
        SemanticType::Phone,
        SemanticType::Cep,
        SemanticType::Cpf,
        SemanticType::Cnpj,
        SemanticType::FullName,
        SemanticType::Numeric,
        SemanticType::Date,
        SemanticType::FreeText,
        SemanticType::PrimaryKey,
        SemanticType::ForeignKey,
        SemanticType::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        todo!("P2: stable wire name per variant")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub semantic_type: SemanticType,
    pub confidence: f64,
}

/// Column name heuristics first, then value-shape evidence from the sample.
pub fn classify(_name: &str, _sample: &[Value]) -> Classification {
    todo!("P2: classification")
}
