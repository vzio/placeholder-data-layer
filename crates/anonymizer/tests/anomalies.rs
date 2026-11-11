use std::collections::BTreeMap;

use anonymizer::anomaly::{self, AnomalyKind, NameShape};
use anonymizer::classify::{Classification, SemanticType};
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType};
use anonymizer::profile;
use anonymizer::transform::Strategy;
use anonymizer::{Dataset, Policy, Table, Value};

fn text_column(name: &str, values: &[&str]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Text,
            nullable: false,
            pk: false,
            fk: None,
        },
        values: values.iter().map(|v| Value::Text(v.to_string())).collect(),
    }
}

fn id_column(values: &[i64]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: "id".to_owned(),
            ty: ColumnType::Integer,
            nullable: false,
            pk: true,
            fk: None,
        },
        values: values.iter().map(|&v| Value::Int(v)).collect(),
    }
}

/// Six emails, two of which are not email-shaped at all.
fn email_values() -> &'static [&'static str] {
    &[
        "ana@gmail.com",
        "bruno@outlook.com",
        "carla@gmail.com",
        "davi@yahoo.com",
        "not-an-email",
        "also@@broken.com",
    ]
}

/// Six names, one all-caps and one all-lowercase.
fn full_name_values() -> &'static [&'static str] {
    &[
        "Ana Souza",
        "Bruno Lima",
        "Carla Dias",
        "Davi Pereira",
        "ALL CAPS NAME",
        "all lower name",
    ]
}

fn fixture() -> Dataset {
    Dataset {
        tables: vec![Table {
            name: "customers".into(),
            columns: vec![
                id_column(&[1, 2, 3, 4, 5, 6]),
                text_column("email", email_values()),
                text_column("full_name", full_name_values()),
            ],
        }],
    }
}

fn counts(column: &ColumnData, semantic_type: SemanticType) -> BTreeMap<AnomalyKind, usize> {
    let classification = Classification {
        semantic_type,
        confidence: 1.0,
    };
    let column_profile = profile::profile_column(&column.spec.name, column, classification);
    anomaly::detect(column, &column_profile).counts()
}

fn email_column(dataset: &Dataset) -> &ColumnData {
    dataset
        .table("customers")
        .expect("customers table")
        .column("email")
        .expect("email column")
}

fn full_name_column(dataset: &Dataset) -> &ColumnData {
    dataset
        .table("customers")
        .expect("customers table")
        .column("full_name")
        .expect("full_name column")
}

/// Functional test #3: per-kind anomaly counts are recomputed independently
/// on the input and on the output, rather than trusted from the report, so
/// the assertion stays meaningful once P4 makes the two datasets differ.
/// Passthrough makes before and after the same dataset today; the point of
/// recomputing rather than reusing the report's numbers is that this test
/// keeps testing the same thing once that stops being true.
#[test]
fn anomaly_counts_survive_the_pipeline_exactly() {
    let dataset = fixture();
    let before_email = counts(email_column(&dataset), SemanticType::Email);
    let before_name = counts(full_name_column(&dataset), SemanticType::FullName);

    assert_eq!(before_email[&AnomalyKind::MalformedEmail], 2);
    assert_eq!(before_name[&AnomalyKind::MalformedName], 2);

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");

    let after_email = counts(email_column(&outcome.dataset), SemanticType::Email);
    let after_name = counts(full_name_column(&outcome.dataset), SemanticType::FullName);

    assert_eq!(before_email, after_email);
    assert_eq!(before_name, after_name);
}

/// Mirrors the P2 privacy regression tests: a column profile and a fidelity
/// report are compliance artifacts, and must never carry a source value that
/// tripped a detector, however distinctive that value is.
#[test]
fn anomaly_reporting_never_leaks_the_values_it_flagged() {
    let dataset = fixture();

    let profile = anonymizer::profile(&dataset, &Policy::default()).expect("profile");
    let profile_json = serde_json::to_string(&profile).expect("profile serializes");

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let report_json = serde_json::to_string(&outcome.report).expect("report serializes");

    for leaked in [
        "not-an-email",
        "also@@broken.com",
        "ALL CAPS NAME",
        "all lower name",
    ] {
        assert!(!profile_json.contains(leaked), "profile leaked `{leaked}`");
        assert!(!report_json.contains(leaked), "report leaked `{leaked}`");
    }
}

