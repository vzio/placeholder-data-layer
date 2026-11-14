//! What the report and profile record about anomalies, non-production rows
//! and preserved columns, compared with what the fixture is known to hold.

use super::{After, AfterKeyword, Approx, Count, Ctx, Finding, Judged, PreserveWanted, Source};

/// Rates are per row of the table, nulls included. An anomaly kind the
/// report does not list has rate zero.
pub fn anomaly_rate(
    ctx: &Ctx,
    column: &str,
    anomaly: &str,
    before: Approx,
    after: After,
) -> Judged {
    let entry = &ctx.report_column(column)?["anomalies"][anomaly];
    let observed_before = entry["before"].as_f64().unwrap_or(0.0);
    let observed_after = entry["after"].as_f64().unwrap_or(0.0);
    let mut failures = Vec::new();
    if !before.admits(observed_before) {
        failures.push(format!(
            "rate before is {observed_before:.5}, wanted {} ± {}",
            before.value, before.tol
        ));
    }
    let after_ok = match after {
        After::Keyword(AfterKeyword::EqualsBefore) => {
            (observed_after - observed_before).abs() < 1e-12
        }
        After::Approx(wanted) => wanted.admits(observed_after),
    };
    if !after_ok {
        failures.push(format!(
            "rate after is {observed_after:.5} against {observed_before:.5} before"
        ));
    }
    if failures.is_empty() {
        Ok(Finding::Holds { evaluated: 2 })
    } else {
        Err(Finding::Fails {
            detail: format!("{column} {anomaly}: {}", failures.join("; ")),
        })
    }
}

pub fn non_production_rows(
    ctx: &Ctx,
    table: &str,
    flagged: Count,
    mixed: Count,
    source: Source,
) -> Judged {
    let document = match source {
        Source::Report => ctx.report()?,
        Source::Profile => ctx.profile()?,
    };
    let entry = &document["non_production_rows"][table];
    let observed_flagged = entry["flagged"].as_u64().unwrap_or(0) as usize;
    let observed_mixed = entry["mixed"].as_u64().unwrap_or(0) as usize;
    if flagged.admits(observed_flagged) && mixed.admits(observed_mixed) {
        Ok(Finding::Holds { evaluated: 2 })
    } else {
        Err(Finding::Fails {
            detail: format!(
                "{table}: {observed_flagged} flagged and {observed_mixed} mixed, wanted {flagged:?} and {mixed:?}"
            ),
        })
    }
}

pub fn preserved(ctx: &Ctx, column: &str, reason: PreserveWanted) -> Judged {
    let observed = ctx.report()?["preserved_columns"][column].as_str();
    let wanted = match reason {
        PreserveWanted::Requested => "explicit",
        PreserveWanted::Propagated => "class_propagation",
    };
    if observed == Some(wanted) {
        Ok(Finding::Holds { evaluated: 1 })
    } else {
        Err(Finding::Fails {
            detail: format!("`{column}` preserved as {observed:?}, wanted {wanted}"),
        })
    }
}
