//! A CPF or CNPJ stored in an `Integer` column anonymizes to fresh,
//! check-digit-valid documents that are still integers, and a column whose
//! declared type cannot run its classified strategy never fails the run.

use std::collections::{BTreeMap, BTreeSet};

use anonymizer::classify::{SemanticType, is_valid_cnpj, is_valid_cpf};
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
use anonymizer::transform::Strategy;
use anonymizer::{Dataset, Policy, Table, Value};

const SEED: u64 = 7;

fn policy() -> Policy {
    Policy {
        seed: Some(SEED),
        ..Policy::default()
    }
}

fn column(name: &str, ty: ColumnType, values: Vec<Value>) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty,
            nullable: values.iter().any(Value::is_null),
            pk: false,
            fk: None,
        },
        values,
    }
}

fn int_column(name: &str, values: &[i64]) -> ColumnData {
    column(
        name,
        ColumnType::Integer,
        values.iter().copied().map(Value::Int).collect(),
    )
}

fn digits_of(text: &str) -> Vec<u32> {
    text.chars().filter_map(|c| c.to_digit(10)).collect()
}

fn to_integer(digits: &[u32]) -> i64 {
    digits.iter().fold(0i64, |acc, d| acc * 10 + i64::from(*d))
}

fn cpf_check_digit(digits: &[u32]) -> u32 {
    let first_weight = digits.len() as u32 + 1;
    let sum: u32 = digits
        .iter()
        .enumerate()
        .map(|(i, d)| d * (first_weight - i as u32))
        .sum();
    let remainder = (sum * 10) % 11;
    if remainder == 10 { 0 } else { remainder }
}

fn cnpj_check_digit(digits: &[u32]) -> u32 {
    const WEIGHTS: [u32; 13] = [6, 5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2];
    let offset = WEIGHTS.len() - digits.len();
    let sum: u32 = digits
        .iter()
        .enumerate()
        .map(|(i, d)| d * WEIGHTS[offset + i])
        .sum();
    let remainder = sum % 11;
    if remainder < 2 { 0 } else { 11 - remainder }
}

/// The nine-digit `base` (zero-padded) with its two CPF check digits,
/// stored the way an integer column stores it: without leading zeros.
fn cpf_integer(base: usize) -> i64 {
    let mut digits = digits_of(&format!("{base:09}"));
    for _ in 0..2 {
        digits.push(cpf_check_digit(&digits));
    }
    to_integer(&digits)
}

fn cnpj_integer(base: usize) -> i64 {
    let mut digits = digits_of(&format!("{base:08}0001"));
    for _ in 0..2 {
        digits.push(cnpj_check_digit(&digits));
    }
    to_integer(&digits)
}

fn cpf_integers(rows: usize) -> Vec<i64> {
    (0..rows)
        .map(|i| cpf_integer(123_456_000 + i * 37))
        .collect()
}

fn cnpj_integers(rows: usize) -> Vec<i64> {
    (0..rows)
        .map(|i| cnpj_integer(11_222_333 + i * 11))
        .collect()
}

fn single(name: &str, column_name: &str, values: &[i64]) -> Dataset {
    Dataset {
        tables: vec![Table {
            name: name.to_owned(),
            columns: vec![int_column(column_name, values)],
        }],
    }
}

fn ints(table: &Table, name: &str) -> Vec<i64> {
    table
        .column(name)
        .expect("column present")
        .values
        .iter()
        .map(|value| match value {
            Value::Int(n) => *n,
            other => panic!("expected an integer, got {other:?}"),
        })
        .collect()
}

fn padded(n: i64, width: usize) -> String {
    format!("{n:0width$}")
}

#[test]
fn an_integer_cpf_column_anonymizes_to_valid_cpfs_stored_as_integers() {
    let source = cpf_integers(30);
    let outcome = anonymizer::anonymize(single("clientes", "cpf", &source), policy())
        .expect("an integer cpf column anonymizes");
    let table = &outcome.dataset.tables[0];

    assert_eq!(
        table.column("cpf").expect("cpf").spec.ty,
        ColumnType::Integer
    );
    let output = ints(table, "cpf");
    assert_eq!(output.len(), source.len());
    let originals: BTreeSet<i64> = source.iter().copied().collect();
    for value in &output {
        assert!(
            is_valid_cpf(&padded(*value, 11)),
            "{value} is not a check-digit-valid CPF"
        );
        assert!(!originals.contains(value), "{value} is a source value");
    }
}

