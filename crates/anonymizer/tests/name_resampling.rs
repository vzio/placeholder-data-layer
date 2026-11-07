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
fn a_large_column_of_names_outside_a_small_corpus_slice_passes_and_stays_varied() {
    let escalated = assert_every_seed_passes_with_varied_names(&[
        "Tiago Ribeiro",
        "Paula Nunes",
        "Hugo Barros",
    ]);
    assert!(escalated > 0, "no run escalated");
}

#[test]
fn a_large_column_of_names_inside_a_small_corpus_slice_passes_and_stays_varied() {
    let escalated =
        assert_every_seed_passes_with_varied_names(&["Ana Silva", "Bruno Souza", "Carla Lima"]);
    assert!(escalated > 0, "no run escalated");
}
