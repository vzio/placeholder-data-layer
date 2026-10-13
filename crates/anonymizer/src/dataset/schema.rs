use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::dataset::{ColumnData, Dataset, Table, Value};
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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

/// Rejects datasets the engine cannot process: no tables at all, ragged
/// tables, duplicate or unknown names, foreign keys that point nowhere, key
/// columns of a type that cannot be minted injectively. Coerces an integer
/// literal in a float column to a float in place, since the wire format
/// cannot distinguish `2` from `2.0`.
pub fn validate(dataset: &mut Dataset) -> Result<(), Error> {
    check_structure(dataset)?;

    for table in &mut dataset.tables {
        let table_name = table.name.clone();
        for column in &mut table.columns {
            coerce_values(&table_name, column)?;
        }
    }

    check_foreign_keys(dataset)
}

/// The read-only half of [`validate`]: the same rejections, but without the
/// float coercion. A caller that only ever reads values back out — profiling
/// is blind to the difference between an integer and the float it would
/// coerce to — can check a dataset it does not own mutably, and without
/// paying for a clone just to get a `&mut` to discard.
pub fn check(dataset: &Dataset) -> Result<(), Error> {
    check_structure(dataset)?;

    for table in &dataset.tables {
        for column in &table.columns {
            check_values(&table.name, column)?;
        }
    }

    check_foreign_keys(dataset)
}

fn check_structure(dataset: &Dataset) -> Result<(), Error> {
    if dataset.tables.is_empty() {
        return Err(Error::EmptyDataset);
    }

    let mut table_names = BTreeSet::new();
    for table in &dataset.tables {
        if !table_names.insert(table.name.as_str()) {
            return Err(Error::DuplicateTable(table.name.clone()));
        }
    }

    for table in &dataset.tables {
        validate_table_shape(table)?;
    }

    Ok(())
}

fn check_foreign_keys(dataset: &Dataset) -> Result<(), Error> {
    let columns_by_name: BTreeMap<(&str, &str), &ColumnData> = dataset
        .tables
        .iter()
        .flat_map(|table| {
            table
                .columns
                .iter()
                .map(move |column| ((table.name.as_str(), column.spec.name.as_str()), column))
        })
        .collect();

    for table in &dataset.tables {
        for column in &table.columns {
            if let Some(fk) = &column.spec.fk {
                validate_foreign_key(&columns_by_name, table, column, fk)?;
            }
        }
    }

    Ok(())
}

fn validate_table_shape(table: &Table) -> Result<(), Error> {
    let mut column_names = BTreeSet::new();
    for column in &table.columns {
        if !column_names.insert(column.spec.name.as_str()) {
            return Err(Error::DuplicateColumn(column.qualified_name(&table.name)));
        }
    }

    let row_count = table.row_count();
    for column in &table.columns {
        if column.len() != row_count {
            return Err(Error::RaggedTable {
                table: table.name.clone(),
                column: column.qualified_name(&table.name),
                len: column.len(),
                row_count,
            });
        }

        if column.spec.is_key() && !column.spec.ty.is_key_admissible() {
            return Err(Error::UnsupportedKeyType {
                column: column.qualified_name(&table.name),
                ty: column.spec.ty,
            });
        }
    }

    Ok(())
}

fn coerce_values(table_name: &str, column: &mut ColumnData) -> Result<(), Error> {
    let qualified = column.qualified_name(table_name);
    let nullable = column.spec.nullable;
    let ty = column.spec.ty;

    for value in &mut column.values {
        if value.is_null() {
            if !nullable {
                return Err(Error::NullNotAllowed { column: qualified });
            }
            continue;
        }

        if ty == ColumnType::Float {
            if let Value::Int(n) = *value {
                *value = Value::Float(n as f64);
                continue;
            }
        }

        if !matches_declared_type(value, ty) {
            return Err(Error::TypeMismatch {
                column: qualified,
                declared: ty,
                found: value_kind(value),
            });
        }
    }

    Ok(())
}

fn check_values(table_name: &str, column: &ColumnData) -> Result<(), Error> {
    let qualified = column.qualified_name(table_name);
    let nullable = column.spec.nullable;
    let ty = column.spec.ty;

    for value in &column.values {
        if value.is_null() {
            if !nullable {
                return Err(Error::NullNotAllowed { column: qualified });
            }
            continue;
        }

        if ty == ColumnType::Float && matches!(value, Value::Int(_)) {
            continue;
        }

        if !matches_declared_type(value, ty) {
            return Err(Error::TypeMismatch {
                column: qualified,
                declared: ty,
                found: value_kind(value),
            });
        }
    }

    Ok(())
}

fn matches_declared_type(value: &Value, ty: ColumnType) -> bool {
    matches!(
        (value, ty),
        (Value::Bool(_), ColumnType::Boolean)
            | (Value::Int(_), ColumnType::Integer)
            | (Value::Float(_), ColumnType::Float)
            | (Value::Text(_), ColumnType::Text)
    )
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Int(_) => "integer",
        Value::Float(_) => "float",
        Value::Text(_) => "text",
    }
}

fn validate_foreign_key(
    columns_by_name: &BTreeMap<(&str, &str), &ColumnData>,
    table: &Table,
    column: &ColumnData,
    fk: &ForeignKey,
) -> Result<(), Error> {
    let qualified = column.qualified_name(&table.name);

    let target = columns_by_name
        .get(&(fk.table.as_str(), fk.column.as_str()))
        .copied()
        .ok_or_else(|| Error::DanglingForeignKey {
            column: qualified.clone(),
            target: fk.clone(),
        })?;

    if !target.spec.pk {
        return Err(Error::ForeignKeyTargetNotPrimaryKey {
            column: qualified,
            target: fk.clone(),
        });
    }

    Ok(())
}
