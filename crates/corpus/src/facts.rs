//! Engine-free statements about a scenario's source dataset. They pin what
//! the fixture holds, so an expectation's numbers are traceable to the data
//! and a generator change that moves them fails loudly.

use std::collections::BTreeSet;

use anonymizer::Dataset;
use serde::Deserialize;

use crate::expect::{column_of, orphan_positions, text_of};
use crate::generate::docs::Document;
use crate::shapes::ShapeRef;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Fact {
    RowCount {
        table: String,
        equals: usize,
    },
    NullCount {
        column: String,
        equals: usize,
    },
    /// Cells whose whole value is the shape.
    ShapeCount {
        column: String,
        shape: ShapeRef,
        equals: usize,
    },
    /// Cells holding the shape somewhere inside.
    ContainsCount {
        column: String,
        shape: ShapeRef,
        equals: usize,
    },
    ValidDocumentCount {
        column: String,
        document: Document,
        equals: usize,
    },
    OrphanCount {
        column: String,
        equals: usize,
    },
    DistinctCount {
        column: String,
        equals: usize,
    },
}

impl Fact {
    /// The observed count, or why it cannot be counted.
    pub fn observe(&self, dataset: &Dataset) -> Result<usize, String> {
        let column = |name: &str| column_of(dataset, name).map_err(|finding| finding.describe());
        let texts = |name: &str| -> Result<Vec<String>, String> {
            Ok(column(name)?.values.iter().filter_map(text_of).collect())
        };
        match self {
            Fact::RowCount { table, .. } => dataset
                .table(table)
                .map(|t| t.row_count())
                .ok_or_else(|| format!("no table `{table}`")),
            Fact::NullCount { column: name, .. } => {
                Ok(column(name)?.values.iter().filter(|v| v.is_null()).count())
            }
            Fact::ShapeCount {
                column: name,
                shape,
                ..
            } => {
                let shape = shape.compile().map_err(|e| e.0)?;
                Ok(texts(name)?.iter().filter(|t| shape.is(t)).count())
            }
            Fact::ContainsCount {
                column: name,
                shape,
                ..
            } => {
                let shape = shape.compile().map_err(|e| e.0)?;
                Ok(texts(name)?.iter().filter(|t| shape.occurs_in(t)).count())
            }
            Fact::ValidDocumentCount {
                column: name,
                document,
                ..
            } => Ok(texts(name)?.iter().filter(|t| document.is_valid(t)).count()),
            Fact::OrphanCount { column: name, .. } => {
                let references = column(name)?;
                let fk = references
                    .spec
                    .fk
                    .as_ref()
                    .ok_or_else(|| format!("`{name}` declares no foreign key"))?;
                let targets = column(&format!("{}.{}", fk.table, fk.column))?;
                Ok(orphan_positions(&references.values, &targets.values).len())
            }
            Fact::DistinctCount { column: name, .. } => {
                Ok(texts(name)?.into_iter().collect::<BTreeSet<_>>().len())
            }
        }
    }

    pub fn wanted(&self) -> usize {
        match self {
            Fact::RowCount { equals, .. }
            | Fact::NullCount { equals, .. }
            | Fact::ShapeCount { equals, .. }
            | Fact::ContainsCount { equals, .. }
            | Fact::ValidDocumentCount { equals, .. }
            | Fact::OrphanCount { equals, .. }
            | Fact::DistinctCount { equals, .. } => *equals,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Fact::RowCount { table, .. } => format!("row_count {table}"),
            Fact::NullCount { column, .. } => format!("null_count {column}"),
            Fact::ShapeCount { column, shape, .. } => {
                format!("shape_count {column} {}", shape.label())
            }
            Fact::ContainsCount { column, shape, .. } => {
                format!("contains_count {column} {}", shape.label())
            }
            Fact::ValidDocumentCount {
                column, document, ..
            } => {
                format!("valid_document_count {column} {document:?}")
            }
            Fact::OrphanCount { column, .. } => format!("orphan_count {column}"),
            Fact::DistinctCount { column, .. } => format!("distinct_count {column}"),
        }
    }
}
