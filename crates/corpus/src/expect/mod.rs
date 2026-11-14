//! The assertion vocabulary and its evaluation.
//!
//! Every assertion is judged by corpus code against the source dataset, the
//! output dataset and the JSON of the report and profile. None reuses an
//! engine check: an expectation that asked the engine whether it succeeded
//! would hold by construction.

mod bookkeeping;
mod classification;
mod fidelity;
mod format;
mod outcome;
mod privacy;
mod structure;

use anonymizer::dataset::ColumnData;
use anonymizer::{Dataset, Value};
use serde::Deserialize;

use crate::generate::docs::Document;
use crate::load::Loaded;
use crate::runs::RunResult;
use crate::scenario::RunSpec;
use crate::shapes::ShapeRef;

pub use fidelity::total_variation_distance;
pub use structure::orphan_positions;

/// What evaluating one expectation found.
#[derive(Debug, Clone, PartialEq)]
pub enum Finding {
    /// The assertion holds over `evaluated` inspected cells, rows or fields.
    Holds {
        evaluated: usize,
    },
    Fails {
        detail: String,
    },
    /// Nothing the assertion names could be inspected, so it says nothing.
    Unevaluable {
        detail: String,
    },
}

impl Finding {
    pub fn holds(&self) -> bool {
        matches!(self, Finding::Holds { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Finding::Holds { evaluated } => format!("holds over {evaluated} inspected"),
            Finding::Fails { detail } => format!("fails: {detail}"),
            Finding::Unevaluable { detail } => format!("unevaluable: {detail}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assertion {
    Verdict {
        is: String,
    },
    Refused {
        error: String,
        #[serde(default)]
        column: Option<String>,
    },
    AcceptedAtStrengthAtMost {
        strength: String,
    },
    Deterministic {
        threads: Vec<usize>,
    },
    SemanticType {
        column: String,
        is: String,
        #[serde(default)]
        source: TypeSource,
    },
    SemanticTypeOneOf {
        column: String,
        any_of: Vec<String>,
    },
    NotSemanticType {
        column: String,
        is_not: String,
    },
    Strategy {
        column: String,
        is: String,
    },
    ProfileAgreesWithReport {},
    NoSourceSurvival {
        columns: Vec<String>,
        #[serde(default)]
        sources: Option<Vec<String>>,
        scope: Scope,
        #[serde(default)]
        whole: bool,
        #[serde(default)]
        fragments: Option<Fragments>,
    },
    ReportFreeOfSourceValues {
        columns: Vec<String>,
        min_len: usize,
    },
    ValuesChanged {
        column: String,
        min_fraction: f64,
    },
    FormatPreserved {
        column: String,
        shapes: Vec<ShapeRef>,
    },
    CheckDigitValidityPreserved {
        column: String,
        document: Document,
    },
    NullPositionsPreserved {
        column: String,
    },
    RowCountPreserved {
        tables: Tables,
    },
    ReferentialIntegrity {
        column: String,
        orphans: usize,
    },
    KeyIdentity {
        columns: Vec<String>,
    },
    SameSourceSameOutput {
        column: String,
        normalize: Normalize,
    },
    ValuesUnchanged {
        column: String,
    },
    Distribution {
        column: String,
        feature: Feature,
        max_tvd: f64,
    },
    DistinctRatio {
        column: String,
        min: f64,
        max: f64,
    },
    NumericSummary {
        column: String,
        mean_rel_tol: f64,
        #[serde(default)]
        p50_rel_tol: Option<f64>,
        #[serde(default)]
        p90_rel_tol: Option<f64>,
    },
    AnomalyRate {
        column: String,
        anomaly: String,
        before: Approx,
        after: After,
    },
    NonProductionRows {
        table: String,
        flagged: Count,
        mixed: Count,
        source: Source,
    },
    Preserved {
        column: String,
        reason: PreserveWanted,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeSource {
    Report,
    Profile,
    #[default]
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Report,
    Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Each output cell against the source cells of its own row.
    Row,
    /// Each output cell against every source cell of the source columns.
    Column,
}

/// Fragment lengths at and above which a surviving piece of a source value
/// counts: a run of `letters` letters, or `digits` consecutive digits once
/// punctuation is dropped.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fragments {
    #[serde(default)]
    pub letters: Option<usize>,
    #[serde(default)]
    pub digits: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Tables {
    All(AllTables),
    Listed(Vec<String>),
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub enum AllTables {
    #[serde(rename = "*")]
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalize {
    None,
    Digits,
    Casefold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Value,
    EmailDomain,
    PhoneDdd,
    CepPrefix3,
    Length,
    Shape,
    Null,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approx {
    pub value: f64,
    pub tol: f64,
}

impl Approx {
    pub fn admits(&self, observed: f64) -> bool {
        (observed - self.value).abs() <= self.tol
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum After {
    Keyword(AfterKeyword),
    Approx(Approx),
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterKeyword {
    EqualsBefore,
}

/// `{"equals": n}`, or a range with either bound optional.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Count {
    #[serde(default)]
    pub equals: Option<usize>,
    #[serde(default)]
    pub min: Option<usize>,
    #[serde(default)]
    pub max: Option<usize>,
}

impl Count {
    pub fn admits(&self, observed: usize) -> bool {
        self.equals.is_none_or(|n| n == observed)
            && self.min.is_none_or(|n| observed >= n)
            && self.max.is_none_or(|n| observed <= n)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreserveWanted {
    Requested,
    Propagated,
}

/// Everything an assertion may look at.
pub struct Ctx<'a> {
    pub scenario: &'a str,
    pub loaded: &'a Loaded,
    pub spec: &'a RunSpec,
    pub run: &'a RunResult,
}

/// Early exit for an evaluator: either way the value is the finding.
pub type Judged = Result<Finding, Finding>;

impl Ctx<'_> {
    pub fn source(&self, column: &str) -> Result<&ColumnData, Finding> {
        column_of(&self.loaded.source, column)
    }

    pub fn output_dataset(&self) -> Result<&Dataset, Finding> {
        self.run
            .output
            .as_ref()
            .ok_or_else(|| self.not_anonymized())
    }

    pub fn output(&self, column: &str) -> Result<&ColumnData, Finding> {
        column_of(self.output_dataset()?, column)
    }

    pub fn report(&self) -> Result<&serde_json::Value, Finding> {
        self.run
            .report
            .as_ref()
            .ok_or_else(|| self.not_anonymized())
    }

    pub fn profile(&self) -> Result<&serde_json::Value, Finding> {
        self.run.profile.as_ref().ok_or_else(|| Finding::Fails {
            detail: format!("run `{}` produced no profile", self.spec.id),
        })
    }

    /// The report's entry for `column`.
    pub fn report_column(&self, column: &str) -> Result<&serde_json::Value, Finding> {
        self.report()?["columns"]
            .as_array()
            .and_then(|columns| columns.iter().find(|c| c["column"] == column))
            .ok_or_else(|| Finding::Fails {
                detail: format!("the report has no entry for `{column}`"),
            })
    }

    fn not_anonymized(&self) -> Finding {
        let why = match &self.run.error {
            Some(error) => format!("was refused: {}", error.display),
            None => "is a profile run".to_owned(),
        };
        Finding::Fails {
            detail: format!("run `{}` {why}", self.spec.id),
        }
    }
}

pub fn column_of<'d>(dataset: &'d Dataset, column: &str) -> Result<&'d ColumnData, Finding> {
    column
        .split_once('.')
        .and_then(|(table, name)| dataset.table(table)?.column(name))
        .ok_or_else(|| Finding::Fails {
            detail: format!("no column `{column}`"),
        })
}

/// A cell as text: integers and floats print as they would in a dump.
pub fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Int(n) => Some(n.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Text(s) => Some(s.clone()),
    }
}

pub fn digits_of(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

/// The finding for `failures` out of `evaluated` inspected items, quoting at
/// most three examples.
pub fn tally(evaluated: usize, failures: &[String], what: &str) -> Finding {
    if evaluated == 0 {
        return Finding::Unevaluable {
            detail: format!("no {what} to inspect"),
        };
    }
    if failures.is_empty() {
        return Finding::Holds { evaluated };
    }
    let examples: Vec<&str> = failures.iter().take(3).map(String::as_str).collect();
    Finding::Fails {
        detail: format!(
            "{} of {evaluated} {what} fail, e.g. {}",
            failures.len(),
            examples.join("; ")
        ),
    }
}

pub fn evaluate(assertion: &Assertion, ctx: &Ctx) -> Finding {
    use Assertion as A;
    let judged = match assertion {
        A::Verdict { is } => outcome::verdict(ctx, is),
        A::Refused { error, column } => outcome::refused(ctx, error, column.as_deref()),
        A::AcceptedAtStrengthAtMost { strength } => outcome::strength_at_most(ctx, strength),
        A::Deterministic { threads } => outcome::deterministic(ctx, threads),
        A::SemanticType { column, is, source } => {
            classification::semantic_type(ctx, column, is, *source)
        }
        A::SemanticTypeOneOf { column, any_of } => classification::one_of(ctx, column, any_of),
        A::NotSemanticType { column, is_not } => classification::not_type(ctx, column, is_not),
        A::Strategy { column, is } => classification::strategy(ctx, column, is),
        A::ProfileAgreesWithReport {} => classification::profile_agrees(ctx),
        A::NoSourceSurvival {
            columns,
            sources,
            scope,
            whole,
            fragments,
        } => privacy::no_source_survival(
            ctx,
            columns,
            sources.as_deref(),
            *scope,
            *whole,
            *fragments,
        ),
        A::ReportFreeOfSourceValues { columns, min_len } => {
            privacy::report_free(ctx, columns, *min_len)
        }
        A::ValuesChanged {
            column,
            min_fraction,
        } => privacy::values_changed(ctx, column, *min_fraction),
        A::FormatPreserved { column, shapes } => format::format_preserved(ctx, column, shapes),
        A::CheckDigitValidityPreserved { column, document } => {
            format::validity_preserved(ctx, column, *document)
        }
        A::NullPositionsPreserved { column } => format::nulls_preserved(ctx, column),
        A::RowCountPreserved { tables } => structure::row_counts(ctx, tables),
        A::ReferentialIntegrity { column, orphans } => {
            structure::referential(ctx, column, *orphans)
        }
        A::KeyIdentity { columns } => structure::key_identity(ctx, columns),
        A::SameSourceSameOutput { column, normalize } => {
            structure::same_source_same_output(ctx, column, *normalize)
        }
        A::ValuesUnchanged { column } => structure::unchanged(ctx, column),
        A::Distribution {
            column,
            feature,
            max_tvd,
        } => fidelity::distribution(ctx, column, *feature, *max_tvd),
        A::DistinctRatio { column, min, max } => fidelity::distinct_ratio(ctx, column, *min, *max),
        A::NumericSummary {
            column,
            mean_rel_tol,
            p50_rel_tol,
            p90_rel_tol,
        } => fidelity::numeric_summary(ctx, column, *mean_rel_tol, *p50_rel_tol, *p90_rel_tol),
        A::AnomalyRate {
            column,
            anomaly,
            before,
            after,
        } => bookkeeping::anomaly_rate(ctx, column, anomaly, *before, *after),
        A::NonProductionRows {
            table,
            flagged,
            mixed,
            source,
        } => bookkeeping::non_production_rows(ctx, table, *flagged, *mixed, *source),
        A::Preserved { column, reason } => bookkeeping::preserved(ctx, column, *reason),
    };
    judged.unwrap_or_else(|finding| finding)
}
