use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;

use crate::Outcome;
use crate::anomaly::{self, AnomalyKind, AnomalyScan};
use crate::classify::{self, Classification, SemanticType};
use crate::compliance::Verdict;
use crate::context::RunContext;
use crate::dataset::{ColumnData, Dataset, Table, Value, schema};
use crate::error::Error;
use crate::policy::Policy;
use crate::preserve::{self, ResolvedPreserve};
use crate::profile::{self, ColumnProfile, Distribution};
use crate::report::{AnomalyRates, ColumnReport, FidelityReport};
use crate::transform::ledger::{ClassId, EquivalenceClasses, Ledger, LedgerBuilder};
use crate::transform::{self, Strategy, Strength};

/// Pipeline stages, in order. Also the key type of the report's timings map,
/// whose ordering follows this declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Validate,
    Classify,
    Profile,
    Anomaly,
    Transform,
    Compliance,
}

/// Validates the dataset and policy, then runs spec steps 2 to 6: classify,
/// profile, detect anomalies, transform, check, retry, report. Stages return
/// their timings rather than mutating shared state, which is what lets the
/// context be `&mut` while building and `&` while transforming.
// TODO(P5): run compliance checks and the retry loop.
pub fn run(mut dataset: Dataset, policy: Policy) -> Result<Outcome, Error> {
    let ((classes, mut context), validate_ms) = timed_stage(Stage::Validate, || {
        schema::validate(&mut dataset)?;
        policy.validate(&dataset)?;
        let classes = EquivalenceClasses::build(&dataset)?;
        Ok((classes, RunContext::new(policy)))
    })?;

    let flat = dataset.qualified_columns();

    let (classifications, classify_ms) = timed_stage(Stage::Classify, || {
        Ok::<_, Error>(
            flat.par_iter()
                .map(|(name, column)| {
                    (
                        name.clone(),
                        classify::classify(&column.spec.name, &column.values),
                    )
                })
                .collect::<BTreeMap<String, Classification>>(),
        )
    })?;

    let resolved_preserve = preserve::resolve(&context.policy.preserve, &dataset, &classes);
    for qualified in resolved_preserve.columns.keys() {
        let semantic_type = classification_of(&classifications, qualified).semantic_type;
        let refused = semantic_type.is_direct_identifier() || semantic_type == SemanticType::Uuid;
        if refused && !context.policy.force_preserve {
            return Err(Error::PreserveRefused(qualified.clone()));
        }
    }

    let (profiles, profile_ms) = timed_stage(Stage::Profile, || {
        Ok::<_, Error>(
            flat.par_iter()
                .map(|(name, column)| {
                    let classification = classification_of(&classifications, name);
                    (
                        name.clone(),
                        profile::profile_column(name, column, classification),
                    )
                })
                .collect::<BTreeMap<String, ColumnProfile>>(),
        )
    })?;

    let (anomaly_scans, anomaly_ms) = timed_stage(Stage::Anomaly, || {
        Ok::<_, Error>(
            flat.par_iter()
                .map(|(name, column)| {
                    let scan = match profiles.get(name) {
                        Some(profile) => anomaly::detect(column, profile),
                        None => AnomalyScan::default(),
                    };
                    (name.clone(), scan)
                })
                .collect::<BTreeMap<String, AnomalyScan>>(),
        )
    })?;

    let (transformed, transform_ms) = timed_stage(Stage::Transform, || {
        transform_dataset(
            &dataset,
            &classes,
            &classifications,
            &profiles,
            &anomaly_scans,
            &resolved_preserve,
            &mut context,
        )
    })?;

    let ((), compliance_ms) = timed_stage(Stage::Compliance, || Ok::<_, Error>(()))?;

    let columns = build_column_reports(
        &flat,
        &classifications,
        &profiles,
        &anomaly_scans,
        &transformed,
    );

    let mut timings_ms = BTreeMap::new();
    timings_ms.insert(Stage::Validate, validate_ms);
    timings_ms.insert(Stage::Classify, classify_ms);
    timings_ms.insert(Stage::Profile, profile_ms);
    timings_ms.insert(Stage::Anomaly, anomaly_ms);
    timings_ms.insert(Stage::Transform, transform_ms);
    timings_ms.insert(Stage::Compliance, compliance_ms);

    let report = FidelityReport {
        verdict: Verdict::Pass,
        columns,
        checks: Vec::new(),
        preserved_columns: resolved_preserve.columns,
        timings_ms,
    };

    Ok(Outcome {
        dataset: transformed.dataset,
        report,
    })
}

