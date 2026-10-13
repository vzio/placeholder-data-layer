use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroUsize;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::config::Config;
use server::http::dto::DatasetDto;
use server::http::{AppState, router};
use server::job::JobKind;
use tower::ServiceExt;

const FK_CHAIN_FIXTURE: &str = include_str!("../../../fixtures/multi_table_fk_chain.json");
const DANGLING_FK_FIXTURE: &str = include_str!("../../../fixtures/dangling_fk.json");
const ANOMALIES_FIXTURE: &str = include_str!("../../../fixtures/anomalies.json");

fn minimal_config() -> Config {
    Config {
        bind: SocketAddr::from(([127, 0, 0, 1], 0)),
        body_limit_bytes: 32 * 1024 * 1024,
        rayon_threads: None,
        worker_pool_size: NonZeroUsize::new(4).expect("4 is nonzero"),
        log_level: "error".to_owned(),
    }
}

fn app() -> Router {
    router(AppState::new(minimal_config()).expect("worker pool starts"))
}

fn json_request(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

fn assert_problem_json(response: &axum::response::Response) {
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
}

fn fk_chain_request() -> Value {
    let dataset: Value = serde_json::from_str(FK_CHAIN_FIXTURE).expect("fixture parses");
    json!({ "dataset": dataset, "options": {} })
}

fn table_by_name<'a>(dataset: &'a DatasetDto, name: &str) -> &'a server::http::dto::TableDto {
    dataset
        .tables
        .iter()
        .find(|table| table.name == name)
        .unwrap_or_else(|| panic!("table `{name}` is present"))
}

fn column_index(table: &server::http::dto::TableDto, name: &str) -> usize {
    table
        .columns
        .iter()
        .position(|column| column.name == name)
        .unwrap_or_else(|| panic!("column `{name}` is present on `{}`", table.name))
}

/// The multiset of group sizes produced by grouping `table`'s rows by the
/// value in `fk_column`. Grouped by the value's `Debug` representation
/// rather than the value itself: [`anonymizer::Value`] excludes `Eq`/`Ord`
/// so that a `Float` can never become a map key, but a key column is
/// always `Int` or `Text`, both of which format unambiguously.
fn group_sizes(table: &server::http::dto::TableDto, fk_column: &str) -> Vec<usize> {
    let index = column_index(table, fk_column);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for row in &table.rows {
        *counts.entry(format!("{:?}", row[index])).or_insert(0) += 1;
    }
    let mut sizes: Vec<usize> = counts.into_values().collect();
    sizes.sort_unstable();
    sizes
}

#[tokio::test]
async fn happy_path_anonymizes_a_multi_table_dataset() {
    let response = app()
        .oneshot(json_request("/v1/anonymize", fk_chain_request()))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert_eq!(body["report"]["verdict"], "pass");
    assert_eq!(body["report"]["checks"], json!([]));

    let input: DatasetDto = serde_json::from_str(FK_CHAIN_FIXTURE).expect("fixture parses");
    let output: DatasetDto =
        serde_json::from_value(body["dataset"].clone()).expect("response dataset parses");

    assert_eq!(input.tables.len(), output.tables.len());
    for (before, after) in input.tables.iter().zip(&output.tables) {
        assert_eq!(before.name, after.name);
        assert_eq!(before.columns, after.columns, "schema must be preserved");
        assert_eq!(
            before.rows.len(),
            after.rows.len(),
            "row count must be preserved"
        );
        assert_ne!(
            before.rows, after.rows,
            "table `{}` must not pass through unchanged",
            before.name
        );
    }

    // Requirement #2/#4: grouping orders by customer_id, and order_items by
    // order_id, must produce the same partition shape before and after —
    // the anonymized FK values still single out the same groups.
    assert_eq!(
        group_sizes(table_by_name(&input, "orders"), "customer_id"),
        group_sizes(table_by_name(&output, "orders"), "customer_id"),
    );
    assert_eq!(
        group_sizes(table_by_name(&input, "order_items"), "order_id"),
        group_sizes(table_by_name(&output, "order_items"), "order_id"),
    );
}

/// Functional test #3 (anomalies reproduced), exercised through the HTTP
/// surface: with no transform yet landed, before and after come from the
/// same passthrough dataset, so they must match exactly, not approximately.
#[tokio::test]
async fn anonymize_report_carries_matching_before_and_after_anomaly_rates() {
    let dataset: Value = serde_json::from_str(ANOMALIES_FIXTURE).expect("fixture parses");
    let request = json!({ "dataset": dataset, "options": {} });

    let response = app()
        .oneshot(json_request("/v1/anonymize", request))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    let columns = body["report"]["columns"]
        .as_array()
        .expect("columns is an array");
    let email_column = columns
        .iter()
        .find(|column| column["column"] == "customers.email")
        .expect("customers.email is reported");

    let email_anomalies = email_column["anomalies"]["malformed_email"]
        .as_object()
        .expect("malformed_email rates are reported");
    assert_eq!(email_anomalies["before"], 0.1);
    assert_eq!(email_anomalies["after"], 0.1);
}

#[tokio::test]
async fn malformed_payload_is_rejected() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/anonymize")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{ this is not json"))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_problem_json(&response);
}

