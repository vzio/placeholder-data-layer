//! Distributional fidelity, measured by the corpus's own total variation
//! distance over a named feature of each cell.

use std::collections::{BTreeMap, BTreeSet};

use anonymizer::Value;

use super::{Ctx, Feature, Finding, Judged, digits_of, text_of};
use crate::shapes;

const NULL: &str = "<null>";
const OTHER: &str = "<other>";

/// Half the L1 distance between two empirical distributions given as counts.
pub fn total_variation_distance(a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>) -> f64 {
    let total_a: usize = a.values().sum();
    let total_b: usize = b.values().sum();
    if total_a == 0 || total_b == 0 {
        return if total_a == total_b { 0.0 } else { 1.0 };
    }
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    keys.into_iter()
        .map(|k| {
            let pa = a.get(k).copied().unwrap_or(0) as f64 / total_a as f64;
            let pb = b.get(k).copied().unwrap_or(0) as f64 / total_b as f64;
            (pa - pb).abs()
        })
        .sum::<f64>()
        / 2.0
}

fn phone_ddd(text: &str) -> String {
    let digits = digits_of(text);
    let national = match digits.len() {
        12 | 13 if digits.starts_with("55") => &digits[2..],
        10 | 11 => &digits[..],
        _ => return OTHER.to_owned(),
    };
    national[..2].to_owned()
}

fn feature_of(value: &Value, feature: Feature) -> String {
    let Some(text) = text_of(value) else {
        return NULL.to_owned();
    };
    match feature {
        Feature::Value => text,
        Feature::EmailDomain => text
            .rsplit_once('@')
            .map_or_else(|| OTHER.to_owned(), |(_, d)| d.trim().to_lowercase()),
        Feature::PhoneDdd => phone_ddd(&text),
        Feature::CepPrefix3 => {
            let digits = digits_of(&text);
            if digits.len() == 8 {
                digits[..3].to_owned()
            } else {
                OTHER.to_owned()
            }
        }
        Feature::Length => text.chars().count().to_string(),
        Feature::Shape => shapes::classify(&text).unwrap_or(OTHER).to_owned(),
        Feature::Null => "value".to_owned(),
    }
}

fn histogram(values: &[Value], feature: Feature) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(feature_of(value, feature)).or_insert(0) += 1;
    }
    counts
}

pub fn distribution(ctx: &Ctx, column: &str, feature: Feature, max_tvd: f64) -> Judged {
    let source = ctx.source(column)?;
    let output = ctx.output(column)?;
    if source.values.is_empty() {
        return Err(Finding::Unevaluable {
            detail: format!("`{column}` is empty"),
        });
    }
    let tvd = total_variation_distance(
        &histogram(&source.values, feature),
        &histogram(&output.values, feature),
    );
    if tvd <= max_tvd {
        Ok(Finding::Holds {
            evaluated: source.values.len(),
        })
    } else {
        Err(Finding::Fails {
            detail: format!("total variation distance {tvd:.6} exceeds {max_tvd}"),
        })
    }
}

fn distinct(values: &[Value]) -> usize {
    values
        .iter()
        .filter_map(text_of)
        .collect::<BTreeSet<_>>()
        .len()
}

/// Distinct non-null output values over distinct non-null source values.
pub fn distinct_ratio(ctx: &Ctx, column: &str, min: f64, max: f64) -> Judged {
    let before = distinct(&ctx.source(column)?.values);
    let after = distinct(&ctx.output(column)?.values);
    if before == 0 {
        return Err(Finding::Unevaluable {
            detail: format!("`{column}` has no non-null source value"),
        });
    }
    let ratio = after as f64 / before as f64;
    if (min..=max).contains(&ratio) {
        Ok(Finding::Holds { evaluated: before })
    } else {
        Err(Finding::Fails {
            detail: format!(
                "{after} distinct out of {before} in: ratio {ratio:.4} outside [{min}, {max}]"
            ),
        })
    }
}

fn numbers(values: &[Value]) -> Vec<f64> {
    let mut numbers: Vec<f64> = values
        .iter()
        .filter_map(|v| match v {
            Value::Int(n) => Some(*n as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        })
        .collect();
    numbers.sort_by(f64::total_cmp);
    numbers
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn relative_error(before: f64, after: f64) -> f64 {
    if before == 0.0 {
        after.abs()
    } else {
        ((after - before) / before).abs()
    }
}

pub fn numeric_summary(
    ctx: &Ctx,
    column: &str,
    mean_tol: f64,
    p50_tol: Option<f64>,
    p90_tol: Option<f64>,
) -> Judged {
    let before = numbers(&ctx.source(column)?.values);
    let after = numbers(&ctx.output(column)?.values);
    if before.is_empty() || after.is_empty() {
        return Err(Finding::Unevaluable {
            detail: format!("`{column}` has no numeric values on one side"),
        });
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let mut checks = vec![("mean", mean(&before), mean(&after), mean_tol)];
    if let Some(tol) = p50_tol {
        checks.push(("p50", quantile(&before, 0.5), quantile(&after, 0.5), tol));
    }
    if let Some(tol) = p90_tol {
        checks.push(("p90", quantile(&before, 0.9), quantile(&after, 0.9), tol));
    }
    let failures: Vec<String> = checks
        .iter()
        .filter(|(_, b, a, tol)| relative_error(*b, *a) > *tol)
        .map(|(name, b, a, tol)| format!("{name} {b:.4} -> {a:.4} (tolerance {tol})"))
        .collect();
    if failures.is_empty() {
        Ok(Finding::Holds {
            evaluated: before.len(),
        })
    } else {
        Err(Finding::Fails {
            detail: failures.join("; "),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, usize)]) -> BTreeMap<String, usize> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    }

    #[test]
    fn tvd_is_half_the_l1_distance() {
        let a = counts(&[("x", 3), ("y", 1)]);
        let b = counts(&[("x", 1), ("y", 1), ("z", 2)]);
        assert!((total_variation_distance(&a, &b) - 0.5).abs() < 1e-12);
        assert_eq!(total_variation_distance(&a, &a), 0.0);
    }

    #[test]
    fn features_read_the_part_they_name() {
        let email = Value::Text("Ana@GMAIL.com".into());
        assert_eq!(feature_of(&email, Feature::EmailDomain), "gmail.com");
        let phone = Value::Text("+55 (21) 98765-4321".into());
        assert_eq!(feature_of(&phone, Feature::PhoneDdd), "21");
        assert_eq!(feature_of(&Value::Null, Feature::Shape), NULL);
        assert_eq!(
            feature_of(&Value::Text("01310-100".into()), Feature::CepPrefix3),
            "013"
        );
    }
}
