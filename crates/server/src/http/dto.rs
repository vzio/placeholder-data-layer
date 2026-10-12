use std::collections::BTreeMap;

use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, Dataset, ForeignKey};
use anonymizer::transform::Strategy;
use anonymizer::{DatasetProfile, FidelityReport, Policy, Table, Value};
use serde::{Deserialize, Serialize};

use crate::http::error::ApiError;

/// Wire types are row-major because that is how dumps are shaped; the engine
/// is column-major because that is how it works. The pivot happens here, at
/// the edge, and nowhere else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetDto {
    pub tables: Vec<TableDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableDto {
    pub name: String,
    pub columns: Vec<ColumnDto>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// The resulting [`Dataset`] still has to pass [`anonymizer::dataset::schema::validate`];
    /// this only establishes the shape that validation checks.
    pub fn into_dataset(self) -> Result<Dataset, ApiError> {
        let mut tables = Vec::with_capacity(self.tables.len());

        for TableDto {
            name,
            columns,
            rows,
        } in self.tables
        {
            let declared_width = columns.len();
            let mut data: Vec<ColumnData> = columns
                .into_iter()
                .map(|column| ColumnData {
                    spec: ColumnSpec {
                        name: column.name,
                        ty: column.ty.into(),
                        nullable: column.nullable,
                        pk: column.pk,
                        fk: column.fk,
                    },
                    values: Vec::with_capacity(rows.len()),
                })
                .collect();

            for (row_index, row) in rows.into_iter().enumerate() {
                if row.len() != declared_width {
                    return Err(anonymizer::Error::RaggedRow {
                        table: name.clone(),
                        row: row_index,
                        len: row.len(),
                        expected: declared_width,
                    }
                    .into());
                }
                for (column, value) in data.iter_mut().zip(row) {
                    column.values.push(value);
                }
            }

            tables.push(Table {
                name,
                columns: data,
            });
        }

        let mut dataset = Dataset { tables };
        anonymizer::dataset::schema::validate(&mut dataset)?;
        Ok(dataset)
    }

    /// Column-major back to row-major, preserving column and row order.
    ///
    /// Every column of a table is expected to carry the same number of
    /// values; a table where it does not is rejected rather than silently
    /// truncated to the shortest column.
    pub fn from_dataset(dataset: &Dataset) -> Result<Self, ApiError> {
        let mut tables = Vec::with_capacity(dataset.tables.len());

        for table in &dataset.tables {
            let columns = table
                .columns
                .iter()
                .map(|column| ColumnDto {
                    name: column.spec.name.clone(),
                    ty: column.spec.ty.into(),
                    nullable: column.spec.nullable,
                    pk: column.spec.pk,
                    fk: column.spec.fk.clone(),
                })
                .collect();

            let width = table.columns.iter().map(ColumnData::len).max().unwrap_or(0);
            for column in &table.columns {
                if column.len() != width {
                    return Err(anonymizer::Error::RaggedTable {
                        table: table.name.clone(),
                        column: column.qualified_name(&table.name),
                        len: column.len(),
                        row_count: width,
                    }
                    .into());
                }
            }

            let mut rows = vec![Vec::with_capacity(table.columns.len()); width];
            for column in &table.columns {
                for (row, value) in rows.iter_mut().zip(&column.values) {
                    row.push(value.clone());
                }
            }

            tables.push(TableDto {
                name: table.name.clone(),
                columns,
                rows,
            });
        }

        Ok(DatasetDto { tables })
    }
}

impl OptionsDto {
    pub fn into_policy(self) -> Result<Policy, ApiError> {
        let mut column_overrides = BTreeMap::new();
        for (column, strategy_name) in self.column_overrides {
            let strategy = parse_strategy(&strategy_name).ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "override for `{column}` names an unknown strategy `{strategy_name}`"
                ))
            })?;
            column_overrides.insert(column, strategy);
        }

        Ok(Policy {
            seed: self.seed,
            column_overrides,
        })
    }
}

fn parse_strategy(name: &str) -> Option<Strategy> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}
