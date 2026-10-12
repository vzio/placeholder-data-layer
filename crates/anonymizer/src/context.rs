use std::collections::BTreeMap;

use rand_chacha::ChaCha8Rng;

use crate::pipeline::Stage;
use crate::policy::Policy;
use crate::transform::ledger::Ledger;

/// Request-scoped state. Owned by the request and dropped with the response:
/// nothing survives across requests.
#[derive(Debug)]
pub struct RunContext {
    /// Random per request unless pinned by the policy. Never reported.
    pub seed: u64,
    pub policy: Policy,
    /// `None` until the single-threaded build phase freezes it.
    pub ledger: Option<Ledger>,
    pub timings_ms: BTreeMap<Stage, u64>,
}

impl RunContext {
    pub fn new(policy: Policy) -> Self {
        let seed = policy.seed.unwrap_or_else(rand::random);
        Self {
            seed,
            policy,
            ledger: None,
            timings_ms: BTreeMap::new(),
        }
    }

    /// Derives an independent, reproducible substream from the run seed and a
    /// stable label, so a column's values do not depend on how work was
    /// scheduled across threads.
    pub fn rng_for(&self, _label: &str) -> ChaCha8Rng {
        todo!("P4: blake3::keyed_hash(seed, label) -> ChaCha8Rng::from_seed")
    }
}
