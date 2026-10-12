use serde::{Deserialize, Serialize};

use crate::dataset::Dataset;
use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    Boolean,
    Integer,
    Float,
    Text,
}

impl ColumnType {
    /// Whether a column of this type may carry a primary or foreign key.
    pub fn is_key_admissible(self) -> bool {
        matches!(self, ColumnType::Integer | ColumnType::Text)
    }
}

/// A foreign key edge, always pointing at a single column of another table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ForeignKey {
    pub table: String,
    pub column: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub name: String,
    pub ty: ColumnType,
    pub nullable: bool,
    pub pk: bool,
    pub fk: Option<ForeignKey>,
}

impl ColumnSpec {
    pub fn is_key(&self) -> bool {
        self.pk || self.fk.is_some()
    }
}

/// The shape of one table, independent of its data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnSpec>,
}

impl TableSchema {
    pub fn column(&self, name: &str) -> Option<&ColumnSpec> {
        self.columns.iter().find(|spec| spec.name == name)
    }
}

/// Rejects datasets the engine cannot process: ragged tables, duplicate or
/// unknown names, foreign keys that point nowhere, key columns of a type that
/// cannot be minted injectively.
pub fn validate(_dataset: &Dataset) -> Result<(), Error> {
    todo!("P1: schema validation")
}
