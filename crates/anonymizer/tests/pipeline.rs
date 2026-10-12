use anonymizer::compliance::Verdict;
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
use anonymizer::{Dataset, Policy, Table, Value};

fn fixture() -> Dataset {
    Dataset {
        tables: vec![
            Table {
                name: "customers".into(),
                columns: vec![
                    ColumnData {
                        spec: ColumnSpec {
                            name: "id".into(),
                            ty: ColumnType::Integer,
                            nullable: false,
                            pk: true,
                            fk: None,
                        },
                        values: vec![Value::Int(1), Value::Int(2)],
                    },
                    ColumnData {
                        spec: ColumnSpec {
                            name: "email".into(),
                            ty: ColumnType::Text,
                            nullable: true,
                            pk: false,
                            fk: None,
                        },
                        values: vec![Value::Text("ana@gmail.com".into()), Value::Null],
                    },
                ],
            },
            Table {
                name: "orders".into(),
                columns: vec![
                    ColumnData {
                        spec: ColumnSpec {
                            name: "id".into(),
                            ty: ColumnType::Integer,
                            nullable: false,
                            pk: true,
                            fk: None,
                        },
                        values: vec![Value::Int(10), Value::Int(11), Value::Int(12)],
                    },
                    ColumnData {
                        spec: ColumnSpec {
                            name: "customer_id".into(),
                            ty: ColumnType::Integer,
                            nullable: false,
                            pk: false,
                            fk: Some(ForeignKey {
                                table: "customers".into(),
                                column: "id".into(),
                            }),
                        },
                        values: vec![Value::Int(1), Value::Int(1), Value::Int(2)],
                    },
                ],
            },
        ],
    }
}

// Serializing rather than comparing with `PartialEq` is deliberate: once P4
// lands float jitter, `PartialEq` would call two `NaN`s unequal even when
// both runs produced the same bit pattern, and would call `-0.0` and `0.0`
// equal even though a sign-flip bug should fail this test.
fn serialize_outcome(outcome: &anonymizer::Outcome) -> String {
    serde_json::to_string(&(&outcome.dataset, &outcome.report)).expect("serialize outcome")
}

#[test]
fn a_pinned_seed_reproduces_byte_identical_output() {
    let policy = Policy {
        seed: Some(42),
        column_overrides: Default::default(),
    };

    let first = anonymizer::anonymize(fixture(), policy.clone()).expect("first run");
    let second = anonymizer::anonymize(fixture(), policy).expect("second run");

    assert_eq!(serialize_outcome(&first), serialize_outcome(&second));
}

#[test]
fn schema_is_preserved_across_the_pipeline() {
    let source = fixture();
    let outcome = anonymizer::anonymize(fixture(), Policy::default()).expect("anonymize");

    assert_eq!(source.tables.len(), outcome.dataset.tables.len());
    for (before, after) in source.tables.iter().zip(&outcome.dataset.tables) {
        assert_eq!(before.name, after.name);
        assert_eq!(
            before
                .columns
                .iter()
                .map(|c| c.spec.name.clone())
                .collect::<Vec<_>>(),
            after
                .columns
                .iter()
                .map(|c| c.spec.name.clone())
                .collect::<Vec<_>>(),
            "column order must be preserved"
        );
        for (before_column, after_column) in before.columns.iter().zip(&after.columns) {
            assert_eq!(before_column.spec, after_column.spec);
            assert_eq!(
                before_column.len(),
                after_column.len(),
                "column `{}` length must be preserved",
                before_column.qualified_name(&before.name)
            );
        }
    }
}

// TODO(P5): delete — passthrough-only invariant.
#[test]
fn passthrough_reports_a_pass_verdict_with_no_checks() {
    let outcome = anonymizer::anonymize(fixture(), Policy::default()).expect("anonymize");
    assert_eq!(outcome.report.verdict, Verdict::Pass);
    assert!(outcome.report.checks.is_empty());
    assert!(outcome.report.columns.is_empty());
}
