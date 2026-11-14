//! Whether each row keeps its own format, document validity and nullness.

use super::{Ctx, Finding, Judged, tally, text_of};
use crate::generate::docs::Document;
use crate::shapes::ShapeRef;

/// A row whose source is one of `shapes` comes back in that same shape.
pub fn format_preserved(ctx: &Ctx, column: &str, shapes: &[ShapeRef]) -> Judged {
    let source = ctx.source(column)?;
    let output = ctx.output(column)?;
    let compiled = shapes
        .iter()
        .map(|s| s.compile().map(|c| (s.label(), c)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Finding::Unevaluable { detail: e.0 })?;
    let mut evaluated = 0;
    let mut failures = Vec::new();
    for (row, (src, out)) in source.values.iter().zip(&output.values).enumerate() {
        let Some(src) = text_of(src) else { continue };
        let Some((label, shape)) = compiled.iter().find(|(_, shape)| shape.is(&src)) else {
            continue;
        };
        evaluated += 1;
        if !text_of(out).is_some_and(|out| shape.is(&out)) {
            failures.push(format!("row {row} was {label}"));
        }
    }
    Ok(tally(evaluated, &failures, "rows in a listed shape"))
}

/// A valid document stays valid and an invalid one stays invalid.
pub fn validity_preserved(ctx: &Ctx, column: &str, document: Document) -> Judged {
    let source = ctx.source(column)?;
    let output = ctx.output(column)?;
    let mut evaluated = 0;
    let mut failures = Vec::new();
    for (row, (src, out)) in source.values.iter().zip(&output.values).enumerate() {
        let Some(src) = text_of(src) else { continue };
        evaluated += 1;
        let was_valid = document.is_valid(&src);
        let is_valid = text_of(out).is_some_and(|out| document.is_valid(&out));
        if was_valid != is_valid {
            let change = if was_valid { "lost" } else { "gained" };
            failures.push(format!("row {row} {change} validity"));
        }
    }
    Ok(tally(evaluated, &failures, "non-null rows"))
}

pub fn nulls_preserved(ctx: &Ctx, column: &str) -> Judged {
    let source = ctx.source(column)?;
    let output = ctx.output(column)?;
    let failures: Vec<String> = source
        .values
        .iter()
        .zip(&output.values)
        .enumerate()
        .filter(|(_, (src, out))| src.is_null() != out.is_null())
        .map(|(row, _)| format!("row {row}"))
        .collect();
    Ok(tally(source.values.len(), &failures, "rows"))
}