#[tokio::test]
async fn dangling_foreign_key_is_rejected_with_a_useful_message() {
    let dataset: Value = serde_json::from_str(DANGLING_FK_FIXTURE).expect("fixture parses");
    let request = json!({ "dataset": dataset, "options": {} });

    let response = app()
        .oneshot(json_request("/v1/anonymize", request))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_problem_json(&response);
    let body = body_json(response).await;
    let detail = body["detail"].as_str().expect("detail is a string");
    assert!(
        detail.contains("orders.customer_id"),
        "detail was: {detail}"
    );
    assert!(detail.contains("customers"), "detail was: {detail}");
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let mut config = minimal_config();
    config.body_limit_bytes = 64;
    let app = router(AppState::new(config).expect("worker pool starts"));

    let oversized = json!({
        "dataset": { "tables": [] },
        "padding": "x".repeat(4096),
    });

    let response = app
        .oneshot(json_request("/v1/anonymize", oversized))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_problem_json(&response);
}

#[tokio::test]
async fn override_naming_an_unknown_column_is_rejected() {
    let request = json!({
        "dataset": {
            "tables": [{
                "name": "customers",
                "columns": [{ "name": "id", "type": "integer", "pk": true }],
                "rows": [[1]],
            }],
        },
        "options": {
            "column_overrides": { "customers.phone": "redact" },
        },
    });

    let response = app()
        .oneshot(json_request("/v1/anonymize", request))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_problem_json(&response);
    let body = body_json(response).await;
    let detail = body["detail"].as_str().expect("detail is a string");
    assert!(detail.contains("customers.phone"), "detail was: {detail}");
}

#[tokio::test]
async fn preserve_naming_an_unknown_table_is_rejected() {
    let request = json!({
        "dataset": {
            "tables": [{
                "name": "customers",
                "columns": [{ "name": "id", "type": "integer", "pk": true }],
                "rows": [[1]],
            }],
        },
        "options": {
            "preserve": ["no_such_table"],
        },
    });

    let response = app()
        .oneshot(json_request("/v1/anonymize", request))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_problem_json(&response);
    let body = body_json(response).await;
    let detail = body["detail"].as_str().expect("detail is a string");
    assert!(detail.contains("no_such_table"), "detail was: {detail}");
}

/// Exercises the `preserve` and `force_preserve` options through the HTTP
/// boundary end to end: a lookup table's column survives untouched and the
/// response's fidelity report names it, which only passes if `OptionsDto`
/// actually carries both fields into the `Policy` the engine runs.
#[tokio::test]
async fn preserve_wired_through_the_http_boundary_keeps_the_column_and_reports_it() {
    let request = json!({
        "dataset": {
            "tables": [{
                "name": "payment_types",
                "columns": [
                    { "name": "id", "type": "integer", "pk": true },
                    { "name": "label", "type": "string" },
                ],
                "rows": [[1, "Credit"], [2, "Debit"]],
            }],
        },
        "options": {
            "preserve": ["payment_types.label"],
        },
    });

    let response = app()
        .oneshot(json_request("/v1/anonymize", request))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let rows = &body["dataset"]["tables"][0]["rows"];
    assert_eq!(rows[0][1], "Credit");
    assert_eq!(rows[1][1], "Debit");
    assert_eq!(
        body["report"]["preserved_columns"]["payment_types.label"],
        "explicit"
    );
}

#[tokio::test]
async fn override_naming_an_unknown_strategy_is_rejected() {
    let request = json!({
        "dataset": {
            "tables": [{
                "name": "customers",
                "columns": [{ "name": "id", "type": "integer", "pk": true }],
                "rows": [[1]],
            }],
        },
        "options": {
            "column_overrides": { "customers.id": "not_a_real_strategy" },
        },
    });

    let response = app()
        .oneshot(json_request("/v1/anonymize", request))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_problem_json(&response);
    let body = body_json(response).await;
    let detail = body["detail"].as_str().expect("detail is a string");
    assert!(
        detail.contains("not_a_real_strategy"),
        "detail was: {detail}"
    );
}

#[tokio::test]
async fn worker_pool_saturation_returns_429() {
    let mut config = minimal_config();
    config.worker_pool_size = NonZeroUsize::new(1).expect("1 is nonzero");

    let state = AppState::new(config).expect("worker pool starts");
    let app = router(state.clone());

    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let occupier_jobs = state.jobs.clone();
    let occupier = tokio::spawn(async move {
        let _ = occupier_jobs
            .submit(JobKind::Anonymize, 0, move || {
                let _ = release_rx.recv();
            })
            .await;
    });

    for _ in 0..10_000 {
        if !state.jobs.list().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        state.jobs.list().len(),
        1,
        "occupier job must be admitted first"
    );

    let small_request = json!({
        "dataset": {
            "tables": [{
                "name": "customers",
                "columns": [{ "name": "id", "type": "integer", "pk": true }],
                "rows": [[1]],
            }],
        },
        "options": {},
    });

    let response = app
        .oneshot(json_request("/v1/anonymize", small_request))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_problem_json(&response);

    release_tx.send(()).expect("release the occupier");
    occupier.await.expect("occupier task completes");
}
