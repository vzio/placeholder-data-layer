//! The target corpus: realistic fixtures and, for each, the behaviour the
//! engine is wanted to have on them once it is ready.
//!
//! An expectation is `met` (it holds today and a regression fails the suite)
//! or a `target` (wanted, not yet delivered; its test is ignored). The corpus
//! reaches the engine only through `anonymize`, `profile` and the wire
//! dataset type, and judges every result with its own code, so no
//! expectation can hold merely because the engine agrees with itself.
//! Never shipped: this crate is test and tooling only.

#![forbid(unsafe_code)]

pub mod expect;
pub mod facts;
pub mod generate;
pub mod load;
pub mod packs;
pub mod runs;
pub mod scenario;
pub mod shapes;

use expect::{Ctx, Finding};
use scenario::{DatasetSource, Status};

/// Why the corpus itself could not do what was asked: a missing or
/// malformed fixture, an unknown name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusError(pub String);

impl std::fmt::Display for CorpusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CorpusError {}

/// Every way `scenario`'s fixture fails to be what its `expected.json`
/// says: format problems, a digest or preview that no longer matches the
/// generator, a fact that no longer holds. Empty when well formed.
pub fn malformations(scenario: &str) -> Result<Vec<String>, CorpusError> {
    let directory = load::scenarios_dir().join(scenario);
    let mut problems = Vec::new();
    if !directory.join("README.md").is_file() {
        problems.push("README.md is missing".to_owned());
    }
    let loaded = load::load(scenario)?;
    problems.extend(loaded.file.problems(scenario));
    if let DatasetSource::Generator {
        digest,
        preview_rows,
        ..
    } = &loaded.file.dataset
    {
        let regenerated = load::digest(&loaded.wire)?;
        if &regenerated != digest {
            problems.push(format!(
                "digest is {regenerated}, expected.json says {digest}"
            ));
        }
        match load::read_preview(scenario) {
            Ok(committed) if committed == load::preview(&loaded.wire, *preview_rows) => {}
            Ok(_) => problems.push("preview.json no longer matches the generator".to_owned()),
            Err(e) => problems.push(e.0),
        }
    }
    for fact in &loaded.file.facts {
        match fact.observe(&loaded.source) {
            Ok(observed) if observed == fact.wanted() => {}
            Ok(observed) => problems.push(format!(
                "fact {}: observed {observed}, expected.json says {}",
                fact.describe(),
                fact.wanted()
            )),
            Err(e) => problems.push(format!("fact {}: {e}", fact.describe())),
        }
    }
    Ok(problems)
}

/// Evaluates one expectation against its (cached) run.
pub fn evaluate(scenario: &str, expectation: &str) -> Result<Finding, CorpusError> {
    let loaded = load::load(scenario)?;
    let wanted = loaded
        .file
        .expectation(expectation)
        .ok_or_else(|| CorpusError(format!("{scenario}: no expectation `{expectation}`")))?;
    let spec = loaded
        .file
        .run(&wanted.run)
        .ok_or_else(|| CorpusError(format!("{scenario}: no run `{}`", wanted.run)))?;
    let run = runs::run(scenario, &wanted.run)?;
    let ctx = Ctx {
        scenario,
        loaded: &loaded,
        spec,
        run: &run,
    };
    Ok(expect::evaluate(&wanted.assert, &ctx))
}

/// Targets of `scenario` that hold today and so must be promoted to `met`.
pub fn unpromoted_targets(scenario: &str) -> Result<Vec<String>, CorpusError> {
    let loaded = load::load(scenario)?;
    let mut holding = Vec::new();
    for expectation in &loaded.file.expectations {
        if expectation.status == Status::Target && evaluate(scenario, &expectation.id)?.holds() {
            holding.push(expectation.id.clone());
        }
    }
    Ok(holding)
}