#[test]
fn an_integer_cpf_column_is_still_reported_as_a_cpf() {
    let outcome = anonymizer::anonymize(single("clientes", "cpf", &cpf_integers(30)), policy())
        .expect("anonymizes");
    let report = outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == "clientes.cpf")
        .expect("column reported");
    assert_eq!(report.semantic_type, SemanticType::Cpf);
    assert_eq!(report.strategy, Strategy::RegenerateDocument);
}

#[test]
fn an_integer_cnpj_column_anonymizes_to_valid_cnpjs_stored_as_integers() {
    let source = cnpj_integers(30);
    let outcome = anonymizer::anonymize(single("empresas", "cnpj", &source), policy())
        .expect("an integer cnpj column anonymizes");
    let table = &outcome.dataset.tables[0];
    let originals: BTreeSet<i64> = source.iter().copied().collect();
    for value in ints(table, "cnpj") {
        assert!(
            is_valid_cnpj(&padded(value, 14)),
            "{value} is not a check-digit-valid CNPJ"
        );
        assert!(!originals.contains(&value), "{value} is a source value");
    }
}

#[test]
fn a_cpf_whose_leading_zero_was_dropped_is_still_recognised() {
    let mut source = cpf_integers(30);
    let leading_zero = cpf_integer(12_345_678);
    assert!(leading_zero < 10_000_000_000, "fixture must drop its zero");
    source[0] = leading_zero;
    let outcome =
        anonymizer::anonymize(single("clientes", "cpf", &source), policy()).expect("anonymizes");
    let report = outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == "clientes.cpf")
        .expect("column reported");
    assert_eq!(report.semantic_type, SemanticType::Cpf);
}

#[test]
fn an_integer_cpf_column_made_only_of_dropped_leading_zeros_is_still_a_cpf() {
    let source: Vec<i64> = (0..20).map(|i| cpf_integer(12_345_678 + i * 37)).collect();
    assert!(source.iter().all(|n| *n < 10_000_000_000));
    let outcome =
        anonymizer::anonymize(single("clientes", "cpf", &source), policy()).expect("anonymizes");
    let report = &outcome.report.columns[0];
    assert_eq!(report.semantic_type, SemanticType::Cpf);
    for value in ints(&outcome.dataset.tables[0], "cpf") {
        assert!(is_valid_cpf(&padded(value, 11)));
    }
}

#[test]
fn an_integer_cpf_primary_key_mints_through_the_ledger_and_keeps_referential_integrity() {
    let keys = cpf_integers(6);
    let referencing: Vec<i64> = (0..18).map(|i| keys[i % keys.len()]).collect();
    let mut parent = int_column("cpf", &keys);
    parent.spec.pk = true;
    let mut child = int_column("cpf", &referencing);
    child.spec.fk = Some(ForeignKey {
        table: "clientes".to_owned(),
        column: "cpf".to_owned(),
    });
    let dataset = Dataset {
        tables: vec![
            Table {
                name: "clientes".to_owned(),
                columns: vec![parent],
            },
            Table {
                name: "pedidos".to_owned(),
                columns: vec![child],
            },
        ],
    };
    let outcome = anonymizer::anonymize(dataset, policy()).expect("anonymizes");

    let strategy_of = |name: &str| {
        outcome
            .report
            .columns
            .iter()
            .find(|c| c.column == name)
            .map(|c| c.strategy)
    };
    assert_eq!(strategy_of("clientes.cpf"), Some(Strategy::MintKey));
    assert_eq!(strategy_of("pedidos.cpf"), Some(Strategy::MintKey));

    let parent_out = ints(&outcome.dataset.tables[0], "cpf");
    let child_out = ints(&outcome.dataset.tables[1], "cpf");
    for (i, value) in child_out.iter().enumerate() {
        assert_eq!(*value, parent_out[i % parent_out.len()]);
    }
    let distinct: BTreeSet<i64> = parent_out.iter().copied().collect();
    assert_eq!(distinct.len(), parent_out.len());
}

fn all_null(name: &str, ty: ColumnType) -> Dataset {
    Dataset {
        tables: vec![Table {
            name: "records".to_owned(),
            columns: vec![column(name, ty, vec![Value::Null; 5])],
        }],
    }
}

