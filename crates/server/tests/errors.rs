use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use server::config::Config;
use server::http::{AppState, router};
use tower::ServiceExt;

fn app() -> axum::Router {
    let config = Config::from_env().expect("defaults are valid");
    router(AppState::new(config).expect("worker pool starts"))
}

async fn problem_body(response: axum::response::Response) -> serde_json::Value {
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json"),
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("problem+json body")
}

#[tokio::test]
async fn an_unmatched_route_is_rfc_9457() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/does-not-exist")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let problem = problem_body(response).await;
    assert_eq!(problem["status"], 404);
    assert_eq!(problem["type"], "urn:placeholder:data-layer:not-found");
}

#[tokio::test]
async fn a_wrong_method_on_a_real_route_is_rfc_9457() {
    let response = app()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let problem = problem_body(response).await;
    assert_eq!(problem["status"], 405);
    assert_eq!(
        problem["type"],
        "urn:placeholder:data-layer:method-not-allowed"
    );
}

#[tokio::test]
async fn a_get_on_a_post_only_route_is_rfc_9457() {
    let response = app()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/anonymize")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let problem = problem_body(response).await;
    assert_eq!(problem["status"], 405);
}

#[tokio::test]
async fn a_head_request_on_a_get_route_still_works() {
    let response = app()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
}
