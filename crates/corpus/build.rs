//! Writes one test per expectation of every scenario under
//! `fixtures/scenarios`, so each expectation passes, fails or is ignored on
//! its own line of `cargo test` output.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const RESERVED: &[&str] = &["fixture_is_well_formed", "no_target_passes_unpromoted"];

fn is_test_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && name.contains('_')
        && !name.ends_with('_')
        && !name.contains("__")
}

fn field<'a>(
    value: &'a serde_json::Value,
    key: &str,
    at: &Path,
) -> Result<&'a str, Box<dyn Error>> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("{}: `{key}` is missing or not a string", at.display()).into())
}

fn scenario_tests(dir: &Path, out: &mut String) -> Result<(), Box<dyn Error>> {
    let path = dir.join("expected.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path)?;
    let file: serde_json::Value = serde_json::from_str(&text)?;
    let scenario = field(&file, "scenario", &path)?;
    if !is_test_name(scenario) || RESERVED.contains(&scenario) {
        return Err(format!(
            "{}: scenario `{scenario}` is not a usable module name",
            path.display()
        )
        .into());
    }
    let ci = field(&file, "tier", &path)? == "ci";
    writeln!(out, "mod {scenario} {{\n    use super::*;\n")?;
    writeln!(
        out,
        "    #[test]\n    fn fixture_is_well_formed() {{\n        well_formed({scenario:?})\n    }}\n"
    )?;
    if !ci {
        writeln!(out, "}}\n")?;
        return Ok(());
    }
    writeln!(
        out,
        "    #[test]\n    fn no_target_passes_unpromoted() {{\n        no_unpromoted_target({scenario:?})\n    }}\n"
    )?;
    let expectations = file["expectations"]
        .as_array()
        .ok_or_else(|| format!("{}: `expectations` is not a list", path.display()))?;
    let mut seen = BTreeSet::new();
    for expectation in expectations {
        let id = field(expectation, "id", &path)?;
        if !is_test_name(id) || RESERVED.contains(&id) || !seen.insert(id) {
            return Err(format!(
                "{}: expectation id `{id}` must be unique snake_case with an underscore and not reserved",
                path.display()
            )
            .into());
        }
        writeln!(out, "    #[test]")?;
        if field(expectation, "status", &path)? != "met" {
            let gap = expectation["gap"]
                .as_str()
                .map_or_else(|| id.replace('_', " "), str::to_owned);
            let reason = format!("TODO: fine tune to catch this — {scenario}: {gap}");
            writeln!(out, "    #[ignore = {reason:?}]")?;
        }
        writeln!(
            out,
            "    fn {id}() {{\n        holds({scenario:?}, {id:?})\n    }}\n"
        )?;
    }
    writeln!(out, "}}\n")?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let root = manifest.join("../../fixtures/scenarios");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("expected.json").is_file())
        .collect();
    dirs.sort();
    let mut out = String::new();
    for dir in &dirs {
        scenario_tests(dir, &mut out)?;
    }
    let target = PathBuf::from(std::env::var("OUT_DIR")?).join("scenario_tests.rs");
    std::fs::write(target, out)?;
    Ok(())
}
