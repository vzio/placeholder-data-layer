//! Engine invocations, through the public API only. Each (scenario, run) is
//! executed at most once per process and shared by every expectation on it.
//!
//! Engine output is read back as JSON (`serde_json::to_value` of the report
//! and the profile), so an expectation names types and strategies as strings
//! and can name one the engine does not have yet.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use anonymizer::{Dataset, Policy};
use server::http::dto::OptionsDto;

use crate::CorpusError;
use crate::load::load;
use crate::scenario::{RunMode, RunSpec};

/// A refusal, as the public `Error` exposes it: its variant name and texts.
#[derive(Debug, Clone)]
pub struct EngineError {
    pub variant: String,
    pub debug: String,
    pub display: String,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub output: Option<Dataset>,
    pub report: Option<serde_json::Value>,
    pub profile: Option<serde_json::Value>,
    pub error: Option<EngineError>,
    /// Everything the run produced except stage timings, serialised: equal
    /// fingerprints are byte-identical results.
    pub fingerprint: String,
}

pub fn policy_for(spec: &RunSpec) -> Result<Policy, CorpusError> {
    OptionsDto {
        seed: Some(spec.seed),
        column_overrides: spec.column_overrides.clone(),
        preserve: spec.preserve.clone(),
        force_preserve: spec.force_preserve,
    }
    .into_policy()
    .map_err(|e| CorpusError(format!("run `{}`: {e}", spec.id)))
}

/// Runs `spec` against `source` on whatever rayon pool is current.
pub fn execute(source: &Dataset, spec: &RunSpec) -> Result<RunResult, CorpusError> {
    let policy = policy_for(spec)?;
    let profile = anonymizer::profile(source, &policy);
    if spec.mode == RunMode::Profile {
        return match profile {
            Ok(profile) => {
                let profile = to_json(&profile)?;
                Ok(RunResult {
                    fingerprint: profile.to_string(),
                    output: None,
                    report: None,
                    profile: Some(profile),
                    error: None,
                })
            }
            Err(error) => Ok(refused(&error)),
        };
    }
    let profile = profile.ok().map(|p| to_json(&p)).transpose()?;
    match anonymizer::anonymize(source.clone(), policy) {
        Ok(outcome) => {
            let mut report = to_json(&outcome.report)?;
            if let Some(fields) = report.as_object_mut() {
                fields.remove("timings_ms");
            }
            let dataset = to_json(&outcome.dataset)?;
            Ok(RunResult {
                fingerprint: serde_json::json!([dataset, report]).to_string(),
                output: Some(outcome.dataset),
                report: Some(report),
                profile,
                error: None,
            })
        }
        Err(error) => Ok(refused(&error)),
    }
}

fn refused(error: &anonymizer::Error) -> RunResult {
    let debug = format!("{error:?}");
    RunResult {
        fingerprint: format!("refused: {debug}"),
        output: None,
        report: None,
        profile: None,
        error: Some(EngineError {
            variant: variant_of(&debug).to_owned(),
            display: error.to_string(),
            debug,
        }),
    }
}

/// The enum variant a `Debug` rendering starts with.
pub fn variant_of(debug: &str) -> &str {
    let end = debug.find(['(', ' ', '{']).unwrap_or(debug.len());
    &debug[..end]
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, CorpusError> {
    serde_json::to_value(value).map_err(|e| CorpusError(e.to_string()))
}

type Slot = Arc<OnceLock<Result<Arc<RunResult>, CorpusError>>>;

/// The result of run `run_id` of `scenario`, computed at most once.
pub fn run(scenario: &str, run_id: &str) -> Result<Arc<RunResult>, CorpusError> {
    static CACHE: OnceLock<Mutex<BTreeMap<(String, String), Slot>>> = OnceLock::new();
    let slot = {
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| CorpusError("run cache poisoned".to_owned()))?;
        Arc::clone(
            cache
                .entry((scenario.to_owned(), run_id.to_owned()))
                .or_default(),
        )
    };
    slot.get_or_init(|| {
        let loaded = load(scenario)?;
        let spec = loaded
            .file
            .run(run_id)
            .ok_or_else(|| CorpusError(format!("{scenario}: no run `{run_id}`")))?;
        execute(&loaded.source, spec).map(Arc::new)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variant_is_read_off_every_debug_shape() {
        assert_eq!(variant_of("PreserveRefused(\"t.c\")"), "PreserveRefused");
        assert_eq!(
            variant_of("NullNotAllowed { column: \"t.c\" }"),
            "NullNotAllowed"
        );
        assert_eq!(variant_of("EmptyDataset"), "EmptyDataset");
    }
}
