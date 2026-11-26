//! Numeric output held inside its source range, and `Strategy::Shuffle` on
//! low-cardinality columns, through the public API.

use anonymizer::anomaly::AnomalyKind;
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
use anonymizer::transform::Strategy;
use anonymizer::{Dataset, Outcome, Policy, Table, Value, anonymize};

const PINNED_SEED: u64 = 7_331;
const ROWS: usize = 60;

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

fn id_column(rows: usize) -> ColumnData {
    let mut id = column(
        "id",
        ColumnType::Integer,
        (1..=rows as i64).map(Value::Int).collect(),
    );
    id.spec.pk = true;
    id
}

fn cycled_texts(vocabulary: &[&str], rows: usize) -> Vec<Value> {
    (0..rows)
        .map(|i| Value::Text(vocabulary[i % vocabulary.len()].to_owned()))
        .collect()
}

/// Order totals clustered around a few hundred, with two far outliers the
/// detector flags as out of range.
fn order_totals() -> Vec<Value> {
    let mut values: Vec<Value> = (0..ROWS)
        .map(|i| Value::Float(300.0 + (i % 17) as f64 * 11.5))
        .collect();
    values[7] = Value::Float(25_000.0);
    values[41] = Value::Float(31_000.0);
    values[3] = Value::Float(0.0);
    values
}

fn ratings() -> Vec<Value> {
    (0..ROWS).map(|i| Value::Int((i % 5) as i64 + 1)).collect()
}

fn fixture() -> Dataset {
    let mut status = cycled_texts(&["pago", "pendente", "cancelado", "estornado"], ROWS);
    status[5] = Value::Null;
    status[30] = Value::Null;
    Dataset {
        tables: vec![
            Table {
                name: "pedidos".into(),
                columns: vec![
                    id_column(ROWS),
                    column("status", ColumnType::Text, status),
                    column("nota", ColumnType::Integer, ratings()),
                    column("valor_total", ColumnType::Float, order_totals()),
                    column(
                        "quantidade",
                        ColumnType::Integer,
                        (0..ROWS as i64).map(|i| Value::Int(10 + i * 3)).collect(),
                    ),
                    column(
                        "codigo_rastreio",
                        ColumnType::Text,
                        (0..ROWS)
                            .map(|i| Value::Text(format!("BR{i:09}")))
                            .collect(),
                    ),
                    column(
                        "observacao",
                        ColumnType::Text,
                        (0..ROWS)
                            .map(|i| Value::Text(if i == 0 { "urgente" } else { "normal" }.into()))
                            .collect(),
                    ),
                    column(
                        "created_by",
                        ColumnType::Text,
                        cycled_texts(&["ana.lima", "bruno.reis", "carla.dias"], ROWS),
                    ),
                    column("canal", ColumnType::Text, cycled_texts(&["loja"], ROWS)),
                ],
            },
            Table {
                name: "itens".into(),
                columns: vec![id_column(ROWS), {
                    let mut fk = column(
                        "pedido_id",
                        ColumnType::Integer,
                        (0..ROWS).map(|i| Value::Int((i % 3) as i64 + 1)).collect(),
                    );
                    fk.spec.fk = Some(ForeignKey {
                        table: "pedidos".into(),
                        column: "id".into(),
                    });
                    fk
                }],
            },
        ],
    }
}

fn pinned() -> Policy {
    Policy {
        seed: Some(PINNED_SEED),
        ..Policy::default()
    }
}

fn run(dataset: &Dataset, policy: Policy) -> Outcome {
    match anonymize(dataset.clone(), policy) {
        Ok(outcome) => outcome,
        Err(error) => panic!("the fixture anonymizes: {error}"),
    }
}

fn values_of<'a>(dataset: &'a Dataset, table: &str, name: &str) -> &'a [Value] {
    let column = dataset
        .tables
        .iter()
        .find(|t| t.name == table)
        .and_then(|t| t.columns.iter().find(|c| c.spec.name == name));
    match column {
        Some(column) => &column.values,
        None => panic!("`{table}.{name}` is in the dataset"),
    }
}

fn strategy_of(outcome: &Outcome, qualified: &str) -> Strategy {
    match outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == qualified)
    {
        Some(report) => report.strategy,
        None => panic!("`{qualified}` is reported"),
    }
}

