use std::collections::{BTreeMap, BTreeSet};

use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
use anonymizer::transform::Strategy;
use anonymizer::{Dataset, Error, Policy, Table, Value};

fn uuid_column(name: &str, pk: bool, values: &[&str]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Text,
            nullable: false,
            pk,
            fk: None,
        },
        values: values
            .iter()
            .map(|v| Value::Text((*v).to_owned()))
            .collect(),
    }
}

fn fk_uuid_column(
    name: &str,
    target_table: &str,
    target_column: &str,
    values: &[&str],
) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Text,
            nullable: false,
            pk: false,
            fk: Some(ForeignKey {
                table: target_table.to_owned(),
                column: target_column.to_owned(),
            }),
        },
        values: values
            .iter()
            .map(|v| Value::Text((*v).to_owned()))
            .collect(),
    }
}

fn int_pk_column(values: &[i64]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: "id".to_owned(),
            ty: ColumnType::Integer,
            nullable: false,
            pk: true,
            fk: None,
        },
        values: values.iter().copied().map(Value::Int).collect(),
    }
}

fn table(name: &str, columns: Vec<ColumnData>) -> Table {
    Table {
        name: name.to_owned(),
        columns,
    }
}

fn text_values(table: &Table, column: &str) -> Vec<String> {
    table
        .column(column)
        .expect("column present")
        .values
        .iter()
        .map(|value| match value {
            Value::Text(s) => s.clone(),
            other => panic!("expected text, got {other:?}"),
        })
        .collect()
}

/// `users.uuid` and `audit_log.entity_uuid` carry the same UUID in some rows
/// with no declared foreign key between the two tables — the ordinary shape
/// of an audit/log table. Both columns are declared primary keys of their
/// own table so each lands in its own singleton equivalence class under the
/// class-based ledger, yet the shared value must still mint to the same
/// output in both tables.
#[test]
fn the_same_uuid_in_two_unrelated_tables_mints_to_the_same_output() {
    let shared = "550e8400-e29b-41d4-a716-446655440000";
    let dataset = Dataset {
        tables: vec![
            table(
                "users",
                vec![uuid_column(
                    "uuid",
                    true,
                    &[shared, "6ba7b810-9dad-11d1-80b4-00c04fd430c8"],
                )],
            ),
            table(
                "audit_log",
                vec![uuid_column(
                    "entity_uuid",
                    true,
                    &[shared, "00000000-0000-0000-0000-000000000001"],
                )],
            ),
        ],
    };

    let policy = Policy {
        seed: Some(11),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(dataset, policy).expect("anonymize");

    let users = outcome.dataset.table("users").expect("users");
    let audit_log = outcome.dataset.table("audit_log").expect("audit_log");
    let users_uuid = text_values(users, "uuid");
    let audit_uuid = text_values(audit_log, "entity_uuid");

    assert_eq!(
        users_uuid[0], audit_uuid[0],
        "the shared UUID must mint to the same output in both tables"
    );
    assert_ne!(users_uuid[1], audit_uuid[1]);
}

/// Distinct UUIDs across the whole dataset never collide, and every minted
/// value is itself well-formed and the same shape as its source.
#[test]
fn distinct_uuids_never_collide_and_minted_values_are_well_formed() {
    let sources = [
        "550e8400-e29b-41d4-a716-446655440000",
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        "00000000-0000-0000-0000-000000000001",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "550e8400e29b41d4a716446655440000",
    ];
    let dataset = Dataset {
        tables: vec![table("entities", vec![uuid_column("uuid", true, &sources)])],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let entities = outcome.dataset.table("entities").expect("entities");
    let minted = text_values(entities, "uuid");

    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        sources.len(),
        "minted UUIDs must be pairwise distinct"
    );

    for (source, minted) in sources.iter().zip(&minted) {
        assert_ne!(minted, source, "`{source}` must actually be minted");
        assert_eq!(
            minted.len(),
            source.len(),
            "`{source}` must keep its shape's length"
        );
    }
}

/// `users.id = 1` and `products.id = 1` must mint independently: integers
/// never merge across tables regardless of sharing a value, and each
/// table's id space stays injective on its own.
#[test]
fn integers_do_not_merge_across_unrelated_tables() {
    let ids = [1, 2, 3, 4, 5];
    let dataset = Dataset {
        tables: vec![
            table("users", vec![int_pk_column(&ids)]),
            table("products", vec![int_pk_column(&ids)]),
        ],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let users = outcome.dataset.table("users").expect("users");
    let products = outcome.dataset.table("products").expect("products");

    let minted_users: Vec<i64> = users
        .column("id")
        .expect("id")
        .values
        .iter()
        .map(|v| match v {
            Value::Int(n) => *n,
            other => panic!("expected int, got {other:?}"),
        })
        .collect();
    let minted_products: Vec<i64> = products
        .column("id")
        .expect("id")
        .values
        .iter()
        .map(|v| match v {
            Value::Int(n) => *n,
            other => panic!("expected int, got {other:?}"),
        })
        .collect();

    let distinct_users: BTreeSet<i64> = minted_users.iter().copied().collect();
    let distinct_products: BTreeSet<i64> = minted_products.iter().copied().collect();
    assert_eq!(
        distinct_users.len(),
        ids.len(),
        "users.id must stay injective on its own"
    );
    assert_eq!(
        distinct_products.len(),
        ids.len(),
        "products.id must stay injective on its own"
    );
}

/// `users.uuid` and `orders.user_uuid` share a declared foreign key, so
/// they fall in the same equivalence class. `users.uuid`
/// resolves to `MintUuid` by default; overriding only `orders.user_uuid`
/// to `MintKey` would mint it through a different, class-scoped domain
/// than its parent, dangling every row that referenced it. The mismatch
/// must be rejected before the ledger is built, not leave a silently
/// broken reference in the output.
#[test]
fn a_mint_key_override_on_one_side_of_a_uuid_linked_class_is_rejected() {
    let shared = "550e8400-e29b-41d4-a716-446655440000";
    let dataset = Dataset {
        tables: vec![
            table("users", vec![uuid_column("uuid", true, &[shared])]),
            table(
                "orders",
                vec![fk_uuid_column("user_uuid", "users", "uuid", &[shared])],
            ),
        ],
    };

    let mut overrides = BTreeMap::new();
    overrides.insert("orders.user_uuid".to_owned(), Strategy::MintKey);
    let policy = Policy {
        column_overrides: overrides,
        ..Default::default()
    };

    let error = anonymizer::anonymize(dataset, policy).expect_err("mismatch must be rejected");
    assert!(matches!(error, Error::InconsistentClassMint { .. }));
}
