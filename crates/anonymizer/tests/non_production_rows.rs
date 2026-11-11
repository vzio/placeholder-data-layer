//! Non-production rows through the public API: which source rows read as
//! test, admin or seed fixtures, how the report and the profile count them,
//! and that the count never carries a source value.

use std::collections::BTreeMap;

use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
use anonymizer::provenance::{NonProductionRows, NonProductionSignal};
use anonymizer::{Dataset, Policy, Table, Value};

const PINNED_SEED: u64 = 20_250_311;
const CLEAN_ROWS: usize = 24;

const FIRST_NAMES: [&str; 8] = [
    "Ana", "Bruno", "Carla", "Diego", "Elisa", "Felipe", "Gabriela", "Henrique",
];
const LAST_NAMES: [&str; 6] = ["Silva", "Souza", "Lima", "Dias", "Moreira", "Rocha"];

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

fn text_column(name: &str, values: &[String]) -> ColumnData {
    column(
        name,
        ColumnType::Text,
        values.iter().cloned().map(Value::Text).collect(),
    )
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

fn table(name: &str, columns: Vec<ColumnData>) -> Table {
    Table {
        name: name.to_owned(),
        columns,
    }
}

fn policy(seed: u64) -> Policy {
    Policy {
        seed: Some(seed),
        ..Policy::default()
    }
}

/// The two mod-11 check digits completing `base`, CNPJ's wrapping weights
/// when `wrap` is set.
fn check_digits(base: &[u32], wrap: bool) -> [u32; 2] {
    let digit = |digits: &[u32]| {
        let mut weight = 2;
        let mut sum = 0;
        for &d in digits.iter().rev() {
            sum += d * weight;
            weight = if wrap && weight == 9 { 2 } else { weight + 1 };
        }
        let remainder = sum % 11;
        if remainder < 2 { 0 } else { 11 - remainder }
    };
    let first = digit(base);
    let mut extended = base.to_vec();
    extended.push(first);
    [first, digit(&extended)]
}

fn digits_of(text: &str) -> Vec<u32> {
    text.chars().filter_map(|c| c.to_digit(10)).collect()
}

/// A check-digit-valid CPF, bare, whose nine-digit body is `200_000_000 + i`:
/// never one digit repeated and never a published example.
fn valid_cpf_digits(i: usize) -> String {
    let body = format!("{:09}", 200_000_000 + i);
    let [a, b] = check_digits(&digits_of(&body), false);
    format!("{body}{a}{b}")
}

fn valid_cpf(i: usize) -> String {
    let d = valid_cpf_digits(i);
    format!("{}.{}.{}-{}", &d[0..3], &d[3..6], &d[6..9], &d[9..11])
}

fn valid_cnpj(i: usize) -> String {
    let body = format!("{:08}0001", 33_445_566 + i);
    let [a, b] = check_digits(&digits_of(&body), true);
    format!(
        "{}.{}.{}/{}-{a}{b}",
        &body[0..2],
        &body[2..5],
        &body[5..8],
        &body[8..12]
    )
}

fn full_name(i: usize) -> String {
    format!(
        "{} {}",
        FIRST_NAMES[i % FIRST_NAMES.len()],
        LAST_NAMES[(i / FIRST_NAMES.len()) % LAST_NAMES.len()]
    )
}

fn email(i: usize) -> String {
    let first = FIRST_NAMES[i % FIRST_NAMES.len()].to_lowercase();
    let last = LAST_NAMES[(i / FIRST_NAMES.len()) % LAST_NAMES.len()].to_lowercase();
    format!("{first}.{last}{i}@gmail.com")
}

fn phone(i: usize) -> String {
    format!("(11) 98765-{:04}", 1000 + i)
}

/// A real-looking customer row: email, full name, phone, CPF.
struct Customer {
    email: String,
    full_name: String,
    phone: String,
    cpf: String,
}

fn clean_customer(i: usize) -> Customer {
    Customer {
        email: email(i),
        full_name: full_name(i),
        phone: phone(i),
        cpf: valid_cpf(i),
    }
}

fn customers_table(customers: &[Customer]) -> Table {
    let pick = |f: fn(&Customer) -> &String| -> Vec<String> {
        customers.iter().map(|c| f(c).clone()).collect()
    };
    table(
        "customers",
        vec![
            id_column(customers.len()),
            text_column("email", &pick(|c| &c.email)),
            text_column("full_name", &pick(|c| &c.full_name)),
            text_column("phone", &pick(|c| &c.phone)),
            text_column("cpf", &pick(|c| &c.cpf)),
        ],
    )
}

/// Clean customers with `odd` placed in the middle.
fn customers_with(odd: Customer) -> Dataset {
    let mut customers: Vec<Customer> = (0..CLEAN_ROWS).map(clean_customer).collect();
    customers.insert(CLEAN_ROWS / 2, odd);
    Dataset {
        tables: vec![customers_table(&customers)],
    }
}

fn report_rows(dataset: Dataset) -> BTreeMap<String, NonProductionRows> {
    anonymizer::anonymize(dataset, policy(PINNED_SEED))
        .expect("dataset anonymizes")
        .report
        .non_production_rows
}

fn profile_rows(dataset: &Dataset) -> BTreeMap<String, NonProductionRows> {
    anonymizer::profile(dataset, &policy(PINNED_SEED))
        .expect("dataset profiles")
        .non_production_rows
}

fn signals(entries: &[(NonProductionSignal, usize)]) -> BTreeMap<NonProductionSignal, usize> {
    entries.iter().copied().collect()
}

/// The report's counts for `table`, asserting the profile agrees.
fn rows_of(dataset: Dataset, table: &str) -> NonProductionRows {
    let profiled = profile_rows(&dataset);
    let reported = report_rows(dataset);
    assert_eq!(profiled, reported, "profile and report disagree");
    reported
        .get(table)
        .cloned()
        .unwrap_or_else(|| panic!("`{table}` is not flagged: {reported:?}"))
}

#[test]
fn a_clean_dataset_reports_no_non_production_rows() {
    let customers: Vec<Customer> = (0..CLEAN_ROWS).map(clean_customer).collect();
    let dataset = Dataset {
        tables: vec![customers_table(&customers)],
    };

    let profile = anonymizer::profile(&dataset, &policy(PINNED_SEED)).expect("profiles");
    let outcome = anonymizer::anonymize(dataset, policy(PINNED_SEED)).expect("anonymizes");

    assert!(profile.non_production_rows.is_empty());
    assert!(outcome.report.non_production_rows.is_empty());
    let report = serde_json::to_value(&outcome.report).expect("report serializes");
    assert_eq!(report["non_production_rows"], serde_json::json!({}));
    let profile = serde_json::to_value(&profile).expect("profile serializes");
    assert_eq!(profile["non_production_rows"], serde_json::json!({}));
}

#[test]
fn a_unanimously_placeholder_row_is_flagged_and_not_mixed() {
    let rows = rows_of(
        customers_with(Customer {
            email: "admin@mailinator.com".to_owned(),
            full_name: "Fulano de Tal".to_owned(),
            phone: "(00) 00000-0000".to_owned(),
            cpf: "000.000.000-00".to_owned(),
        }),
        "customers",
    );

    use NonProductionSignal::*;
    assert_eq!(rows.flagged, 1);
    assert_eq!(rows.mixed, 0);
    let expected: BTreeMap<String, BTreeMap<NonProductionSignal, usize>> = [
        (
            "customers.email".to_owned(),
            signals(&[(PlaceholderLocalPart, 1), (PlaceholderDomain, 1)]),
        ),
        (
            "customers.full_name".to_owned(),
            signals(&[(PlaceholderName, 1)]),
        ),
        (
            "customers.phone".to_owned(),
            signals(&[(PlaceholderPhone, 1)]),
        ),
        (
            "customers.cpf".to_owned(),
            signals(&[(PlaceholderDocument, 1)]),
        ),
    ]
    .into_iter()
    .collect();
    assert_eq!(rows.fields, expected);
}

#[test]
fn a_published_example_cpf_beside_a_real_looking_email_and_name_is_mixed() {
    let rows = rows_of(
        customers_with(Customer {
            email: "marina.costa@gmail.com".to_owned(),
            full_name: "Marina Costa".to_owned(),
            phone: "(21) 97654-3210".to_owned(),
            cpf: "111.444.777-35".to_owned(),
        }),
        "customers",
    );

    assert_eq!(rows.flagged, 1);
    assert_eq!(rows.mixed, 1);
    let expected: BTreeMap<String, BTreeMap<NonProductionSignal, usize>> = [(
        "customers.cpf".to_owned(),
        signals(&[(NonProductionSignal::PlaceholderDocument, 1)]),
    )]
    .into_iter()
    .collect();
    assert_eq!(rows.fields, expected);
}

#[test]
fn the_architecture_example_counts_seven_flagged_and_one_mixed() {
    const ROWS: usize = 100_000;
    let placeholders: [(&str, &str, &str); 7] = [
        ("test@example.com", "Test User", "111.111.111-11"),
        ("admin@empresa.com.br", "Admin Sistema", "123.456.789-09"),
        ("teste1@teste.com", "Teste Teste", "000.000.000-00"),
        ("noreply@mailinator.com", "Fulano de Tal", "111.444.777-35"),
        ("qa+login@yopmail.com", "John Doe", "999.999.999-99"),
        ("dummy@x.test", "Usuário Demo", "12345678909"),
        ("marina.costa@gmail.com", "Marina Costa", "111.444.777-35"),
    ];
    let mut emails: Vec<String> = (0..ROWS)
        .map(|i| format!("cliente{i}@empresa.com.br"))
        .collect();
    let mut names: Vec<String> = (0..ROWS).map(full_name).collect();
    let mut cpfs: Vec<String> = (0..ROWS).map(valid_cpf).collect();
    for (k, (email, name, cpf)) in placeholders.iter().enumerate() {
        let at = (k + 1) * (ROWS / 8);
        emails.insert(at, (*email).to_owned());
        names.insert(at, (*name).to_owned());
        cpfs.insert(at, (*cpf).to_owned());
    }
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                id_column(emails.len()),
                text_column("email", &emails),
                text_column("full_name", &names),
                text_column("cpf", &cpfs),
            ],
        )],
    };

    let rows = rows_of(dataset, "customers");

    use NonProductionSignal::*;
    assert_eq!(rows.flagged, 7);
    assert_eq!(rows.mixed, 1);
    assert_eq!(
        rows.fields["customers.email"],
        signals(&[(PlaceholderLocalPart, 6), (PlaceholderDomain, 5)])
    );
    assert_eq!(
        rows.fields["customers.full_name"],
        signals(&[(PlaceholderName, 6)])
    );
    assert_eq!(
        rows.fields["customers.cpf"],
        signals(&[(PlaceholderDocument, 7)])
    );
}

