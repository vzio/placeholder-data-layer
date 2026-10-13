use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::dataset::Dataset;
use crate::error::Error;
use crate::transform::Strategy;

/// Per-request knobs. Nothing here survives the request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Policy {
    /// Pins the run seed. Absent means a fresh random seed per request. Never
    /// echoed into the report: it is a re-identification key.
    pub seed: Option<u64>,
    /// Forces a strategy on a column, keyed by `table.column`.
    pub column_overrides: BTreeMap<String, Strategy>,
    /// Exempts a column (`table.column`) or a whole table (`table`) from
    /// anonymization. See [`crate::preserve`] for how this expands into the
    /// full set of columns [`Strategy::Keep`] runs under.
    pub preserve: BTreeSet<String>,
    /// Suppresses the refusal [`crate::preserve::resolve`]'s caller raises
    /// when a preserved column is classified a direct identifier. Off by
    /// default: preserving a lookup table should stay a one-line policy,
    /// but silently publishing a direct identifier never should.
    pub force_preserve: bool,
}

impl Policy {
    /// Rejects an override or a preserve entry naming a table or column
    /// absent from `dataset`.
    pub fn validate(&self, dataset: &Dataset) -> Result<(), Error> {
        for key in self.column_overrides.keys() {
            let target = key
                .split_once('.')
                .and_then(|(table, column)| dataset.table(table)?.column(column));
            if target.is_none() {
                return Err(Error::UnknownOverride(key.clone()));
            }
        }
        for name in &self.preserve {
            if !preserve_target_exists(dataset, name) {
                return Err(Error::UnknownPreserve(name.clone()));
            }
        }
        Ok(())
    }
}

/// `name` is either `table` (a whole table) or `table.column` (one column);
/// either must actually exist in `dataset`.
fn preserve_target_exists(dataset: &Dataset, name: &str) -> bool {
    match name.split_once('.') {
        Some((table, column)) => dataset
            .table(table)
            .and_then(|t| t.column(column))
            .is_some(),
        None => dataset.table(name).is_some(),
    }
}
