use std::collections::BTreeMap;

use anonymizer::anomaly::{self, AnomalyKind};
use anonymizer::classify::{Classification, SemanticType};
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType};
use anonymizer::profile;
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
