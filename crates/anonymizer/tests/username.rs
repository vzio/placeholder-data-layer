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

/// Usernames are frequently unique-keyed. This is a scale sanity check
/// through the public API, not the adversarial proof: at the real
/// `handle_len` floor of `8` characters the candidate space is large
/// enough (`~5.7 x 10^11`) that two thousand draws would not collide even
/// with no retry at all, so this passing is necessary but not sufficient
/// evidence the retry exists. The proof that a non-retrying implementation
/// provably fails — at a length small enough to make collision and
/// exhaustion both reachable within a few thousand candidates — lives at
/// the unit level: `transform::ledger::tests::mint_handle_at_len_is_injective_under_real_collision_pressure`
/// and `mint_handle_at_len_fails_loudly_once_the_domain_is_truly_exhausted`.
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

/// Regression for a defect found in review: a `users.login` primary key
/// (named like a handle, so it classifies `Username` on its own evidence)
/// and `audit.performed_by` — a foreign key onto it, named nothing like a
/// handle — must not disagree on mint strategy. Classified independently,
/// `audit.performed_by` has no name evidence for `Username` at all and
/// would resolve to `FreeText` → `MintKey`, while `users.login` resolves
/// to `MintHandle`: two members of one referential equivalence class
/// minting through different spaces, which is exactly what
/// `validate_class_mint_consistency` exists to reject. `origin/main`
/// anonymizes this dataset shape successfully (neither column is
/// `Username` there, both just mint as keys); this branch must keep
/// succeeding on it by making every class member agree on `Username` once
/// any one of them earns it, not by 422ing with an equivalence-class
/// mint-strategy conflict.
#[test]
fn an_oddly_named_fk_onto_a_login_key_does_not_conflict_on_mint_strategy() {
    use anonymizer::dataset::ForeignKey;

    let login_values = ["ana.costa1", "bruno_lima2", "carla.rocha3"];
    let users = Table {
        name: "users".into(),
        columns: vec![username_column_pk("login", &login_values)],
    };

    let audit_values: Vec<&str> = (0..200)
        .map(|i| login_values[i % login_values.len()])
        .collect();
    let audit = Table {
        name: "audit".into(),
        columns: vec![ColumnData {
            spec: ColumnSpec {
                name: "performed_by".into(),
                ty: ColumnType::Text,
                nullable: false,
                pk: false,
                fk: Some(ForeignKey {
                    table: "users".into(),
                    column: "login".into(),
                }),
            },
            values: audit_values
                .iter()
                .map(|v| Value::Text((*v).to_owned()))
                .collect(),
        }],
    };

    let dataset = Dataset {
        tables: vec![users, audit],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect(
        "a login primary key and an oddly-named foreign key onto it must not conflict on mint strategy",
    );

    // Referential integrity: every minted audit.performed_by value must be
    // one of the minted users.login values, exactly as it was before
    // anonymizing.
    let minted_logins: BTreeSet<&str> = outcome
        .dataset
        .table("users")
        .expect("users table survives")
        .column("login")
        .expect("login column survives")
        .values
        .iter()
        .map(|v| match v {
            Value::Text(t) => t.as_str(),
            other => panic!("login value must stay text-shaped, got {other:?}"),
        })
        .collect();
    for value in &outcome
        .dataset
        .table("audit")
        .expect("audit table survives")
        .column("performed_by")
        .expect("performed_by column survives")
        .values
    {
        let Value::Text(text) = value else {
            panic!("audit.performed_by value must stay text-shaped, got {value:?}");
        };
        assert!(
            minted_logins.contains(text.as_str()),
            "`{text}` in audit.performed_by is not one of the minted users.login values"
        );
    }
}

fn username_column_pk(name: &str, values: &[&str]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Text,
            nullable: false,
            pk: true,
            fk: None,
        },
        values: values
            .iter()
            .map(|v| Value::Text((*v).to_owned()))
            .collect(),
    }
}

/// Regression for a second defect found in review: an all-NULL column
/// whose name matches a handle hint (`last_login`) but is declared a
/// non-`Text` type must not classify `Username` from the name alone —
/// doing so routed it to `Strategy::MintHandle`, which rejects a
/// non-`Text` column, turning a column that is `Unknown` → `Redact` (and
/// trivially succeeds, since every value is null) on `origin/main` into a
/// hard failure on this branch.
#[test]
fn an_all_null_integer_column_named_like_a_login_still_anonymizes() {
    let dataset = Dataset {
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
                    values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
                },
                ColumnData {
                    spec: ColumnSpec {
                        name: "last_login".into(),
                        ty: ColumnType::Integer,
                        nullable: true,
                        pk: false,
                        fk: None,
                    },
                    values: vec![Value::Null, Value::Null, Value::Null],
                },
            ],
        }],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default())
        .expect("an all-null column named like a login must still anonymize successfully");

    let column = outcome
        .dataset
        .table("users")
        .expect("users table survives")
        .column("last_login")
        .expect("last_login column survives");
    assert!(column.values.iter().all(Value::is_null));
}

/// Regression for a defect found in review: `MintHandle`'s output must be
/// excluded from `anomaly::reinject`, the same as `MintKey` and `MintUuid`
/// already are. Two rows holding the identical padded source login — the
/// padding trips `WhitespacePadding` at both positions, whose `corrupt`
/// draws from a shared, position-advancing `rng` — must still agree after
/// anonymization. Before `apply`, both positions redact deterministically
/// to the same value (neither the ledger lookup nor `redact` itself draws
/// any randomness); `reinject` is the only step in the pipeline capable of
/// making them diverge, by drawing two different random paddings from the
/// advancing stream for what was, until that point, one identical value in
/// two rows.
#[test]
fn an_anomaly_flagged_repeated_login_still_mints_identically_in_every_row() {
    let mut values: Vec<&str> = vec![
        "ana.costa1",
        "bruno_lima2",
        "carla.rocha3",
        "davi_santos4",
        "elisa.souza5",
        "fabio_costa6",
        "giovana.lima7",
        "hugo_santos8",
    ];
    // Identical, leading-and-trailing-whitespace-padded in both positions —
    // `has_whitespace_anomaly` flags both, `corrupt_whitespace` draws from
    // `rng` on each.
    values.push("  iris.fernandes9  ");
    values.push("  iris.fernandes9  ");

    let dataset = dataset_with(username_column("login", &values), values.len());
    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");

    let column = outcome
        .dataset
        .table("users")
        .expect("users table survives")
        .column("login")
        .expect("login column survives");

    let last = column.values.len() - 1;
    assert_eq!(
        column.values[last],
        column.values[last - 1],
        "two anomaly-flagged rows holding the same source login must mint identically"
    );
}
