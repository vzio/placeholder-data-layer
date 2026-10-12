use axum::Json;
use serde::Serialize;

use crate::http::error::ApiError;

#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
    pub version: &'static str,
}

pub async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub async fn detectors() -> ApiError {
    ApiError::NotImplemented
}

pub async fn jobs() -> ApiError {
    ApiError::NotImplemented
}

pub async fn profile() -> ApiError {
    ApiError::NotImplemented
}

pub async fn anonymize() -> ApiError {
    ApiError::NotImplemented
}
