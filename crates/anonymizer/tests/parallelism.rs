use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
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

fn texts(values: impl IntoIterator<Item = String>) -> Vec<Value> {
    values.into_iter().map(Value::Text).collect()
}

/// Wide enough (two tables, a dozen-plus columns of every transformed
/// shape, two dozen rows each) that an 8-thread rayon pool actually splits
/// the column-parallel stages across more than one worker, which is what
/// would expose a mutable ledger shared under `par_iter`.
fn fixture() -> Dataset {
    let n: i64 = 24;

    Dataset {
        tables: vec![
            Table {
                name: "customers".into(),
                columns: vec![
                    column(
                        "id",
                        ColumnType::Integer,
                        false,
                        true,
                        None,
                        (1..=n).map(Value::Int).collect(),
                    ),
                    column(
                        "email",
                        ColumnType::Text,
                        false,
                        false,
                        None,
                        texts((0..n).map(|i| {
                            let domain =
                                ["gmail.com", "outlook.com", "yahoo.com"][(i % 3) as usize];
                            format!("user{i}@{domain}")
                        })),
                    ),
                    column(
                        "full_name",
                        ColumnType::Text,
                        false,
                        false,
                        None,
                        texts((0..n).map(|i| format!("Pessoa Numero{i}"))),
                    ),
                    column(
                        "phone",
                        ColumnType::Text,
                        false,
                        false,
                        None,
                        texts((0..n).map(|i| format!("(11) 9{i:04}-{i:04}"))),
                    ),
                    column(
                        "cpf",
                        ColumnType::Text,
                        false,
                        false,
                        None,
                        texts(std::iter::repeat_n("111.444.777-35".to_owned(), n as usize)),
                    ),
                    column(
                        "signup_date",
                        ColumnType::Text,
                        false,
                        false,
                        None,
                        texts(std::iter::repeat_n("2021-05-15".to_owned(), n as usize)),
                    ),
                    column(
                        "age",
                        ColumnType::Integer,
                        false,
                        false,
                        None,
                        (0..n).map(|i| Value::Int(20 + i % 40)).collect(),
                    ),
                    column(
                        "notes",
                        ColumnType::Text,
                        true,
                        false,
                        None,
                        texts((0..n).map(|i| format!("free text note number {i}"))),
                    ),
                ],
            },
            Table {
                name: "orders".into(),
                columns: vec![
                    column(
                        "id",
                        ColumnType::Integer,
                        false,
                        true,
                        None,
                        (1000..1000 + n).map(Value::Int).collect(),
                    ),
                    column(
                        "customer_id",
                        ColumnType::Integer,
                        false,
                        false,
                        Some(ForeignKey {
                            table: "customers".into(),
                            column: "id".into(),
                        }),
                        (0..n).map(|i| Value::Int(1 + i % n)).collect(),
                    ),
                    column(
                        "total",
                        ColumnType::Float,
                        false,
                        false,
                        None,
                        (0..n)
                            .map(|i| Value::Float(10.0 + i as f64 * 1.5))
                            .collect(),
                    ),
                ],
            },
        ],
    }
}

fn run_with_threads(n: usize, dataset: Dataset, policy: Policy) -> String {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build()
        .expect("thread pool builds");
    pool.install(|| {
        let outcome = anonymizer::anonymize(dataset, policy).expect("anonymize");
        let mut report = outcome.report.clone();
        report.timings_ms.clear();
        serde_json::to_string(&(&outcome.dataset, &report)).expect("serialize outcome")
    })
}

/// Functional test #9: the same pinned seed under a single-threaded and an
/// eight-threaded rayon pool must produce byte-identical output. This is
/// specifically the test that catches a mutable ledger shared under
/// `par_iter` — the ledger here is built single-threaded before either
/// pool touches it, so thread count must not matter.
#[test]
fn pinned_seed_is_byte_identical_under_one_and_eight_rayon_threads() {
    let policy = Policy {
        seed: Some(20_240_101),
        column_overrides: Default::default(),
    };

    let single_threaded = run_with_threads(1, fixture(), policy.clone());
    let eight_threaded = run_with_threads(8, fixture(), policy);

    assert_eq!(single_threaded, eight_threaded);
}
