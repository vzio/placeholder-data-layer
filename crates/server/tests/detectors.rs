use anonymizer::classify::SemanticType;
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

/// Pinned against `SemanticType::ALL` itself, in its own declared order,
/// rather than a hand-maintained literal list: a hand-maintained list can
/// drift (as the one this replaced did, silently omitting `"uuid"`) and
/// checking membership alone leaves the wire order unpinned by anything.
#[tokio::test]
async fn detectors_lists_every_semantic_type_in_all_in_order() {
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

    let expected: Vec<&str> = SemanticType::ALL.iter().map(|ty| ty.as_str()).collect();
    assert_eq!(types, expected);
}
