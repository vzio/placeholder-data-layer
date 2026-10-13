use std::collections::BTreeSet;

use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey, KeyValue};
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

fn pk_int(name: &str, values: &[i64]) -> ColumnData {
    column(
        name,
        ColumnType::Integer,
        false,
        true,
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

/// Functional test #8: an empty table (zero rows) must anonymize cleanly,
/// with the schema intact and nothing to transform.
#[test]
fn an_empty_table_anonymizes_cleanly() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                pk_int("id", &[]),
                column("email", ColumnType::Text, true, false, None, vec![]),
            ],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let customers = outcome.dataset.table("customers").expect("customers");
    assert_eq!(customers.row_count(), 0);
}

/// An all-null column (every value `Null`) must anonymize without error and
/// stay all-null: there is nothing to pseudonymize, and nothing in
/// [`Strategy::apply`] should manufacture a value out of nothing.
#[test]
fn an_all_null_column_stays_all_null() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                pk_int("id", &[1, 2, 3]),
                column(
                    "email",
                    ColumnType::Text,
                    true,
                    false,
                    None,
                    vec![Value::Null, Value::Null, Value::Null],
                ),
            ],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let email = outcome
        .dataset
        .table("customers")
        .expect("customers")
        .column("email")
        .expect("email");
    assert!(email.values.iter().all(Value::is_null));
}

/// A single-row table: every per-column statistic (median, MAD, length
/// norms) degenerates gracefully rather than dividing by a near-empty
/// sample.
#[test]
fn a_single_row_table_anonymizes_without_error() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                pk_int("id", &[1]),
                column(
                    "email",
                    ColumnType::Text,
                    false,
                    false,
                    None,
                    vec![Value::Text("ana@gmail.com".into())],
                ),
            ],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let customers = outcome.dataset.table("customers").expect("customers");
    assert_eq!(customers.row_count(), 1);
}

