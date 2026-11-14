//! Row counts, joins and key identity, recovered by positional join the way
//! a caller holding the source dump would recover them.

use std::collections::{BTreeMap, BTreeSet};

use anonymizer::Value;

use super::{Ctx, Finding, Judged, Normalize, Tables, column_of, digits_of, tally, text_of};

/// A cell's identity as a key: type and value, so `1` and `"1"` differ.
fn key(value: &Value) -> Option<String> {
    (!value.is_null()).then(|| format!("{value:?}"))
}

/// Rows of `references` holding a non-null value absent from `targets`.
pub fn orphan_positions(references: &[Value], targets: &[Value]) -> Vec<usize> {
    let known: BTreeSet<String> = targets.iter().filter_map(key).collect();
    references
        .iter()
        .enumerate()
        .filter(|(_, value)| key(value).is_some_and(|k| !known.contains(&k)))
        .map(|(row, _)| row)
        .collect()
}

pub fn row_counts(ctx: &Ctx, tables: &Tables) -> Judged {
    let names: Vec<String> = match tables {
        Tables::All(_) => ctx
            .loaded
            .source
            .tables
            .iter()
            .map(|t| t.name.clone())
            .collect(),
        Tables::Listed(names) => names.clone(),
    };
    let output = ctx.output_dataset()?;
    let mut failures = Vec::new();
    for name in &names {
        let before = ctx.loaded.source.table(name).map(|t| t.row_count());
        let after = output.table(name).map(|t| t.row_count());
        if before.is_none() || before != after {
            failures.push(format!("{name}: {before:?} rows in, {after:?} out"));
        }
    }
    Ok(tally(names.len(), &failures, "tables"))
}

pub fn referential(ctx: &Ctx, column: &str, orphans: usize) -> Judged {
    let source = ctx.source(column)?;
    let Some(fk) = &source.spec.fk else {
        return Err(Finding::Unevaluable {
            detail: format!("`{column}` declares no foreign key"),
        });
    };
    let target_name = format!("{}.{}", fk.table, fk.column);
    let source_target = ctx.source(&target_name)?;
    let output = ctx.output(column)?;
    let output_target = column_of(ctx.output_dataset()?, &target_name)?;

    let source_orphans = orphan_positions(&source.values, &source_target.values);
    let output_orphans = orphan_positions(&output.values, &output_target.values);
    let mut failures = Vec::new();
    if source_orphans.len() != orphans {
        failures.push(format!(
            "the source holds {} orphans, the expectation says {orphans}",
            source_orphans.len()
        ));
    }
    if source_orphans != output_orphans {
        failures.push(format!(
            "orphans moved: {} in, {} out",
            source_orphans.len(),
            output_orphans.len()
        ));
    }
    let target_row: BTreeMap<String, usize> = source_target
        .values
        .iter()
        .enumerate()
        .filter_map(|(row, value)| Some((key(value)?, row)))
        .collect();
    let mut evaluated = 0;
    for (row, (src, out)) in source.values.iter().zip(&output.values).enumerate() {
        let Some(&joined) = key(src).as_ref().and_then(|k| target_row.get(k)) else {
            continue;
        };
        evaluated += 1;
        if output_target.values.get(joined) != Some(out) {
            failures.push(format!("row {row} no longer joins target row {joined}"));
        }
    }
    Ok(tally(evaluated, &failures, "joined rows"))
}

/// Equal source keys map to equal output keys and distinct ones to distinct
/// ones, across every listed column.
pub fn key_identity(ctx: &Ctx, columns: &[String]) -> Judged {
    let mut forward: BTreeMap<String, String> = BTreeMap::new();
    let mut backward: BTreeMap<String, String> = BTreeMap::new();
    let mut evaluated = 0;
    let mut failures = Vec::new();
    for column in columns {
        let source = ctx.source(column)?;
        let output = ctx.output(column)?;
        for (row, (src, out)) in source.values.iter().zip(&output.values).enumerate() {
            let (Some(src), Some(out)) = (key(src), key(out)) else {
                continue;
            };
            evaluated += 1;
            if forward.entry(src.clone()).or_insert_with(|| out.clone()) != &out {
                failures.push(format!("{column} row {row}: one source key, two outputs"));
            }
            if backward.entry(out).or_insert_with(|| src.clone()) != &src {
                failures.push(format!("{column} row {row}: two source keys, one output"));
            }
        }
    }
    Ok(tally(evaluated, &failures, "key cells"))
}

fn normalized(value: &Value, normalize: Normalize) -> Option<String> {
    let text = text_of(value)?;
    let normal = match normalize {
        Normalize::None => text,
        Normalize::Digits => digits_of(&text),
        Normalize::Casefold => text.trim().to_lowercase(),
    };
    (!normal.is_empty()).then_some(normal)
}

/// Rows that agree once normalised still agree in the output.
pub fn same_source_same_output(ctx: &Ctx, column: &str, normalize: Normalize) -> Judged {
    let source = ctx.source(column)?;
    let output = ctx.output(column)?;
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (row, value) in source.values.iter().enumerate() {
        if let Some(normal) = normalized(value, normalize) {
            groups.entry(normal).or_default().push(row);
        }
    }
    let mut evaluated = 0;
    let mut failures = Vec::new();
    for rows in groups.values().filter(|rows| rows.len() > 1) {
        evaluated += rows.len();
        let outputs: BTreeSet<Option<String>> = rows
            .iter()
            .map(|row| {
                output
                    .values
                    .get(*row)
                    .and_then(|v| normalized(v, normalize))
            })
            .collect();
        if outputs.len() > 1 {
            failures.push(format!("rows {rows:?} diverge"));
        }
    }
    Ok(tally(evaluated, &failures, "rows sharing a source value"))
}

pub fn unchanged(ctx: &Ctx, column: &str) -> Judged {
    let source = ctx.source(column)?;
    let output = ctx.output(column)?;
    let failures: Vec<String> = source
        .values
        .iter()
        .zip(&output.values)
        .enumerate()
        .filter(|(_, (src, out))| src != out)
        .map(|(row, _)| format!("row {row}"))
        .collect();
    Ok(tally(source.values.len(), &failures, "rows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphans_are_found_by_value_and_type() {
        let targets = [Value::Int(1), Value::Int(2)];
        let references = [
            Value::Int(1),
            Value::Null,
            Value::Int(3),
            Value::Text("2".into()),
            Value::Int(2),
        ];
        assert_eq!(orphan_positions(&references, &targets), vec![2, 3]);
    }
}
