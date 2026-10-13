use std::collections::{BTreeMap, BTreeSet};

use rayon::prelude::*;
use serde::Serialize;

use crate::anomaly::AnomalyKind;
use crate::classify::{self, Classification, SemanticType};
use crate::dataset::{ColumnData, Dataset, Value};

/// How many distinct values a categorical distribution names individually
/// before the remainder is folded into [`CategoricalProfile::other`].
const TOP_K: usize = 10;

/// How many equal-width buckets a numeric histogram is divided into.
const NUMERIC_BUCKETS: usize = 10;

/// The (low, high) percentiles a numeric histogram's range is bucketed over.
const NUMERIC_RANGE_PERCENTILES: (f64, f64) = (0.05, 0.95);

/// Below this many non-null values, a column is too small for the
/// high-cardinality ratio to mean anything: a short sample is exactly where
/// confidence should be lowest, so it is folded away rather than shown in
/// full.
const HIGH_CARDINALITY_MIN_SAMPLE: usize = 20;

/// Above this fraction of distinct-to-total, a categorical breakdown is
/// mostly single- or few-occurrence values: not useful as a summary, and for
/// a column that turns out to be quasi-identifying, not safe either.
const HIGH_CARDINALITY_RATIO: f64 = 0.1;

/// A value occurring fewer than this many times is folded into
/// [`CategoricalProfile::other`] regardless of its rank, independent of the
/// cardinality ratio: a couple, a household or a shared device clears the
/// ratio check in a column otherwise safe to summarise, but a count this low
/// still singles out a small group.
const MIN_KEY_COUNT: usize = 3;

/// Stands in for an email that has no `@` to split on, so a malformed value
/// never ends up as a raw categorical key.
const INVALID_EMAIL_DOMAIN: &str = "(invalid)";

/// The key [`crate::report::ColumnReport`] uses for everything outside a
/// categorical distribution's top-k. A source value spelled exactly this way
/// is folded into that bucket rather than kept as a top-k key, so it can
/// never collide with the sentinel in [`Distribution::shares`].
pub const OTHER_BUCKET: &str = "__other__";