fn classification_of(
    classifications: &BTreeMap<String, Classification>,
    name: &str,
) -> Classification {
    classifications
        .get(name)
        .copied()
        .unwrap_or(Classification {
            semantic_type: SemanticType::Unknown,
            confidence: 0.0,
        })
}

/// The transformed dataset plus everything [`build_column_reports`] needs to
/// describe it honestly: the strategy each column actually ran under, and a
/// fresh profile and anomaly scan taken on the transformed values rather
/// than assumed from the plan that produced them.
struct TransformOutput {
    dataset: Dataset,
    strategies: BTreeMap<String, Strategy>,
    after_profiles: BTreeMap<String, ColumnProfile>,
    after_scans: BTreeMap<String, AnomalyScan>,
}

/// Runs spec step 4: mints the ledger single-threaded, then transforms every
/// column in parallel against the frozen ledger and re-injects anomalies at
/// their observed rate. Anomaly counts on the result are recomputed by a
/// fresh [`anomaly::detect`] pass rather than trusted from the injection
/// plan, since one kind's corruption can incidentally read as another.
fn transform_dataset(
    dataset: &Dataset,
    classes: &EquivalenceClasses,
    classifications: &BTreeMap<String, Classification>,
    profiles: &BTreeMap<String, ColumnProfile>,
    anomaly_scans: &BTreeMap<String, AnomalyScan>,
    preserved: &ResolvedPreserve,
    context: &mut RunContext,
) -> Result<TransformOutput, Error> {
    let flat = dataset.qualified_columns();
    let strategies = resolve_strategies(
        &flat,
        classifications,
        &context.policy.column_overrides,
        preserved,
    )?;
    validate_class_mint_consistency(classes, &strategies)?;

    let ledger = build_ledger(dataset, classes, &strategies, context)?;
    let context: &RunContext = context;

    let per_column: BTreeMap<String, Vec<Value>> = flat
        .par_iter()
        .map(|(qualified, column)| {
            let strategy = strategies[qualified];
            let class = classes.class_of(qualified);
            let mut values =
                transform_values(context, qualified, column, strategy, &ledger, class)?;

            // Neither `MintKey` nor `MintUuid` gets a second corrupt() pass:
            // both mint by redrawing character-by-character and keeping every
            // other character verbatim, so a padded or malformed source
            // emerges still padded or malformed, without needing one. `Keep`
            // passes the raw value through unchanged, so there is no
            // anonymized base value here to re-corrupt in the first place.
            if !matches!(
                strategy,
                Strategy::MintKey | Strategy::MintUuid | Strategy::Keep
            ) {
                if let Some(scan) = anomaly_scans.get(qualified) {
                    if !scan.positions.is_empty() {
                        let norm = profiles.get(qualified).and_then(|profile| profile.length);
                        let mut rng = context.rng_for(qualified);
                        let achieved = anomaly::reinject(&mut values, scan, norm, &mut rng);
                        tracing::debug!(column = %qualified, ?achieved, "anomalies reinjected");
                    }
                }
            }

            Ok::<_, Error>((qualified.clone(), values))
        })
        .collect::<Result<BTreeMap<_, _>, Error>>()?;

    let new_dataset = rebuild_dataset(dataset, &per_column);
    let new_flat = new_dataset.qualified_columns();

    let after_profiles: BTreeMap<String, ColumnProfile> = new_flat
        .par_iter()
        .map(|(qualified, column)| {
            let classification = classification_of(classifications, qualified);
            (
                qualified.clone(),
                profile::profile_column(qualified, column, classification),
            )
        })
        .collect();

    let after_scans: BTreeMap<String, AnomalyScan> = new_flat
        .par_iter()
        .map(|(qualified, column)| {
            let scan = after_profiles
                .get(qualified)
                .map(|profile| anomaly::detect(column, profile))
                .unwrap_or_default();
            (qualified.clone(), scan)
        })
        .collect();

    Ok(TransformOutput {
        dataset: new_dataset,
        strategies,
        after_profiles,
        after_scans,
    })
}

