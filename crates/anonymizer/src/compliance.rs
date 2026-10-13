use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::dataset::{ColumnData, Dataset, KeyValue, Value};
use crate::preserve::ResolvedPreserve;
use crate::profile::{ColumnProfile, Distribution};
use crate::transform::Strategy;

/// Minimum observed equivalence-class size below which [`observe_linkage`]
/// warns rather than passes.
const LINKAGE_WARN_THRESHOLD: usize = 5;

/// Weight that makes a single leaked row outscore any realistic number of
/// healed orphans in [`score`]. Leakage is a privacy failure and referential
/// is a fidelity failure, and the two must never trade off against each
/// other: a run that leaks one row is worse than a run that heals every
/// orphan in the dataset.
const LEAKAGE_SCORE_WEIGHT: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Enforced. No output value of a direct-identifier column appears in
    /// that column's source value set.
    Leakage,
    /// Enforced. The output orphan set is identical to the input's, not empty:
    /// dangling foreign keys are a production bug that must survive.
    Referential,
    /// Reported only. Forcing k would distort the distribution the engine
    /// exists to preserve.
    Linkage,
}

/// Serialises to the `checks` entries of the report. Every map or set here is
/// a [`BTreeMap`] or [`BTreeSet`]: output ordering is part of the
/// determinism contract. Never a raw value, only counts and column names.
#[derive(Debug, Clone, Serialize)]
pub struct CheckOutcome {
    pub check: Check,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphans_in: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphans_out: Option<usize>,
    /// Size of the symmetric difference between the input and output orphan
    /// position sets: zero exactly when the two sets are identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphan_mismatches: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k_observed: Option<usize>,
    /// Cells flagged by [`Check::Leakage`], summed across every in-scope
    /// column: one column with three leaked rows and another with one
    /// contribute four, so this can exceed any single table's row count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flagged_cells: Option<usize>,
    /// Columns [`Check::Leakage`] found at least one leaked cell in.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub flagged_columns: BTreeSet<String>,
    /// Direct-identifier columns exempted from [`Check::Leakage`] because
    /// they are in the resolved preserve set — named here as preserved and
    /// unchecked, never silently dropped from the report.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub preserved_columns: BTreeSet<String>,
    /// Columns [`Check::Leakage`] actually compared output against source
    /// for. Named explicitly so an empty scope — nothing classified a
    /// direct identifier, everything preserved — is visible in the report
    /// rather than reading identically to a scope that was checked and
    /// found clean.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub checked_columns: BTreeSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl CheckOutcome {
    fn empty(check: Check, verdict: Verdict) -> CheckOutcome {
        CheckOutcome {
            check,
            verdict,
            orphans_in: None,
            orphans_out: None,
            orphan_mismatches: None,
            k_observed: None,
            flagged_cells: None,
            flagged_columns: BTreeSet::new(),
            preserved_columns: BTreeSet::new(),
            checked_columns: BTreeSet::new(),
            reason: None,
        }
    }
}

/// Runs the enforced checks only: [`Check::Leakage`] and [`Check::Referential`].
/// A [`Verdict::Fail`] on either drives a retry at the next
/// [`crate::transform::Strength`]. `profiles` and `strategies` describe
/// `before`, not `after`: classification and strategy resolution happen once
/// and do not change across a retry's escalating attempts.
pub fn enforced(
    before: &Dataset,
    after: &Dataset,
    profiles: &BTreeMap<String, ColumnProfile>,
    strategies: &BTreeMap<String, Strategy>,
    preserved: &ResolvedPreserve,
) -> Vec<CheckOutcome> {
    vec![
        leakage(before, after, profiles, strategies, preserved),
        referential(before, after),
    ]
}

