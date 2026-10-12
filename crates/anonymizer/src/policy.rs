use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::transform::Strategy;

/// Per-request knobs. Nothing here survives the request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Policy {
    /// Pins the run seed. Absent means a fresh random seed per request. Never
    /// echoed into the report: it is a re-identification key.
    pub seed: Option<u64>,
    /// Forces a strategy on a column, keyed by `table.column`.
    pub column_overrides: BTreeMap<String, Strategy>,
}
