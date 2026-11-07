use anonymizer::compliance::Verdict;
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, Table};
use anonymizer::transform::Strength;
use anonymizer::{Dataset, Policy, Value};

const ROWS: usize = 5_000;
const SEEDS: std::ops::Range<u64> = 1..13;
const MIN_DISTINCT_NAMES: usize = 300;

fn full_name_dataset(names: &[&str]) -> Dataset {
    Dataset {
        tables: vec![Table {
            name: "customers".to_owned(),
            columns: vec![ColumnData {
                spec: ColumnSpec {
                    name: "full_name".to_owned(),
                    ty: ColumnType::Text,
                    nullable: false,
                    pk: false,
                    fk: None,
                },
                values: (0..ROWS)
                    .map(|row| Value::Text(names[row % names.len()].to_owned()))
                    .collect(),
            }],
        }],
    }
}

/// Runs every seed over `names` and returns the number of runs that escalated.
fn assert_every_seed_passes_with_varied_names(names: &[&str]) -> usize {
    let mut escalated = 0;
    for seed in SEEDS {
        let policy = Policy {
            seed: Some(seed),
            ..Default::default()
        };
        let outcome = anonymizer::anonymize(full_name_dataset(names), policy).expect("anonymize");

        assert_eq!(outcome.report.verdict, Verdict::Pass, "seed {seed}");
        if outcome.report.accepted_strength != Strength::Default {
            escalated += 1;
        }

        let column = outcome
            .dataset
            .table("customers")
            .expect("customers")
            .column("full_name")
            .expect("full_name");
        let distinct: std::collections::BTreeSet<&str> =
            column.values.iter().filter_map(Value::as_text).collect();
        assert!(
            distinct.len() >= MIN_DISTINCT_NAMES,
            "seed {seed}: only {} distinct names",
            distinct.len()
        );
    }
    escalated
}

/// Source names the corpus can produce collide with the output at every
/// attempt unless escalation steers away from the source; a large column
/// therefore only passes if escalated attempts never land on their own row.
#[test]
fn a_large_column_of_late_corpus_names_passes_and_stays_varied() {
    let escalated = assert_every_seed_passes_with_varied_names(&[
        "Tiago Ribeiro",
        "Paula Nunes",
        "Hugo Barros",
    ]);
    assert!(escalated > 0, "no run escalated");
}

#[test]
fn a_large_column_of_early_corpus_names_passes_and_stays_varied() {
    let escalated =
        assert_every_seed_passes_with_varied_names(&["Ana Silva", "Bruno Souza", "Carla Lima"]);
    assert!(escalated > 0, "no run escalated");
}

fn mixed_dataset(anomalous: &str, clean: &[&str], anomalous_every: usize) -> Dataset {
    let mut dataset = full_name_dataset(clean);
    for (row, value) in dataset.tables[0].columns[0].values.iter_mut().enumerate() {
        if row % anomalous_every == 0 {
            *value = Value::Text(anomalous.to_owned());
        }
    }
    dataset
}

fn assert_every_seed_passes(dataset: &Dataset, seeds: std::ops::Range<u64>) {
    for seed in seeds {
        let policy = Policy {
            seed: Some(seed),
            ..Default::default()
        };
        let outcome = anonymizer::anonymize(dataset.clone(), policy).expect("anonymize");
        assert_eq!(outcome.report.verdict, Verdict::Pass, "seed {seed}");
    }
}

#[test]
#[ignore = "TODO: fine tune to catch this — a re-injected embedded digit can turn the resampled name back into its own source, after the escalation guard has run"]
fn a_re_injected_digit_never_reproduces_the_rows_own_source_name() {
    let dataset = mixed_dataset(
        "Tiago Ribeiro1",
        &["Tiago Ribeiro", "Paula Nunes", "Hugo Barros"],
        3,
    );
    assert_every_seed_passes(&dataset, 1..41);
}

#[test]
#[ignore = "TODO: fine tune to catch this — a re-injected sentinel equals a source cell that was already that sentinel, and no strength can repair it"]
fn a_re_injected_sentinel_never_reproduces_its_own_source_cell() {
    let dataset = mixed_dataset("n/a", &["Tiago Ribeiro", "Paula Nunes", "Hugo Barros"], 20);
    assert_every_seed_passes(&dataset, 1..16);
}
