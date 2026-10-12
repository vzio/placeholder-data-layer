use anonymizer::classify::SemanticType;
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType};
use anonymizer::profile::Distribution;
use anonymizer::{Dataset, Policy, Table, Value};

fn email_column(domains: &[(&str, usize)]) -> ColumnData {
    let mut values = Vec::new();
    for (domain, count) in domains {
        for i in 0..*count {
            values.push(Value::Text(format!("user{i}@{domain}")));
        }
    }
    ColumnData {
        spec: ColumnSpec {
            name: "email".into(),
            ty: ColumnType::Text,
            nullable: false,
            pk: false,
            fk: None,
        },
        values,
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
        values: values.iter().map(|v| Value::Text(v.to_string())).collect(),
    }
}

/// Functional test #1: the email distribution is computed over the domain,
/// not the address, and the proportions match exactly. Passthrough makes
/// before and after trivially equal today; once P4 lands real transforms,
/// this still has to hold because the domain is kept verbatim.
#[test]
fn email_domain_distribution_matches_exact_proportions() {
    let dataset = Dataset {
        tables: vec![Table {
            name: "customers".into(),
            columns: vec![email_column(&[
                ("gmail.com", 54),
                ("outlook.com", 21),
                ("hotmail.com", 12),
                ("yahoo.com", 13),
            ])],
        }],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");
    let report = outcome
        .report
        .columns
        .iter()
        .find(|column| column.column == "customers.email")
        .expect("email column is reported");

    assert_eq!(report.semantic_type, SemanticType::Email);
    assert_eq!(report.distribution_before.get("gmail.com"), Some(&0.54));
    assert_eq!(report.distribution_before.get("outlook.com"), Some(&0.21));
    assert_eq!(report.distribution_before.get("hotmail.com"), Some(&0.12));
    assert_eq!(report.distribution_before.get("yahoo.com"), Some(&0.13));
    assert!(
        report
            .distribution_before
            .keys()
            .all(|key| !key.contains('@'))
    );
    assert_eq!(report.distribution_before, report.distribution_after);
    assert_eq!(report.total_variation_distance, 0.0);
}

/// A column whose name lies about its contents, in both directions: one
/// named like a generic document but holding real CPFs, one named `cpf` but
/// holding none. Classification must follow the values, not the name.
#[test]
fn classification_follows_values_over_a_misleading_column_name() {
    let dataset = Dataset {
        tables: vec![Table {
            name: "customers".into(),
            columns: vec![
                text_column(
                    "documento",
                    &["111.444.777-35", "123.456.789-09", "529.982.247-25"],
                ),
                text_column("cpf", &["not-a-cpf", "also garbage", "still junk"]),
            ],
        }],
    };

    let profile = anonymizer::profile(&dataset, &Policy::default()).expect("profile");

    let documento = profile
        .columns
        .get("customers.documento")
        .expect("documento column profiled");
    assert_eq!(documento.semantic_type, SemanticType::Cpf);
    assert!(
        documento.confidence > 0.9,
        "confidence was {}",
        documento.confidence
    );

    let cpf = profile
        .columns
        .get("customers.cpf")
        .expect("cpf column profiled");
    assert_ne!(cpf.semantic_type, SemanticType::Cpf);
    assert!(matches!(
        cpf.distribution,
        Distribution::Categorical(_) | Distribution::Unsummarised
    ));
}