/// Builds and freezes the ledger single-threaded, in the deterministic order
/// [`LedgerBuilder::observe_dataset`] documents, before any column-parallel
/// work begins. `strategies` decides which columns observe into the
/// dataset-wide UUID domain rather than their class's, and which columns are
/// skipped entirely for being preserved: [`resolve_strategies`] must run
/// first so this already knows the final routing, including any override.
fn build_ledger(
    dataset: &Dataset,
    classes: &EquivalenceClasses,
    strategies: &BTreeMap<String, Strategy>,
    context: &mut RunContext,
) -> Result<Ledger, Error> {
    let uuid_columns: BTreeSet<String> = strategies
        .iter()
        .filter(|&(_, &strategy)| strategy == Strategy::MintUuid)
        .map(|(qualified, _)| qualified.clone())
        .collect();
    let preserved: BTreeSet<String> = strategies
        .iter()
        .filter(|&(_, &strategy)| strategy == Strategy::Keep)
        .map(|(qualified, _)| qualified.clone())
        .collect();

    let mut builder = LedgerBuilder::new(classes.clone());
    builder.observe_dataset(dataset, &uuid_columns, &preserved);
    builder.freeze(context)
}

/// Resolves the strategy every column runs under: [`Strategy::Keep`] for a
/// column [`preserve::resolve`] put in `preserved` — bypassing
/// [`transform::resolve`] entirely, since `Keep` is valid on a key column
/// without a ledger class — and [`transform::resolve`]'s usual override,
/// key, or semantic-type dispatch for everything else.
///
/// A column named in both `preserved` and `overrides` is rejected rather
/// than silently resolved to `Keep`: an explicit override is a request to
/// transform the column, which preservation would otherwise discard without
/// a trace.
fn resolve_strategies(
    flat: &[(String, &ColumnData)],
    classifications: &BTreeMap<String, Classification>,
    overrides: &BTreeMap<String, Strategy>,
    preserved: &ResolvedPreserve,
) -> Result<BTreeMap<String, Strategy>, Error> {
    flat.iter()
        .map(|(qualified, column)| {
            if preserved.contains(qualified) {
                if overrides.contains_key(qualified) {
                    return Err(Error::PreserveOverrideConflict(qualified.clone()));
                }
                return Ok((qualified.clone(), Strategy::Keep));
            }
            let semantic_type = classification_of(classifications, qualified).semantic_type;
            let strategy = transform::resolve(overrides, qualified, &column.spec, semantic_type)?;
            Ok((qualified.clone(), strategy))
        })
        .collect()
}

/// Rejects a referential equivalence class whose members do not all resolve
/// to the same mint space. A class is the unit referential integrity holds
/// over: if one member mints through [`Strategy::MintKey`]'s per-class
/// domain while another mints through [`Strategy::MintUuid`]'s dataset-wide
/// one — reachable only through a column override, since the default
/// resolution in [`transform::resolve`] already keys both off the same
/// semantic type — every row referencing the FK-divergent column dangles
/// silently once minting is done.
fn validate_class_mint_consistency(
    classes: &EquivalenceClasses,
    strategies: &BTreeMap<String, Strategy>,
) -> Result<(), Error> {
    let mut mint_space: BTreeMap<ClassId, Strategy> = BTreeMap::new();
    for (qualified, &class) in &classes.members {
        let Some(&strategy) = strategies.get(qualified) else {
            continue;
        };
        if !matches!(strategy, Strategy::MintKey | Strategy::MintUuid) {
            continue;
        }
        match mint_space.get(&class) {
            Some(&existing) if existing != strategy => {
                return Err(Error::InconsistentClassMint {
                    column: qualified.clone(),
                });
            }
            Some(_) => {}
            None => {
                mint_space.insert(class, strategy);
            }
        }
    }
    Ok(())
}

/// Transforms one column's values, row by row, keying each row's RNG on its
/// position (`qualified` and `row`) rather than on the value — a pseudonym
/// consistent across recurrences is [`Strategy::MintKey`]'s job via the
/// ledger, keyed on the value itself; every other strategy here is free to
/// vary row by row, and position-keying is what keeps that independent of
/// how rayon happens to split the column.
fn transform_values(
    context: &RunContext,
    qualified: &str,
    column: &ColumnData,
    strategy: Strategy,
    ledger: &Ledger,
    class: Option<ClassId>,
) -> Result<Vec<Value>, Error> {
    column
        .values
        .iter()
        .enumerate()
        .map(|(row, value)| {
            let mut rng = context.rng_for_row_position(qualified, row);
            strategy.apply(qualified, value, Strength::Default, ledger, class, &mut rng)
        })
        .collect()
}