#[test]
fn profile_surfaces_anomaly_rates_per_kind() {
    let dataset = fixture();
    let profile = anonymizer::profile(&dataset, &Policy::default()).expect("profile");

    let email = profile
        .columns
        .get("customers.email")
        .expect("email column profiled");
    assert_eq!(
        email.anomaly_rates.get(&AnomalyKind::MalformedEmail),
        Some(&(2.0 / 6.0))
    );

    let full_name = profile
        .columns
        .get("customers.full_name")
        .expect("full_name column profiled");
    assert_eq!(
        full_name.anomaly_rates.get(&AnomalyKind::MalformedName),
        Some(&(2.0 / 6.0))
    );
}

const NULL_SENTINELS: &[&str] = &["null", "n/a", "na", "none", "undefined", "nil", "-", "--"];

/// Two hundred clean addresses with every null sentinel written into the
/// column a few times over, so whichever sentinel re-injection draws, some
/// row of the source already holds it.
fn email_column_with_every_sentinel() -> Dataset {
    let mut values: Vec<String> = (0..200)
        .map(|i| format!("cliente{i}@example.com"))
        .collect();
    for (index, sentinel) in NULL_SENTINELS.iter().cycle().take(24).enumerate() {
        values[index * 8] = (*sentinel).to_owned();
    }
    let values: Vec<&str> = values.iter().map(String::as_str).collect();
    let ids: Vec<i64> = (1..=200).collect();
    Dataset {
        tables: vec![Table {
            name: "customers".into(),
            columns: vec![id_column(&ids), text_column("email", &values)],
        }],
    }
}

/// An email column is compared against its whole source value set, so a
/// re-injected sentinel that any row of the source already holds would be
/// flagged at every strength. The column must still pass, with every
/// sentinel re-injected.
#[test]
fn a_re_injected_sentinel_never_equals_any_source_value_of_an_email_column() {
    let dataset = email_column_with_every_sentinel();
    let profile = anonymizer::profile(&dataset, &Policy::default()).expect("profile");
    assert_eq!(
        profile.columns["customers.email"].semantic_type,
        SemanticType::Email
    );
    let before = counts(email_column(&dataset), SemanticType::Email);
    assert_eq!(before.get(&AnomalyKind::UnexpectedNull), Some(&24));

    for seed in 1..11 {
        let policy = Policy {
            seed: Some(seed),
            ..Default::default()
        };
        let outcome = anonymizer::anonymize(dataset.clone(), policy).expect("anonymize");
        assert_eq!(
            outcome.report.verdict,
            anonymizer::compliance::Verdict::Pass,
            "seed {seed}"
        );
        let after = counts(email_column(&outcome.dataset), SemanticType::Email);
        assert_eq!(
            after.get(&AnomalyKind::UnexpectedNull),
            before.get(&AnomalyKind::UnexpectedNull),
            "seed {seed}"
        );
    }
}

/// First names the resampling corpus never draws, so no resampled name lands
/// on its own row's source and no re-injected name needs a fallback shape.
const FIRST_NAMES_OUTSIDE_THE_CORPUS: &[&str] =
    &["Ulisses", "Valter", "Wagner", "Xavier", "Yara", "Zuleica"];
const SURNAMES: &[&str] = &[
    "Souza", "Lima", "Dias", "Pereira", "Costa", "Rocha", "Alves", "Nunes", "Campos", "Lopes",
];
const MIXED_ROWS: usize = 60;
const NAME_SEEDS: std::ops::Range<u64> = 0..20;

fn name_outside_the_corpus(row: usize) -> String {
    let first = FIRST_NAMES_OUTSIDE_THE_CORPUS[row % FIRST_NAMES_OUTSIDE_THE_CORPUS.len()];
    let surname = SURNAMES[(row / FIRST_NAMES_OUTSIDE_THE_CORPUS.len()) % SURNAMES.len()];
    format!("{first} {surname}")
}

