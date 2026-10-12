use std::collections::BTreeMap;

use server::http::dto::{DatasetDto, OptionsDto};

const FK_CHAIN_FIXTURE: &str = include_str!("../../../fixtures/multi_table_fk_chain.json");
const DANGLING_FK_FIXTURE: &str = include_str!("../../../fixtures/dangling_fk.json");
const UNICODE_FIXTURE: &str = include_str!("../../../fixtures/unicode_and_nulls.json");

fn load(json: &str) -> DatasetDto {
    serde_json::from_str(json).expect("fixture parses as a DatasetDto")
}

#[test]
fn pivoting_to_the_engine_shape_and_back_is_lossless() {
    for fixture in [FK_CHAIN_FIXTURE, UNICODE_FIXTURE] {
        let original = load(fixture);
        let dataset = original.clone().into_dataset().expect("valid fixture");
        let round_tripped = DatasetDto::from_dataset(&dataset).expect("uniform dataset converts");
        assert_eq!(
            original, round_tripped,
            "round trip must preserve shape and order"
        );
    }
}

#[test]
fn a_dangling_foreign_key_is_rejected_with_a_useful_message() {
    let dataset = load(DANGLING_FK_FIXTURE);
    let error = dataset
        .into_dataset()
        .expect_err("dangling fk must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("orders.customer_id"),
        "message was: {message}"
    );
    assert!(message.contains("customers"), "message was: {message}");
}

#[test]
fn an_unknown_strategy_name_is_rejected_at_the_dto_boundary() {
    let mut column_overrides = BTreeMap::new();
    column_overrides.insert(
        "customers.email".to_owned(),
        "not_a_real_strategy".to_owned(),
    );
    let options = OptionsDto {
        seed: None,
        column_overrides,
    };

    let error = options
        .into_policy()
        .expect_err("unknown strategy must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("not_a_real_strategy"),
        "message was: {message}"
    );
}

#[test]
fn a_known_strategy_name_resolves() {
    let mut column_overrides = BTreeMap::new();
    column_overrides.insert("customers.email".to_owned(), "redact".to_owned());
    let options = OptionsDto {
        seed: Some(7),
        column_overrides,
    };

    let policy = options.into_policy().expect("known strategy resolves");
    assert_eq!(policy.seed, Some(7));
    assert_eq!(policy.column_overrides.len(), 1);
}
