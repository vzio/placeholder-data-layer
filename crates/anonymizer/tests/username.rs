//! The `Username` semantic type through the public API: which columns it
//! claims, which it must leave alone, and what `MintHandle` emits for them.

use std::collections::{BTreeMap, BTreeSet};

use anonymizer::anomaly::AnomalyKind;
use anonymizer::classify::SemanticType;
use anonymizer::compliance::{Check, Verdict};
use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey};
use anonymizer::transform::{Strategy, Strength};
use anonymizer::{Dataset, Error, Outcome, Policy, Table, Value};

const PINNED_SEED: u64 = 20_240_607;
const ROWS: usize = 24;

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

fn primary_key(mut column: ColumnData) -> ColumnData {
    column.spec.pk = true;
    column
}

fn references(mut column: ColumnData, table: &str, target: &str) -> ColumnData {
    column.spec.fk = Some(ForeignKey {
        table: table.to_owned(),
        column: target.to_owned(),
    });
    column
}

fn table(name: &str, columns: Vec<ColumnData>) -> Table {
    Table {
        name: name.to_owned(),
        columns,
    }
}

fn single_table(name: &str, columns: Vec<ColumnData>) -> Dataset {
    Dataset {
        tables: vec![table(name, columns)],
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

fn handles(rows: usize) -> Vec<String> {
    const FIRST: [&str; 6] = ["ana", "bruno", "carla", "diego", "elisa", "felipe"];
    const LAST: [&str; 4] = ["silva", "souza", "lima", "dias"];
    (0..rows)
        .map(|i| format!("{}_{}{i}", FIRST[i % FIRST.len()], LAST[i % LAST.len()]))
        .collect()
}

fn cycle(values: &[&str], rows: usize) -> Vec<String> {
    (0..rows)
        .map(|i| values[i % values.len()].to_owned())
        .collect()
}

fn policy(seed: u64) -> Policy {
    Policy {
        seed: Some(seed),
        ..Policy::default()
    }
}

fn pinned() -> Policy {
    policy(PINNED_SEED)
}

fn preserving(targets: &[&str], force_preserve: bool) -> Policy {
    Policy {
        preserve: targets.iter().map(|t| (*t).to_owned()).collect(),
        force_preserve,
        ..pinned()
    }
}

fn overriding(overrides: &[(&str, Strategy)]) -> Policy {
    Policy {
        column_overrides: overrides
            .iter()
            .map(|(column, strategy)| ((*column).to_owned(), *strategy))
            .collect(),
        ..pinned()
    }
}

/// Anonymizes `dataset` under `policy`, after asserting that `profile`
/// classifies every column exactly as the fidelity report does.
fn anonymize(dataset: &Dataset, policy: Policy) -> Outcome {
    let profile = match anonymizer::profile(dataset, &policy) {
        Ok(profile) => profile,
        Err(error) => panic!("profile failed: {error}"),
    };
    let outcome = match anonymizer::anonymize(dataset.clone(), policy) {
        Ok(outcome) => outcome,
        Err(error) => panic!("anonymize failed: {error}"),
    };
    for report in &outcome.report.columns {
        let profiled = profile
            .columns
            .get(&report.column)
            .map(|column| column.semantic_type);
        assert_eq!(
            profiled,
            Some(report.semantic_type),
            "profile and report disagree on `{}`",
            report.column
        );
    }
    outcome
}

fn decision(outcome: &Outcome, qualified: &str) -> (SemanticType, Strategy) {
    match outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == qualified)
    {
        Some(report) => (report.semantic_type, report.strategy),
        None => panic!("no report for `{qualified}`"),
    }
}

fn output_texts(outcome: &Outcome, table: &str, column: &str) -> Vec<String> {
    values_of(&outcome.dataset, table, column)
        .iter()
        .map(|value| match value {
            Value::Text(text) => text.clone(),
            other => panic!("`{table}.{column}` holds a non-text value {other:?}"),
        })
        .collect()
}

fn values_of<'a>(dataset: &'a Dataset, table: &str, column: &str) -> &'a [Value] {
    match dataset.table(table).and_then(|t| t.column(column)) {
        Some(column) => &column.values,
        None => panic!("no column `{table}.{column}`"),
    }
}

