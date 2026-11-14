//! Whether source values survive into the output or the report, judged by
//! the corpus's own comparison rather than the engine's leakage check.
//!
//! Comparisons fold case and ignore surrounding whitespace. A letter
//! fragment is only ever sought in the same row's output, never across a
//! column: a common surname reappearing elsewhere is not a leak. In an email
//! only the local part is compared for fragments, since the domain is kept
//! on purpose and identifies no one. A value or digit run made of one
//! repeated character (`(00) 00000-0000`, `000000000`) carries no
//! information and is never a source to protect: it is also what a mask
//! looks like.

use std::collections::BTreeSet;

use super::{Ctx, Finding, Fragments, Judged, Scope, digits_of, tally, text_of};

fn fold(text: &str) -> String {
    text.trim().to_lowercase()
}

/// Whether the letters and digits of `text` are more than one character
/// repeated.
fn informative(text: &str) -> bool {
    let mut symbols = text
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase);
    match symbols.next() {
        Some(first) => symbols.any(|c| c != first),
        None => false,
    }
}

/// The part of a value fragments are compared on: an email's local part,
/// anything else whole.
fn identifying(text: &str) -> &str {
    text.rsplit_once('@').map_or(text, |(local, _)| local)
}

fn letter_runs(text: &str, min: usize) -> Vec<String> {
    identifying(text)
        .split(|c: char| !c.is_alphabetic())
        .filter(|run| run.chars().count() >= min)
        .map(str::to_lowercase)
        .collect()
}

fn digit_windows(text: &str, width: usize) -> Vec<String> {
    let digits = digits_of(text);
    if width == 0 || digits.len() < width {
        return Vec::new();
    }
    (0..=digits.len() - width)
        .map(|start| digits[start..start + width].to_owned())
        .filter(|window| informative(window))
        .collect()
}

pub fn no_source_survival(
    ctx: &Ctx,
    columns: &[String],
    sources: Option<&[String]>,
    scope: Scope,
    whole: bool,
    fragments: Option<Fragments>,
) -> Judged {
    let sources = sources.unwrap_or(columns);
    let letters = fragments.and_then(|f| f.letters);
    let digits = fragments.and_then(|f| f.digits);
    if !whole && letters.is_none() && digits.is_none() {
        return Err(Finding::Unevaluable {
            detail: "neither whole values nor fragments are compared".to_owned(),
        });
    }
    match scope {
        Scope::Row => row_scope(ctx, columns, sources, whole, letters, digits),
        Scope::Column if letters.is_some() => Err(Finding::Unevaluable {
            detail: "letter fragments are compared within a row only".to_owned(),
        }),
        Scope::Column => column_scope(ctx, columns, sources, whole, digits),
    }
}

fn table_of(column: &str) -> &str {
    column.split_once('.').map_or(column, |(table, _)| table)
}

fn row_scope(
    ctx: &Ctx,
    columns: &[String],
    sources: &[String],
    whole: bool,
    letters: Option<usize>,
    digits: Option<usize>,
) -> Judged {
    let mut evaluated = 0;
    let mut failures = Vec::new();
    for column in columns {
        let output = ctx.output(column)?;
        for source_name in sources.iter().filter(|s| table_of(s) == table_of(column)) {
            let source = ctx.source(source_name)?;
            for (row, (out, src)) in output.values.iter().zip(&source.values).enumerate() {
                let (Some(out), Some(src)) = (text_of(out), text_of(src)) else {
                    continue;
                };
                if !informative(&src) {
                    continue;
                }
                evaluated += 1;
                if whole && fold(&out) == fold(&src) {
                    failures.push(format!("{column} row {row} equals its {source_name}"));
                    continue;
                }
                if let Some(min) = letters {
                    let out_part = identifying(&out).to_lowercase();
                    if letter_runs(&src, min)
                        .iter()
                        .any(|run| out_part.contains(run))
                    {
                        failures.push(format!("{column} row {row} keeps a word of {source_name}"));
                        continue;
                    }
                }
                if let Some(width) = digits {
                    let out_digits = digits_of(&out);
                    if digit_windows(&src, width)
                        .iter()
                        .any(|w| out_digits.contains(w))
                    {
                        failures.push(format!(
                            "{column} row {row} keeps {width} digits of {source_name}"
                        ));
                    }
                }
            }
        }
    }
    Ok(tally(evaluated, &failures, "cell comparisons"))
}

