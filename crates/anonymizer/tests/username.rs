use std::collections::BTreeSet;

use anonymizer::classify::SemanticType;
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType};
use anonymizer::{Dataset, Policy, Table, Value};

/// Login-shaped values: lowercase, no spaces, each carrying a digit or a
/// separator, and each pairwise distinct — a login column is near-unique by
/// nature, which is exactly what tells it apart from a status or slug
/// column of the same per-value shape.
const LOGINS: &[&str] = &[
    "joao.silva92",
    "ana_costa3",
    "bruno77lima",
    "carla.rocha1",
    "davi99santos",
    "elisa_fab21",
    "giovana1souza",
    "hugo.martins4",
    "iris_fern2",
    "paula88costa",
];

fn username_column(name: &str, values: &[&str]) -> ColumnData {
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

fn dataset_with(column: ColumnData, row_count: usize) -> Dataset {
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
                    values: (1..=row_count as i64).map(Value::Int).collect(),
                },
                column,
            ],
        }],
    }
}

fn dataset_with_logins() -> Dataset {
    dataset_with(username_column("username", LOGINS), LOGINS.len())
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

/// The anonymized replacement must be handle-shaped — lowercase, no
/// whitespace, no digit-only prefix — and must never be the source login
/// itself. Length is not pinned to the source's own length: a short login
/// mints at a floored length so its candidate space stays large enough to
/// mint injectively (see `ARCHITECTURE.md`'s ledger-based handle identity
/// space), so "handle-shaped" here means "in a realistic handle's length
/// range," not "character-for-character the same length as the input."
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
        let len = text.chars().count();
        assert!(
            (8..=32).contains(&len),
            "`{text}` (length {len}) is outside the handle length range"
        );
        assert_ne!(text, source, "`{text}` must not equal the source login");
    }
}

/// Profiling the anonymized output must classify the handle column
/// `Username` again, not `FreeText` — a freshly-anonymized dump re-profiled
/// (a routine refresh, or the compliance team double-checking the sandbox)
/// must see the same semantic type the original classification produced,
/// or the second pass silently downgrades the strategy it applies next.
#[test]
fn the_anonymized_handle_column_still_classifies_as_username() {
    let outcome =
        anonymizer::anonymize(dataset_with_logins(), Policy::default()).expect("anonymize");

    let reprofiled = anonymizer::profile(&outcome.dataset, &Policy::default()).expect("re-profile");
    let username = reprofiled
        .columns
        .get("users.username")
        .expect("username column re-profiled");

    assert_eq!(username.semantic_type, SemanticType::Username);
}

/// Usernames are frequently unique-keyed. Ten distinct short logins passing
/// is cheap for any random-token generator to satisfy by sheer luck — the
/// claim worth pinning is that the engine does not merely draw from a space
/// too small to matter: two thousand distinct four-to-five-character
/// logins is a scale at which a non-injective generator (a bare random
/// draw with no collision retry against a shared domain) collides dozens
/// of times. The ledger-backed `MintHandle` strategy must still produce as
/// many distinct outputs as inputs.
#[test]
fn two_thousand_distinct_short_logins_mint_two_thousand_distinct_handles() {
    let logins: Vec<String> = (0..2000).map(|i| format!("u{i:04}")).collect();
    let login_refs: Vec<&str> = logins.iter().map(String::as_str).collect();
    let dataset = dataset_with(username_column("login", &login_refs), logins.len());

    let policy = Policy {
        seed: Some(7),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(dataset, policy).expect("anonymize");

    let users = outcome
        .dataset
        .table("users")
        .expect("users table survives");
    let column = users.column("login").expect("login column survives");

    let distinct: BTreeSet<&str> = column
        .values
        .iter()
        .map(|value| match value {
            Value::Text(text) => text.as_str(),
            other => panic!("login value must stay text-shaped, got {other:?}"),
        })
        .collect();

    assert_eq!(
        distinct.len(),
        logins.len(),
        "every distinct source login must mint a distinct handle, even at scale"
    );
}

/// The same source login appearing in two different rows must mint to the
/// same handle in both: a login used elsewhere as a join key (denormalized
/// into a second table, say) must stay joinable after anonymization, which
/// requires the mapping to be a function of the source value alone, not of
/// which row happened to observe it first.
#[test]
fn repeated_occurrences_of_the_same_login_mint_to_the_same_handle() {
    let values = ["ana.costa1", "bruno_lima2", "ana.costa1", "carla.rocha3"];
    let dataset = dataset_with(username_column("login", &values), values.len());

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let users = outcome
        .dataset
        .table("users")
        .expect("users table survives");
    let column = users.column("login").expect("login column survives");

    assert_eq!(
        column.values[0], column.values[2],
        "two rows holding the same source login must mint to the same handle"
    );
    assert_ne!(
        column.values[0], column.values[1],
        "two rows holding different source logins must not collide"
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
