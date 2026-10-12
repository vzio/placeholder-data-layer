use std::collections::BTreeMap;

use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey, TableSchema};
use anonymizer::transform::Strategy;
use anonymizer::{Dataset, Policy, Table, Value};

fn column(
    name: &str,
    ty: ColumnType,
    nullable: bool,
    pk: bool,
    fk: Option<ForeignKey>,
) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty,
            nullable,
            pk,
            fk,
        },
        values: Vec::new(),
    }
}

fn with_values(mut column: ColumnData, values: Vec<Value>) -> ColumnData {
    column.values = values;
    column
}

fn table(name: &str, columns: Vec<ColumnData>) -> Table {
    Table {
        name: name.to_owned(),
        columns,
    }
}

#[test]
fn duplicate_table_names_are_rejected() {
    let dataset = Dataset {
        tables: vec![
            table(
                "customers",
                vec![column("id", ColumnType::Integer, false, true, None)],
            ),
            table(
                "customers",
                vec![column("id", ColumnType::Integer, false, true, None)],
            ),
        ],
    };

    let error = anonymizer::anonymize(dataset, Policy::default()).expect_err("duplicate table");
    assert!(matches!(error, anonymizer::Error::DuplicateTable(name) if name == "customers"));
}

#[test]
fn duplicate_column_names_within_a_table_are_rejected() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                column("id", ColumnType::Integer, false, true, None),
                column("id", ColumnType::Text, true, false, None),
            ],
        )],
    };

    let error = anonymizer::anonymize(dataset, Policy::default()).expect_err("duplicate column");
    assert!(matches!(error, anonymizer::Error::DuplicateColumn(name) if name == "customers.id"));
}

#[test]
fn ragged_columns_are_rejected() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                with_values(
                    column("id", ColumnType::Integer, false, true, None),
                    vec![Value::Int(1), Value::Int(2)],
                ),
                with_values(
                    column("email", ColumnType::Text, true, false, None),
                    vec![Value::Text("a@example.com".into())],
                ),
            ],
        )],
    };

    let error = anonymizer::anonymize(dataset, Policy::default()).expect_err("ragged table");
    assert!(matches!(
        error,
        anonymizer::Error::RaggedTable { table, column, len: 1, row_count: 2 }
            if table == "customers" && column == "customers.email"
    ));
}

#[test]
fn dangling_foreign_key_target_table_is_rejected() {
    let dataset = Dataset {
        tables: vec![table(
            "orders",
            vec![column(
                "customer_id",
                ColumnType::Integer,
                false,
                false,
                Some(ForeignKey {
                    table: "customers".into(),
                    column: "id".into(),
                }),
            )],
        )],
    };

    let error = anonymizer::anonymize(dataset, Policy::default()).expect_err("dangling fk");
    assert!(matches!(
        error,
        anonymizer::Error::DanglingForeignKey { column, .. } if column == "orders.customer_id"
    ));
}

#[test]
fn dangling_foreign_key_target_column_is_rejected() {
    let dataset = Dataset {
        tables: vec![
            table(
                "customers",
                vec![column("id", ColumnType::Integer, false, true, None)],
            ),
            table(
                "orders",
                vec![column(
                    "customer_id",
                    ColumnType::Integer,
                    false,
                    false,
                    Some(ForeignKey {
                        table: "customers".into(),
                        column: "missing".into(),
                    }),
                )],
            ),
        ],
    };

    let error = anonymizer::anonymize(dataset, Policy::default()).expect_err("dangling fk");
    assert!(matches!(
        error,
        anonymizer::Error::DanglingForeignKey { column, .. } if column == "orders.customer_id"
    ));
}

