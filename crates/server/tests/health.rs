use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use server::config::Config;
use server::http::{AppState, router};
use tower::ServiceExt;

fn app() -> axum::Router {
    let config = Config::from_env().expect("defaults are valid");
    router(AppState::new(config))
}

#[tokio::test]
async fn health_reports_ok() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/health")
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
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn unimplemented_routes_answer_with_problem_json() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/v1/detectors")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
}
