use std::collections::BTreeSet;

use anonymizer::classify::SemanticType;
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType};
use anonymizer::{Dataset, Policy, Table, Value};

/// Login-shaped values: lowercase, no spaces, each carrying a digit or a
/// separator so none of them is mistaken for a plain dictionary word.
const LOGINS: &[&str] = &[
    "joao.silva92",
    "ana_costa3",
    "bruno77lima",
    "carla.rocha",
    "davi99santos",
    "elisa_fab21",
    "giovana1souza",
    "hugo.martins4",
    "iris_fern2",
    "paula88costa",
];

fn username_column() -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: "username".into(),
            ty: ColumnType::Text,
            nullable: false,
            pk: false,
            fk: None,
        },
        values: LOGINS
            .iter()
            .map(|v| Value::Text((*v).to_owned()))
            .collect(),
    }
}

fn dataset_with_logins() -> Dataset {
    Dataset {
        tables: vec![Table {
            name: "users".into(),
            columns: vec![
                ColumnData {
                    spec: ColumnSpec {
                        name: "id".into(),
                        ty: ColumnType::Integer,
                        nullable: false,
                        pk: true,
                        fk: None,
                    },
                    values: (1..=LOGINS.len() as i64).map(Value::Int).collect(),
                },
                username_column(),
            ],
        }],
    }
}

/// A login column must be classified `Username`, not `FullName` (which
/// would produce a person name where a handle belongs) and not `FreeText`
/// (which would only redact it, destroying the handle shape entirely).
#[test]
fn a_login_column_is_classified_username_not_full_name_or_free_text() {
    let dataset = dataset_with_logins();
    let profile = anonymizer::profile(&dataset, &Policy::default()).expect("profile");

    let username = profile
        .columns
        .get("users.username")
        .expect("username column profiled");

    assert_eq!(username.semantic_type, SemanticType::Username);
    assert_ne!(username.semantic_type, SemanticType::FullName);
    assert_ne!(username.semantic_type, SemanticType::FreeText);
}

/// The anonymized replacement must be handle-shaped: lowercase, no
/// whitespace, and the same length as the sanitized source — never a
/// person name, never the source's own characters surviving in place.
#[test]
fn a_login_columns_output_is_handle_shaped() {
    let outcome =
        anonymizer::anonymize(dataset_with_logins(), Policy::default()).expect("anonymize");

    let users = outcome
        .dataset
        .table("users")
        .expect("users table survives");
    let column = users.column("username").expect("username column survives");

    for (source, value) in LOGINS.iter().zip(column.values.iter()) {
        let Value::Text(text) = value else {
            panic!("username value must stay text-shaped, got {value:?}");
        };
        assert!(
            text.chars().all(|c| !c.is_whitespace()),
            "`{text}` contains whitespace"
        );
        assert!(
            text.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "`{text}` is not lowercase-alphanumeric"
        );
        assert_eq!(
            text.chars().count(),
            source.chars().count(),
            "`{text}` is not length-similar to source `{source}`"
        );
        assert_ne!(text, source, "`{text}` must not equal the source login");
    }
}

/// Usernames are frequently unique-keyed, so distinct source logins must
/// mint distinct replacements.
#[test]
fn distinct_logins_mint_distinct_handles() {
    let policy = Policy {
        seed: Some(7),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(dataset_with_logins(), policy).expect("anonymize");

    let users = outcome
        .dataset
        .table("users")
        .expect("users table survives");
    let column = users.column("username").expect("username column survives");

    let distinct: BTreeSet<&str> = column
        .values
        .iter()
        .map(|value| match value {
            Value::Text(text) => text.as_str(),
            other => panic!("username value must stay text-shaped, got {other:?}"),
        })
        .collect();

    assert_eq!(
        distinct.len(),
        LOGINS.len(),
        "every distinct source login must mint a distinct handle"
    );
}

/// A pinned seed reproduces byte-identical handles across runs, same as
/// every other strategy's determinism contract.
#[test]
fn a_pinned_seed_reproduces_byte_identical_usernames() {
    let policy = Policy {
        seed: Some(99),
        ..Default::default()
    };

    let first = anonymizer::anonymize(dataset_with_logins(), policy.clone()).expect("first run");
    let second = anonymizer::anonymize(dataset_with_logins(), policy).expect("second run");

    let first_values = first
        .dataset
        .table("users")
        .expect("users table")
        .column("username")
        .expect("username column")
        .values
        .clone();
    let second_values = second
        .dataset
        .table("users")
        .expect("users table")
        .column("username")
        .expect("username column")
        .values
        .clone();

    assert_eq!(first_values, second_values);
}
