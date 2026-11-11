use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;

use crate::Outcome;
use crate::anomaly::{self, AnomalyKind, AnomalyScan};
use crate::classify::{self, Classification, SemanticType};
use crate::compliance::{self, CheckOutcome, Verdict};
use crate::context::RunContext;
use crate::dataset::{ColumnData, Dataset, Table, Value, schema};
use crate::error::Error;
use crate::policy::Policy;
use crate::preserve::{self, PreserveReason, ResolvedPreserve};
use crate::profile::{self, ColumnProfile, Distribution};
use crate::provenance;
use crate::report::{AnomalyRates, ColumnReport, FidelityReport};
use crate::transform::ledger::{ClassId, EquivalenceClasses, KeyDocument, Ledger, LedgerBuilder};
use crate::transform::{self, ColumnRole, Strategy, Strength};

/// Pipeline stages, in order. Also the key type of the report's timings map,
/// whose ordering follows this declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
                        classify::classify(&column.spec, &column.values),
                    )
                })
                .collect::<BTreeMap<String, Classification>>(),
        )
    })?;

    let resolved_preserve = preserve::resolve(&context.policy.preserve, &dataset, &classes);
    if !context.policy.force_preserve {
        if let Some(refused) = preserve_refusal(&resolved_preserve, &classifications) {
            return Err(Error::PreserveRefused(refused.to_owned()));
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

    // Timed under `Stage::Anomaly`: a stage of its own would change the
    // report's `timings_ms` keys. The result reaches the report only, never
    // the retry plan, so it cannot change what is emitted.
    let (non_production_rows, provenance_ms) = timed_stage(Stage::Anomaly, || {
        let semantic_types: BTreeMap<String, SemanticType> = classifications
            .iter()
            .map(|(name, classification)| (name.clone(), classification.semantic_type))
            .collect();
        Ok::<_, Error>(provenance::scan(&dataset, &semantic_types))
    })?;

    let (strategies, strategy_ms) = timed_stage(Stage::Transform, || {
        let strategies = resolve_strategies(
            &flat,
            &classifications,
            &context.policy.column_overrides,
            &resolved_preserve,
        )?;
        validate_class_mint_consistency(&classes, &strategies)?;
        Ok(strategies)
    })?;

    let (ledger, ledger_ms) = timed_stage(Stage::Transform, || {
        build_ledger(
            &dataset,
            &classes,
            &classifications,
            &strategies,
            &mut context,
        )
    })?;

    let context: &RunContext = &context;

    let plan = RetryPlan {
        classes: &classes,
        classifications: &classifications,
        profiles: &profiles,
        anomaly_scans: &anomaly_scans,
        strategies: &strategies,
        ledger: &ledger,
        preserved: &resolved_preserve,
    };
    let retry = retry_until_enforced(&dataset, &plan, context)?;

    let (linkage, linkage_ms) = timed_stage(Stage::Compliance, || {
        Ok::<_, Error>(compliance::observe_linkage(
            &retry.output.dataset,
            &retry.output.after_profiles,
            &resolved_preserve,
        ))
    })?;

    let mut checks = retry.outcomes;
    checks.push(linkage);
    // The top-level verdict reflects the enforced checks only: `Linkage` is
    // reported, never enforced, so a `warn` there must never pull a
    // consumer gating on `verdict == "pass"` into treating a clean run as a
    // failure.
    let verdict = checks
        .iter()
        .filter(|outcome| {
            matches!(
                outcome.check,
                compliance::Check::Leakage | compliance::Check::Referential
            )
        })
        .map(|outcome| outcome.verdict)
        .max()
        .unwrap_or(Verdict::Pass);

    let columns = build_column_reports(
        &flat,
        &classifications,
        &profiles,
        &anomaly_scans,
        &retry.output,
    )?;

    let mut timings_ms = BTreeMap::new();
    timings_ms.insert(Stage::Validate, validate_ms);
    timings_ms.insert(Stage::Classify, classify_ms);
    timings_ms.insert(Stage::Profile, profile_ms);
    timings_ms.insert(Stage::Anomaly, anomaly_ms + provenance_ms);
    timings_ms.insert(
        Stage::Transform,
        strategy_ms + ledger_ms + retry.transform_ms,
    );
    timings_ms.insert(Stage::Compliance, retry.compliance_ms + linkage_ms);

    let report = FidelityReport {
        verdict,
        columns,
        checks,
        preserved_columns: resolved_preserve.columns,
        timings_ms,
        accepted_strength: retry.accepted_strength,
        attempts: retry.attempts,
        non_production_rows,
    };

    Ok(Outcome {
        dataset: retry.output.dataset,
        report,
    })
}

/// Everything about how to transform and check `dataset` that is fixed
/// before the retry loop begins: resolved once because strategy resolution
/// and key minting are both strength-independent, so rebuilding either per
/// attempt would waste the work and risk a pseudonym drifting across
/// attempts.
struct RetryPlan<'a> {
    classes: &'a EquivalenceClasses,
    classifications: &'a BTreeMap<String, Classification>,
    profiles: &'a BTreeMap<String, ColumnProfile>,
    anomaly_scans: &'a BTreeMap<String, AnomalyScan>,
    strategies: &'a BTreeMap<String, Strategy>,
    ledger: &'a Ledger,
    preserved: &'a ResolvedPreserve,
}

