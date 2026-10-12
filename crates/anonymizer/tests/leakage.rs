use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType};
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
        values: values
            .iter()
            .map(|v| Value::Text((*v).to_owned()))
            .collect(),
    }
}

fn pk_column(values: &[i64]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: "id".to_owned(),
            ty: ColumnType::Integer,
            nullable: false,
            pk: true,
            fk: None,
        },
        values: values.iter().copied().map(Value::Int).collect(),
    }
}

/// Every direct-identifier value this fixture carries, so the test can
/// assert none of them survive into the output under any column. Includes
/// two values per direct-identifier column shaped nothing like the column's
/// semantic type (`phone`, `cpf`) — the production-dirty rows that used to
/// fall through every transform's shape-mismatch branch and survive
/// byte-identical (C2).
fn source_values() -> Vec<&'static str> {
    vec![
        "ana.souza@gmail.com",
        "bruno.lima@outlook.com",
        "carla.dias@yahoo.com.br",
        "Ana Paula Souza",
        "Bruno Henrique Lima",
        "Carla Regina Dias",
        "(11) 98765-4321",
        "(21) 91234-5678",
        "(31) 99876-5432",
        "NAO TEM TELEFONE",
        "ligar no trabalho",
        "111.444.777-35",
        "123.456.789-09",
        "529.982.247-25",
        "123.456.789-0",
        "SEM DOCUMENTO",
    ]
}

fn fixture() -> Dataset {
    Dataset {
        tables: vec![Table {
            name: "customers".into(),
            columns: vec![
                pk_column(&[1, 2, 3, 4, 5]),
                text_column(
                    "email",
                    &[
                        "ana.souza@gmail.com",
                        "bruno.lima@outlook.com",
                        "carla.dias@yahoo.com.br",
                        "diana.alves@gmail.com",
                        "erica.nunes@hotmail.com",
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
                    ],
                ),
                text_column(
                    "phone",
                    &[
                        "(11) 98765-4321",
                        "(21) 91234-5678",
                        "(31) 99876-5432",
                        "NAO TEM TELEFONE",
                        "ligar no trabalho",
                    ],
                ),
                text_column(
                    "cpf",
                    &[
                        "111.444.777-35",
                        "123.456.789-09",
                        "529.982.247-25",
                        "123.456.789-0",
                        "SEM DOCUMENTO",
                    ],
                ),
            ],
        }],
    }
}

/// Functional test #5 / no-leakage guard: no raw source value for any
/// direct-identifier column (email, full name, phone, document) survives
/// into the anonymized output, and none leaks into the compliance report
/// either.
#[test]
fn no_raw_direct_identifier_value_survives_into_the_output_or_report() {
    let dataset = fixture();
    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");

    let dataset_json = serde_json::to_string(&outcome.dataset).expect("dataset serializes");
    let report_json = serde_json::to_string(&outcome.report).expect("report serializes");

    for value in source_values() {
        assert!(
            !dataset_json.contains(value),
            "output dataset leaked `{value}`"
        );
        assert!(!report_json.contains(value), "report leaked `{value}`");
    }
}

/// The same guard run repeatedly across many pinned seeds, since a
/// pseudonym generator that merely usually avoids echoing the source is not
/// the same guarantee as one that structurally cannot.
#[test]
fn no_leakage_holds_across_many_seeds() {
    for seed in 0..40u64 {
        let policy = Policy {
            seed: Some(seed),
            column_overrides: Default::default(),
        };
        let outcome = anonymizer::anonymize(fixture(), policy).expect("anonymize");
        let dataset_json = serde_json::to_string(&outcome.dataset).expect("dataset serializes");

        for value in source_values() {
            assert!(
                !dataset_json.contains(value),
                "seed {seed}: output leaked `{value}`"
            );
        }
    }
}