/// The fields of `table`'s non-production rows, asserting exactly one row is
/// flagged.
fn only_signal(
    dataset: Dataset,
    table: &str,
) -> BTreeMap<String, BTreeMap<NonProductionSignal, usize>> {
    let rows = rows_of(dataset, table);
    assert_eq!(rows.flagged, 1);
    rows.fields
}

fn one_field(
    column: &str,
    signal: NonProductionSignal,
) -> BTreeMap<String, BTreeMap<NonProductionSignal, usize>> {
    [(column.to_owned(), signals(&[(signal, 1)]))]
        .into_iter()
        .collect()
}

fn customer_with_email(email: &str) -> Customer {
    Customer {
        email: email.to_owned(),
        ..clean_customer(CLEAN_ROWS + 1)
    }
}

#[test]
fn each_signal_fires_on_its_own_shape() {
    use NonProductionSignal::*;

    assert_eq!(
        only_signal(
            customers_with(customer_with_email("admin@empresa.com.br")),
            "customers"
        ),
        one_field("customers.email", PlaceholderLocalPart)
    );
    assert_eq!(
        only_signal(
            customers_with(customer_with_email("joana@mailinator.com")),
            "customers"
        ),
        one_field("customers.email", PlaceholderDomain)
    );
    assert_eq!(
        only_signal(
            customers_with(customer_with_email("joana@x.test")),
            "customers"
        ),
        one_field("customers.email", PlaceholderDomain)
    );
    assert_eq!(
        only_signal(
            customers_with(Customer {
                full_name: "Fulano de Tal".to_owned(),
                ..clean_customer(CLEAN_ROWS + 1)
            }),
            "customers"
        ),
        one_field("customers.full_name", PlaceholderName)
    );
    assert_eq!(
        only_signal(
            customers_with(Customer {
                phone: "(11) 99999-9999".to_owned(),
                ..clean_customer(CLEAN_ROWS + 1)
            }),
            "customers"
        ),
        one_field("customers.phone", PlaceholderPhone)
    );
    assert_eq!(
        only_signal(
            customers_with(Customer {
                cpf: "000.000.000-00".to_owned(),
                ..clean_customer(CLEAN_ROWS + 1)
            }),
            "customers"
        ),
        one_field("customers.cpf", PlaceholderDocument)
    );

    let mut cnpjs: Vec<String> = (0..CLEAN_ROWS).map(valid_cnpj).collect();
    cnpjs.insert(CLEAN_ROWS / 2, "11.222.333/0001-81".to_owned());
    let companies = Dataset {
        tables: vec![table(
            "companies",
            vec![id_column(cnpjs.len()), text_column("cnpj", &cnpjs)],
        )],
    };
    assert_eq!(
        only_signal(companies, "companies"),
        one_field("companies.cnpj", PlaceholderDocument)
    );

    let mut integer_cpfs: Vec<Value> = (0..CLEAN_ROWS)
        .map(|i| Value::Int(valid_cpf_digits(i).parse().expect("digits")))
        .collect();
    integer_cpfs.insert(CLEAN_ROWS / 2, Value::Int(11_144_477_735));
    let people = Dataset {
        tables: vec![table(
            "people",
            vec![
                id_column(integer_cpfs.len()),
                column("cpf", ColumnType::Integer, integer_cpfs),
            ],
        )],
    };
    assert_eq!(
        only_signal(people, "people"),
        one_field("people.cpf", PlaceholderDocument)
    );
}