#[test]
fn an_all_null_non_text_column_never_resolves_a_text_only_strategy() {
    let hint_names = [
        "email",
        "cpf",
        "cnpj",
        "phone",
        "cep",
        "uuid",
        "full_name",
        "nome",
        "telefone",
        "date",
    ];
    let mut failures = Vec::new();
    for ty in [ColumnType::Integer, ColumnType::Float, ColumnType::Boolean] {
        for name in hint_names {
            if let Err(error) = anonymizer::anonymize(all_null(name, ty), policy()) {
                failures.push(format!("{ty:?} `{name}`: {error}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn small_code_dataset() -> Dataset {
    let bank_codes: Vec<i64> = (0..30)
        .map(|i| if i % 3 == 2 { 250 } else { 191 })
        .collect();
    Dataset {
        tables: vec![Table {
            name: "pedidos".to_owned(),
            columns: vec![
                int_column("status", &[191; 30]),
                int_column("codigo_banco", &bank_codes),
            ],
        }],
    }
}

#[test]
fn small_integer_codes_are_not_mistaken_for_documents() {
    let outcome = anonymizer::anonymize(small_code_dataset(), policy()).expect("anonymizes");
    for report in &outcome.report.columns {
        assert_eq!(
            report.semantic_type,
            SemanticType::NumericId,
            "{} was classified {:?}",
            report.column,
            report.semantic_type
        );
    }
}

#[test]
fn small_integer_codes_can_be_preserved() {
    let policy = Policy {
        seed: Some(SEED),
        preserve: BTreeSet::from(["pedidos".to_owned()]),
        ..Policy::default()
    };
    let outcome = anonymizer::anonymize(small_code_dataset(), policy)
        .expect("a table of small integer codes is preservable");
    assert_eq!(ints(&outcome.dataset.tables[0], "status"), vec![191; 30]);
}

fn small_root_cnpj_integers(rows: usize) -> Vec<i64> {
    (0..rows).map(|i| cnpj_integer(100 + i)).collect()
}

#[test]
fn a_cnpj_named_column_of_small_root_cnpjs_is_classified_cnpj() {
    let source = small_root_cnpj_integers(30);
    assert!(
        source
            .iter()
            .all(|n| (100_000_000..1_000_000_000).contains(n))
    );
    let outcome =
        anonymizer::anonymize(single("empresas", "cnpj", &source), policy()).expect("anonymizes");
    assert_eq!(outcome.report.columns[0].semantic_type, SemanticType::Cnpj);
}

#[test]
fn a_cnpj_column_containing_short_integers_emits_only_valid_cnpjs() {
    let mut source = small_root_cnpj_integers(30);
    source[0] = cnpj_integer(0);
    assert_eq!(source[0], 191);
    let outcome =
        anonymizer::anonymize(single("empresas", "cnpj", &source), policy()).expect("anonymizes");
    assert_eq!(outcome.report.columns[0].semantic_type, SemanticType::Cnpj);
    for value in ints(&outcome.dataset.tables[0], "cnpj") {
        assert!(
            is_valid_cnpj(&padded(value, 14)),
            "{value} is not a valid CNPJ"
        );
    }
}

#[test]
fn a_redact_override_on_an_integer_cpf_column_fails_the_leakage_check() {
    let policy = Policy {
        seed: Some(SEED),
        column_overrides: BTreeMap::from([("clientes.cpf".to_owned(), Strategy::Redact)]),
        ..Policy::default()
    };
    let result = anonymizer::anonymize(single("clientes", "cpf", &cpf_integers(30)), policy);
    let leaked = match result {
        Ok(outcome) => outcome.report.verdict == anonymizer::compliance::Verdict::Fail,
        Err(error) => panic!("expected a failed verdict, got {error}"),
    };
    assert!(
        leaked,
        "an unchanged integer CPF column must not pass leakage"
    );
}

#[test]
#[ignore = "TODO: fine tune to catch this — an integer document column is never scanned for malformed documents, so bad check digits are neither detected nor re-injected"]
fn bad_check_digits_in_an_integer_cpf_column_are_detected_and_survive_the_run() {
    use anonymizer::anomaly::{self, AnomalyKind};
    use anonymizer::classify::Classification;

    let mut source = cpf_integers(30);
    for value in source.iter_mut().take(3) {
        *value += 1;
    }
    let malformed = |dataset: &Dataset| {
        let column = dataset.tables[0].column("cpf").expect("cpf column");
        let classification = Classification {
            semantic_type: SemanticType::Cpf,
            confidence: 1.0,
        };
        let profile = anonymizer::profile::profile_column("clientes.cpf", column, classification);
        anomaly::detect(column, &profile)
            .counts()
            .get(&AnomalyKind::MalformedDocument)
            .copied()
            .unwrap_or(0)
    };

    let dataset = single("clientes", "cpf", &source);
    assert_eq!(malformed(&dataset), 3);
    let outcome = anonymizer::anonymize(dataset, policy()).expect("anonymizes");
    assert_eq!(malformed(&outcome.dataset), 3);
}
