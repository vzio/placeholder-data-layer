use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use server::config::Config;
use server::http::{AppState, router};
use tower::ServiceExt;

fn app() -> Router {
    let config = Config::from_env().expect("defaults are valid");
    router(AppState::new(config).expect("worker pool starts"))
}

#[tokio::test]
async fn detectors_lists_every_semantic_type_this_build_recognises() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/v1/detectors")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);

    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let types: Vec<String> = serde_json::from_slice(&body).expect("json array of strings");

    for expected in [
        "email",
        "cpf",
        "cnpj",
        "phone",
        "cep",
        "full_name",
        "date",
        "numeric_id",
        "free_text",
        "unknown",
    ] {
        assert!(
            types.iter().any(|t| t == expected),
            "missing detector `{expected}` in {types:?}"
        );
    }
}