#[test]
fn near_misses_raise_no_signal() {
    let near_misses = [
        customer_with_email("badminton@gmail.com"),
        customer_with_email("contest@gmail.com"),
        Customer {
            full_name: "Ana Testoni".to_owned(),
            ..clean_customer(CLEAN_ROWS + 1)
        },
        Customer {
            phone: "(11) 99999-9998".to_owned(),
            ..clean_customer(CLEAN_ROWS + 1)
        },
        Customer {
            cpf: valid_cpf(CLEAN_ROWS + 1),
            ..clean_customer(CLEAN_ROWS + 1)
        },
        Customer {
            cpf: "CPF: 111.444.777-35 (antigo)".to_owned(),
            ..clean_customer(CLEAN_ROWS + 1)
        },
    ];
    for odd in near_misses {
        let dataset = customers_with(odd);
        assert!(profile_rows(&dataset).is_empty());
        assert!(report_rows(dataset).is_empty());
    }
}

#[test]
fn a_null_or_malformed_identifier_does_not_make_a_row_mixed() {
    let mut emails: Vec<Value> = (0..CLEAN_ROWS).map(|i| Value::Text(email(i))).collect();
    let mut names: Vec<Value> = (0..CLEAN_ROWS).map(|i| Value::Text(full_name(i))).collect();
    let mut cpfs: Vec<Value> = (0..CLEAN_ROWS).map(|i| Value::Text(valid_cpf(i))).collect();
    emails.insert(3, Value::Null);
    names.insert(3, Value::Text("ana".to_owned()));
    cpfs.insert(3, Value::Text("123.456.789-09".to_owned()));
    emails.insert(9, Value::Text("not an email".to_owned()));
    names.insert(9, Value::Null);
    cpfs.insert(9, Value::Text("111.444.777-35".to_owned()));
    let dataset = Dataset {
        tables: vec![table(
            "customers",
            vec![
                id_column(emails.len()),
                column("email", ColumnType::Text, emails),
                column("full_name", ColumnType::Text, names),
                column("cpf", ColumnType::Text, cpfs),
            ],
        )],
    };

    let rows = rows_of(dataset, "customers");

    assert_eq!(rows.flagged, 2);
    assert_eq!(rows.mixed, 0);
}

