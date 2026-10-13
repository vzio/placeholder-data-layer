use std::collections::BTreeMap;

use anonymizer::compliance::{Check, Verdict};
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
use anonymizer::preserve::PreserveReason;
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

fn int_column(name: &str, pk: bool, fk: Option<ForeignKey>, values: &[i64]) -> ColumnData {
    column(
        name,
        ColumnType::Integer,
        false,
        pk,
        fk,
        values.iter().copied().map(Value::Int).collect(),
    )
}

fn text_column(name: &str, values: &[&str]) -> ColumnData {
    column(
        name,
        ColumnType::Text,
        false,
        false,
        None,
        values
            .iter()
            .map(|v| Value::Text((*v).to_owned()))
            .collect(),
    )
}

fn table(name: &str, columns: Vec<ColumnData>) -> Table {
    Table {
        name: name.to_owned(),
        columns,
    }
}

fn check(outcome: &anonymizer::Outcome, check: Check) -> &anonymizer::compliance::CheckOutcome {
    outcome
        .report
        .checks
        .iter()
        .find(|outcome| outcome.check == check)
        .unwrap_or_else(|| panic!("{check:?} is reported"))
}

/// A dataset with no direct identifiers at risk, no orphans and no
/// categorical quasi-identifier column. Every check must pass cleanly.
#[test]
fn a_clean_dataset_passes_every_check_with_nothing_flagged() {
    let dataset = Dataset {
        tables: vec![
            table("customers", vec![int_column("id", true, None, &[1, 2, 3])]),
            table(
                "orders",
                vec![
                    int_column("id", true, None, &[10, 11, 12]),
                    int_column(
                        "customer_id",
                        false,
                        Some(ForeignKey {
                            table: "customers".into(),
                            column: "id".into(),
                        }),
                        &[1, 2, 3],
                    ),
                ],
            ),
        ],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    assert_eq!(outcome.report.verdict, Verdict::Pass);

    let leakage = check(&outcome, Check::Leakage);
    assert_eq!(leakage.verdict, Verdict::Pass);
    assert!(leakage.flagged_columns.is_empty());

    let referential = check(&outcome, Check::Referential);
    assert_eq!(referential.verdict, Verdict::Pass);
    assert_eq!(referential.orphans_in, referential.orphans_out);
}

/// Deliberately dirty direct identifiers — malformed documents, framing
/// text, mixed scripts — the shape of corruption that a well-formed-only
/// leakage fixture never exercises. Padded with well-formed values so each
/// column still classifies as its intended semantic type.
fn dirty_direct_identifiers_fixture() -> Dataset {
    Dataset {
        tables: vec![table(
            "customers",
            vec![
                int_column("id", true, None, &[1, 2, 3, 4, 5, 6, 7, 8, 9]),
                text_column(
                    "cpf",
                    &[
                        "111.444.777-35",
                        "123.456.789-09",
                        "529.982.247-25",
                        "111.444.777-35",
                        "123.456.789-09",
                        "529.982.247-25",
                        "NAO TEM",
                        "000.000.000-00",
                        "CPF: 111.444.777-35 (antigo)",
                    ],
                ),
                text_column(
                    "phone",
                    &[
                        "(11) 98765-4321",
                        "(21) 91234-5678",
                        "(31) 99876-5432",
                        "(11) 98765-4321",
                        "(21) 91234-5678",
                        "(31) 99876-5432",
                        "(11) 98765-4321",
                        "NASCIDO EM 1972",
                        "(11) 98765-4321 ramal 22",
                    ],
                ),
                text_column(
                    "email",
                    &[
                        "ana.souza@gmail.com",
                        "bruno.lima@outlook.com",
                        "carla.dias@yahoo.com.br",
                        "diana.alves@gmail.com",
                        "erica.nunes@hotmail.com",
                        "felipe.rocha@gmail.com",
                        "giulia.pinto@outlook.com",
                        "sem email",
                        "joao arroba example ponto com",
                    ],
                ),
                text_column(
                    "full_name",
                    &[
                        "Ana Paula Souza",
                        "Bruno Henrique Lima",
                        "Carla Regina Dias",
                        "Diana Alves Ferreira",
                        "Erica Nunes Barros",
                        "Felipe Rocha Martins",
                        "日本語",
                        "CONTRATO-SÃO-PAULO-001",
                        "MARIA  DE   SOUZA",
                    ],
                ),
            ],
        )],
    }
}

/// The dirty direct-identifier fixture against the leakage check: no output
/// value in any in-scope column equals a value from that column's own
/// source set. This is the check's actual contract — whole-value equality —
/// not a claim that every malformed input is scrubbed of its original
/// content: `"CPF: 111.444.777-35 (antigo)"` carries eleven digits, so
/// `RegenerateDocument` reads it as CPF-shaped and draws a fresh,
/// independently random document; `"NAO TEM"` and `"sem email"` carry none,
/// so their columns' strategies fall back to length-preserving redaction;
/// and `"MARIA  DE   SOUZA"` and `"日本語"` are resampled from the bundled
/// name corpus. None of that proves no *partial* residue survives — the
/// leakage check's own documentation states that gap — only that no row
/// here produces a leaked exact cell.
#[test]
fn dirty_direct_identifiers_clear_the_leakage_check() {
    let outcome = anonymizer::anonymize(dirty_direct_identifiers_fixture(), Policy::default())
        .expect("anonymize");

    let leakage = check(&outcome, Check::Leakage);
    assert_eq!(leakage.verdict, Verdict::Pass);
    assert!(leakage.flagged_columns.is_empty());
    assert_eq!(leakage.flagged_cells, None);
}

/// Pins the contract the leakage check cannot yet enforce: a value that
/// survives in large part, not merely one that survives whole, must be
/// flagged. `"NASCIDO EM 1972 EM SAO PAULO"` anonymizing to
/// `"NASCIDO EM 1309 EM SAO PAULO"` changes three digits and keeps the other
/// twenty-six characters verbatim — a whole-value comparison sees two
/// different strings and calls it clean, because it is never told to look
/// for a residue, only for an exact match.
#[test]
#[ignore = "TODO: fine tune to catch this — partial verbatim survival is not detected; whole-value comparison only"]
fn a_value_that_survives_in_large_part_is_flagged_as_leaked() {
    let before = Dataset {
        tables: vec![table(
            "customers",
            vec![text_column("notes", &["NASCIDO EM 1972 EM SAO PAULO"])],
        )],
    };
    let after = Dataset {
        tables: vec![table(
            "customers",
            vec![text_column("notes", &["NASCIDO EM 1309 EM SAO PAULO"])],
        )],
    };

    let mut profiles = BTreeMap::new();
    profiles.insert(
        "customers.notes".to_owned(),
        anonymizer::profile::ColumnProfile {
            column: "customers.notes".to_owned(),
            semantic_type: anonymizer::classify::SemanticType::FullName,
            confidence: 1.0,
            null_rate: 0.0,
            distinct: 1,
            length: None,
            distribution: anonymizer::profile::Distribution::Unsummarised,
            anomaly_rates: BTreeMap::new(),
        },
    );

    let outcomes = anonymizer::compliance::enforced(
        &before,
        &after,
        &profiles,
        &BTreeMap::new(),
        &anonymizer::preserve::ResolvedPreserve::default(),
    );
    let leakage = outcomes
        .iter()
        .find(|outcome| outcome.check == Check::Leakage)
        .expect("leakage outcome is present");

    assert_eq!(
        leakage.verdict,
        Verdict::Fail,
        "a value that survives in large part must be flagged as leaked"
    );
}

/// A direct-identifier column forced to `Strategy::Keep` through an
/// explicit override — not preserved, so not exempt — must fail loudly at
/// every escalation rung, since `Keep` never transforms the value in the
/// first place and no strength can repair that.
#[test]
fn an_explicit_keep_override_on_a_direct_identifier_fails_at_every_strength() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                int_column("id", true, None, &[1, 2, 3, 4, 5]),
                text_column(
                    "email",
                    &[
                        "ana@gmail.com",
                        "bruno@gmail.com",
                        "carla@outlook.com",
                        "diana@hotmail.com",
                        "erica@gmail.com",
                    ],
                ),
            ],
        )],
    };

    let mut overrides = BTreeMap::new();
    overrides.insert("customers.email".to_owned(), Strategy::Keep);
    let policy = Policy {
        column_overrides: overrides,
        ..Default::default()
    };

    let outcome = anonymizer::anonymize(dataset, policy).expect("anonymize");

    assert_eq!(outcome.report.verdict, Verdict::Fail);
    let leakage = check(&outcome, Check::Leakage);
    assert_eq!(leakage.verdict, Verdict::Fail);
    assert!(leakage.flagged_columns.contains("customers.email"));

    // `Keep` is strength-independent, so the column is still byte-identical
    // to the source after the retry loop gives up.
    let email = outcome
        .dataset
        .table("customers")
        .expect("customers")
        .column("email")
        .expect("email");
    assert_eq!(email.values[0], Value::Text("ana@gmail.com".to_owned()));
}