/// Unicode text, including surrogate-adjacent emoji and CJK, must survive
/// classification, profiling and transform without panicking on a
/// non-ASCII byte boundary.
#[test]
fn unicode_values_anonymize_without_panicking() {
    let dataset = Dataset {
        tables: vec![table(
            "profiles",
            vec![
                pk_int("id", &[1, 2, 3, 4]),
                column(
                    "display_name",
                    ColumnType::Text,
                    true,
                    false,
                    None,
                    vec![
                        Value::Text("José da Silva".into()),
                        Value::Null,
                        Value::Text("李雷".into()),
                        Value::Text("Ñoño Ñandú".into()),
                    ],
                ),
                column(
                    "bio",
                    ColumnType::Text,
                    true,
                    false,
                    None,
                    vec![
                        Value::Text("São Paulo → Rio 🇧🇷".into()),
                        Value::Null,
                        Value::Text("喜欢编程和咖啡 ☕".into()),
                        Value::Text(String::new()),
                    ],
                ),
            ],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let profiles = outcome.dataset.table("profiles").expect("profiles");
    assert_eq!(profiles.row_count(), 4);
}

/// A text primary key made of non-ASCII content must actually mint as a
/// changed value, since [`anonymizer::transform::ledger`] redraws
/// alphanumeric content in any script, not only ASCII digits and letters. A
/// key with none (pure punctuation) has nothing to redraw and is expected
/// to pass through.
#[test]
fn a_non_ascii_text_key_is_actually_minted() {
    let dataset = Dataset {
        tables: vec![table(
            "profiles",
            vec![column(
                "external_ref",
                ColumnType::Text,
                false,
                true,
                None,
                vec![
                    Value::Text("中文合同编号".into()),
                    Value::Text("ПРОЕКТ-2024".into()),
                    Value::Text("CONTRATO-SÃO-PAULO-001".into()),
                    Value::Text("-----".into()),
                ],
            )],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let profiles = outcome.dataset.table("profiles").expect("profiles");
    let minted = &profiles
        .column("external_ref")
        .expect("external_ref")
        .values;

    assert_ne!(minted[0], Value::Text("中文合同编号".into()));
    assert_ne!(minted[1], Value::Text("ПРОЕКТ-2024".into()));
    assert_ne!(minted[2], Value::Text("CONTRATO-SÃO-PAULO-001".into()));
}

/// A self-referencing FK (`employees.manager_id -> employees.id`) shares one
/// equivalence class with its own target; the ledger must still mint an
/// injective mapping for it, including the null top-level manager.
#[test]
fn a_self_referencing_fk_anonymizes_consistently() {
    let dataset = Dataset {
        tables: vec![table(
            "employees",
            vec![
                pk_int("id", &[1, 2, 3]),
                column(
                    "manager_id",
                    ColumnType::Integer,
                    true,
                    false,
                    Some(ForeignKey {
                        table: "employees".into(),
                        column: "id".into(),
                    }),
                    vec![Value::Null, Value::Int(1), Value::Int(1)],
                ),
            ],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let employees = outcome.dataset.table("employees").expect("employees");
    let ids: Vec<i64> = employees
        .column("id")
        .expect("id")
        .values
        .iter()
        .map(|v| match v {
            Value::Int(n) => *n,
            other => panic!("expected int, got {other:?}"),
        })
        .collect();
    let managers = &employees.column("manager_id").expect("manager_id").values;

    assert!(managers[0].is_null(), "the null manager must stay null");
    assert_eq!(
        managers[1], managers[2],
        "both reports must mint to the same manager"
    );
    assert_eq!(
        managers[1],
        Value::Int(ids[0]),
        "the minted manager_id must equal the minted id of employee 1"
    );

    let distinct_ids: BTreeSet<i64> = ids.iter().copied().collect();
    assert_eq!(distinct_ids.len(), 3, "minted ids must stay distinct");
}

/// A composite primary key (two PK columns on the same table) is two
/// independent nodes per P1's design; each must mint through its own class
/// without the pair being correlated with each other. "Uncorrelated" here
/// means a column's minted value depends only on its own source value,
/// never on which value it happens to be paired with in the other key
/// column of the same row.
#[test]
fn a_composite_primary_key_anonymizes_both_columns_injectively_and_independently() {
    let policy = Policy {
        seed: Some(7),
        ..Default::default()
    };
    let dataset = Dataset {
        tables: vec![table(
            "line_items",
            vec![
                column(
                    "order_id",
                    ColumnType::Integer,
                    false,
                    true,
                    None,
                    vec![101, 101, 202, 202]
                        .into_iter()
                        .map(Value::Int)
                        .collect(),
                ),
                column(
                    "product_id",
                    ColumnType::Integer,
                    false,
                    true,
                    None,
                    vec![9001, 9002, 9001, 9002]
                        .into_iter()
                        .map(Value::Int)
                        .collect(),
                ),
            ],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, policy).expect("anonymize");
    let line_items = outcome.dataset.table("line_items").expect("line_items");

    let order_values = &line_items.column("order_id").expect("order_id").values;
    let product_values = &line_items.column("product_id").expect("product_id").values;

    let order_ids: BTreeSet<KeyValue> = order_values
        .iter()
        .filter_map(KeyValue::from_value)
        .collect();
    let product_ids: BTreeSet<KeyValue> = product_values
        .iter()
        .filter_map(KeyValue::from_value)
        .collect();

    assert_eq!(
        order_ids.len(),
        2,
        "order_id must keep its own two distinct minted values"
    );
    assert_eq!(
        product_ids.len(),
        2,
        "product_id must keep its own two distinct minted values"
    );

    assert_ne!(
        *order_values,
        vec![
            Value::Int(101),
            Value::Int(101),
            Value::Int(202),
            Value::Int(202)
        ],
        "order_id must actually be minted, not passed through"
    );
    assert_ne!(
        *product_values,
        vec![
            Value::Int(9001),
            Value::Int(9002),
            Value::Int(9001),
            Value::Int(9002)
        ],
        "product_id must actually be minted, not passed through"
    );

    assert_eq!(
        order_values[0], order_values[1],
        "order_id 101 must mint the same value regardless of which product_id it is paired with"
    );
    assert_eq!(
        order_values[2], order_values[3],
        "order_id 202 must mint the same value regardless of which product_id it is paired with"
    );
    assert_eq!(
        product_values[0], product_values[2],
        "product_id 9001 must mint the same value regardless of which order_id it is paired with"
    );
    assert_eq!(
        product_values[1], product_values[3],
        "product_id 9002 must mint the same value regardless of which order_id it is paired with"
    );
}

/// Mutual FKs (`a.id -> b.id` and `b.id -> a.id`) dissolve into one
/// equivalence class per P1's union-find; the ledger must mint one
/// consistent mapping across the cycle rather than failing or looping.
#[test]
fn an_fk_cycle_anonymizes_without_error() {
    let dataset = Dataset {
        tables: vec![
            table(
                "a",
                vec![column(
                    "id",
                    ColumnType::Integer,
                    false,
                    true,
                    Some(ForeignKey {
                        table: "b".into(),
                        column: "id".into(),
                    }),
                    vec![Value::Int(101), Value::Int(202)],
                )],
            ),
            table(
                "b",
                vec![column(
                    "id",
                    ColumnType::Integer,
                    false,
                    true,
                    Some(ForeignKey {
                        table: "a".into(),
                        column: "id".into(),
                    }),
                    vec![Value::Int(101), Value::Int(202)],
                )],
            ),
        ],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let a = outcome.dataset.table("a").expect("a");
    let b = outcome.dataset.table("b").expect("b");
    let a_ids = &a.column("id").unwrap().values;
    let b_ids = &b.column("id").unwrap().values;
    assert_eq!(
        a_ids, b_ids,
        "both sides of the cycle must share one mapping"
    );
    assert_ne!(
        *a_ids,
        vec![Value::Int(101), Value::Int(202)],
        "the shared class must actually be minted, not passed through"
    );
}

/// A dangling FK (a `customer_id` with no matching `customers.id` row) must
/// survive as a dangling reference in the output, per requirement #3: it is
/// the canonical production bug, and "orphans must become zero" would fail
/// every realistic run.
#[test]
fn a_dangling_fk_value_survives_as_dangling() {
    let dataset = Dataset {
        tables: vec![
            table("customers", vec![pk_int("id", &[1, 2])]),
            table(
                "orders",
                vec![
                    pk_int("id", &[10, 11, 12]),
                    column(
                        "customer_id",
                        ColumnType::Integer,
                        false,
                        false,
                        Some(ForeignKey {
                            table: "customers".into(),
                            column: "id".into(),
                        }),
                        vec![Value::Int(1), Value::Int(2), Value::Int(404)],
                    ),
                ],
            ),
        ],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let customers = outcome.dataset.table("customers").expect("customers");
    let orders = outcome.dataset.table("orders").expect("orders");

    let customer_ids: BTreeSet<KeyValue> = customers
        .column("id")
        .expect("id")
        .values
        .iter()
        .filter_map(KeyValue::from_value)
        .collect();
    let order_customer_ids: Vec<KeyValue> = orders
        .column("customer_id")
        .expect("customer_id")
        .values
        .iter()
        .filter_map(KeyValue::from_value)
        .collect();

    let orphans = order_customer_ids
        .iter()
        .filter(|id| !customer_ids.contains(id))
        .count();
    assert_eq!(
        orphans, 1,
        "exactly one orphan must survive, matching the input"
    );
}