fn is_minted_handle(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    (8..=32).contains(&chars.len())
        && chars[0].is_ascii_lowercase()
        && chars[chars.len() - 1].is_ascii_digit()
        && chars
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn leakage(outcome: &Outcome) -> Verdict {
    match outcome
        .report
        .checks
        .iter()
        .find(|check| check.check == Check::Leakage)
    {
        Some(check) => check.verdict,
        None => panic!("no leakage check in the report"),
    }
}

fn assert_passes_first_time(outcome: &Outcome) {
    assert_eq!(outcome.report.verdict, Verdict::Pass);
    assert_eq!(leakage(outcome), Verdict::Pass);
    assert_eq!(outcome.report.attempts, 1);
    assert_eq!(outcome.report.accepted_strength, Strength::Default);
}

#[test]
fn a_login_column_mints_distinct_handle_shaped_values_never_equal_to_a_source() {
    let sources: Vec<String> = (0..2000)
        .map(|i| match i % 10 {
            0 => "bob".to_owned(),
            1 => "ana".to_owned(),
            2 => "Bob".to_owned(),
            _ => format!("user.{i}_x"),
        })
        .collect();
    let dataset = single_table(
        "users",
        vec![id_column(2000), text_column("login", &sources)],
    );

    let outcome = anonymize(&dataset, pinned());

    assert_eq!(
        decision(&outcome, "users.login"),
        (SemanticType::Username, Strategy::MintHandle)
    );
    assert_passes_first_time(&outcome);
    let outputs = output_texts(&outcome, "users", "login");
    let mut handle_of: BTreeMap<&str, &str> = BTreeMap::new();
    for (source, output) in sources.iter().zip(&outputs) {
        assert!(
            is_minted_handle(output),
            "`{output}` is not a minted handle"
        );
        let first = handle_of.entry(source).or_insert(output);
        assert_eq!(*first, output, "`{source}` minted two different handles");
    }
    let distinct_outputs: BTreeSet<&String> = outputs.iter().collect();
    assert_eq!(distinct_outputs.len(), handle_of.len());
    let lowercase_sources: BTreeSet<String> = sources.iter().map(|s| s.to_lowercase()).collect();
    for output in &outputs {
        assert!(
            !lowercase_sources.contains(output),
            "`{output}` equals a source"
        );
    }
}

#[test]
fn every_spelling_of_a_username_column_name_classifies_username() {
    let names = [
        "login",
        "LOGIN",
        "username",
        "user_name",
        "userName",
        "user-name",
        "nickname",
        "nick_name",
        "handle",
    ];
    let mut columns = vec![id_column(ROWS)];
    columns.extend(names.iter().map(|name| text_column(name, &handles(ROWS))));
    let outcome = anonymize(&single_table("members", columns), pinned());

    for name in names {
        assert_eq!(
            decision(&outcome, &format!("members.{name}")),
            (SemanticType::Username, Strategy::MintHandle),
            "for `{name}`"
        );
    }
    assert_passes_first_time(&outcome);
}

/// Columns whose values or names resemble a login without being one, the
/// ones a classification by value shape, distinct count or substring name
/// match would claim: each stays `FreeText` and its output is byte for byte
/// what an explicit `redact` produces.
#[test]
fn columns_that_only_resemble_a_login_are_redacted_as_free_text() {
    let names = [
        "status",
        "locale",
        "slug",
        "sku",
        "file_name",
        "event_name",
        "tag",
        "login_method",
        "login_type",
        "login_provider",
        "handler",
        "blogin",
        "nickname_history",
    ];
    let natural_values: [(&str, Vec<String>); 7] = [
        ("status", cycle(&["active", "inactive", "pending"], ROWS)),
        ("locale", cycle(&["pt-BR", "en-US", "es-ES"], ROWS)),
        (
            "slug",
            (0..ROWS).map(|i| format!("produto-numero-{i}")).collect(),
        ),
        ("sku", (0..ROWS).map(|i| format!("SKU-{i:05}-BR")).collect()),
        (
            "file_name",
            (0..ROWS).map(|i| format!("relatorio_{i}.pdf")).collect(),
        ),
        (
            "event_name",
            (0..ROWS)
                .map(|i| format!("user.login.attempt{i}"))
                .collect(),
        ),
        ("tag", cycle(&["promo", "vip", "beta"], ROWS)),
    ];
    let mut handle_columns = vec![id_column(ROWS)];
    handle_columns.extend(names.iter().map(|name| text_column(name, &handles(ROWS))));
    let mut natural_columns = vec![id_column(ROWS)];
    natural_columns.extend(
        natural_values
            .iter()
            .map(|(name, values)| text_column(name, values)),
    );
    let dataset = Dataset {
        tables: vec![
            table("with_handles", handle_columns),
            table("with_own_values", natural_columns),
        ],
    };

    let outcome = anonymize(&dataset, pinned());

    let mut redact_everywhere = Vec::new();
    for report in &outcome.report.columns {
        if report.column.ends_with(".id") {
            continue;
        }
        assert_eq!(
            (report.semantic_type, report.strategy),
            (SemanticType::FreeText, Strategy::Redact),
            "for `{}`",
            report.column
        );
        redact_everywhere.push((report.column.as_str(), Strategy::Redact));
    }
    assert_eq!(redact_everywhere.len(), names.len() + natural_values.len());
    let redacted = anonymize(&dataset, overriding(&redact_everywhere));
    assert_eq!(outcome.dataset, redacted.dataset);
}

#[test]
fn a_structured_type_wins_over_username_and_unhandle_like_values_stay_free_text() {
    let uuid = |i: usize| format!("550e8400-e29b-41d4-a716-4466554400{i:02}");
    let dataset = single_table(
        "accounts",
        vec![
            id_column(ROWS),
            text_column(
                "login",
                &(0..ROWS)
                    .map(|i| format!("pessoa{i}@gmail.com"))
                    .collect::<Vec<_>>(),
            ),
            text_column("username", &cycle(&["Ana Souza", "Bruno Lima"], ROWS)),
            text_column(
                "nickname",
                &(0..ROWS)
                    .map(|i| format!("(11) 9{:04}-{:04}", 1000 + i, 2000 + i))
                    .collect::<Vec<_>>(),
            ),
            text_column(
                "handle",
                &cycle(&["111.444.777-35", "123.456.789-09"], ROWS),
            ),
            text_column("user_name", &(0..ROWS).map(uuid).collect::<Vec<_>>()),
            text_column("nick_name", &cycle(&["2024-01-31", "1990-07-04"], ROWS)),
        ],
    );
    let outcome = anonymize(&dataset, pinned());
    for (column, expected) in [
        ("login", SemanticType::Email),
        ("username", SemanticType::FullName),
        ("nickname", SemanticType::Phone),
        ("handle", SemanticType::Cpf),
        ("user_name", SemanticType::Uuid),
        ("nick_name", SemanticType::Date),
    ] {
        assert_eq!(
            decision(&outcome, &format!("accounts.{column}")).0,
            expected,
            "for `{column}`"
        );
    }

    let free_text = single_table(
        "accounts",
        vec![
            id_column(ROWS),
            text_column("login", &cycle(&["ana souza", "bruno lima"], ROWS)),
            text_column("username", &cycle(&["123", "4567"], ROWS)),
            text_column("nickname", &vec![String::new(); ROWS]),
            text_column(
                "handle",
                &handles(ROWS)
                    .into_iter()
                    .map(|h| format!(" {h} "))
                    .collect::<Vec<_>>(),
            ),
        ],
    );
    let outcome = anonymize(&free_text, pinned());
    for column in ["login", "username", "nickname", "handle"] {
        assert_eq!(
            decision(&outcome, &format!("accounts.{column}")),
            (SemanticType::FreeText, Strategy::Redact),
            "for `{column}`"
        );
    }
}

#[test]
fn a_key_column_named_like_a_login_mints_as_a_key_and_is_never_username() {
    let keys = handles(8);
    let referencing: Vec<String> = (0..ROWS).map(|i| keys[i % keys.len()].clone()).collect();
    let with_foreign_key = Dataset {
        tables: vec![
            table("accounts", vec![primary_key(text_column("login", &keys))]),
            table(
                "posts",
                vec![
                    id_column(ROWS),
                    references(text_column("username", &referencing), "accounts", "login"),
                ],
            ),
        ],
    };
    let outcome = anonymize(&with_foreign_key, pinned());
    for column in ["accounts.login", "posts.username"] {
        assert_eq!(
            decision(&outcome, column),
            (SemanticType::FreeText, Strategy::MintKey),
            "for `{column}`"
        );
    }
    let parents = output_texts(&outcome, "accounts", "login");
    for child in output_texts(&outcome, "posts", "username") {
        assert!(parents.contains(&child), "`{child}` no longer joins");
    }

    let standalone = single_table("accounts", vec![primary_key(text_column("login", &keys))]);
    let outcome = anonymize(&standalone, pinned());
    assert_eq!(
        decision(&outcome, "accounts.login"),
        (SemanticType::FreeText, Strategy::MintKey)
    );
    let preserved = anonymize(&standalone, preserving(&["accounts.login"], false));
    assert_eq!(preserved.dataset, standalone);

    let mixed_type_class = Dataset {
        tables: vec![
            table(
                "accounts",
                vec![primary_key(text_column("login", &handles(ROWS)))],
            ),
            table(
                "audits",
                vec![
                    id_column(ROWS),
                    references(
                        column(
                            "account_ref",
                            ColumnType::Integer,
                            (0..ROWS as i64).map(|i| Value::Int(i % 5)).collect(),
                        ),
                        "accounts",
                        "login",
                    ),
                ],
            ),
        ],
    };
    let outcome = anonymize(&mixed_type_class, pinned());
    assert_eq!(
        decision(&outcome, "accounts.login"),
        (SemanticType::FreeText, Strategy::MintKey)
    );
}

#[test]
fn preserving_a_username_column_is_refused_unless_forced() {
    let dataset = single_table(
        "users",
        vec![
            id_column(ROWS),
            text_column("login", &handles(ROWS)),
            text_column("plan", &cycle(&["free", "pro"], ROWS)),
        ],
    );

    for targets in [&["users.login"][..], &["users"][..]] {
        match anonymizer::anonymize(dataset.clone(), preserving(targets, false)) {
            Err(Error::PreserveRefused(column)) => assert_eq!(column, "users.login"),
            other => panic!("preserving {targets:?} was not refused: {other:?}"),
        }
    }

    let forced = anonymize(&dataset, preserving(&["users.login"], true));
    assert_eq!(
        values_of(&forced.dataset, "users", "login"),
        values_of(&dataset, "users", "login")
    );
    assert_eq!(
        decision(&forced, "users.login"),
        (SemanticType::Username, Strategy::Keep)
    );
}

#[test]
fn an_all_null_login_named_column_is_unknown_and_redacted_whatever_its_type() {
    let columns = [
        ("last_login", ColumnType::Integer),
        ("login", ColumnType::Integer),
        ("username", ColumnType::Float),
        ("handle", ColumnType::Boolean),
        ("login_count", ColumnType::Integer),
        ("nickname", ColumnType::Text),
    ];
    for (name, ty) in columns {
        let dataset = single_table(
            "records",
            vec![id_column(ROWS), column(name, ty, vec![Value::Null; ROWS])],
        );
        let outcome = anonymize(&dataset, pinned());
        assert_eq!(
            decision(&outcome, &format!("records.{name}")),
            (SemanticType::Unknown, Strategy::Redact),
            "for `{name}` declared {ty:?}"
        );
    }
}

#[test]
fn a_pinned_seed_mints_the_same_handles_and_one_login_mints_one_handle_across_tables() {
    let shared = handles(8);
    let dataset = Dataset {
        tables: vec![
            table(
                "users",
                vec![id_column(8), text_column("username", &shared)],
            ),
            table(
                "audit",
                vec![
                    id_column(ROWS),
                    text_column(
                        "login",
                        &(0..ROWS)
                            .map(|i| shared[(i * 3) % 8].clone())
                            .collect::<Vec<_>>(),
                    ),
                ],
            ),
        ],
    };

    let first = anonymize(&dataset, pinned());
    let second = anonymize(&dataset, pinned());
    let serialized = |outcome: &Outcome| match serde_json::to_vec(&outcome.dataset) {
        Ok(bytes) => bytes,
        Err(error) => panic!("serialization failed: {error}"),
    };
    assert_eq!(serialized(&first), serialized(&second));

    let handle_of: BTreeMap<String, String> = shared
        .iter()
        .cloned()
        .zip(output_texts(&first, "users", "username"))
        .collect();
    let audit_sources = values_of(&dataset, "audit", "login");
    for (source, output) in audit_sources
        .iter()
        .zip(output_texts(&first, "audit", "login"))
    {
        let Value::Text(source) = source else {
            panic!("non-text source");
        };
        assert_eq!(handle_of.get(source), Some(&output), "for `{source}`");
    }
}

/// Sources include values shaped exactly like minted handles, at the length
/// the shortest source mints at, so a coincidence would surface here first.
#[test]
fn a_username_column_passes_leakage_at_default_strength_across_many_seeds() {
    let mut sources = handles(200);
    sources.extend((0..200).map(|i| format!("a{:06}{}", i * 7, i % 10)));
    sources.extend(["bob", "ana", "x1"].iter().map(|s| (*s).to_owned()));
    let dataset = single_table(
        "users",
        vec![id_column(sources.len()), text_column("nickname", &sources)],
    );
    for seed in 0..40u64 {
        let outcome = anonymize(&dataset, policy(seed));
        assert_eq!(
            decision(&outcome, "users.nickname"),
            (SemanticType::Username, Strategy::MintHandle)
        );
        assert_passes_first_time(&outcome);
    }
}

#[test]
fn anomalies_in_a_username_column_survive_at_the_same_rate_and_never_as_their_source() {
    let mut sources = handles(60);
    for (row, anomalous) in [
        (3, "n/a"),
        (11, "  carla_lima  "),
        (17, "joÃ£o_silva"),
        (29, "N/A"),
        (41, " diego_dias"),
        (53, "felipeÃ§"),
    ] {
        sources[row] = anomalous.to_owned();
    }
    let dataset = single_table("users", vec![id_column(60), text_column("login", &sources)]);

    let outcome = anonymize(&dataset, pinned());

    assert_eq!(
        decision(&outcome, "users.login"),
        (SemanticType::Username, Strategy::MintHandle)
    );
    assert_passes_first_time(&outcome);
    let report = match outcome
        .report
        .columns
        .iter()
        .find(|c| c.column == "users.login")
    {
        Some(report) => report,
        None => panic!("no report for users.login"),
    };
    for kind in [
        AnomalyKind::UnexpectedNull,
        AnomalyKind::WhitespacePadding,
        AnomalyKind::EncodingArtifact,
    ] {
        let rates = report.anomalies.get(&kind);
        assert!(
            rates.is_some_and(|r| r.before > 0.0),
            "{kind:?} was not detected in the source"
        );
    }
    for (kind, rates) in &report.anomalies {
        assert_eq!(rates.before, rates.after, "{kind:?} rate changed");
    }
    for (source, output) in sources.iter().zip(output_texts(&outcome, "users", "login")) {
        assert_ne!(source.to_lowercase(), output.to_lowercase());
    }
}

#[test]
fn a_mint_handle_override_mints_a_neutral_text_column_and_is_refused_elsewhere() {
    let notes = cycle(&["ligar depois", "cliente antigo", "sem retorno"], ROWS);
    let dataset = Dataset {
        tables: vec![
            table(
                "accounts",
                vec![
                    primary_key(text_column("code", &handles(8))),
                    column(
                        "age",
                        ColumnType::Integer,
                        (0..8).map(|i| Value::Int(20 + i)).collect(),
                    ),
                ],
            ),
            table("notes", vec![id_column(ROWS), text_column("body", &notes)]),
        ],
    };

    let outcome = anonymize(
        &dataset,
        overriding(&[("notes.body", Strategy::MintHandle)]),
    );
    assert_eq!(decision(&outcome, "notes.body").1, Strategy::MintHandle);
    let outputs = output_texts(&outcome, "notes", "body");
    assert!(outputs.iter().all(|output| is_minted_handle(output)));
    assert_eq!(outputs.iter().collect::<BTreeSet<_>>().len(), 3);

    for column in ["accounts.age", "accounts.code"] {
        match anonymizer::anonymize(
            dataset.clone(),
            overriding(&[(column, Strategy::MintHandle)]),
        ) {
            Err(Error::IncompatibleStrategy {
                column: refused,
                strategy: Strategy::MintHandle,
            }) => assert_eq!(refused, column),
            other => panic!("`mint_handle` on `{column}` was not refused: {other:?}"),
        }
    }
}

/// Recognising a handle column by its values alone would also claim slugs,
/// SKUs, statuses and tags, so a column has to be named as a login to be
/// one.
#[test]
fn an_unnamed_handle_column_is_not_classified_username() {
    let dataset = single_table(
        "profiles",
        vec![
            id_column(ROWS),
            text_column("alias", &handles(ROWS)),
            text_column("member", &strings(&["ana_souza1"; ROWS])),
        ],
    );
    let outcome = anonymize(&dataset, pinned());
    for column in ["profiles.alias", "profiles.member"] {
        assert_eq!(
            decision(&outcome, column),
            (SemanticType::FreeText, Strategy::Redact),
            "for `{column}`"
        );
    }
}