fn sorted_keys(values: &[Value]) -> Vec<String> {
    let mut keys: Vec<String> = values
        .iter()
        .filter(|value| !value.is_null())
        .map(|value| format!("{value:?}"))
        .collect();
    keys.sort();
    keys
}

#[test]
fn a_low_cardinality_column_is_shuffled_keeping_its_multiset_and_nulls() {
    let dataset = fixture();
    let outcome = run(&dataset, pinned());
    for name in ["status", "nota"] {
        assert_eq!(
            strategy_of(&outcome, &format!("pedidos.{name}")),
            Strategy::Shuffle,
            "for `{name}`"
        );
        let before = values_of(&dataset, "pedidos", name);
        let after = values_of(&outcome.dataset, "pedidos", name);
        assert_ne!(before, after, "`{name}` moved no value");
        assert_eq!(sorted_keys(before), sorted_keys(after), "for `{name}`");
        for (row, (b, a)) in before.iter().zip(after).enumerate() {
            assert_eq!(b.is_null(), a.is_null(), "`{name}` row {row}");
        }
    }
}

#[test]
fn a_column_failing_a_guard_keeps_its_default_strategy() {
    let outcome = run(&fixture(), pinned());
    for (qualified, expected) in [
        ("pedidos.codigo_rastreio", Strategy::Redact),
        ("pedidos.observacao", Strategy::Redact),
        ("pedidos.created_by", Strategy::Redact),
        ("pedidos.canal", Strategy::Redact),
        ("pedidos.quantidade", Strategy::Jitter),
    ] {
        assert_eq!(
            strategy_of(&outcome, qualified),
            expected,
            "for `{qualified}`"
        );
    }
}

#[test]
fn a_low_cardinality_key_is_minted_never_shuffled() {
    let outcome = run(&fixture(), pinned());
    assert_eq!(strategy_of(&outcome, "itens.pedido_id"), Strategy::MintKey);
}

#[test]
fn a_shuffle_override_runs_on_a_column_the_guards_would_refuse() {
    let mut policy = pinned();
    policy
        .column_overrides
        .insert("pedidos.created_by".to_owned(), Strategy::Shuffle);
    let dataset = fixture();
    let outcome = run(&dataset, policy);
    assert_eq!(
        strategy_of(&outcome, "pedidos.created_by"),
        Strategy::Shuffle
    );
    assert_eq!(
        sorted_keys(values_of(&dataset, "pedidos", "created_by")),
        sorted_keys(values_of(&outcome.dataset, "pedidos", "created_by"))
    );
}

#[test]
fn jittered_numbers_stay_inside_the_source_range_with_outliers_reinjected() {
    let dataset = fixture();
    for seed in 0..20 {
        let outcome = run(
            &dataset,
            Policy {
                seed: Some(seed),
                ..Policy::default()
            },
        );
        for value in values_of(&outcome.dataset, "pedidos", "valor_total") {
            let Value::Float(f) = value else {
                panic!("seed {seed}: a float column produced {value:?}");
            };
            assert!(
                (0.0..=31_000.0).contains(f),
                "seed {seed}: {f} left the range"
            );
        }
        for value in values_of(&outcome.dataset, "pedidos", "quantidade") {
            let Value::Int(n) = value else {
                panic!("seed {seed}: an integer column produced {value:?}");
            };
            assert!((10..=187).contains(n), "seed {seed}: {n} left the range");
        }
        let report = outcome
            .report
            .columns
            .iter()
            .find(|c| c.column == "pedidos.valor_total");
        let rates = report.and_then(|r| r.anomalies.get(&AnomalyKind::OutOfRange));
        let Some(rates) = rates else {
            panic!("seed {seed}: the outliers are reported");
        };
        assert!(rates.before > 0.0);
        assert_eq!(rates.before, rates.after, "seed {seed}");
    }
}

#[test]
fn shuffle_and_range_output_is_identical_across_thread_counts() {
    let dataset = fixture();
    let in_pool = |threads: usize| {
        let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
            Ok(pool) => pool,
            Err(error) => panic!("a {threads}-thread pool builds: {error}"),
        };
        pool.install(|| run(&dataset, pinned()))
    };
    let single = in_pool(1);
    let eight = in_pool(8);
    assert_eq!(single.dataset, eight.dataset);
    assert_eq!(
        serde_json::to_string(&single.report.columns).ok(),
        serde_json::to_string(&eight.report.columns).ok()
    );
}
