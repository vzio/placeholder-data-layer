use std::collections::BTreeMap;

use anonymizer::dataset::{ColumnType, Dataset, ForeignKey};
use anonymizer::{DatasetProfile, FidelityReport, Policy, Value};
use serde::{Deserialize, Serialize};

use crate::http::error::ApiError;

/// Wire types are row-major because that is how dumps are shaped; the engine
/// is column-major because that is how it works. The pivot happens here, at
/// the edge, and nowhere else.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetDto {
    pub tables: Vec<TableDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDto {
    pub name: String,
    pub columns: Vec<ColumnDto>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDto {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: WireType,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub pk: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fk: Option<ForeignKey>,
}

/// The wire spelling of [`ColumnType`]: callers say `string`, the engine says
/// `Text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireType {
    Boolean,
    Integer,
    Float,
    String,
}

impl From<WireType> for ColumnType {
    fn from(ty: WireType) -> Self {
        match ty {
            WireType::Boolean => ColumnType::Boolean,
            WireType::Integer => ColumnType::Integer,
            WireType::Float => ColumnType::Float,
            WireType::String => ColumnType::Text,
        }
    }
}

impl From<ColumnType> for WireType {
    fn from(ty: ColumnType) -> Self {
        match ty {
            ColumnType::Boolean => WireType::Boolean,
            ColumnType::Integer => WireType::Integer,
            ColumnType::Float => WireType::Float,
            ColumnType::Text => WireType::String,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OptionsDto {
    #[serde(default)]
    pub seed: Option<u64>,
    /// Keyed by `table.column`; the value is a strategy name.
    #[serde(default)]
    pub column_overrides: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnonymizeRequest {
    pub dataset: DatasetDto,
    #[serde(default)]
    pub options: OptionsDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnonymizeResponse {
    pub run_id: String,
    pub dataset: DatasetDto,
    pub report: FidelityReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileResponse {
    pub run_id: String,
    pub profile: DatasetProfile,
}

impl DatasetDto {
    /// Row-major to column-major, rejecting ragged rows and unknown types.
    pub fn into_dataset(self) -> Result<Dataset, ApiError> {
        todo!("P1: pivot and validate")
    }

    /// Column-major back to row-major, preserving column and row order.
    pub fn from_dataset(_dataset: &Dataset) -> Self {
        todo!("P1: pivot back")
    }
}

impl OptionsDto {
    pub fn into_policy(self) -> Result<Policy, ApiError> {
        todo!("P1: strategy name resolution and override validation")
    }
}
