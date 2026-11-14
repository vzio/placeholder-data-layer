//! Builds wire-format tables row by row.

use anonymizer::Value;
use anonymizer::dataset::ForeignKey;
use server::http::dto::{ColumnDto, TableDto, WireType};

use super::GenError;

pub struct TableBuilder {
    name: String,
    columns: Vec<ColumnDto>,
    rows: Vec<Vec<Value>>,
}

impl TableBuilder {
    pub fn new(name: &str) -> TableBuilder {
        TableBuilder {
            name: name.to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
        }
    }

    fn with(mut self, name: &str, ty: WireType, pk: bool, fk: Option<ForeignKey>) -> Self {
        self.columns.push(ColumnDto {
            name: name.to_owned(),
            ty,
            nullable: !pk,
            pk,
            fk,
        });
        self
    }

    /// A nullable column.
    pub fn column(self, name: &str, ty: WireType) -> Self {
        self.with(name, ty, false, None)
    }

    pub fn primary_key(self, name: &str, ty: WireType) -> Self {
        self.with(name, ty, true, None)
    }

    /// A nullable foreign key onto `table.column`.
    pub fn foreign_key(self, name: &str, ty: WireType, table: &str, column: &str) -> Self {
        let target = ForeignKey {
            table: table.to_owned(),
            column: column.to_owned(),
        };
        self.with(name, ty, false, Some(target))
    }

    pub fn push(&mut self, row: Vec<Value>) -> Result<(), GenError> {
        if row.len() != self.columns.len() {
            return Err(GenError(format!(
                "table `{}` row {} has {} values for {} columns",
                self.name,
                self.rows.len(),
                row.len(),
                self.columns.len()
            )));
        }
        self.rows.push(row);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn build(self) -> TableDto {
        TableDto {
            name: self.name,
            columns: self.columns,
            rows: self.rows,
        }
    }
}

pub fn text(value: impl Into<String>) -> Value {
    Value::Text(value.into())
}

pub fn maybe_text(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::Text)
}
