pub mod schema;
pub mod value;

pub use schema::{ColumnSpec, ColumnType, ForeignKey, TableSchema};
pub use value::{KeyValue, Value};

#[derive(Debug, Clone, Default)]
pub struct Dataset {
    pub tables: Vec<Table>,
}

impl Dataset {
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|table| table.name == name)
    }
}

/// Column-major: every pipeline stage works a column at a time, and rayon
/// parallelises over columns.
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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
