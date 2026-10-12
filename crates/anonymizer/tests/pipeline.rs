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
//
// Timings are wall-clock measurements, not output: two runs of identical
// work can legitimately take a different number of milliseconds, so they
// are excluded from the byte-identical contract rather than compared.
fn serialize_outcome(outcome: &anonymizer::Outcome) -> String {
    let mut report = outcome.report.clone();
    report.timings_ms.clear();
    serde_json::to_string(&(&outcome.dataset, &report)).expect("serialize outcome")
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

/// M2 regression: a high-cardinality `FreeText` column is `Unsummarised`
/// before transform (no histogram to begin with) and can legitimately
/// collapse to a handful of distinct values after length-preserving
/// redaction. Scoring an empty "before" map against a populated "after" one
/// used to report that collapse as total variation distance, a false bug
/// signal on a column where nothing went wrong.
#[test]
fn an_unsummarised_column_reports_zero_total_variation_distance() {
    let mut notes: Vec<Value> = Vec::new();
    for i in 0..20 {
        notes.push(Value::Text(format!("sentence{i:02}")));
    }
    for i in 0..20 {
        notes.push(Value::Text(format!("paragraph{i:02}")));
    }

    let dataset = Dataset {
        tables: vec![Table {
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
                    values: (0..40).map(Value::Int).collect(),
                },
                ColumnData {
                    spec: ColumnSpec {
                        name: "notes".into(),
                        ty: ColumnType::Text,
                        nullable: false,
                        pk: false,
                        fk: None,
                    },
                    values: notes,
                },
            ],
        }],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let notes_report = outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == "customers.notes")
        .expect("customers.notes is reported");

    assert_eq!(notes_report.total_variation_distance, 0.0);
}

// TODO(P5): delete the `checks.is_empty()` assertion once the retry loop runs.
#[test]
fn passthrough_reports_a_pass_verdict_with_one_column_entry_per_column() {
    let outcome = anonymizer::anonymize(fixture(), Policy::default()).expect("anonymize");
    assert_eq!(outcome.report.verdict, Verdict::Pass);
    assert!(outcome.report.checks.is_empty());

    let expected: Vec<String> = fixture()
        .tables
        .iter()
        .flat_map(|table| {
            table
                .columns
                .iter()
                .map(move |column| column.qualified_name(&table.name))
        })
        .collect();
    let reported: Vec<String> = outcome
        .report
        .columns
        .iter()
        .map(|c| c.column.clone())
        .collect();
    assert_eq!(reported, expected);
}
