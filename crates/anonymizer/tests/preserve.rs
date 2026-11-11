use std::collections::{BTreeMap, BTreeSet};

use anonymizer::classify::SemanticType;
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey, KeyValue};
use anonymizer::preserve::PreserveReason;
use anonymizer::{Dataset, Error, Policy, Table, Value};

fn int_column(name: &str, pk: bool, fk: Option<ForeignKey>, values: &[i64]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Integer,
            nullable: false,
            pk,
            fk,
        },
        values: values.iter().copied().map(Value::Int).collect(),
    }
}

fn text_column(name: &str, values: &[&str]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Text,
            nullable: false,
            pk: false,
            fk: None,
        },
        values: values
            .iter()
            .map(|v| Value::Text((*v).to_owned()))
            .collect(),
    }
}

fn table(name: &str, columns: Vec<ColumnData>) -> Table {
    Table {
        name: name.to_owned(),
        columns,
    }
}

/// A `payment_types` lookup table — `1 Credit, 2 Debit` — referenced by
/// `orders.payment_type_id`, plus an unrelated `orders.customer_name` direct
/// identifier used by the refusal tests.
fn fixture() -> Dataset {
    Dataset {
        tables: vec![
            table(
                "payment_types",
                vec![
                    int_column("id", true, None, &[1, 2]),
                    text_column("label", &["Credit", "Debit"]),
                ],
            ),
            table(
                "orders",
                vec![
                    int_column("id", true, None, &[10, 11, 12]),
                    int_column(
                        "payment_type_id",
                        false,
                        Some(ForeignKey {
                            table: "payment_types".into(),
                            column: "id".into(),
                        }),
                        &[1, 2, 1],
                    ),
                    text_column("customer_name", &["Ana Souza", "Bruno Lima", "Carla Dias"]),
                ],
            ),
        ],
    }
}