#[test]
fn rows_are_judged_within_one_table() {
    let customers: Vec<Customer> = (0..CLEAN_ROWS).map(clean_customer).collect();
    let mut order_cpfs: Vec<String> = (0..CLEAN_ROWS).map(|i| valid_cpf(i + 500)).collect();
    order_cpfs[5] = "123.456.789-09".to_owned();
    let mut customer_id = column(
        "customer_id",
        ColumnType::Integer,
        (1..=CLEAN_ROWS as i64).map(Value::Int).collect(),
    );
    customer_id.spec.fk = Some(ForeignKey {
        table: "customers".to_owned(),
        column: "id".to_owned(),
    });
    let dataset = Dataset {
        tables: vec![
            customers_table(&customers),
            table(
                "orders",
                vec![
                    id_column(CLEAN_ROWS),
                    customer_id,
                    text_column("billing_cpf", &order_cpfs),
                ],
            ),
        ],
    };

    let profiled = profile_rows(&dataset);
    let reported = report_rows(dataset);

    assert_eq!(profiled, reported);
    assert_eq!(reported.keys().collect::<Vec<_>>(), ["orders"]);
    assert_eq!(reported["orders"].flagged, 1);
    assert_eq!(
        reported["orders"].mixed, 0,
        "the joined customer's real-looking email and name belong to another table"
    );
}

