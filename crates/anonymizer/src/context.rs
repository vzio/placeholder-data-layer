use std::collections::BTreeMap;

use rand_chacha::ChaCha8Rng;

use crate::pipeline::Stage;
use crate::policy::Policy;
use crate::transform::Strength;
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
    /// stable label, so work keyed on the same label never depends on how it
    /// was scheduled across threads.
    pub fn rng_for(&self, label: &str) -> ChaCha8Rng {
        crate::rng::derive(self.seed, label.as_bytes())
    }

    /// Derives the RNG for one row's position within `column` at `strength`:
    /// keyed on the run seed, `column`, `row_index` and `strength`, never on
    /// iteration order. See [`crate::rng::derive_for_row_position`].
    ///
    /// Keyed on *position*, not on the value stored there — a pseudonym
    /// must key on the value itself via [`Self::rng_for`], so that the same
    /// source value maps to the same anonymized one everywhere it recurs
    /// (requirement #4). Using this for a pseudonym would give one source
    /// value a different stream at every row it appears in and silently
    /// break that guarantee.
    ///
    /// `strength` is folded into the label, not just into how the stream is
    /// used, so that an escalation retry draws a genuinely independent
    /// stream rather than the same one squeezed into a different strategy
    /// parameter — the difference between a retry that can actually heal a
    /// collision and one that reproduces it.
    pub fn rng_for_row_position(
        &self,
        column: &str,
        row_index: usize,
        strength: Strength,
    ) -> ChaCha8Rng {
        crate::rng::derive_for_row_position(self.seed, column, row_index, strength as u8)
    }
}