/// A value-level orphan foreign key — a row referencing a parent
/// that was never there — is legal input and must survive at the same
/// position, with a different (minted) value.
#[test]
fn a_value_level_orphan_fk_survives_at_the_same_position() {
    let dataset = Dataset {
        tables: vec![
            table("customers", vec![int_column("id", true, None, &[1, 2, 3])]),
            table(
                "orders",
                vec![
                    int_column("id", true, None, &[10, 11, 12]),
                    int_column(
                        "customer_id",
                        false,
                        Some(ForeignKey {
                            table: "customers".into(),
                            column: "id".into(),
                        }),
                        &[1, 2, 999],
                    ),
                ],
            ),
        ],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");

    assert_eq!(outcome.report.verdict, Verdict::Pass);
    let referential = check(&outcome, Check::Referential);
    assert_eq!(referential.verdict, Verdict::Pass);
    assert_eq!(referential.orphans_in, Some(1));
    assert_eq!(referential.orphans_out, Some(1));

    let customer_ids: Vec<i64> = outcome
        .dataset
        .table("customers")
        .expect("customers")
        .column("id")
        .expect("id")
        .values
        .iter()
        .map(|v| match v {
            Value::Int(n) => *n,
            other => panic!("expected an int, got {other:?}"),
        })
        .collect();
    let order_fks = outcome
        .dataset
        .table("orders")
        .expect("orders")
        .column("customer_id")
        .expect("customer_id");
    let Value::Int(third) = &order_fks.values[2] else {
        panic!("expected an int");
    };
    assert!(
        !customer_ids.contains(third),
        "the orphan's minted value must still be absent from the minted customer id domain"
    );
}

/// A lookup table preserved end to end, with `force_preserve` since
/// one of its columns classifies as a direct identifier. The column must not
/// be flagged by leakage, must be named as preserved-and-unchecked in the
/// leakage outcome, and must appear in the top-level preserved-columns map.
#[test]
fn a_preserved_direct_identifier_lookup_column_is_named_but_not_flagged() {
    let dataset = Dataset {
        tables: vec![table(
            "support_contacts",
            vec![
                int_column("id", true, None, &[1, 2]),
                text_column("email", &["support@acme.example", "billing@acme.example"]),
            ],
        )],
    };

    let mut preserve = std::collections::BTreeSet::new();
    preserve.insert("support_contacts.email".to_owned());
    let policy = Policy {
        preserve,
        force_preserve: true,
        ..Default::default()
    };

    let outcome = anonymizer::anonymize(dataset, policy).expect("anonymize");

    let leakage = check(&outcome, Check::Leakage);
    assert!(!leakage.flagged_columns.contains("support_contacts.email"));
    assert!(leakage.preserved_columns.contains("support_contacts.email"));
    assert_eq!(leakage.verdict, Verdict::Pass);

    assert_eq!(
        outcome.report.preserved_columns["support_contacts.email"],
        PreserveReason::Explicit
    );

    let email = outcome
        .dataset
        .table("support_contacts")
        .expect("table")
        .column("email")
        .expect("email");
    assert_eq!(
        email.values[0],
        Value::Text("support@acme.example".to_owned())
    );
}

/// A categorical quasi-identifier column with one row whose value is
/// unique must observe `k_observed == 1` and warn — without mutating any
/// value, since linkage is reported, never enforced. Because linkage is
/// never enforced, its `warn` must not pull the top-level verdict down:
/// leakage and referential are both clean here, so the report's overall
/// verdict stays `pass`.
///
/// `tier` classifies `FreeText` (no structured detector matches a bare
/// single token) and transforms under length-preserving, character-class
/// masking: every `"REGULAR"`-shaped row masks to the same `"XXXXXXX"`
/// deterministically, and the one differently-shaped `"VIP"` row masks to
/// its own, differently-lengthed `"XXX"` — so the pre-transform grouping
/// structure (one large group, one singleton) survives the transform
/// exactly, with no RNG involved for either shape.
#[test]
fn a_unique_quasi_identifier_combination_warns_without_mutating_values() {
    let mut values = vec!["REGULAR"; 19];
    values.push("VIP");

    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                int_column("id", true, None, &(1..=20).collect::<Vec<_>>()),
                text_column("tier", &values),
            ],
        )],
    };

    let policy = Policy {
        seed: Some(7),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(dataset, policy).expect("anonymize");

    let linkage = check(&outcome, Check::Linkage);
    assert_eq!(linkage.k_observed, Some(1));
    assert_eq!(linkage.verdict, Verdict::Warn);
    assert_eq!(
        outcome.report.verdict,
        Verdict::Pass,
        "a linkage warn must not drag the top-level verdict down"
    );

    let tier = outcome
        .dataset
        .table("customers")
        .expect("customers")
        .column("tier")
        .expect("tier");
    let masked: std::collections::BTreeSet<&str> =
        tier.values.iter().filter_map(Value::as_text).collect();
    assert_eq!(
        masked.len(),
        2,
        "the two distinct source shapes must still be two distinct masked shapes"
    );
}

