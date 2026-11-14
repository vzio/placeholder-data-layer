//! The corpus judges the engine only from outside, and its scenario files
//! fail loudly rather than assert nothing.

use std::path::{Path, PathBuf};

use corpus::expect::Assertion;
use corpus::scenario::ScenarioFile;
use regex::Regex;

const INTERNAL_MODULES: &str =
    "classify|transform|anomaly|provenance|compliance|rng|context|pipeline|preserve";

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        panic!("cannot read {}", dir.display());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn corpus_uses_only_the_engines_public_api() {
    let direct = Regex::new(&format!(r"anonymizer::({INTERNAL_MODULES})\b")).expect("pattern");
    let grouped =
        Regex::new(&format!(r"anonymizer::\{{[^}}]*\b({INTERNAL_MODULES})\b")).expect("pattern");
    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(!files.is_empty());
    let mut offenders = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("readable source");
        if direct.is_match(&text) || grouped.is_match(&text) {
            offenders.push(file.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "these files reach engine internals, so their judgement could agree with the engine by construction: {offenders:?}"
    );
}

fn minimal_scenario(assertion: &str) -> String {
    format!(
        r#"{{"format": 1, "scenario": "x_y", "pack": "seed", "tier": "ci", "summary": "s",
            "dataset": {{"source": "file", "path": "dataset.json"}},
            "runs": [{{"id": "default", "seed": 1}}],
            "expectations": [{{"id": "a_b", "status": "met", "ruling": "obvious", "assert": {assertion}}}]}}"#
    )
}

#[test]
fn a_well_formed_scenario_parses() {
    let text = minimal_scenario(r#"{"kind": "verdict", "is": "pass"}"#);
    assert!(serde_json::from_str::<ScenarioFile>(&text).is_ok());
}

#[test]
fn an_unknown_field_is_rejected_at_every_level() {
    let misspelt_assertion = minimal_scenario(r#"{"kind": "verdict", "is": "pass", "iss": "x"}"#);
    assert!(serde_json::from_str::<ScenarioFile>(&misspelt_assertion).is_err());

    let misspelt_top = minimal_scenario(r#"{"kind": "verdict", "is": "pass"}"#).replacen(
        "\"summary\"",
        "\"sumary\": \"\", \"summary\"",
        1,
    );
    assert!(serde_json::from_str::<ScenarioFile>(&misspelt_top).is_err());

    let misspelt_fragment = r#"{"kind": "no_source_survival", "columns": ["t.c"], "scope": "row",
        "fragments": {"letter": 5}}"#;
    assert!(serde_json::from_str::<Assertion>(misspelt_fragment).is_err());

    let unknown_kind = r#"{"kind": "verdikt", "is": "pass"}"#;
    assert!(serde_json::from_str::<Assertion>(unknown_kind).is_err());
}

#[test]
fn every_scenario_directory_holds_a_readme_and_its_own_expected_json() {
    let names = corpus::load::scenario_names().expect("scenarios directory");
    assert!(!names.is_empty());
    for name in names {
        let dir = corpus::load::scenarios_dir().join(&name);
        assert!(dir.join("README.md").is_file(), "{name} has no README.md");
        let file = corpus::load::read_scenario(&name).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(file.scenario, name, "expected.json names another scenario");
    }
}
