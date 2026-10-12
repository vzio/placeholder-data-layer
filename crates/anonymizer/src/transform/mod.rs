pub mod ledger;

use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::classify::SemanticType;
use crate::dataset::Value;
use crate::error::Error;
use crate::transform::ledger::{ClassId, Ledger};

/// Every transform is local to the value and preserves the part that carries
/// the distribution, so fidelity holds by construction rather than by
/// resampling. Enum dispatch: no `dyn Strategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Leave the value untouched.
    Keep,
    /// Length-preserving redaction. The conservative default.
    Redact,
    /// New local part, domain kept verbatim.
    PseudonymizeLocalPart,
    /// Area code and prefix kept, remainder randomized.
    PreservePrefix,
    /// Regenerated with valid check digits.
    RegenerateDocument,
    /// Resampled from the bundled pt-BR corpus; token count and casing kept.
    ResampleName,
    /// Moved within a bounded window.
    Jitter,
    /// Minted through the ledger.
    MintKey,
}

impl Strategy {
    pub fn for_semantic_type(_semantic_type: SemanticType) -> Strategy {
        todo!("P4: default strategy per semantic type")
    }

    pub fn apply(
        self,
        _value: &Value,
        _strength: Strength,
        _ledger: &Ledger,
        _class: Option<ClassId>,
        _rng: &mut ChaCha8Rng,
    ) -> Result<Value, Error> {
        todo!("P4: value-local transforms")
    }
}

/// Escalation ladder for the compliance retry loop. Termination is proven by
/// the type: the ladder is finite and `next` eventually returns `None`, so no
/// `max_retries` counter is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Default,
    Aggressive,
    Maximum,
}

impl Strength {
    pub fn next(self) -> Option<Strength> {
        todo!("P4: ladder")
    }
}