/// One escalation attempt's result, plus how long the transform and the
/// enforced checks took.
struct Attempt {
    output: TransformOutput,
    outcomes: Vec<CheckOutcome>,
    transform_ms: u64,
    compliance_ms: u64,
}

/// Transforms `dataset` once at `strength` against the already-frozen
/// ledger, then runs the enforced compliance checks against the result.
fn attempt(
    dataset: &Dataset,
    plan: &RetryPlan,
    strength: Strength,
    context: &RunContext,
) -> Result<Attempt, Error> {
    let (output, transform_ms) = timed_stage(Stage::Transform, || {
        transform_dataset(dataset, plan, strength, context)
    })?;
    let (outcomes, compliance_ms) = timed_stage(Stage::Compliance, || {
        Ok::<_, Error>(compliance::enforced(
            dataset,
            &output.dataset,
            plan.profiles,
            plan.strategies,
            plan.preserved,
        ))
    })?;
    Ok(Attempt {
        output,
        outcomes,
        transform_ms,
        compliance_ms,
    })
}

fn enforced_passed(outcomes: &[CheckOutcome]) -> bool {
    outcomes
        .iter()
        .all(|outcome| outcome.verdict == Verdict::Pass)
}

/// The retry loop's result: the accepted attempt's output and enforced
/// outcomes, the [`Strength`] that attempt ran at, how many attempts were
/// made in total, plus the summed transform and compliance time across
/// every attempt made.
struct RetryOutcome {
    output: TransformOutput,
    outcomes: Vec<CheckOutcome>,
    accepted_strength: Strength,
    attempts: usize,
    transform_ms: u64,
    compliance_ms: u64,
}

/// Runs [`attempt`] at [`Strength::Default`], then escalates through
/// [`Strength::next`] until either every enforced check passes or the ladder
/// is exhausted. Termination is proven by the type: `next` eventually
/// returns `None`, so no attempt counter is needed.
///
/// The first attempt whose enforced outcomes all [`Verdict::Pass`] is
/// accepted immediately. Otherwise the best-scoring attempt so far is kept,
/// strictly — a tie keeps the earlier, weaker attempt, since it preserves
/// more of the distribution. If no attempt ever passes, the best-scoring
/// attempt is returned anyway, carrying its own (failing) outcomes as the
/// honest reason.
fn retry_until_enforced(
    dataset: &Dataset,
    plan: &RetryPlan,
    context: &RunContext,
) -> Result<RetryOutcome, Error> {
    let mut strength = Strength::Default;
    let first = attempt(dataset, plan, strength, context)?;

    let mut attempts = 1usize;
    let mut transform_ms = first.transform_ms;
    let mut compliance_ms = first.compliance_ms;
    let mut best_score = compliance::score(&first.outcomes);
    let mut best_output = first.output;
    let mut best_outcomes = first.outcomes;
    let mut best_strength = strength;

    while !enforced_passed(&best_outcomes) {
        let Some(next_strength) = strength.next() else {
            break;
        };
        strength = next_strength;

        let next = attempt(dataset, plan, strength, context)?;
        attempts += 1;
        transform_ms += next.transform_ms;
        compliance_ms += next.compliance_ms;

        if enforced_passed(&next.outcomes) {
            best_output = next.output;
            best_outcomes = next.outcomes;
            best_strength = strength;
            break;
        }

        let score = compliance::score(&next.outcomes);
        if score < best_score {
            best_output = next.output;
            best_outcomes = next.outcomes;
            best_score = score;
            best_strength = strength;
        }
    }

    Ok(RetryOutcome {
        output: best_output,
        outcomes: best_outcomes,
        accepted_strength: best_strength,
        attempts,
        transform_ms,
        compliance_ms,
    })
}

