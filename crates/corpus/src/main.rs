//! `corpus` — the target corpus's command line.
//!
//! ```text
//! corpus scoreboard [--all] [--verbose]   met and unmet expectations per scenario
//! corpus dump <scenario>                  the scenario's full wire dataset
//! corpus preview <scenario>               the preview.json a generated scenario commits
//! corpus digest <scenario>                the digest a generated scenario commits
//! corpus run <scenario> [--run <id>] [--dataset]
//!                                         the report (or refusal), or the output, of one run
//! ```
//!
//! `scoreboard` exits non-zero when a `met` expectation regressed, a target
//! holds without having been promoted, or a fixture is malformed.

use std::process::ExitCode;

use corpus::expect::Finding;
use corpus::scenario::{DatasetSource, Ruling, Status, Tier};
use corpus::{CorpusError, evaluate, load, malformations, runs};
use server::http::dto::DatasetDto;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let positional = args.get(1).map(String::as_str);
    let result =
        match (args.first().map(String::as_str), positional) {
            (Some("scoreboard"), _) => scoreboard(flag("--all"), flag("--verbose")),
            (Some("dump"), Some(name)) => dump(name, None),
            (Some("preview"), Some(name)) => preview(name),
            (Some("digest"), Some(name)) => load::load(name)
                .and_then(|l| load::digest(&l.wire))
                .map(|d| {
                    println!("{d}");
                    true
                }),
            (Some("run"), Some(name)) => {
                let run = args
                    .iter()
                    .position(|a| a == "--run")
                    .and_then(|i| args.get(i + 1))
                    .map_or("default", String::as_str);
                show_run(name, run, flag("--dataset"))
            }
            _ => {
                eprintln!(
                    "usage: corpus scoreboard [--all] [--verbose] | dump <scenario> | \
                 preview <scenario> | digest <scenario> | run <scenario> [--run <id>] [--dataset]"
                );
                return ExitCode::from(2);
            }
        };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn pretty<T: serde::Serialize>(value: &T) -> Result<String, CorpusError> {
    serde_json::to_string_pretty(value).map_err(|e| CorpusError(e.to_string()))
}

fn dump(name: &str, rows: Option<usize>) -> Result<bool, CorpusError> {
    let loaded = load::load(name)?;
    let dataset = match rows {
        Some(rows) => load::preview(&loaded.wire, rows),
        None => loaded.wire.clone(),
    };
    println!("{}", pretty(&dataset)?);
    Ok(true)
}

fn preview(name: &str) -> Result<bool, CorpusError> {
    let loaded = load::load(name)?;
    match &loaded.file.dataset {
        DatasetSource::Generator { preview_rows, .. } => dump(name, Some(*preview_rows)),
        DatasetSource::File { .. } => Err(CorpusError(format!(
            "{name} commits its dataset; only a generated scenario has a preview"
        ))),
    }
}

fn show_run(name: &str, run: &str, dataset: bool) -> Result<bool, CorpusError> {
    let result = runs::run(name, run)?;
    if let (true, Some(output)) = (dataset, &result.output) {
        let wire = DatasetDto::from_dataset(output).map_err(|e| CorpusError(e.to_string()))?;
        println!("{}", pretty(&wire)?);
        return Ok(true);
    }
    match (&result.report, &result.profile, &result.error) {
        (Some(report), _, _) => println!("{}", pretty(report)?),
        (None, Some(profile), _) => println!("{}", pretty(profile)?),
        (_, _, Some(error)) => println!("refused: {}", error.debug),
        _ => println!("the run produced nothing"),
    }
    Ok(true)
}

#[derive(Default)]
struct Tally {
    met: usize,
    total: usize,
    unmet: usize,
    promotable: usize,
    regressed: usize,
    awaiting: usize,
}

fn scoreboard(all: bool, verbose: bool) -> Result<bool, CorpusError> {
    let mut healthy = true;
    let mut overall = Tally::default();
    for name in load::scenario_names()? {
        let problems = malformations(&name)?;
        let loaded = load::load(&name)?;
        if loaded.file.tier == Tier::Extended && !all {
            continue;
        }
        let mut tally = Tally::default();
        let mut notes = Vec::new();
        for expectation in &loaded.file.expectations {
            let finding = evaluate(&name, &expectation.id)?;
            tally.total += 1;
            if expectation.ruling == Ruling::NeedsOwner {
                tally.awaiting += 1;
            }
            let label = match (expectation.status, finding.holds()) {
                (Status::Met, true) => {
                    tally.met += 1;
                    continue;
                }
                (Status::Met, false) => {
                    tally.regressed += 1;
                    "REGRESSED"
                }
                (Status::Target, true) => {
                    tally.promotable += 1;
                    "PROMOTABLE"
                }
                (Status::Target, false) => {
                    tally.unmet += 1;
                    "target"
                }
            };
            let ruling = match expectation.ruling {
                Ruling::Obvious => "",
                Ruling::NeedsOwner => " [needs owner]",
                Ruling::OwnerRuled => " [owner ruled]",
            };
            let why = expectation
                .rationale
                .as_deref()
                .map(|r| format!("\n        why: {r}"))
                .unwrap_or_default();
            let found = match &finding {
                Finding::Holds { evaluated } => format!("holds over {evaluated}"),
                other => other.describe(),
            };
            notes.push(format!(
                "    {label} {}{ruling}: {found}{why}",
                expectation.id
            ));
        }
        let mut line = format!(
            "{name:<28}{}/{} expectations met  ({} target unmet, {} promotable, {} awaiting owner ruling)",
            tally.met, tally.total, tally.unmet, tally.promotable, tally.awaiting
        );
        if tally.regressed > 0 {
            line.push_str(&format!(" {} REGRESSED", tally.regressed));
        }
        println!("{line}");
        for problem in &problems {
            println!("    MALFORMED: {problem}");
        }
        if verbose || tally.regressed + tally.promotable > 0 {
            for note in notes
                .iter()
                .filter(|n| verbose || !n.trim_start().starts_with("target"))
            {
                println!("{note}");
            }
        }
        healthy &= problems.is_empty() && tally.regressed == 0 && tally.promotable == 0;
        overall.met += tally.met;
        overall.total += tally.total;
    }
    println!("{:<28}{}/{}", "TOTAL", overall.met, overall.total);
    Ok(healthy)
}