/// No output value of an in-scope column may appear in that column's source
/// value set. In scope is every column whose classification
/// [`crate::classify::SemanticType::is_direct_identifier`] — there is no
/// sample-size or cardinality floor below which a column is skipped, since a
/// small or low-cardinality column is exactly where a leaked value is most
/// identifying, not least.
///
/// The comparison mode is keyed on the column's resolved strategy rather
/// than on anything about the input: [`Strategy::ResampleName`] draws from a
/// small bundled corpus, so an output name coinciding with some unrelated
/// source name elsewhere in the column is expected and not a leak — only a
/// row landing back on its own source value is. Both sides of that
/// comparison normalise case first, since the strategy itself re-cases
/// every draw to Title Case regardless of the source's own casing. Every
/// other strategy draws from a space large enough that set membership is
/// the right test.
///
/// A column named in `preserved` is exempt — preservation is an explicit,
/// intentional request — but is still named in the outcome as preserved and
/// unchecked. A column merely resolved to [`Strategy::Keep`] through a
/// [`crate::policy::Policy::column_overrides`] entry is not in `preserved`
/// and is not exempt: it fails loudly, since no amount of escalation can
/// repair an override that never transforms the value in the first place.
///
/// A column this check cannot evaluate — no profile for it at all, or one
/// present in `before` but absent from `after` — fails rather than falling
/// out of scope silently: a check that goes quiet instead of flagging the
/// thing it cannot see is indistinguishable from a pass to anyone reading
/// the report. The same applies to a row position that exists in `after`
/// but not in `before`: a row-count mismatch inside an in-scope column is a
/// defect this check is positioned to catch, not a reason to skip the row.
///
/// The comparison itself is whole-value equality, not substring containment:
/// a transform that preserves part of a value verbatim — [`Strategy::PreservePrefix`]'s
/// area code, by design — is invisible to this check unless the *entire*
/// value survives. A fixed, narrow residue detector would conflict with that
/// design rather than complement it, so this is a known gap, not a bug to
/// close blindly.
fn leakage(
    before: &Dataset,
    after: &Dataset,
    profiles: &BTreeMap<String, ColumnProfile>,
    strategies: &BTreeMap<String, Strategy>,
    preserved: &ResolvedPreserve,
) -> CheckOutcome {
    let after_columns: BTreeMap<String, &ColumnData> =
        after.qualified_columns().into_iter().collect();

    let mut flagged_cells = 0usize;
    let mut flagged_columns = BTreeSet::new();
    let mut preserved_columns = BTreeSet::new();
    let mut checked_columns = BTreeSet::new();
    let mut unevaluable: BTreeMap<String, &'static str> = BTreeMap::new();

    for (qualified, before_column) in before.qualified_columns() {
        let Some(profile) = profiles.get(&qualified) else {
            unevaluable.insert(qualified.clone(), "no profile is available for it");
            continue;
        };
        if !profile.semantic_type.is_direct_identifier() {
            continue;
        }
        if preserved.contains(&qualified) {
            preserved_columns.insert(qualified);
            continue;
        }
        let Some(&after_column) = after_columns.get(&qualified) else {
            unevaluable.insert(
                qualified.clone(),
                "it is missing from the anonymized output",
            );
            continue;
        };
        checked_columns.insert(qualified.clone());

        let positional = strategies.get(&qualified).copied() == Some(Strategy::ResampleName);
        let source_values: BTreeSet<String> = before_column
            .values
            .iter()
            .filter_map(Value::as_text)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase)
            .collect();

        let mut column_flagged = false;
        for (row, output) in after_column.values.iter().enumerate() {
            let Some(output_text) = output.as_text() else {
                continue;
            };
            if output_text.is_empty() {
                continue;
            }
            let leaked = if positional {
                match before_column.values.get(row).and_then(Value::as_text) {
                    None => true,
                    Some("") => false,
                    Some(input) => input.to_lowercase() == output_text.to_lowercase(),
                }
            } else {
                source_values.contains(&output_text.to_lowercase())
            };
            if leaked {
                flagged_cells += 1;
                column_flagged = true;
            }
        }
        if column_flagged {
            flagged_columns.insert(qualified);
        }
    }

    let verdict = if flagged_columns.is_empty() && unevaluable.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail
    };

    let reason = if !flagged_columns.is_empty() || !unevaluable.is_empty() {
        let mut parts = Vec::new();
        if !flagged_columns.is_empty() {
            parts.push(format!(
                "{} column(s) leaked a source value: {}",
                flagged_columns.len(),
                flagged_columns
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !unevaluable.is_empty() {
            let detail: Vec<String> = unevaluable
                .iter()
                .map(|(column, why)| format!("{column} ({why})"))
                .collect();
            parts.push(format!(
                "{} column(s) could not be evaluated: {}",
                unevaluable.len(),
                detail.join(", ")
            ));
        }
        Some(parts.join("; "))
    } else if checked_columns.is_empty() {
        Some(if preserved_columns.is_empty() {
            "no column classified as a direct identifier was in scope".to_owned()
        } else {
            "every direct identifier in scope was preserved and exempted from this check".to_owned()
        })
    } else {
        None
    };

    CheckOutcome {
        flagged_cells: (flagged_cells > 0).then_some(flagged_cells),
        flagged_columns,
        preserved_columns,
        checked_columns,
        reason,
        ..CheckOutcome::empty(Check::Leakage, verdict)
    }
}

/// The output's orphan foreign-key position set must be identical to the
/// input's — not merely equal in count, and not empty. A dangling foreign
/// key is legal input; [`crate::dataset::schema`] only ever rejects a
/// foreign key whose *target column* does not exist, never a row whose
/// value is absent from that column's data, so an orphan surviving the run
/// unchanged is success, not a defect to clear.
fn referential(before: &Dataset, after: &Dataset) -> CheckOutcome {
    let orphans_in = orphan_positions(before);
    let orphans_out = orphan_positions(after);
    let mismatches = orphans_in.symmetric_difference(&orphans_out).count();
    let mismatched_columns: BTreeSet<&str> = orphans_in
        .symmetric_difference(&orphans_out)
        .map(|(column, _)| column.as_str())
        .collect();

    let reason = (mismatches > 0).then(|| {
        format!(
            "{mismatches} foreign-key position(s) differ between the input and output orphan \
             sets, across {} column(s): {}",
            mismatched_columns.len(),
            mismatched_columns
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .join(", ")
        )
    });

    CheckOutcome {
        orphans_in: Some(orphans_in.len()),
        orphans_out: Some(orphans_out.len()),
        orphan_mismatches: (mismatches > 0).then_some(mismatches),
        reason,
        ..CheckOutcome::empty(
            Check::Referential,
            if mismatches == 0 {
                Verdict::Pass
            } else {
                Verdict::Fail
            },
        )
    }
}

/// Every `(qualified_column, row_index)` whose non-null foreign-key value is
/// absent from the referenced column's value set. Positions, not values: a
/// value changes by design under minting, but which positions are orphaned
/// must not.
///
/// A value that cannot even be read as a key — not
/// [`crate::dataset::ColumnType::Integer`] or
/// [`crate::dataset::ColumnType::Text`], which should not reach here past
/// schema validation but is not proven absent by construction — counts as
/// orphaned
/// rather than being skipped: this check exists to catch a dangling
/// reference, and a value it cannot resolve is not evidence the reference
/// resolves.
fn orphan_positions(dataset: &Dataset) -> BTreeSet<(String, usize)> {
    let mut positions = BTreeSet::new();
    for table in &dataset.tables {
        for column in &table.columns {
            let Some(fk) = &column.spec.fk else { continue };
            let Some(target) = dataset.table(&fk.table).and_then(|t| t.column(&fk.column)) else {
                continue;
            };
            let target_values: BTreeSet<KeyValue> = target
                .values
                .iter()
                .filter_map(KeyValue::from_value)
                .collect();

            let qualified = column.qualified_name(&table.name);
            for (row, value) in column.values.iter().enumerate() {
                if value.is_null() {
                    continue;
                }
                let is_orphan = match KeyValue::from_value(value) {
                    Some(key) => !target_values.contains(&key),
                    None => true,
                };
                if is_orphan {
                    positions.insert((qualified.clone(), row));
                }
            }
        }
    }
    positions
}

/// Observed minimum equivalence-class size over each table's
/// quasi-identifier set, surfaced as `k_observed`. Reported only: a low `k`
/// never fails the run and never drives a retry, since forcing it up would
/// mean mutating values away from the distribution the engine exists to
/// preserve.
///
/// A table's quasi-identifier set is its columns that are not keys, not
/// classified a direct identifier, not in `preserved`, and whose observed
/// [`Distribution`] is [`Distribution::Categorical`]. Categorical is the
/// only distribution this build will group on: an exact-valued numeric
/// column or free text would make every group size 1 by construction and
/// report a `k` that says nothing about real re-identification risk. This
/// build does not generalise such columns into coarser buckets, so — as a
/// known limitation — a quasi-identifier carried as free text or a raw
/// number is invisible to this check rather than counted against it.
pub fn observe_linkage(
    after: &Dataset,
    after_profiles: &BTreeMap<String, ColumnProfile>,
    preserved: &ResolvedPreserve,
) -> CheckOutcome {
    let mut k_observed: Option<usize> = None;

    for table in &after.tables {
        if table.row_count() == 0 {
            continue;
        }
        let qi_columns: Vec<&ColumnData> = table
            .columns
            .iter()
            .filter(|column| is_quasi_identifier(column, &table.name, after_profiles, preserved))
            .collect();
        if qi_columns.is_empty() {
            continue;
        }

        let mut groups: BTreeMap<Vec<String>, usize> = BTreeMap::new();
        for row in 0..table.row_count() {
            let key: Vec<String> = qi_columns
                .iter()
                .map(|column| crate::profile::canonical_key(&column.values[row]))
                .collect();
            *groups.entry(key).or_insert(0) += 1;
        }

        let table_min = groups.values().copied().min().unwrap_or(0);
        k_observed = Some(k_observed.map_or(table_min, |current| current.min(table_min)));
    }

    match k_observed {
        None => CheckOutcome {
            reason: Some(
                "no table has a non-key, non-identifier, non-preserved categorical column to \
                 group on"
                    .to_owned(),
            ),
            ..CheckOutcome::empty(Check::Linkage, Verdict::Pass)
        },
        Some(k) => CheckOutcome {
            k_observed: Some(k),
            ..CheckOutcome::empty(
                Check::Linkage,
                if k < LINKAGE_WARN_THRESHOLD {
                    Verdict::Warn
                } else {
                    Verdict::Pass
                },
            )
        },
    }
}

fn is_quasi_identifier(
    column: &ColumnData,
    table: &str,
    profiles: &BTreeMap<String, ColumnProfile>,
    preserved: &ResolvedPreserve,
) -> bool {
    if column.spec.is_key() {
        return false;
    }
    let qualified = column.qualified_name(table);
    if preserved.contains(&qualified) {
        return false;
    }
    let Some(profile) = profiles.get(&qualified) else {
        return false;
    };
    if profile.semantic_type.is_direct_identifier() {
        return false;
    }
    matches!(profile.distribution, Distribution::Categorical(_))
}

/// Lower is better; `0` means every enforced check passed. Used to pick the
/// best of several failing attempts when none clears every enforced check.
/// Leakage outweighs referential by [`LEAKAGE_SCORE_WEIGHT`] so the two
/// never trade off: a single leaked row always scores worse than any
/// realistic number of orphan mismatches, since leakage is a privacy failure
/// and referential is a fidelity one. [`Check::Linkage`] never contributes —
/// it is reported, never enforced, and never factors into which attempt wins.
pub fn score(outcomes: &[CheckOutcome]) -> u64 {
    outcomes
        .iter()
        .map(|outcome| match outcome.check {
            Check::Leakage => outcome.flagged_cells.unwrap_or(0) as u64 * LEAKAGE_SCORE_WEIGHT,
            Check::Referential => outcome.orphan_mismatches.unwrap_or(0) as u64,
            Check::Linkage => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::SemanticType;
    use crate::dataset::{ColumnSpec, ColumnType, ForeignKey, Table};

    fn pk_column(name: &str, values: &[i64]) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty: ColumnType::Integer,
                nullable: false,
                pk: true,
                fk: None,
            },
            values: values.iter().copied().map(Value::Int).collect(),
        }
    }

    fn fk_column(
        name: &str,
        target_table: &str,
        target_column: &str,
        values: &[i64],
    ) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty: ColumnType::Integer,
                nullable: false,
                pk: false,
                fk: Some(ForeignKey {
                    table: target_table.to_owned(),
                    column: target_column.to_owned(),
                }),
            },
            values: values.iter().copied().map(Value::Int).collect(),
        }
    }

    fn dataset(customer_ids: &[i64], order_fks: &[i64]) -> Dataset {
        Dataset {
            tables: vec![
                Table {
                    name: "customers".into(),
                    columns: vec![pk_column("id", customer_ids)],
                },
                Table {
                    name: "orders".into(),
                    columns: vec![fk_column("customer_id", "customers", "id", order_fks)],
                },
            ],
        }
    }

    fn empty_profiles() -> BTreeMap<String, ColumnProfile> {
        BTreeMap::new()
    }

    fn empty_strategies() -> BTreeMap<String, Strategy> {
        BTreeMap::new()
    }

    fn text_column(name: &str, values: &[&str]) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty: ColumnType::Text,
                nullable: false,
                pk: false,
                fk: None,
            },
            values: values
                .iter()
                .map(|v| Value::Text((*v).to_owned()))
                .collect(),
        }
    }

    fn direct_identifier_profile(column: &str, semantic_type: SemanticType) -> ColumnProfile {
        ColumnProfile {
            column: column.to_owned(),
            semantic_type,
            confidence: 1.0,
            null_rate: 0.0,
            distinct: 0,
            length: None,
            distribution: Distribution::Unsummarised,
            anomaly_rates: BTreeMap::new(),
        }
    }

    /// An orphan present in the input but healed (made to resolve) in the
    /// output must fail the referential check: the orphan position set
    /// changed, even though it changed toward fewer orphans rather than
    /// more.
    #[test]
    fn healing_an_orphan_fails_the_referential_check() {
        let before = dataset(&[1, 2, 3], &[1, 999]);
        let after = dataset(&[1, 2, 3], &[1, 2]);

        let outcomes = enforced(
            &before,
            &after,
            &empty_profiles(),
            &empty_strategies(),
            &ResolvedPreserve::default(),
        );
        let referential = outcomes
            .iter()
            .find(|outcome| outcome.check == Check::Referential)
            .expect("referential outcome is present");

        assert_eq!(referential.verdict, Verdict::Fail);
        assert_eq!(referential.orphans_in, Some(1));
        assert_eq!(referential.orphans_out, Some(0));
    }

    /// An orphan that survives at the same position, with a different
    /// (minted) value, must pass: the position set is unchanged even though
    /// the value is not.
    #[test]
    fn a_surviving_orphan_at_the_same_position_passes() {
        let before = dataset(&[1, 2, 3], &[1, 999]);
        let after = dataset(&[10, 20, 30], &[10, 888]);

        let outcomes = enforced(
            &before,
            &after,
            &empty_profiles(),
            &empty_strategies(),
            &ResolvedPreserve::default(),
        );
        let referential = outcomes
            .iter()
            .find(|outcome| outcome.check == Check::Referential)
            .expect("referential outcome is present");

        assert_eq!(referential.verdict, Verdict::Pass);
        assert_eq!(referential.orphans_in, Some(1));
        assert_eq!(referential.orphans_out, Some(1));
    }

    /// Leakage is weighted to dominate referential: one leaked row must
    /// outscore any number of orphan mismatches a realistic dataset could
    /// produce.
    #[test]
    fn leakage_dominates_referential_in_scoring() {
        let heavy_referential = CheckOutcome {
            orphan_mismatches: Some(10_000),
            ..CheckOutcome::empty(Check::Referential, Verdict::Fail)
        };
        let light_leakage = CheckOutcome {
            flagged_cells: Some(1),
            ..CheckOutcome::empty(Check::Leakage, Verdict::Fail)
        };

        assert!(score(&[light_leakage]) > score(&[heavy_referential]));
    }

    #[test]
    fn a_clean_attempt_scores_zero() {
        let leakage = CheckOutcome::empty(Check::Leakage, Verdict::Pass);
        let referential = CheckOutcome::empty(Check::Referential, Verdict::Pass);
        assert_eq!(score(&[leakage, referential]), 0);
    }

    /// A set-membership comparison (the default for any strategy other than
    /// [`Strategy::ResampleName`]) must fail when an output row lands on a
    /// value from anywhere in the source column, not only its own row.
    #[test]
    fn a_set_membership_collision_fails_the_leakage_check() {
        let before = Dataset {
            tables: vec![Table {
                name: "customers".into(),
                columns: vec![text_column(
                    "email",
                    &["ana@example.com", "bruno@example.com"],
                )],
            }],
        };
        let after = Dataset {
            tables: vec![Table {
                name: "customers".into(),
                columns: vec![text_column(
                    "email",
                    &["carla@example.com", "ana@example.com"],
                )],
            }],
        };

        let mut profiles = BTreeMap::new();
        profiles.insert(
            "customers.email".to_owned(),
            direct_identifier_profile("customers.email", SemanticType::Email),
        );
        let mut strategies = BTreeMap::new();
        strategies.insert("customers.email".to_owned(), Strategy::Redact);

        let outcomes = enforced(
            &before,
            &after,
            &profiles,
            &strategies,
            &ResolvedPreserve::default(),
        );
        let leakage = outcomes
            .iter()
            .find(|outcome| outcome.check == Check::Leakage)
            .expect("leakage outcome is present");

        assert_eq!(leakage.verdict, Verdict::Fail);
        assert_eq!(leakage.flagged_cells, Some(1));
        assert!(leakage.flagged_columns.contains("customers.email"));
    }

    /// The positional comparison ([`Strategy::ResampleName`]'s mode) must
    /// fail when a row's output lands back on that same row's own source
    /// value, independent of case.
    #[test]
    fn a_positional_self_collision_fails_the_leakage_check() {
        let before = Dataset {
            tables: vec![Table {
                name: "customers".into(),
                columns: vec![text_column("full_name", &["Ana Silva", "Bruno Souza"])],
            }],
        };
        let after = Dataset {
            tables: vec![Table {
                name: "customers".into(),
                columns: vec![text_column("full_name", &["ANA SILVA", "Carla Dias"])],
            }],
        };

        let mut profiles = BTreeMap::new();
        profiles.insert(
            "customers.full_name".to_owned(),
            direct_identifier_profile("customers.full_name", SemanticType::FullName),
        );
        let mut strategies = BTreeMap::new();
        strategies.insert("customers.full_name".to_owned(), Strategy::ResampleName);

        let outcomes = enforced(
            &before,
            &after,
            &profiles,
            &strategies,
            &ResolvedPreserve::default(),
        );
        let leakage = outcomes
            .iter()
            .find(|outcome| outcome.check == Check::Leakage)
            .expect("leakage outcome is present");

        assert_eq!(leakage.verdict, Verdict::Fail);
        assert_eq!(leakage.flagged_cells, Some(1));
        assert!(leakage.flagged_columns.contains("customers.full_name"));
    }
}