fn two_table_dataset_with_placeholders() -> Dataset {
    let mut customers: Vec<Customer> = (0..CLEAN_ROWS).map(clean_customer).collect();
    customers[2] = Customer {
        email: "test@example.com".to_owned(),
        full_name: "Test User".to_owned(),
        phone: "(00) 00000-0000".to_owned(),
        cpf: "111.111.111-11".to_owned(),
    };
    customers[7].cpf = "111.444.777-35".to_owned();
    customers[11].email = "admin@mailinator.com".to_owned();
    let mut cnpjs: Vec<String> = (0..CLEAN_ROWS).map(valid_cnpj).collect();
    cnpjs[4] = "12.345.678/0001-95".to_owned();
    let names: Vec<String> = (0..CLEAN_ROWS).map(|i| full_name(i + 3)).collect();
    Dataset {
        tables: vec![
            customers_table(&customers),
            table(
                "suppliers",
                vec![
                    id_column(CLEAN_ROWS),
                    text_column("cnpj", &cnpjs),
                    text_column("contact_name", &names),
                ],
            ),
        ],
    }
}

#[test]
fn no_source_value_reaches_the_serialized_non_production_summary() {
    let dataset = two_table_dataset_with_placeholders();
    let source_texts: Vec<String> = dataset
        .tables
        .iter()
        .flat_map(|table| table.columns.iter())
        .flat_map(|column| column.values.iter())
        .filter_map(|value| value.as_text().map(str::to_owned))
        .collect();

    let profile = anonymizer::profile(&dataset, &policy(PINNED_SEED)).expect("profiles");
    let outcome = anonymizer::anonymize(dataset, policy(PINNED_SEED)).expect("anonymizes");
    assert!(!outcome.report.non_production_rows.is_empty());

    for summary in [
        serde_json::to_string(&outcome.report.non_production_rows).expect("serializes"),
        serde_json::to_string(&profile.non_production_rows).expect("serializes"),
    ] {
        for source in &source_texts {
            assert!(
                !summary.contains(source.as_str()),
                "a source value reached the summary"
            );
        }
        for fragment in ["example.com", "mailinator", "111.444.777-35", "11144477735"] {
            assert!(
                !summary.contains(fragment),
                "`{fragment}` reached the summary"
            );
        }
    }
}

#[test]
fn the_non_production_summary_is_identical_across_runs() {
    let first = anonymizer::anonymize(two_table_dataset_with_placeholders(), policy(1))
        .expect("anonymizes")
        .report
        .non_production_rows;
    let second = anonymizer::anonymize(two_table_dataset_with_placeholders(), policy(2))
        .expect("anonymizes")
        .report
        .non_production_rows;
    let third = anonymizer::anonymize(two_table_dataset_with_placeholders(), policy(1))
        .expect("anonymizes")
        .report
        .non_production_rows;

    assert_eq!(first.keys().collect::<Vec<_>>(), ["customers", "suppliers"]);
    let serialized = serde_json::to_string(&first).expect("serializes");
    assert_eq!(
        serialized,
        serde_json::to_string(&second).expect("serializes")
    );
    assert_eq!(
        serialized,
        serde_json::to_string(&third).expect("serializes")
    );
}

#[test]
fn profile_and_report_agree_on_non_production_rows() {
    let dataset = two_table_dataset_with_placeholders();

    let profile = anonymizer::profile(&dataset, &policy(PINNED_SEED)).expect("profiles");
    let outcome = anonymizer::anonymize(dataset, policy(PINNED_SEED)).expect("anonymizes");

    assert_eq!(
        profile.non_production_rows,
        outcome.report.non_production_rows
    );
    let customers = &profile.non_production_rows["customers"];
    assert_eq!(customers.flagged, 3);
    assert_eq!(customers.mixed, 2);
    assert_eq!(profile.non_production_rows["suppliers"].flagged, 1);
    assert_eq!(profile.non_production_rows["suppliers"].mixed, 1);
}