fn column_scope(
    ctx: &Ctx,
    columns: &[String],
    sources: &[String],
    whole: bool,
    digits: Option<usize>,
) -> Judged {
    let mut whole_values = BTreeSet::new();
    let mut windows = BTreeSet::new();
    for source_name in sources {
        for value in &ctx.source(source_name)?.values {
            let Some(text) = text_of(value).filter(|t| informative(t)) else {
                continue;
            };
            if let Some(width) = digits {
                windows.extend(digit_windows(&text, width));
            }
            whole_values.insert(fold(&text));
        }
    }
    let mut evaluated = 0;
    let mut failures = Vec::new();
    for column in columns {
        for (row, value) in ctx.output(column)?.values.iter().enumerate() {
            let Some(out) = text_of(value) else { continue };
            evaluated += 1;
            if whole && whole_values.contains(&fold(&out)) {
                failures.push(format!("{column} row {row} equals a source value"));
            } else if let Some(width) = digits {
                if digit_windows(&out, width)
                    .iter()
                    .any(|w| windows.contains(w))
                {
                    failures.push(format!("{column} row {row} keeps {width} source digits"));
                }
            }
        }
    }
    Ok(tally(evaluated, &failures, "output cells"))
}

pub fn report_free(ctx: &Ctx, columns: &[String], min_len: usize) -> Judged {
    let report = ctx.report()?.to_string();
    let mut evaluated = 0;
    let mut failures = Vec::new();
    for column in columns {
        for (row, value) in ctx.source(column)?.values.iter().enumerate() {
            let Some(text) = text_of(value) else { continue };
            let text = text.trim();
            if text.chars().count() < min_len || !informative(text) {
                continue;
            }
            evaluated += 1;
            let escaped = serde_json::Value::String(text.to_owned()).to_string();
            if report.contains(escaped.trim_matches('"')) {
                failures.push(format!("{column} row {row}"));
            }
        }
    }
    Ok(tally(evaluated, &failures, "source values"))
}

pub fn values_changed(ctx: &Ctx, column: &str, min_fraction: f64) -> Judged {
    let source = ctx.source(column)?;
    let output = ctx.output(column)?;
    let pairs: Vec<_> = source
        .values
        .iter()
        .zip(&output.values)
        .filter(|(src, _)| !src.is_null())
        .collect();
    if pairs.is_empty() {
        return Err(Finding::Unevaluable {
            detail: format!("`{column}` has no non-null source value"),
        });
    }
    let changed = pairs.iter().filter(|(src, out)| src != out).count();
    let fraction = changed as f64 / pairs.len() as f64;
    if fraction >= min_fraction {
        Ok(Finding::Holds {
            evaluated: pairs.len(),
        })
    } else {
        Err(Finding::Fails {
            detail: format!(
                "{changed} of {} values changed ({fraction:.4}), wanted at least {min_fraction}",
                pairs.len()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_email_domain_is_not_a_fragment_source() {
        assert_eq!(letter_runs("joao.silva@gmail.com", 5), vec!["silva"]);
        assert_eq!(
            letter_runs("Maria da Conceição", 5),
            vec!["maria", "conceição"]
        );
    }

    #[test]
    fn digit_windows_drop_punctuation() {
        assert_eq!(
            digit_windows("123.456.789-09", 10),
            vec!["1234567890", "2345678909"]
        );
        assert!(digit_windows("12-34", 9).is_empty());
        assert!(digit_windows("(00) 00000-0000", 9).is_empty());
    }

    #[test]
    fn a_repeated_character_carries_no_information() {
        assert!(!informative("(00) 00000-0000"));
        assert!(!informative("xxx-XXX"));
        assert!(informative("teste@teste.com"));
    }
}