fn rebuild_dataset(dataset: &Dataset, per_column: &BTreeMap<String, Vec<Value>>) -> Dataset {
    Dataset {
        tables: dataset
            .tables
            .iter()
            .map(|table| Table {
                name: table.name.clone(),
                columns: table
                    .columns
                    .iter()
                    .map(|column| {
                        let qualified = column.qualified_name(&table.name);
                        let values = per_column.get(&qualified).cloned().unwrap_or_default();
                        ColumnData {
                            spec: column.spec.clone(),
                            values,
                        }
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Builds each column's report entry honestly: `distribution_after` and the
/// anomaly `after` rates come from a fresh profile and scan of the
/// transformed dataset, never copied from `before` or from the injection
/// plan.
fn build_column_reports(
    flat: &[(String, &ColumnData)],
    classifications: &BTreeMap<String, Classification>,
    before_profiles: &BTreeMap<String, ColumnProfile>,
    before_scans: &BTreeMap<String, AnomalyScan>,
    transformed: &TransformOutput,
) -> Vec<ColumnReport> {
    flat.iter()
        .map(|(name, column)| {
            let classification = classification_of(classifications, name);

            let before_shares = before_profiles
                .get(name)
                .map_or_else(BTreeMap::new, |profile| profile.distribution.shares());
            let after_shares = transformed
                .after_profiles
                .get(name)
                .map_or_else(BTreeMap::new, |profile| profile.distribution.shares());

            // An `Unsummarised` side has no histogram to compare, not an empty
            // one: scoring it against a populated map on the other side would
            // report a swing from nothing tracked to something tracked as a
            // full distributional shift, a false signal on a column where
            // nothing went wrong.
            let is_unsummarised = |profile: Option<&ColumnProfile>| {
                matches!(
                    profile.map(|profile| &profile.distribution),
                    Some(Distribution::Unsummarised)
                )
            };
            let total_variation_distance = if is_unsummarised(before_profiles.get(name))
                || is_unsummarised(transformed.after_profiles.get(name))
            {
                0.0
            } else {
                profile::total_variation_distance(&before_shares, &after_shares)
            };

            let before_rates = before_scans
                .get(name)
                .map(|scan| anomaly::rates(scan, column.len()))
                .unwrap_or_default();
            let after_rates = transformed
                .after_scans
                .get(name)
                .map(|scan| anomaly::rates(scan, column.len()))
                .unwrap_or_default();
            let kinds: BTreeSet<AnomalyKind> = before_rates
                .keys()
                .chain(after_rates.keys())
                .copied()
                .collect();
            let anomalies = kinds
                .into_iter()
                .map(|kind| {
                    (
                        kind,
                        AnomalyRates {
                            before: before_rates.get(&kind).copied().unwrap_or(0.0),
                            after: after_rates.get(&kind).copied().unwrap_or(0.0),
                        },
                    )
                })
                .collect();

            let strategy = transformed
                .strategies
                .get(name)
                .copied()
                .unwrap_or(Strategy::Keep);

            ColumnReport {
                column: name.clone(),
                semantic_type: classification.semantic_type,
                confidence: f64::from(classification.confidence),
                strategy,
                distribution_before: before_shares,
                distribution_after: after_shares,
                total_variation_distance,
                anomalies,
            }
        })
        .collect()
}

/// Runs `f` inside a span named after `stage` and returns its result
/// alongside the wall-clock time it took, in milliseconds.
fn timed_stage<T>(stage: Stage, f: impl FnOnce() -> Result<T, Error>) -> Result<(T, u64), Error> {
    let span = tracing::info_span!("pipeline_stage", stage = ?stage);
    let _entered = span.enter();
    let started = Instant::now();
    let value = f()?;
    let elapsed = elapsed_ms(started);
    tracing::debug!(elapsed_ms = elapsed, "stage finished");
    Ok((value, elapsed))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
