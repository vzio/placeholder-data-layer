use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::job::JobError;

const PROBLEM_JSON: &str = "application/problem+json";

/// RFC 9457 problem details. The `type` values are stable identifiers clients
/// may match on; `detail` is prose and may change.
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Problem {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: &'static str,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("the request body exceeds the configured limit")]
    PayloadTooLarge,
    #[error("the worker pool is saturated; retry shortly")]
    Saturated,
    #[error(transparent)]
    Engine(#[from] anonymizer::Error),
    #[error("internal error")]
    Internal,
    #[error("no route matches this path")]
    NotFound,
    #[error("this path does not support that method")]
    MethodNotAllowed,
}

impl From<JobError> for ApiError {
    fn from(error: JobError) -> Self {
        match error {
            JobError::Saturated => ApiError::Saturated,
            JobError::WorkerLost => ApiError::Internal,
            JobError::SpawnFailed(_) => ApiError::Internal,
        }
    }
}

impl From<axum::extract::rejection::JsonRejection> for ApiError {
    fn from(rejection: axum::extract::rejection::JsonRejection) -> Self {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::PayloadTooLarge
        } else {
            ApiError::BadRequest(rejection.body_text())
        }
    }
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::Saturated => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Engine(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            ApiError::BadRequest(_) => "urn:placeholder:data-layer:bad-request",
            ApiError::PayloadTooLarge => "urn:placeholder:data-layer:payload-too-large",
            ApiError::Saturated => "urn:placeholder:data-layer:saturated",
            ApiError::Engine(_) => "urn:placeholder:data-layer:invalid-dataset",
            ApiError::Internal => "urn:placeholder:data-layer:internal",
            ApiError::NotFound => "urn:placeholder:data-layer:not-found",
            ApiError::MethodNotAllowed => "urn:placeholder:data-layer:method-not-allowed",
        }
    }

    fn title(&self) -> &'static str {
        match self {
            ApiError::BadRequest(_) => "Malformed request",
            ApiError::PayloadTooLarge => "Payload too large",
            ApiError::Saturated => "Worker pool saturated",
            ApiError::Engine(_) => "Dataset rejected",
            ApiError::Internal => "Internal error",
            ApiError::NotFound => "Not Found",
            ApiError::MethodNotAllowed => "Method Not Allowed",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        if matches!(self, ApiError::Internal) {
            tracing::error!(error = %self, "request failed");
        }
        let problem = Problem {
            kind: self.kind(),
            title: self.title(),
            status: status.as_u16(),
            detail: Some(self.to_string()),
        };
        let mut response = (status, Json(problem)).into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        response
    }
}
