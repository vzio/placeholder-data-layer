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
    assert_eq!(
        input, output,
        "passthrough must leave the dataset unchanged"
    );
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
