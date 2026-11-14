//! Whether the run was accepted, refused, at which strength, and whether it
//! reproduces under other thread counts.

use super::{Ctx, Finding, Judged};
use crate::runs::execute;

pub fn verdict(ctx: &Ctx, wanted: &str) -> Judged {
    let observed = &ctx.report()?["verdict"];
    if observed == wanted {
        Ok(Finding::Holds { evaluated: 1 })
    } else {
        Err(Finding::Fails {
            detail: format!("verdict is {observed}, wanted \"{wanted}\""),
        })
    }
}

pub fn refused(ctx: &Ctx, error: &str, column: Option<&str>) -> Judged {
    let Some(refusal) = &ctx.run.error else {
        return Err(Finding::Fails {
            detail: format!(
                "run `{}` was accepted, wanted a {error} refusal",
                ctx.spec.id
            ),
        });
    };
    if refusal.variant != error {
        return Err(Finding::Fails {
            detail: format!("refused with {}, wanted {error}", refusal.debug),
        });
    }
    if let Some(column) = column {
        if !refusal.debug.contains(&format!("{column:?}")) {
            return Err(Finding::Fails {
                detail: format!("refusal {} does not name `{column}`", refusal.debug),
            });
        }
    }
    Ok(Finding::Holds { evaluated: 1 })
}

const STRENGTHS: &[&str] = &["default", "aggressive", "maximum"];

pub fn strength_at_most(ctx: &Ctx, ceiling: &str) -> Judged {
    let rank = |name: &str| STRENGTHS.iter().position(|s| *s == name);
    let observed = ctx.report()?["accepted_strength"].as_str().unwrap_or("");
    let (Some(ceiling_rank), Some(observed_rank)) = (rank(ceiling), rank(observed)) else {
        return Err(Finding::Unevaluable {
            detail: format!("cannot order strength `{observed}` against `{ceiling}`"),
        });
    };
    if observed_rank <= ceiling_rank {
        Ok(Finding::Holds { evaluated: 1 })
    } else {
        Err(Finding::Fails {
            detail: format!("accepted at `{observed}`, wanted at most `{ceiling}`"),
        })
    }
}

/// Reruns in explicit pools of each thread count, one after another, and
/// compares every result byte for byte with the cached run.
pub fn deterministic(ctx: &Ctx, threads: &[usize]) -> Judged {
    let mut differing = Vec::new();
    for &count in threads {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(count)
            .build()
            .map_err(|e| Finding::Unevaluable {
                detail: format!("cannot build a {count}-thread pool: {e}"),
            })?;
        let rerun = pool
            .install(|| execute(&ctx.loaded.source, ctx.spec))
            .map_err(|e| Finding::Unevaluable { detail: e.0 })?;
        if rerun.fingerprint != ctx.run.fingerprint {
            differing.push(format!("{count} threads"));
        }
    }
    if threads.is_empty() {
        return Err(Finding::Unevaluable {
            detail: "no thread counts listed".to_owned(),
        });
    }
    if differing.is_empty() {
        Ok(Finding::Holds {
            evaluated: threads.len(),
        })
    } else {
        Err(Finding::Fails {
            detail: format!(
                "output differs from the cached run under {}",
                differing.join(", ")
            ),
        })
    }
}
