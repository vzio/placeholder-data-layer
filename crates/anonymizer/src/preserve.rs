//! Expands a [`crate::policy::Policy::preserve`] request set into the full
//! set of columns [`crate::transform::Strategy::Keep`] must run under.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::dataset::Dataset;
use crate::transform::ledger::EquivalenceClasses;

/// Why a column ended up preserved: named directly by the policy, or pulled
/// in because it shares an equivalence class with a column that was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PreserveReason {
    Explicit,
    ClassPropagation,
}

/// `table.column` to [`PreserveReason`], in sorted key order — the shape the
/// fidelity report serialises directly.
#[derive(Debug, Clone, Default)]
pub struct ResolvedPreserve {
    pub columns: BTreeMap<String, PreserveReason>,
}

impl ResolvedPreserve {
    pub fn contains(&self, qualified: &str) -> bool {
        self.columns.contains_key(qualified)
    }
}

/// Expands `requests` (`table` or `table.column` entries, already checked
/// against `dataset` by [`crate::policy::Policy::validate`]) into every
/// column [`crate::transform::Strategy::Keep`] must run under: every
/// explicitly named column, every column of an explicitly named table, and
/// — for any of those that is itself a key column — every other member of
/// its referential equivalence class. Preserving one side of a foreign key
/// while minting the other would point every referencing row at a value
/// that no longer exists, so the class is the unit that propagation
/// travels in; it reaches only key columns, never an ordinary attribute
/// column of the same table.
pub fn resolve(
    requests: &BTreeSet<String>,
    dataset: &Dataset,
    classes: &EquivalenceClasses,
) -> ResolvedPreserve {
    let mut columns = BTreeMap::new();

    for request in requests {
        match request.split_once('.') {
            Some((table, column)) => {
                if dataset
                    .table(table)
                    .and_then(|t| t.column(column))
                    .is_some()
                {
                    columns.insert(format!("{table}.{column}"), PreserveReason::Explicit);
                }
            }
            None => {
                if let Some(table) = dataset.table(request) {
                    for column in &table.columns {
                        columns
                            .insert(column.qualified_name(&table.name), PreserveReason::Explicit);
                    }
                }
            }
        }
    }

    let explicit: Vec<String> = columns.keys().cloned().collect();
    for qualified in explicit {
        let Some(class) = classes.class_of(&qualified) else {
            continue;
        };
        for (member, &member_class) in &classes.members {
            if member_class == class {
                columns
                    .entry(member.clone())
                    .or_insert(PreserveReason::ClassPropagation);
            }
        }
    }

    ResolvedPreserve { columns }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey, Table};

    fn column(name: &str, pk: bool, fk: Option<ForeignKey>) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty: ColumnType::Integer,
                nullable: false,
                pk,
                fk,
            },
            values: Vec::new(),
        }
    }

    fn text_column(name: &str) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty: ColumnType::Text,
                nullable: false,
                pk: false,
                fk: None,
            },
            values: Vec::new(),
        }
    }

    fn dataset() -> Dataset {
        Dataset {
            tables: vec![
                Table {
                    name: "payment_types".into(),
                    columns: vec![column("id", true, None), text_column("label")],
                },
                Table {
                    name: "orders".into(),
                    columns: vec![
                        column("id", true, None),
                        column(
                            "payment_type_id",
                            false,
                            Some(ForeignKey {
                                table: "payment_types".into(),
                                column: "id".into(),
                            }),
                        ),
                    ],
                },
            ],
        }
    }

    fn requests(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_single_column_request_preserves_only_that_column() {
        let dataset = dataset();
        let classes = EquivalenceClasses::build(&dataset).expect("build");
        let resolved = resolve(&requests(&["payment_types.label"]), &dataset, &classes);
        assert_eq!(resolved.columns.len(), 1);
        assert_eq!(
            resolved.columns["payment_types.label"],
            PreserveReason::Explicit
        );
    }

    #[test]
    fn a_whole_table_request_preserves_every_column_explicitly() {
        let dataset = dataset();
        let classes = EquivalenceClasses::build(&dataset).expect("build");
        let resolved = resolve(&requests(&["payment_types"]), &dataset, &classes);
        assert_eq!(
            resolved.columns["payment_types.id"],
            PreserveReason::Explicit
        );
        assert_eq!(
            resolved.columns["payment_types.label"],
            PreserveReason::Explicit
        );
    }

    /// Preserving the lookup table's key column must propagate to the
    /// referencing FK column so the reference stays joinable, but must not
    /// reach into an unrelated attribute of a different table.
    #[test]
    fn preserving_a_key_column_propagates_across_its_equivalence_class() {
        let dataset = dataset();
        let classes = EquivalenceClasses::build(&dataset).expect("build");
        let resolved = resolve(&requests(&["payment_types"]), &dataset, &classes);

        assert_eq!(
            resolved.columns["orders.payment_type_id"],
            PreserveReason::ClassPropagation
        );
        assert!(!resolved.columns.contains_key("orders.id"));
    }

    #[test]
    fn an_unrelated_table_is_untouched() {
        let dataset = dataset();
        let classes = EquivalenceClasses::build(&dataset).expect("build");
        let resolved = resolve(&requests(&["payment_types.label"]), &dataset, &classes);
        assert!(!resolved.columns.contains_key("orders.id"));
        assert!(!resolved.columns.contains_key("orders.payment_type_id"));
    }
}
