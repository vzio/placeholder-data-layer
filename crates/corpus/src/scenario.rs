//! The `expected.json` format. Every struct rejects unknown fields, so a
//! misspelt key fails loudly instead of silently asserting nothing.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::expect::Assertion;
use crate::facts::Fact;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioFile {
    pub format: u32,
    /// Equal to the scenario's directory name.
    pub scenario: String,
    pub pack: Pack,
    pub tier: Tier,
    pub summary: String,
    pub dataset: DatasetSource,
    pub runs: Vec<RunSpec>,
    #[serde(default)]
    pub facts: Vec<Fact>,
    pub expectations: Vec<Expectation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pack {
    Seed,
    Commerce,
    People,
    Legacy,
}

/// `ci` scenarios evaluate every expectation under `cargo test`; `extended`
/// ones only prove their fixture is well formed there and are evaluated in
/// full by the scoreboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Ci,
    Extended,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum DatasetSource {
    /// A committed wire-format dataset, relative to the scenario directory.
    File { path: String },
    /// Regenerated at test time; `digest` and `preview.json` pin the bytes.
    Generator {
        name: String,
        seed: u64,
        params: serde_json::Value,
        /// `blake3:<hex>` of the compact wire-format JSON.
        digest: String,
        preview_rows: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Anonymize,
    Profile,
}

/// One engine invocation. Its seed is fixed once committed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    pub id: String,
    pub seed: u64,
    #[serde(default)]
    pub mode: RunMode,
    #[serde(default)]
    pub preserve: BTreeSet<String>,
    /// `table.column` to a strategy name, as the HTTP API takes it.
    #[serde(default)]
    pub column_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub force_preserve: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Holds today; a regression fails the suite.
    Met,
    /// The behaviour wanted when the engine is ready; its test is ignored.
    Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ruling {
    /// Nobody would argue with it.
    Obvious,
    /// A product decision the owner has not taken yet.
    NeedsOwner,
    /// A product decision the owner took; `rationale` records it.
    OwnerRuled,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub id: String,
    pub status: Status,
    pub ruling: Ruling,
    /// Required unless the ruling is `obvious`.
    #[serde(default)]
    pub rationale: Option<String>,
    /// The ignore reason of a target, when the id alone does not say it.
    #[serde(default)]
    pub gap: Option<String>,
    #[serde(default = "default_run")]
    pub run: String,
    pub assert: Assertion,
}

fn default_run() -> String {
    "default".to_owned()
}

impl ScenarioFile {
    pub fn run(&self, id: &str) -> Option<&RunSpec> {
        self.runs.iter().find(|run| run.id == id)
    }

    pub fn expectation(&self, id: &str) -> Option<&Expectation> {
        self.expectations.iter().find(|e| e.id == id)
    }

    /// Structural problems the type system cannot express.
    pub fn problems(&self, directory: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if self.format != FORMAT_VERSION {
            problems.push(format!(
                "format is {}, expected {FORMAT_VERSION}",
                self.format
            ));
        }
        if self.scenario != directory {
            problems.push(format!(
                "scenario `{}` does not match its directory `{directory}`",
                self.scenario
            ));
        }
        let mut run_ids = BTreeSet::new();
        for run in &self.runs {
            if !run_ids.insert(run.id.as_str()) {
                problems.push(format!("run `{}` is declared twice", run.id));
            }
        }
        let mut ids = BTreeSet::new();
        for expectation in &self.expectations {
            if !ids.insert(expectation.id.as_str()) {
                problems.push(format!(
                    "expectation `{}` is declared twice",
                    expectation.id
                ));
            }
            if !run_ids.contains(expectation.run.as_str()) {
                problems.push(format!(
                    "expectation `{}` names unknown run `{}`",
                    expectation.id, expectation.run
                ));
            }
            if expectation.ruling != Ruling::Obvious
                && expectation.rationale.as_deref().is_none_or(str::is_empty)
            {
                problems.push(format!(
                    "expectation `{}` needs a rationale for its ruling",
                    expectation.id
                ));
            }
        }
        problems
    }
}