/// No source value from any direct-identifier column of the dirty fixture
/// may appear anywhere in the serialized [`anonymizer::FidelityReport`],
/// regardless of how the leakage check itself lands on that fixture. A
/// categorical, non-identifier column is the deliberate counter-case: its
/// raw category labels are expected to appear in the report, because the
/// report's distribution summaries are built from them and they identify
/// no one.
#[test]
fn no_source_direct_identifier_value_reaches_the_serialized_report() {
    // The profiler only summarises a column's distribution from at least
    // twenty sampled values, below which it reports `Unsummarised` instead
    // — so the dirty fixture's nine rows are cycled out to twenty here, the
    // `tier` column aside, which needs its own fixed split to land both of
    // its values above the three-occurrence floor that keeps a rare raw
    // value out of a categorical breakdown.
    let mut dataset = dirty_direct_identifiers_fixture();
    let table = &mut dataset.tables[0];
    let original_rows = table.row_count();
    for column in &mut table.columns {
        if column.spec.name == "id" {
            let next = column.values.len() as i64 + 1;
            for offset in 0..(20 - original_rows) {
                column.values.push(Value::Int(next + offset as i64));
            }
        } else {
            for row in 0..(20 - original_rows) {
                column
                    .values
                    .push(column.values[row % original_rows].clone());
            }
        }
    }
    let tier_values: Vec<&str> = std::iter::repeat_n("REGULAR", 15)
        .chain(std::iter::repeat_n("VIP", 5))
        .collect();
    table.columns.push(text_column("tier", &tier_values));

    let outcome = anonymizer::anonymize(dataset.clone(), Policy::default()).expect("anonymize");
    let report_json = serde_json::to_string(&outcome.report).expect("report serializes");

    for table in &dataset.tables {
        for column in &table.columns {
            if column.spec.name == "tier" {
                continue;
            }
            for value in &column.values {
                if let Some(text) = value.as_text() {
                    assert!(
                        !report_json.contains(text),
                        "report leaked source value `{text}`"
                    );
                }
            }
        }
    }

    let tier_report = outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == "customers.tier")
        .expect("customers.tier is reported");
    assert!(
        tier_report.distribution_before.contains_key("REGULAR"),
        "a non-identifier categorical column's raw labels legitimately appear in the report"
    );
    assert!(tier_report.distribution_before.contains_key("VIP"));
}

