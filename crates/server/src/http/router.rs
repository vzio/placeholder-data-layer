use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::http::{AppState, routes};

/// Axum 0.8: path parameters are `{name}`, never `:name`.
pub fn router(state: AppState) -> Router {
    let body_limit = state.config.body_limit_bytes;

    Router::new()
        .route(
            "/health",
            get(routes::health).fallback(routes::method_not_allowed),
        )
        .route(
            "/v1/detectors",
            get(routes::detectors).fallback(routes::method_not_allowed),
        )
        .route(
            "/v1/jobs",
            get(routes::jobs).fallback(routes::method_not_allowed),
        )
        .route(
            "/v1/profile",
            post(routes::profile).fallback(routes::method_not_allowed),
        )
        .route(
            "/v1/anonymize",
            post(routes::anonymize).fallback(routes::method_not_allowed),
        )
        .fallback(routes::not_found)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
