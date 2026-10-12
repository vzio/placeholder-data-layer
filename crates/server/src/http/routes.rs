use anonymizer::classify::SemanticType;
use axum::Json;
use axum::extract::State;
use serde::Serialize;
use uuid::Uuid;

use crate::http::AppState;
use crate::http::dto::{AnonymizeResponse, DatasetDto, DatasetRequest, ProfileResponse};
use crate::http::error::ApiError;
use crate::http::extract::JsonBody;
use crate::job::{JobKind, JobSummary};

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

pub async fn detectors() -> Json<&'static [SemanticType]> {
    Json(SemanticType::ALL)
}

pub async fn jobs(State(state): State<AppState>) -> Json<Vec<JobSummary>> {
    Json(state.jobs.list())
}

pub async fn profile(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<DatasetRequest>,
) -> Result<Json<ProfileResponse>, ApiError> {
    let dataset = request.dataset.into_dataset()?;
    let policy = request.options.into_policy()?;
    let rows: usize = dataset
        .tables
        .iter()
        .map(anonymizer::Table::row_count)
        .sum();

    let profile = state
        .jobs
        .submit(JobKind::Profile, rows, move || {
            anonymizer::profile(&dataset, &policy)
        })
        .await??;

    Ok(Json(ProfileResponse {
        run_id: Uuid::new_v4().to_string(),
        profile,
    }))
}

pub async fn anonymize(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<DatasetRequest>,
) -> Result<Json<AnonymizeResponse>, ApiError> {
    let dataset = request.dataset.into_dataset()?;
    let policy = request.options.into_policy()?;
    let rows: usize = dataset
        .tables
        .iter()
        .map(anonymizer::Table::row_count)
        .sum();

    let outcome = state
        .jobs
        .submit(JobKind::Anonymize, rows, move || {
            anonymizer::anonymize(dataset, policy)
        })
        .await??;

    Ok(Json(AnonymizeResponse {
        run_id: Uuid::new_v4().to_string(),
        dataset: DatasetDto::from_dataset(&outcome.dataset)?,
        report: outcome.report,
    }))
}