/// Output of `POST /v1/profile`: steps 2-3 with no transformation.
#[derive(Debug, Clone, Serialize)]
pub struct DatasetProfile {
    /// Keyed by `table.column`.
    pub columns: BTreeMap<String, ColumnProfile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnProfile {
    pub column: String,
    pub semantic_type: SemanticType,
    /// See [`crate::classify::Classification::confidence`]: a match rate,
    /// not a calibrated probability.
    pub confidence: f64,
    pub null_rate: f64,
    pub distinct: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<LengthStats>,
    pub distribution: Distribution,
    pub anomaly_rates: BTreeMap<AnomalyKind, f64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LengthStats {
    pub min: usize,
    pub max: usize,
    pub mean: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Distribution {
    Categorical(CategoricalProfile),
    Numeric(NumericProfile),
    /// Too high-cardinality or too unstructured to summarise usefully, or
    /// the column is a direct identifier whose categorical distribution
    /// would be a list of raw PII values rather than a safe aggregate.
    Unsummarised,
}

impl Distribution {
    /// The only way to produce a populated [`Distribution`]: every path is
    /// decided from `semantic_type` here, so a direct identifier's raw
    /// values cannot reach a categorical or numeric summary by some other
    /// construction site forgetting the check.
    fn new(semantic_type: SemanticType, values: &[&Value]) -> Distribution {
        if values.is_empty() {
            return Distribution::Unsummarised;
        }

        if semantic_type == SemanticType::Email {
            // The domain, unlike the address, is not identifying on its own,
            // so it is exempt from the low-count floor applied below.
            let domains = values
                .iter()
                .filter_map(|value| value.as_text())
                .map(email_domain_key);
            return Distribution::Categorical(categorical_profile(domains, values.len(), 1));
        }

        if semantic_type.is_direct_identifier() {
            return Distribution::Unsummarised;
        }

        let numeric: Vec<f64> = values
            .iter()
            .filter_map(|value| match value {
                Value::Int(n) => Some(*n as f64),
                Value::Float(f) => Some(*f),
                _ => None,
            })
            .collect();
        if numeric.len() == values.len() {
            return Distribution::Numeric(numeric_profile(&numeric));
        }

        let keys: Vec<String> = values.iter().map(|value| canonical_key(value)).collect();
        if is_too_high_cardinality(&keys) {
            return Distribution::Unsummarised;
        }

        let total = keys.len();
        Distribution::Categorical(categorical_profile(keys.into_iter(), total, MIN_KEY_COUNT))
    }

    /// Flattens to the shares-of-domain shape the fidelity report and
    /// [`total_variation_distance`] operate on. A numeric histogram and an
    /// unsummarised column have no safe or meaningful flat projection, so
    /// they contribute no shares at all rather than an approximation.
    pub fn shares(&self) -> BTreeMap<String, f64> {
        match self {
            Distribution::Categorical(profile) => {
                let mut shares = profile.top.clone();
                if profile.other > 0.0 {
                    shares.insert(OTHER_BUCKET.to_owned(), profile.other);
                }
                shares
            }
            Distribution::Numeric(_) | Distribution::Unsummarised => BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoricalProfile {
    /// Share of the top-k most frequent values, by value. Domain keys for
    /// email columns, raw values only for columns safe to summarise this
    /// way; never a direct identifier's raw value.
    top: BTreeMap<String, f64>,
    /// Share held by every value outside the top-k, aggregated into one
    /// bucket rather than named individually.
    other: f64,
    cardinality: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct NumericProfile {
    pub(crate) mean: f64,
    pub(crate) stddev: f64,
    /// The sample median: a center estimate with a 50% breakdown point,
    /// unlike `mean`. Over an odd-sized sample it is literally one row's
    /// value, the same exposure the histogram's edges are built to avoid
    /// (see [`interior_range`]), so it is never serialized.
    #[serde(skip)]
    pub(crate) median: f64,
    /// Median absolute deviation from `median`: the robust counterpart to
    /// `stddev`, with the same 50% breakdown point as `median` rather than
    /// `stddev`'s `1/n`. Not serialized for the same reason `median` is not.
    #[serde(skip)]
    pub(crate) mad: f64,
    histogram: Vec<Bucket>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Bucket {
    pub lower: f64,
    pub upper: f64,
    pub count: usize,
}

/// Classifies and profiles every column of `dataset` in parallel. Results
/// are collected into a [`BTreeMap`] keyed by qualified column name, so the
/// output does not depend on the order rayon happens to finish work in.
///
/// Anomaly detection runs exactly once per column: [`profile_column`] builds
/// everything detection needs without detecting itself, so this is the one
/// place that calls [`crate::anomaly::detect`] to fill in `anomaly_rates`.
pub fn profile_dataset(dataset: &Dataset) -> DatasetProfile {
    let columns: BTreeMap<String, ColumnProfile> = dataset
        .qualified_columns()
        .into_par_iter()
        .map(|(qualified, column)| {
            let classification = classify::classify(&column.spec.name, &column.values);
            let mut profile = profile_column(&qualified, column, classification);
            let scan = crate::anomaly::detect(column, &profile);
            profile.anomaly_rates = crate::anomaly::rates(&scan, column.len());
            (qualified.clone(), profile)
        })
        .collect();

    DatasetProfile { columns }
}

/// Profiles `column` without running anomaly detection. `anomaly_rates` is
/// left empty: detection needs this profile's `length` and `distribution` to
/// run, and a caller that also needs the full [`crate::anomaly::AnomalyScan`]
/// (not just its rates) should detect once of its own accord and fill the
/// field in, rather than this function detecting a second time only to have
/// the result discarded.
pub fn profile_column(
    qualified_name: &str,
    column: &ColumnData,
    classification: Classification,
) -> ColumnProfile {
    let total = column.len();
    let non_null: Vec<&Value> = column
        .values
        .iter()
        .filter(|value| !value.is_null())
        .collect();
    let null_rate = if total == 0 {
        0.0
    } else {
        (total - non_null.len()) as f64 / total as f64
    };

    let length = if classification.semantic_type.is_direct_identifier() {
        None
    } else {
        text_length_stats(&non_null)
    };

    ColumnProfile {
        column: qualified_name.to_owned(),
        semantic_type: classification.semantic_type,
        confidence: f64::from(classification.confidence),
        null_rate,
        distinct: distinct_count(&non_null),
        length,
        distribution: Distribution::new(classification.semantic_type, &non_null),
        anomaly_rates: BTreeMap::new(),
    }
}

/// Total variation distance between two shares-of-domain histograms: half
/// the sum of absolute differences in share, over the union of their keys.
pub fn total_variation_distance(
    before: &BTreeMap<String, f64>,
    after: &BTreeMap<String, f64>,
) -> f64 {
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let total: f64 = keys
        .into_iter()
        .map(|key| {
            let b = before.get(key).copied().unwrap_or(0.0);
            let a = after.get(key).copied().unwrap_or(0.0);
            (b - a).abs()
        })
        .sum();
    // `.abs()` rather than a bare division: an empty sum over `f64` is `-0.0`,
    // and a distance can never be negative.
    (total / 2.0).abs()
}

pub(crate) fn canonical_key(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(s) => s.clone(),
    }
}

fn distinct_count(values: &[&Value]) -> usize {
    values
        .iter()
        .map(|value| canonical_key(value))
        .collect::<BTreeSet<_>>()
        .len()
}

fn text_length_stats(values: &[&Value]) -> Option<LengthStats> {
    let lengths: Vec<usize> = values
        .iter()
        .filter_map(|value| value.as_text())
        .map(|s| s.chars().count())
        .collect();
    if lengths.is_empty() {
        return None;
    }
    let min = lengths.iter().copied().min().unwrap_or(0);
    let max = lengths.iter().copied().max().unwrap_or(0);
    let mean = lengths.iter().sum::<usize>() as f64 / lengths.len() as f64;
    Some(LengthStats { min, max, mean })
}

fn is_too_high_cardinality(keys: &[String]) -> bool {
    if keys.len() < HIGH_CARDINALITY_MIN_SAMPLE {
        return true;
    }
    let distinct = keys.iter().collect::<BTreeSet<_>>().len();
    distinct as f64 / keys.len() as f64 > HIGH_CARDINALITY_RATIO
}

fn email_domain_key(text: &str) -> String {
    text.rsplit_once('@')
        .map(|(_, domain)| domain.to_owned())
        .unwrap_or_else(|| INVALID_EMAIL_DOMAIN.to_owned())
}

/// `min_key_count` is the floor below which a value is folded into
/// [`CategoricalProfile::other`] regardless of rank: 1 for a domain-style
/// breakdown where no individual key is identifying, [`MIN_KEY_COUNT`] for a
/// breakdown over raw values, where a handful of occurrences can single out
/// a small group.
fn categorical_profile<I: Iterator<Item = String>>(
    keys: I,
    total: usize,
    min_key_count: usize,
) -> CategoricalProfile {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for key in keys {
        *counts.entry(key).or_insert(0) += 1;
    }
    let cardinality = counts.len();

    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut top = BTreeMap::new();
    let mut other_count = 0usize;
    for (index, (key, count)) in ranked.into_iter().enumerate() {
        if index < TOP_K && count >= min_key_count && key != OTHER_BUCKET {
            top.insert(key, share(count, total));
        } else {
            other_count += count;
        }
    }

    CategoricalProfile {
        top,
        other: share(other_count, total),
        cardinality,
    }
}

fn share(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64
    }
}

fn numeric_profile(values: &[f64]) -> NumericProfile {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;

    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let (lower, upper) = interior_range(&sorted);
    let median = median_of_sorted(&sorted);

    NumericProfile {
        mean,
        stddev: variance.sqrt(),
        median,
        mad: median_absolute_deviation(values, median),
        histogram: numeric_histogram(values, lower, upper),
    }
}

/// The middle value of a sorted, non-empty sample, or the average of the two
/// middle values when its length is even.
fn median_of_sorted(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// The median of `|value - median|` over `values`: a center-and-spread pair
/// with a 50% breakdown point, unlike mean and standard deviation, whose
/// breakdown point is `1/n` — a single extreme value pulls the mean toward
/// itself and inflates the standard deviation computed from it, so neither
/// stays trustworthy once the sample it is meant to summarise contains the
/// very outliers it exists to flag.
fn median_absolute_deviation(values: &[f64], median: f64) -> f64 {
    let mut deviations: Vec<f64> = values.iter().map(|v| (v - median).abs()).collect();
    deviations.sort_by(f64::total_cmp);
    median_of_sorted(&deviations)
}

/// The 5th/95th order statistics of a sorted, non-empty sample, used as the
/// histogram's range instead of the true min/max: a single extreme value —
/// one birth date, one salary — would otherwise fix a bucket edge to its own
/// exact value, and the serialized profile never exposes raw source values.
fn interior_range(sorted: &[f64]) -> (f64, f64) {
    let last = sorted.len() - 1;
    let (low_p, high_p) = NUMERIC_RANGE_PERCENTILES;
    let lower = sorted[(last as f64 * low_p).floor() as usize];
    let upper = sorted[(last as f64 * high_p).floor() as usize];
    (lower, upper)
}

fn numeric_histogram(values: &[f64], lower: f64, upper: f64) -> Vec<Bucket> {
    if upper - lower <= f64::EPSILON {
        return vec![Bucket {
            lower,
            upper,
            count: values.len(),
        }];
    }

    let width = (upper - lower) / NUMERIC_BUCKETS as f64;
    let mut counts = vec![0usize; NUMERIC_BUCKETS];
    for &value in values {
        let offset = (value - lower) / width;
        let index = if offset <= 0.0 {
            0
        } else {
            (offset as usize).min(NUMERIC_BUCKETS - 1)
        };
        counts[index] += 1;
    }

    counts
        .into_iter()
        .enumerate()
        .map(|(index, count)| Bucket {
            lower: lower + width * index as f64,
            upper: lower + width * (index + 1) as f64,
            count,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{ColumnSpec, ColumnType};

    fn text_column(name: &str, values: &[&str]) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty: ColumnType::Text,
                nullable: true,
                pk: false,
                fk: None,
            },
            values: values.iter().map(|v| Value::Text(v.to_string())).collect(),
        }
    }

    fn classification(semantic_type: SemanticType) -> Classification {
        Classification {
            semantic_type,
            confidence: 1.0,
        }
    }

    #[test]
    fn total_variation_distance_of_identical_histograms_is_zero() {
        let mut before = BTreeMap::new();
        before.insert("gmail.com".to_owned(), 0.54);
        before.insert("outlook.com".to_owned(), 0.46);
        assert_eq!(total_variation_distance(&before, &before), 0.0);
    }

    #[test]
    fn total_variation_distance_of_two_empty_histograms_is_positive_zero() {
        let empty = BTreeMap::new();
        let distance = total_variation_distance(&empty, &empty);
        assert_eq!(distance, 0.0);
        assert!(!distance.is_sign_negative(), "must not serialize as -0.0");
    }

    #[test]
    fn total_variation_distance_of_disjoint_histograms_is_one() {
        let mut before = BTreeMap::new();
        before.insert("a".to_owned(), 1.0);
        let mut after = BTreeMap::new();
        after.insert("b".to_owned(), 1.0);
        assert_eq!(total_variation_distance(&before, &after), 1.0);
    }

    #[test]
    fn total_variation_distance_matches_a_known_partial_shift() {
        let mut before = BTreeMap::new();
        before.insert("a".to_owned(), 0.6);
        before.insert("b".to_owned(), 0.4);
        let mut after = BTreeMap::new();
        after.insert("a".to_owned(), 0.5);
        after.insert("b".to_owned(), 0.5);
        assert!((total_variation_distance(&before, &after) - 0.1).abs() < 1e-12);
    }

    #[test]
    fn email_distribution_is_computed_over_the_domain_not_the_address() {
        let column = text_column(
            "email",
            &[
                "ana@gmail.com",
                "bruno@gmail.com",
                "carla@outlook.com",
                "davi@hotmail.com",
            ],
        );
        let profile = profile_column(
            "customers.email",
            &column,
            classification(SemanticType::Email),
        );
        let Distribution::Categorical(categorical) = profile.distribution else {
            panic!("expected a categorical distribution");
        };
        assert_eq!(categorical.top.get("gmail.com"), Some(&0.5));
        assert_eq!(categorical.top.get("outlook.com"), Some(&0.25));
        assert_eq!(categorical.top.get("hotmail.com"), Some(&0.25));
        assert!(categorical.top.keys().all(|key| !key.contains('@')));
    }

    #[test]
    fn a_direct_identifier_column_never_surfaces_a_raw_value_histogram() {
        let column = text_column(
            "full_name",
            &["Ana Souza", "Bruno Lima", "Ana Souza", "Carla Dias"],
        );
        let profile = profile_column(
            "customers.full_name",
            &column,
            classification(SemanticType::FullName),
        );
        assert!(matches!(profile.distribution, Distribution::Unsummarised));
    }

    #[test]
    fn a_low_cardinality_cep_column_is_safe_to_summarise_categorically() {
        let values: Vec<&str> = std::iter::repeat_n("01310-100", 12)
            .chain(std::iter::repeat_n("20040-020", 12))
            .collect();
        let column = text_column("cep", &values);
        let profile = profile_column("customers.cep", &column, classification(SemanticType::Cep));
        assert!(matches!(profile.distribution, Distribution::Categorical(_)));
    }

    #[test]
    fn a_high_cardinality_free_text_column_is_unsummarised() {
        let values: Vec<String> = (0..30)
            .map(|i| format!("unique sentence number {i}"))
            .collect();
        let refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let column = text_column("bio", &refs);
        let profile = profile_column(
            "customers.bio",
            &column,
            classification(SemanticType::FreeText),
        );
        assert!(matches!(profile.distribution, Distribution::Unsummarised));
    }

    #[test]
    fn a_numeric_column_gets_a_bucketed_histogram_and_summary_stats() {
        let column = ColumnData {
            spec: ColumnSpec {
                name: "age".to_owned(),
                ty: ColumnType::Integer,
                nullable: false,
                pk: false,
                fk: None,
            },
            values: (0..10).map(Value::Int).collect(),
        };
        let profile = profile_column(
            "customers.age",
            &column,
            classification(SemanticType::NumericId),
        );
        let Distribution::Numeric(numeric) = profile.distribution else {
            panic!("expected a numeric distribution");
        };
        assert_eq!(numeric.histogram.iter().map(|b| b.count).sum::<usize>(), 10);
    }

    #[test]
    fn null_rate_and_cardinality_are_computed_over_non_null_values() {
        let column = ColumnData {
            spec: ColumnSpec {
                name: "notes".to_owned(),
                ty: ColumnType::Text,
                nullable: true,
                pk: false,
                fk: None,
            },
            values: vec![
                Value::Text("a".into()),
                Value::Null,
                Value::Text("a".into()),
                Value::Null,
            ],
        };
        let profile = profile_column("t.notes", &column, classification(SemanticType::FreeText));
        assert_eq!(profile.null_rate, 0.5);
        assert_eq!(profile.distinct, 1);
    }

    /// A sample too small for the cardinality ratio to mean anything must
    /// be unsummarised, and contribute no shares at all.
    #[test]
    fn a_short_sample_never_surfaces_raw_values() {
        let values = [
            "paciente recusou",
            "sem contato",
            "endereco incorreto",
            "paciente recusou",
            "sem contato",
            "endereco incorreto",
            "paciente recusou",
            "sem contato",
            "endereco incorreto",
            "paciente recusou",
            "sem contato",
            "endereco incorreto",
        ];
        let column = text_column("observacoes", &values);
        let profile = profile_column(
            "customers.observacoes",
            &column,
            classification(SemanticType::FreeText),
        );
        assert!(matches!(profile.distribution, Distribution::Unsummarised));
        assert!(profile.distribution.shares().is_empty());
        let serialized = serde_json::to_string(&profile).expect("profile serializes");
        for value in values {
            assert!(!serialized.contains(value), "leaked `{value}`");
        }
    }

    /// A numeric column's true min and max must never be reported verbatim,
    /// nor leak a second time through the histogram's own edges: a single
    /// extreme value must not appear anywhere in the serialized profile.
    #[test]
    fn numeric_extremes_never_reach_the_serialized_profile() {
        const MIN_CPF: i64 = 11_144_477_735;
        const MAX_CPF: i64 = 52_998_224_725;
        let mut values: Vec<i64> = (0..100).map(|i| 20_000_000_000 + i * 100_000).collect();
        values[0] = MIN_CPF;
        values[99] = MAX_CPF;

        let column = ColumnData {
            spec: ColumnSpec {
                name: "cpf".to_owned(),
                ty: ColumnType::Integer,
                nullable: false,
                pk: false,
                fk: None,
            },
            values: values.into_iter().map(Value::Int).collect(),
        };
        let profile = profile_column(
            "customers.cpf",
            &column,
            classification(SemanticType::NumericId),
        );
        assert!(matches!(profile.distribution, Distribution::Numeric(_)));
        let serialized = serde_json::to_string(&profile).expect("profile serializes");
        assert!(!serialized.contains(&MIN_CPF.to_string()));
        assert!(!serialized.contains(&MAX_CPF.to_string()));
    }

    /// Length statistics must be suppressed for a direct identifier, since
    /// even a single-row column would otherwise leak its own character
    /// count.
    #[test]
    fn length_is_suppressed_for_a_direct_identifier() {
        let column = text_column("full_name", &["Ana Paula Souza"]);
        let profile = profile_column(
            "customers.full_name",
            &column,
            classification(SemanticType::FullName),
        );
        assert!(profile.length.is_none());
        let serialized = serde_json::to_string(&profile).expect("profile serializes");
        assert!(!serialized.contains("\"length\""));
    }

    /// A value occurring only twice — a household or a couple — must be
    /// folded into `other` rather than surface as a named top-k key, even
    /// when the column's overall cardinality ratio is low enough to pass
    /// the guard.
    #[test]
    fn a_pair_occurring_twice_is_folded_into_other_even_when_the_column_passes_the_ratio_guard() {
        let mut values: Vec<String> = vec!["dominant".to_owned(); 170];
        let mut couples = Vec::new();
        for i in 0..15 {
            let value = format!("couple_{i}");
            couples.push(value.clone());
            values.push(value.clone());
            values.push(value);
        }
        let refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let column = text_column("device_id", &refs);
        let profile = profile_column(
            "customers.device_id",
            &column,
            classification(SemanticType::FreeText),
        );
        let Distribution::Categorical(categorical) = profile.distribution else {
            panic!("expected a categorical distribution");
        };
        for couple in &couples {
            assert!(
                !categorical.top.contains_key(couple),
                "`{couple}` should have been folded into `other`"
            );
        }
        assert!(categorical.other > 0.0);
    }
}
