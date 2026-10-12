use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use server::config::Config;
use server::http::{AppState, router};
use tower::ServiceExt;

fn app() -> axum::Router {
    let config = Config::from_env().expect("defaults are valid");
    router(AppState::new(config).expect("worker pool starts"))
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
