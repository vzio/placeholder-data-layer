pub mod schema;
pub mod value;

use serde::Serialize;

pub use schema::{ColumnSpec, ColumnType, ForeignKey, TableSchema};
pub use value::{KeyValue, Value};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Dataset {
    pub tables: Vec<Table>,
}

impl Dataset {
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|table| table.name == name)
    }

    /// Every column paired with its `table.column` qualified name, flattened
    /// once so every per-column rayon pass walks the same list and the same
    /// order-independent unit of work.
    pub fn qualified_columns(&self) -> Vec<(String, &ColumnData)> {
        self.tables
            .iter()
            .flat_map(|table| {
                table
                    .columns
                    .iter()
                    .map(move |column| (column.qualified_name(&table.name), column))
            })
            .collect()
    }
}

/// Column-major: every pipeline stage works a column at a time, and rayon
/// parallelises over columns.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Table {
    pub name: String,
    pub columns: Vec<ColumnData>,
}

impl Table {
    pub fn column(&self, name: &str) -> Option<&ColumnData> {
        self.columns.iter().find(|column| column.spec.name == name)
    }

    pub fn row_count(&self) -> usize {
        self.columns.first().map_or(0, ColumnData::len)
    }

    pub fn schema(&self) -> TableSchema {
        TableSchema {
            name: self.name.clone(),
            columns: self.columns.iter().map(|c| c.spec.clone()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColumnData {
    pub spec: ColumnSpec,
    pub values: Vec<Value>,
}

impl ColumnData {
    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// `table.column`, the identifier used by policy overrides and the report.
    pub fn qualified_name(&self, table: &str) -> String {
        format!("{table}.{}", self.spec.name)
    }
}
