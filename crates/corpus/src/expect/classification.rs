//! What the engine decided each column is, and how it transformed it,
//! compared by name so a target may name a type the engine lacks.

use super::{Ctx, Finding, Judged, TypeSource};

fn profiled_type<'a>(ctx: &'a Ctx, column: &str) -> Result<&'a str, Finding> {
    ctx.profile()?["columns"][column]["semantic_type"]
        .as_str()
        .ok_or_else(|| Finding::Fails {
            detail: format!("the profile has no entry for `{column}`"),
        })
}

fn reported<'a>(ctx: &'a Ctx, column: &str, field: &str) -> Result<&'a str, Finding> {
    ctx.report_column(column)?[field]
        .as_str()
        .ok_or_else(|| Finding::Fails {
            detail: format!("the report entry for `{column}` has no `{field}`"),
        })
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

pub fn semantic_type(ctx: &Ctx, column: &str, wanted: &str, source: TypeSource) -> Judged {
    let mut observed = Vec::new();
    if source != TypeSource::Profile {
        observed.push(("report", reported(ctx, column, "semantic_type")?));
    }
    if source != TypeSource::Report {
        observed.push(("profile", profiled_type(ctx, column)?));
    }
    let wrong: Vec<String> = observed
        .iter()
        .filter(|(_, ty)| !same(ty, wanted))
        .map(|(origin, ty)| format!("{origin} says {ty}"))
        .collect();
    if wrong.is_empty() {
        Ok(Finding::Holds {
            evaluated: observed.len(),
        })
    } else {
        Err(Finding::Fails {
            detail: format!("`{column}` wanted {wanted}: {}", wrong.join(", ")),
        })
    }
}

pub fn one_of(ctx: &Ctx, column: &str, any_of: &[String]) -> Judged {
    let observed = reported(ctx, column, "semantic_type")?;
    if any_of.iter().any(|ty| same(ty, observed)) {
        Ok(Finding::Holds { evaluated: 1 })
    } else {
        Err(Finding::Fails {
            detail: format!("`{column}` is {observed}, wanted one of {any_of:?}"),
        })
    }
}

pub fn not_type(ctx: &Ctx, column: &str, is_not: &str) -> Judged {
    let observed = reported(ctx, column, "semantic_type")?;
    if same(observed, is_not) {
        Err(Finding::Fails {
            detail: format!("`{column}` is {observed}"),
        })
    } else {
        Ok(Finding::Holds { evaluated: 1 })
    }
}

pub fn strategy(ctx: &Ctx, column: &str, wanted: &str) -> Judged {
    let observed = reported(ctx, column, "strategy")?;
    if same(observed, wanted) {
        Ok(Finding::Holds { evaluated: 1 })
    } else {
        Err(Finding::Fails {
            detail: format!("`{column}` runs {observed}, wanted {wanted}"),
        })
    }
}

pub fn profile_agrees(ctx: &Ctx) -> Judged {
    let columns = ctx.report()?["columns"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut disagreements = Vec::new();
    for entry in &columns {
        let name = entry["column"].as_str().unwrap_or("");
        let reported = entry["semantic_type"].as_str().unwrap_or("");
        let profiled = profiled_type(ctx, name).unwrap_or("absent");
        if reported != profiled {
            disagreements.push(format!("{name}: report {reported}, profile {profiled}"));
        }
    }
    Ok(super::tally(columns.len(), &disagreements, "columns"))
}
