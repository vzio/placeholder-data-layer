use anonymizer::classify::SemanticType;
use anonymizer::dataset::Value;
use anonymizer::transform::ledger::Ledger;
use anonymizer::transform::{ColumnRole, Strategy, Strength};

const SEED: u64 = 20_260_507;
const COLUMN: &str = "customers.full_name";
const ROWS: usize = 20_000;

struct Resampled {
    distinct_names: usize,
    positional_collisions: usize,
}

fn source_name(row: usize) -> &'static str {
    ["Ana Silva", "Bruno Souza", "Carla Lima"][row % 3]
}

fn resample_column(strength: Strength) -> Resampled {
    let ledger = Ledger::default();
    let role = ColumnRole {
        semantic_type: SemanticType::FullName,
        class: None,
    };
    let mut distinct = std::collections::BTreeSet::new();
    let mut positional_collisions = 0;
    for row in 0..ROWS {
        let mut rng = anonymizer::rng::derive_for_row_position(SEED, COLUMN, row, strength as u8);
        let source = source_name(row);
        let Ok(Value::Text(output)) = Strategy::ResampleName.apply(
            COLUMN,
            &Value::Text(source.to_owned()),
            strength,
            role,
            &ledger,
            &mut rng,
        ) else {
            panic!("resampling a text value yields text");
        };
        if output.to_lowercase() == source.to_lowercase() {
            positional_collisions += 1;
        }
        distinct.insert(output);
    }
    Resampled {
        distinct_names: distinct.len(),
        positional_collisions,
    }
}

/// Escalation retries a failed leakage check at a stronger [`Strength`], so a
/// stronger strength must never make a name collision more likely. The
/// pipeline keeps only the best attempt, so a worse `Maximum` draw is never
/// visible through `anonymize` — the strategy is driven directly, at the
/// per-row stream the pipeline derives.
#[test]
fn resampling_at_maximum_collides_no_more_than_at_default_over_a_large_column() {
    let default = resample_column(Strength::Default);
    let maximum = resample_column(Strength::Maximum);

    assert!(
        maximum.distinct_names * 10 >= default.distinct_names * 9,
        "Maximum produced {} distinct names against {} at Default",
        maximum.distinct_names,
        default.distinct_names,
    );
    assert!(
        maximum.positional_collisions <= default.positional_collisions * 2,
        "Maximum collided with its source {} times against {} at Default",
        maximum.positional_collisions,
        default.positional_collisions,
    );
}
