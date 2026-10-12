use std::num::NonZeroUsize;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use server::config::Config;
use server::http::{AppState, router};
use tower::ServiceExt;

const FK_CHAIN_FIXTURE: &str = include_str!("../../../fixtures/multi_table_fk_chain.json");
const MISLEADING_COLUMNS_FIXTURE: &str = include_str!("../../../fixtures/misleading_columns.json");

fn minimal_config() -> Config {
    Config {
        bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
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

fn request_for(fixture: &str) -> Value {
    let dataset: Value = serde_json::from_str(fixture).expect("fixture parses");
    json!({ "dataset": dataset, "options": {} })
}

#[tokio::test]
async fn profile_classifies_and_profiles_without_transforming_the_dataset() {
    let response = app()
        .oneshot(json_request("/v1/profile", request_for(FK_CHAIN_FIXTURE)))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    let email = &body["profile"]["columns"]["customers.email"];
    assert_eq!(email["semantic_type"], "email");
    assert_eq!(email["distribution"]["kind"], "categorical");
    let top = &email["distribution"]["top"];
    assert_eq!(top["gmail.com"], 2.0 / 3.0);
    assert_eq!(top["outlook.com"], 1.0 / 3.0);
    assert!(
        top.as_object()
            .expect("object")
            .keys()
            .all(|key| !key.contains('@'))
    );
}

#[tokio::test]
async fn profile_response_never_carries_a_run_seed() {
    let response = app()
        .oneshot(json_request("/v1/profile", request_for(FK_CHAIN_FIXTURE)))
        .await
        .expect("response");

    let body = body_json(response).await;
    assert!(body.get("seed").is_none());
    assert!(body["profile"].get("seed").is_none());
}

#[tokio::test]
async fn profile_follows_values_over_a_column_name_that_lies_about_its_contents() {
    let response = app()
        .oneshot(json_request(
            "/v1/profile",
            request_for(MISLEADING_COLUMNS_FIXTURE),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert_eq!(
        body["profile"]["columns"]["customers.documento"]["semantic_type"],
        "cpf"
    );
    assert_ne!(
        body["profile"]["columns"]["customers.cpf"]["semantic_type"],
        "cpf"
    );
}

#[tokio::test]
async fn malformed_profile_payload_is_rejected() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/profile")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{ not json"))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
}