/// The column a preserve refusal names, if any preserved column is a direct identifier or a
/// UUID: the first such column the policy listed, in sorted order, so the error points at
/// something the caller wrote; failing that, the first such column class propagation pulled
/// in, since naming a listed column would misreport its classification.
fn preserve_refusal<'a>(
    resolved: &'a ResolvedPreserve,
    classifications: &BTreeMap<String, Classification>,
) -> Option<&'a str> {
    let first_refusable = |reason: PreserveReason| {
        resolved
            .columns
            .iter()
            .filter(|(_, column_reason)| **column_reason == reason)
            .map(|(qualified, _)| qualified.as_str())
            .find(|qualified| {
                let semantic_type = classification_of(classifications, qualified).semantic_type;
                semantic_type.is_direct_identifier() || semantic_type == SemanticType::Uuid
            })
    };
    first_refusable(PreserveReason::Explicit)
        .or_else(|| first_refusable(PreserveReason::ClassPropagation))
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

/// Runs spec step 4 at `strength`: transforms every column in parallel
/// against `plan`'s already-frozen ledger and re-injects anomalies at their
/// observed rate. Anomaly counts on the result are recomputed by a fresh
/// [`anomaly::detect`] pass rather than trusted from the injection plan,
/// since one kind's corruption can incidentally read as another.
fn transform_dataset(
    dataset: &Dataset,
    plan: &RetryPlan,
    strength: Strength,
    context: &RunContext,
) -> Result<TransformOutput, Error> {
    let flat = dataset.qualified_columns();

    let per_column: BTreeMap<String, Vec<Value>> = flat
        .par_iter()
        .map(|(qualified, column)| {
            let strategy = plan.strategies[qualified];
            let class = plan.classes.class_of(qualified);
            let mut values = transform_values(
                context,
                qualified,
                column,
                strategy,
                strength,
                ColumnRole {
                    semantic_type: classification_of(plan.classifications, qualified).semantic_type,
                    class,
                },
                plan.ledger,
            )?;

            // Neither `MintKey` nor `MintUuid` gets a second corrupt() pass:
            // re-corrupting a minted key would change the value its foreign
            // keys join on. Instead the ledger mints each key in its source's
            // shape — sign and width, punctuation and padding, and in a CPF or
            // CNPJ class the validity of its check digits — so a source
            // defect survives at the same rows. `Keep` passes the raw value
            // through unchanged, so there is no anonymized base value here to
            // re-corrupt in the first place.
            if !matches!(
                strategy,
                Strategy::MintKey | Strategy::MintUuid | Strategy::Keep
            ) {
                if let Some(scan) = plan.anomaly_scans.get(qualified) {
                    if !scan.positions.is_empty() {
                        let norm = plan
                            .profiles
                            .get(qualified)
                            .and_then(|profile| profile.length);
                        let source_comparison = plan.profiles.get(qualified).and_then(|profile| {
                            compliance::SourceComparison::for_column(
                                profile,
                                column,
                                Some(strategy),
                            )
                        });
                        let mut rng = context.rng_for(qualified);
                        let achieved = anomaly::reinject(
                            &mut values,
                            scan,
                            norm,
                            &mut rng,
                            |row, candidate| {
                                source_comparison
                                    .as_ref()
                                    .is_some_and(|comparison| comparison.leaks(row, candidate))
                            },
                        );
                        tracing::debug!(column = %qualified, ?achieved, "anomalies reinjected");
                    }
                }
            }

            Ok::<_, Error>((qualified.clone(), values))
        })
        .collect::<Result<BTreeMap<_, _>, Error>>()?;

    let new_dataset = rebuild_dataset(dataset, &per_column)?;
    let new_flat = new_dataset.qualified_columns();

    let after_profiles: BTreeMap<String, ColumnProfile> = new_flat
        .par_iter()
        .map(|(qualified, column)| {
            let classification = classification_of(plan.classifications, qualified);
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
        strategies: plan.strategies.clone(),
        after_profiles,
        after_scans,
    })
}

/// Builds and freezes the ledger single-threaded, in the deterministic order
/// [`LedgerBuilder::observe_dataset`] documents, before any column-parallel
/// work begins. `strategies` decides which columns observe into the
/// dataset-wide UUID domain rather than their class's, which observe into
/// the dataset-wide handle domain, and which columns are
/// skipped entirely for being preserved: [`resolve_strategies`] must run
/// first so this already knows the final routing, including any override.
/// `classifications` decides which classes mint their keys as documents
/// ([`class_documents`]).
fn build_ledger(
    dataset: &Dataset,
    classes: &EquivalenceClasses,
    classifications: &BTreeMap<String, Classification>,
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
    let handle_columns: BTreeSet<String> = strategies
        .iter()
        .filter(|&(_, &strategy)| strategy == Strategy::MintHandle)
        .map(|(qualified, _)| qualified.clone())
        .collect();

    let mut builder = LedgerBuilder::new(classes.clone());
    builder.observe_dataset(dataset, &uuid_columns, &preserved);
    builder.observe_handle_columns(dataset, &handle_columns);
    builder.documents = class_documents(classes, classifications, strategies);
    builder.freeze(context)
}