/// A pinned seed on a dataset that escalates through every rung of
/// the strength ladder (an override-forced `Keep` on a direct identifier
/// never clears, so every attempt runs) still reproduces a byte-identical
/// dataset and identical checks across two independent runs.
#[test]
fn a_pinned_seed_is_byte_identical_across_runs_that_escalate() {
    fn dataset() -> Dataset {
        Dataset {
            tables: vec![table(
                "customers",
                vec![
                    int_column("id", true, None, &[1, 2, 3, 4, 5]),
                    text_column(
                        "email",
                        &[
                            "ana@gmail.com",
                            "bruno@gmail.com",
                            "carla@outlook.com",
                            "diana@hotmail.com",
                            "erica@gmail.com",
                        ],
                    ),
                ],
            )],
        }
    }

    let mut overrides = BTreeMap::new();
    overrides.insert("customers.email".to_owned(), Strategy::Keep);
    let policy = Policy {
        seed: Some(99),
        column_overrides: overrides,
        ..Default::default()
    };

    let first = anonymizer::anonymize(dataset(), policy.clone()).expect("first run");
    let second = anonymizer::anonymize(dataset(), policy).expect("second run");

    assert_eq!(
        first.report.verdict,
        Verdict::Fail,
        "must actually escalate"
    );
    assert_eq!(
        serde_json::to_string(&first.dataset).expect("serialize"),
        serde_json::to_string(&second.dataset).expect("serialize"),
    );
    assert_eq!(
        serde_json::to_string(&first.report.checks).expect("serialize"),
        serde_json::to_string(&second.report.checks).expect("serialize"),
    );
}