#[test]
fn foreign_key_target_that_is_not_a_primary_key_is_rejected() {
    let dataset = Dataset {
        tables: vec![
            table(
                "customers",
                vec![
                    column("id", ColumnType::Integer, false, true, None),
                    column("email", ColumnType::Text, false, false, None),
                ],
            ),
            table(
                "orders",
                vec![column(
                    "customer_email",
                    ColumnType::Text,
                    false,
                    false,
                    Some(ForeignKey {
                        table: "customers".into(),
                        column: "email".into(),
                    }),
                )],
            ),
        ],
    };

    let error = anonymizer::anonymize(dataset, Policy::default()).expect_err("fk not pk");
    assert!(matches!(
        error,
        anonymizer::Error::ForeignKeyTargetNotPrimaryKey { column, .. } if column == "orders.customer_email"
    ));
}

#[test]
fn key_columns_must_be_integer_or_text() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![column("id", ColumnType::Float, false, true, None)],
        )],
    };

    let error =
        anonymizer::anonymize(dataset, Policy::default()).expect_err("unsupported key type");
    assert!(matches!(
        error,
        anonymizer::Error::UnsupportedKeyType { column, ty: ColumnType::Float } if column == "customers.id"
    ));
}

#[test]
fn a_value_contradicting_its_declared_type_is_rejected() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![with_values(
                column("age", ColumnType::Integer, false, false, None),
                vec![Value::Text("not a number".into())],
            )],
        )],
    };

    let error = anonymizer::anonymize(dataset, Policy::default()).expect_err("type mismatch");
    assert!(matches!(
        error,
        anonymizer::Error::TypeMismatch { column, declared: ColumnType::Integer, found: "text" }
            if column == "customers.age"
    ));
}

#[test]
fn a_null_in_a_non_nullable_column_is_rejected() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![with_values(
                column("id", ColumnType::Integer, false, true, None),
                vec![Value::Null],
            )],
        )],
    };

    let error = anonymizer::anonymize(dataset, Policy::default()).expect_err("null not allowed");
    assert!(
        matches!(error, anonymizer::Error::NullNotAllowed { column } if column == "customers.id")
    );
}

#[test]
fn a_null_in_a_nullable_column_is_accepted() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![with_values(
                column("email", ColumnType::Text, true, false, None),
                vec![Value::Null],
            )],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("valid dataset");
    assert_eq!(
        outcome.dataset.table("customers").map(Table::row_count),
        Some(1)
    );
}

#[test]
fn an_override_naming_a_column_absent_from_the_dataset_is_rejected() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![column("id", ColumnType::Integer, false, true, None)],
        )],
    };
    let mut column_overrides = BTreeMap::new();
    column_overrides.insert("customers.phone".to_owned(), Strategy::Redact);
    let policy = Policy {
        seed: None,
        column_overrides,
    };

    let error = anonymizer::anonymize(dataset, policy).expect_err("unknown override column");
    assert!(matches!(
        error,
        anonymizer::Error::UnknownOverride(key) if key == "customers.phone"
    ));
}

#[test]
fn an_integer_literal_in_a_float_column_is_coerced_rather_than_rejected() {
    let dataset = Dataset {
        tables: vec![table(
            "measurements",
            vec![with_values(
                column("value", ColumnType::Float, false, false, None),
                vec![Value::Int(2)],
            )],
        )],
    };

    // Coercion is a validation-time concern: an `Int` literal in a `Float`
    // column must be accepted (not rejected as a type mismatch) and must
    // come out the other side as a `Float`. The transform stage is free to
    // jitter that float afterward, so this does not pin the exact value —
    // only that validation coerced the type rather than erroring.
    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("valid dataset");
    let values = outcome
        .dataset
        .table("measurements")
        .and_then(|t| t.column("value"))
        .map(|c| &c.values);
    assert!(
        matches!(values, Some(values) if matches!(values.as_slice(), [Value::Float(_)])),
        "expected a single coerced float value, got {values:?}"
    );
}

#[test]
fn schema_round_trips_through_the_dataset() {
    let spec = TableSchema {
        name: "customers".into(),
        columns: vec![ColumnSpec {
            name: "id".into(),
            ty: ColumnType::Integer,
            nullable: false,
            pk: true,
            fk: None,
        }],
    };
    assert_eq!(spec.column("id").map(|c| c.ty), Some(ColumnType::Integer));
}