/// The document each class's keys are: a class gets one when every member
/// resolved to [`Strategy::MintKey`] and classified a CPF or a CNPJ agrees on
/// which. Members classified anything else (a `NumericId` foreign key, say)
/// share the class's values and so do not veto it; a class whose members
/// disagree between CPF and CNPJ gets none and mints without regard to check
/// digits.
fn class_documents(
    classes: &EquivalenceClasses,
    classifications: &BTreeMap<String, Classification>,
    strategies: &BTreeMap<String, Strategy>,
) -> BTreeMap<ClassId, KeyDocument> {
    let mut agreed: BTreeMap<ClassId, Option<KeyDocument>> = BTreeMap::new();
    for (qualified, &class) in &classes.members {
        if strategies.get(qualified) != Some(&Strategy::MintKey) {
            continue;
        }
        let semantic_type = classification_of(classifications, qualified).semantic_type;
        let Some(document) = KeyDocument::for_semantic_type(semantic_type) else {
            continue;
        };
        agreed
            .entry(class)
            .and_modify(|agreed_document| {
                if *agreed_document != Some(document) {
                    *agreed_document = None;
                }
            })
            .or_insert(Some(document));
    }
    agreed
        .into_iter()
        .filter_map(|(class, document)| document.map(|document| (class, document)))
        .collect()
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
/// position (`qualified` and `row`) and the attempt's `strength` rather than
/// on the value — a pseudonym consistent across recurrences is
/// [`Strategy::MintKey`]'s job via the ledger, keyed on the value itself;
/// every other strategy here is free to vary row by row, and position-keying
/// is what keeps that independent of how rayon happens to split the column.
/// Folding `strength` in means an escalated retry genuinely redraws rather
/// than replaying the first attempt's stream and reproducing its collision.
fn transform_values(
    context: &RunContext,
    qualified: &str,
    column: &ColumnData,
    strategy: Strategy,
    strength: Strength,
    role: ColumnRole,
    ledger: &Ledger,
) -> Result<Vec<Value>, Error> {
    column
        .values
        .iter()
        .enumerate()
        .map(|(row, value)| {
            let mut rng = context.rng_for_row_position(qualified, row, strength);
            strategy.apply(qualified, value, strength, role, ledger, &mut rng)
        })
        .collect()
}

/// Rebuilds `dataset` from the per-column transformed values. Every column
/// `dataset` declares must have an entry in `per_column` — the transform
/// stage runs over the same flattened column list this walks — so a column
/// absent from `per_column` is a bug in that stage, not a column with
/// nothing to say: silently substituting an empty `Vec` would violate the
/// row-count contract every other column in the table upholds.
fn rebuild_dataset(
    dataset: &Dataset,
    per_column: &BTreeMap<String, Vec<Value>>,
) -> Result<Dataset, Error> {
    let tables = dataset
        .tables
        .iter()
        .map(|table| {
            let columns = table
                .columns
                .iter()
                .map(|column| {
                    let qualified = column.qualified_name(&table.name);
                    let values = per_column
                        .get(&qualified)
                        .cloned()
                        .ok_or_else(|| Error::MissingTransformedColumn(qualified.clone()))?;
                    Ok(ColumnData {
                        spec: column.spec.clone(),
                        values,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            Ok(Table {
                name: table.name.clone(),
                columns,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(Dataset { tables })
}

/// Builds each column's report entry honestly: `distribution_after` and the
/// anomaly `after` rates come from a fresh profile and scan of the
/// transformed dataset, never copied from `before` or from the injection
/// plan. A column absent from `transformed`'s resolved strategies errors
/// rather than defaulting to [`Strategy::Keep`] — that default would claim
/// the column was never anonymized, which is the one direction a report is
/// not allowed to be wrong in.
fn build_column_reports(
    flat: &[(String, &ColumnData)],
    classifications: &BTreeMap<String, Classification>,
    before_profiles: &BTreeMap<String, ColumnProfile>,
    before_scans: &BTreeMap<String, AnomalyScan>,
    transformed: &TransformOutput,
) -> Result<Vec<ColumnReport>, Error> {
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
                .ok_or_else(|| Error::MissingColumnStrategy(name.clone()))?;

            Ok(ColumnReport {
                column: name.clone(),
                semantic_type: classification.semantic_type,
                confidence: f64::from(classification.confidence),
                strategy,
                distribution_before: before_shares,
                distribution_after: after_shares,
                total_variation_distance,
                anomalies,
            })
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