/// A categorical column whose domain is preserved exactly by
/// construction — an email's domain, kept verbatim while only the local
/// part is pseudonymized — must report zero total variation distance.
#[test]
fn a_domain_preserved_categorical_column_reports_zero_total_variation_distance() {
    let mut values = Vec::new();
    for _ in 0..54 {
        values.push("user@gmail.com".to_owned());
    }
    for _ in 0..21 {
        values.push("user@outlook.com".to_owned());
    }
    for _ in 0..25 {
        values.push("user@hotmail.com".to_owned());
    }
    let refs: Vec<&str> = values.iter().map(String::as_str).collect();

    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                int_column("id", true, None, &(1..=100).collect::<Vec<_>>()),
                text_column("email", &refs),
            ],
        )],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let email_report = outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == "customers.email")
        .expect("customers.email is reported");

    assert_eq!(email_report.total_variation_distance, 0.0);
    assert_eq!(
        email_report.distribution_before,
        email_report.distribution_after
    );
}

/// At this pinned seed, a single-row `full_name` column resampling to its
/// own source value fails leakage on the first attempt and only clears it
/// once escalation actually redraws independently of the first attempt's
/// stream — proof that escalating a `ResampleName` failure can heal it,
/// not merely reshuffle the same draw into a smaller pool. The returned
/// dataset and the returned checks must describe the same attempt: a
/// `pass` verdict is only honest if the value it describes genuinely no
/// longer collides with its source.
#[test]
fn escalating_a_resample_name_collision_heals_it_and_reports_the_healed_attempt() {
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![text_column("full_name", &["Ana Silva"])],
        )],
    };
    let policy = Policy {
        seed: Some(352),
        ..Default::default()
    };

    let outcome = anonymizer::anonymize(dataset, policy).expect("anonymize");

    assert_eq!(
        outcome.report.attempts, 2,
        "the first attempt must have failed for escalation to run at all"
    );
    assert_eq!(
        outcome.report.accepted_strength,
        anonymizer::transform::Strength::Aggressive
    );
    assert_eq!(outcome.report.verdict, Verdict::Pass);

    let leakage = check(&outcome, Check::Leakage);
    assert_eq!(leakage.verdict, Verdict::Pass);

    let full_name = outcome
        .dataset
        .table("customers")
        .expect("customers")
        .column("full_name")
        .expect("full_name");
    let Value::Text(output) = &full_name.values[0] else {
        panic!("expected text");
    };
    assert_ne!(
        output.to_lowercase(),
        "ana silva",
        "the dataset the report calls clean must actually be clean"
    );
}