/// Sixty names, spread over the rows: nine all caps, four all lowercase, two
/// with a digit, two all caps with a digit, one all lowercase with a digit,
/// and the rest well formed.
fn skewed_malformed_names() -> Vec<String> {
    (0..MIXED_ROWS)
        .map(|row| {
            let name = name_outside_the_corpus(row);
            match (row * 7) % MIXED_ROWS {
                0..9 => name.to_uppercase(),
                9..13 => name.to_lowercase(),
                13..15 => format!("{name}7"),
                15..17 => format!("{}3", name.to_uppercase()),
                17 => format!("{}5", name.to_lowercase()),
                _ => name,
            }
        })
        .collect()
}

fn full_name_table(names: &[String]) -> Dataset {
    let ids: Vec<i64> = (1..=names.len() as i64).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    Dataset {
        tables: vec![Table {
            name: "customers".into(),
            columns: vec![id_column(&ids), text_column("full_name", &names)],
        }],
    }
}

fn full_name_scan(column: &ColumnData) -> anomaly::AnomalyScan {
    let classification = Classification {
        semantic_type: SemanticType::FullName,
        confidence: 1.0,
    };
    let column_profile = profile::profile_column(&column.spec.name, column, classification);
    anomaly::detect(column, &column_profile)
}

fn assert_resampled_as_full_names(outcome: &anonymizer::Outcome, seed: u64) {
    let report = outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == "customers.full_name")
        .expect("full_name reported");
    assert_eq!(report.semantic_type, SemanticType::FullName, "seed {seed}");
    assert_eq!(report.strategy, Strategy::ResampleName, "seed {seed}");
}

#[test]
fn the_mix_of_malformed_name_shapes_survives_the_pipeline_at_the_same_rows() {
    let dataset = full_name_table(&skewed_malformed_names());
    let before = full_name_scan(full_name_column(&dataset));
    assert_eq!(
        before.name_shape_counts(),
        BTreeMap::from([
            (NameShape::AllUppercase, 11),
            (NameShape::AllLowercase, 5),
            (NameShape::EmbeddedDigit, 5),
        ])
    );
    assert_eq!(
        before.counts(),
        BTreeMap::from([(AnomalyKind::MalformedName, 18)])
    );

    for seed in NAME_SEEDS {
        let policy = Policy {
            seed: Some(seed),
            ..Default::default()
        };
        let outcome = anonymizer::anonymize(dataset.clone(), policy).expect("anonymize");
        assert_resampled_as_full_names(&outcome, seed);
        assert_eq!(
            outcome.report.verdict,
            anonymizer::compliance::Verdict::Pass,
            "seed {seed}"
        );

        let after = full_name_scan(full_name_column(&outcome.dataset));
        assert_eq!(after.name_shapes, before.name_shapes, "seed {seed}");
        assert_eq!(
            after.name_shape_counts(),
            before.name_shape_counts(),
            "seed {seed}"
        );
        assert_eq!(after.counts(), before.counts(), "seed {seed}");
    }
}

#[test]
fn an_all_caps_name_column_comes_back_all_caps() {
    let source: Vec<String> = (0..MIXED_ROWS)
        .map(|row| name_outside_the_corpus(row).to_uppercase())
        .collect();
    let dataset = full_name_table(&source);

    for seed in NAME_SEEDS {
        let policy = Policy {
            seed: Some(seed),
            ..Default::default()
        };
        let outcome = anonymizer::anonymize(dataset.clone(), policy).expect("anonymize");
        assert_resampled_as_full_names(&outcome, seed);

        for (row, value) in full_name_column(&outcome.dataset).values.iter().enumerate() {
            let text = value.as_text().expect("a name stays text");
            assert!(
                text.chars().any(|c| c.is_ascii_alphabetic())
                    && text
                        .chars()
                        .filter(char::is_ascii_alphabetic)
                        .all(|c| c.is_ascii_uppercase()),
                "seed {seed}: row {row} came back as {text:?}"
            );
            assert!(
                !text.chars().any(|c| c.is_ascii_digit()),
                "seed {seed}: row {row} came back as {text:?}"
            );
            assert_ne!(
                text.to_lowercase(),
                source[row].to_lowercase(),
                "seed {seed}: row {row} reproduced its source"
            );
        }
    }
}