fn preserve(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn preserving_a_single_column_keeps_it_raw_and_marks_it_explicit_in_the_report() {
    let policy = Policy {
        preserve: preserve(&["payment_types.label"]),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(fixture(), policy).expect("anonymize");

    let payment_types = outcome.dataset.table("payment_types").expect("table");
    let label = &payment_types.column("label").expect("label").values;
    assert_eq!(
        *label,
        vec![
            Value::Text("Credit".to_owned()),
            Value::Text("Debit".to_owned())
        ]
    );

    assert_eq!(
        outcome.report.preserved_columns["payment_types.label"],
        PreserveReason::Explicit
    );
}

#[test]
fn preserving_a_whole_table_keeps_every_column_raw() {
    let policy = Policy {
        preserve: preserve(&["payment_types"]),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(fixture(), policy).expect("anonymize");

    let payment_types = outcome.dataset.table("payment_types").expect("table");
    assert_eq!(
        payment_types.column("id").expect("id").values,
        vec![Value::Int(1), Value::Int(2)]
    );
    assert_eq!(
        payment_types.column("label").expect("label").values,
        vec![
            Value::Text("Credit".to_owned()),
            Value::Text("Debit".to_owned())
        ]
    );
}

/// Preserving the lookup table's key column must propagate to the
/// referencing `orders.payment_type_id` so the two stay joinable, and the
/// report must name that column as class propagation rather than an
/// explicit request.
#[test]
fn preserving_a_key_column_propagates_to_the_referencing_column_and_is_reported_as_such() {
    let policy = Policy {
        preserve: preserve(&["payment_types"]),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(fixture(), policy).expect("anonymize");

    assert_eq!(
        outcome.report.preserved_columns["payment_types.id"],
        PreserveReason::Explicit
    );
    assert_eq!(
        outcome.report.preserved_columns["orders.payment_type_id"],
        PreserveReason::ClassPropagation
    );
    assert!(!outcome.report.preserved_columns.contains_key("orders.id"));
    assert!(
        !outcome
            .report
            .preserved_columns
            .contains_key("orders.customer_name")
    );

    let orders = outcome.dataset.table("orders").expect("orders");
    assert_eq!(
        orders.column("payment_type_id").expect("fk").values,
        vec![Value::Int(1), Value::Int(2), Value::Int(1)]
    );
}

/// End-to-end lookup table: preserving `payment_types` keeps
/// `orders.payment_type_id` joinable against it with zero new orphans.
#[test]
fn preserving_the_lookup_table_keeps_the_fk_joinable_with_zero_new_orphans() {
    let policy = Policy {
        preserve: preserve(&["payment_types"]),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(fixture(), policy).expect("anonymize");

    let payment_types = outcome.dataset.table("payment_types").expect("table");
    let orders = outcome.dataset.table("orders").expect("orders");

    let ids: BTreeSet<KeyValue> = payment_types
        .column("id")
        .expect("id")
        .values
        .iter()
        .filter_map(KeyValue::from_value)
        .collect();
    let fk_values: Vec<KeyValue> = orders
        .column("payment_type_id")
        .expect("fk")
        .values
        .iter()
        .filter_map(KeyValue::from_value)
        .collect();

    let orphans = fk_values.iter().filter(|id| !ids.contains(id)).count();
    assert_eq!(orphans, 0, "no new orphans must be manufactured");
}

#[test]
fn preserving_a_direct_identifier_is_refused_without_force() {
    let policy = Policy {
        preserve: preserve(&["orders.customer_name"]),
        ..Default::default()
    };
    let error = anonymizer::anonymize(fixture(), policy).expect_err("must be refused");
    assert!(
        matches!(error, Error::PreserveRefused(ref column) if column == "orders.customer_name")
    );
}

#[test]
fn force_preserve_overrides_the_direct_identifier_refusal() {
    let policy = Policy {
        preserve: preserve(&["orders.customer_name"]),
        force_preserve: true,
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(fixture(), policy).expect("force overrides refusal");

    let orders = outcome.dataset.table("orders").expect("orders");
    assert_eq!(
        orders.column("customer_name").expect("name").values,
        vec![
            Value::Text("Ana Souza".to_owned()),
            Value::Text("Bruno Lima".to_owned()),
            Value::Text("Carla Dias".to_owned()),
        ]
    );
    assert_eq!(
        outcome.report.preserved_columns["orders.customer_name"],
        PreserveReason::Explicit
    );
}

#[test]
fn an_unknown_preserve_target_is_rejected() {
    let policy = Policy {
        preserve: preserve(&["no_such_table"]),
        ..Default::default()
    };
    let error = anonymizer::anonymize(fixture(), policy).expect_err("must be rejected");
    assert!(matches!(error, Error::UnknownPreserve(ref name) if name == "no_such_table"));
}

#[test]
fn an_unknown_preserve_column_is_rejected() {
    let policy = Policy {
        preserve: preserve(&["payment_types.no_such_column"]),
        ..Default::default()
    };
    let error = anonymizer::anonymize(fixture(), policy).expect_err("must be rejected");
    assert!(
        matches!(error, Error::UnknownPreserve(ref name) if name == "payment_types.no_such_column")
    );
}

fn uuid_fixture() -> Dataset {
    Dataset {
        tables: vec![table(
            "users",
            vec![text_column(
                "uuid",
                &[
                    "550e8400-e29b-41d4-a716-446655440000",
                    "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
                ],
            )],
        )],
    }
}

/// UUID identity is dataset-wide, not per equivalence class, so preserving
/// one occurrence of a UUID can split an entity's identity across every
/// other table that happens to share the same value with no declared
/// foreign key to propagate through. Preserve must be refused on a UUID
/// column exactly as it is on a direct identifier.
#[test]
fn preserving_a_uuid_column_is_refused_without_force() {
    let policy = Policy {
        preserve: preserve(&["users.uuid"]),
        ..Default::default()
    };
    let error = anonymizer::anonymize(uuid_fixture(), policy).expect_err("must be refused");
    assert!(matches!(error, Error::PreserveRefused(ref column) if column == "users.uuid"));
}

#[test]
fn force_preserve_overrides_the_uuid_refusal() {
    let policy = Policy {
        preserve: preserve(&["users.uuid"]),
        force_preserve: true,
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(uuid_fixture(), policy).expect("force overrides refusal");

    let users = outcome.dataset.table("users").expect("users");
    assert_eq!(
        users.column("uuid").expect("uuid").values,
        vec![
            Value::Text("550e8400-e29b-41d4-a716-446655440000".to_owned()),
            Value::Text("6ba7b810-9dad-11d1-80b4-00c04fd430c8".to_owned()),
        ]
    );
}

/// An explicit strategy override on a column also named by `preserve` is a
/// conflict and must be rejected, not resolved by letting either side win
/// silently.
#[test]
fn an_override_on_a_preserved_column_is_rejected() {
    let mut overrides = BTreeMap::new();
    overrides.insert(
        "payment_types.label".to_owned(),
        anonymizer::transform::Strategy::Redact,
    );
    let policy = Policy {
        preserve: preserve(&["payment_types.label"]),
        column_overrides: overrides,
        ..Default::default()
    };
    let error = anonymizer::anonymize(fixture(), policy).expect_err("conflict must be rejected");
    assert!(
        matches!(error, Error::PreserveOverrideConflict(ref column) if column == "payment_types.label")
    );
}

fn key_text_column(
    name: &str,
    pk: bool,
    fk: Option<ForeignKey>,
    nullable: bool,
    values: Vec<Value>,
) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Text,
            nullable,
            pk,
            fk,
        },
        values,
    }
}

fn texts(values: &[&str]) -> Vec<Value> {
    values
        .iter()
        .map(|v| Value::Text((*v).to_owned()))
        .collect()
}

fn foreign_key(table: &str, column: &str) -> Option<ForeignKey> {
    Some(ForeignKey {
        table: table.into(),
        column: column.into(),
    })
}

/// `people`, keyed by a valid `email` and carrying a non-key `age`, referenced by
/// `events.email`. Both email columns are direct identifiers in one equivalence class, and
/// `events.email` sorts before `people.email`.
fn email_class_fixture() -> Dataset {
    let emails = [
        "ana.souza@example.com",
        "bruno.lima@example.com",
        "carla.dias@example.com",
    ];
    Dataset {
        tables: vec![
            table(
                "people",
                vec![
                    key_text_column("email", true, None, false, texts(&emails)),
                    int_column("age", false, None, &[31, 45, 27]),
                ],
            ),
            table(
                "events",
                vec![key_text_column(
                    "email",
                    false,
                    foreign_key("people", "email"),
                    false,
                    texts(&[emails[0], emails[2], emails[0], emails[1]]),
                )],
            ),
        ],
    }
}

fn refused_column(dataset: Dataset, names: &[&str]) -> String {
    let policy = Policy {
        preserve: preserve(names),
        ..Default::default()
    };
    match anonymizer::anonymize(dataset, policy) {
        Err(Error::PreserveRefused(column)) => column,
        other => panic!("preserve {names:?} must be refused, got {other:?}"),
    }
}

#[test]
fn a_refusal_names_the_listed_column_when_its_unlisted_class_peer_sorts_first() {
    assert_eq!(
        refused_column(email_class_fixture(), &["people.email"]),
        "people.email"
    );
}

#[test]
fn a_refusal_names_the_listed_refusable_column_even_when_it_sorts_last() {
    assert_eq!(
        refused_column(email_class_fixture(), &["people"]),
        "people.email"
    );
    assert_eq!(
        refused_column(email_class_fixture(), &["people.age", "people.email"]),
        "people.email"
    );
}

#[test]
fn a_refusal_names_the_listed_fk_column_over_its_referenced_peer() {
    assert_eq!(
        refused_column(email_class_fixture(), &["events.email"]),
        "events.email"
    );
}

/// `accounts`, keyed by a valid email `login`, referenced by `posts.owner`, a nullable text
/// foreign key holding only nulls. Preserving `posts.owner` alone lists nothing refusable, yet
/// class propagation pulls in `accounts.login`, a direct identifier.
fn propagated_identifier_fixture() -> Dataset {
    Dataset {
        tables: vec![
            table(
                "accounts",
                vec![key_text_column(
                    "login",
                    true,
                    None,
                    false,
                    texts(&["ana.souza@example.com", "bruno.lima@example.com"]),
                )],
            ),
            table(
                "posts",
                vec![key_text_column(
                    "owner",
                    false,
                    foreign_key("accounts", "login"),
                    true,
                    vec![Value::Null, Value::Null, Value::Null],
                )],
            ),
        ],
    }
}

#[test]
fn a_refusal_names_the_propagated_peer_when_no_listed_column_is_refusable() {
    let profile =
        anonymizer::profile(&propagated_identifier_fixture(), &Policy::default()).expect("profile");
    let owner = profile.columns["posts.owner"].semantic_type;
    assert!(
        !owner.is_direct_identifier() && owner != SemanticType::Uuid,
        "the fixture needs a listed column that is not refusable, got {owner:?}"
    );
    assert_eq!(
        profile.columns["accounts.login"].semantic_type,
        SemanticType::Email
    );

    assert_eq!(
        refused_column(propagated_identifier_fixture(), &["posts.owner"]),
        "accounts.login"
    );

    let forced = Policy {
        preserve: preserve(&["posts.owner"]),
        force_preserve: true,
        ..Default::default()
    };
    assert!(anonymizer::anonymize(propagated_identifier_fixture(), forced).is_ok());
}
