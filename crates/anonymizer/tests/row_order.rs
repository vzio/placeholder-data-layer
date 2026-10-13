use std::collections::BTreeMap;

use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
use anonymizer::transform::Strategy;
use anonymizer::{Dataset, Policy, Table, Value};

fn column(
    name: &str,
    ty: ColumnType,
    nullable: bool,
    pk: bool,
    fk: Option<ForeignKey>,
    values: Vec<Value>,
) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty,
            nullable,
            pk,
            fk,
        },
        values,
    }
}

fn marker_column(values: &[i64]) -> ColumnData {
    column(
        "marker",
        ColumnType::Integer,
        false,
        false,
        None,
        values.iter().copied().map(Value::Int).collect(),
    )
}

fn table(name: &str, columns: Vec<ColumnData>) -> Table {
    Table {
        name: name.to_owned(),
        columns,
    }
}

/// A self-referencing table (`employees.manager_id -> employees.id`) and a
/// mutual-FK cycle (`a.id -> b.id`, `b.id -> a.id`), each with a deliberately
/// scrambled row order (not sorted by key) and a `marker` column forced to
/// [`Strategy::Keep`]: if the pipeline ever reordered rows, the marker — a
/// column no transform touches — would no longer line up with its original
/// position.
fn fixture() -> Dataset {
    Dataset {
        tables: vec![
            table(
                "employees",
                vec![
                    column(
                        "id",
                        ColumnType::Integer,
                        false,
                        true,
                        None,
                        vec![Value::Int(30), Value::Int(10), Value::Int(20)],
                    ),
                    column(
                        "manager_id",
                        ColumnType::Integer,
                        true,
                        false,
                        Some(ForeignKey {
                            table: "employees".into(),
                            column: "id".into(),
                        }),
                        vec![Value::Null, Value::Int(30), Value::Int(30)],
                    ),
                    marker_column(&[300, 100, 200]),
                ],
            ),
            table(
                "a",
                vec![
                    column(
                        "id",
                        ColumnType::Integer,
                        false,
                        true,
                        Some(ForeignKey {
                            table: "b".into(),
                            column: "id".into(),
                        }),
                        vec![Value::Int(202), Value::Int(101)],
                    ),
                    marker_column(&[2, 1]),
                ],
            ),
            table(
                "b",
                vec![
                    column(
                        "id",
                        ColumnType::Integer,
                        false,
                        true,
                        Some(ForeignKey {
                            table: "a".into(),
                            column: "id".into(),
                        }),
                        vec![Value::Int(101), Value::Int(202)],
                    ),
                    marker_column(&[1, 2]),
                ],
            ),
        ],
    }
}

/// Row order is a contract: Backend Core recovers any mapping it needs by
/// positional join against the dump it already holds. This must hold even
/// for tables with FK cycles and self-references, where a future
/// implementer might be tempted to reorder rows (e.g. to group a table by
/// its equivalence class before minting).
#[test]
fn row_order_is_preserved_across_self_reference_and_fk_cycle_tables() {
    let dataset = fixture();
    let mut column_overrides = BTreeMap::new();
    for table in &dataset.tables {
        column_overrides.insert(format!("{}.marker", table.name), Strategy::Keep);
    }
    let policy = Policy {
        seed: Some(5),
        column_overrides,
        ..Default::default()
    };

    let outcome = anonymizer::anonymize(dataset.clone(), policy).expect("anonymize");

    for table_name in ["employees", "a", "b"] {
        let before = dataset
            .table(table_name)
            .expect("table")
            .column("marker")
            .expect("marker");
        let after = outcome
            .dataset
            .table(table_name)
            .expect("table")
            .column("marker")
            .expect("marker");
        assert_eq!(
            before.values, after.values,
            "row order for `{table_name}` must be unchanged"
        );
    }
}
