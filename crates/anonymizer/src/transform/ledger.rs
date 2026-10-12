use std::collections::BTreeMap;

use rand_chacha::ChaCha8Rng;

use crate::dataset::{Dataset, KeyValue};
use crate::error::Error;

/// Index of a referential equivalence class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClassId(pub usize);

/// Union-find over key columns joined by foreign key edges. A chain
/// `c.x -> b.y -> a.z` collapses to one class, which is what makes referential
/// integrity hold transitively and dissolves foreign key cycles without a
/// topological sort.
#[derive(Debug, Clone, Default)]
pub struct EquivalenceClasses {
    /// `table.column` to class, in sorted key order.
    pub members: BTreeMap<String, ClassId>,
    pub count: usize,
}

impl EquivalenceClasses {
    pub fn build(_dataset: &Dataset) -> Result<Self, Error> {
        todo!("P1: union-find over key columns")
    }

    pub fn class_of(&self, _qualified_column: &str) -> Option<ClassId> {
        todo!("P1: class lookup")
    }
}

/// Collects the observed key domain of every class. Minting is insertion-order
/// dependent, so this runs single-threaded in a deterministic order: classes
/// sorted by name, values in row order.
#[derive(Debug, Default)]
pub struct LedgerBuilder {
    pub classes: EquivalenceClasses,
    pub domains: BTreeMap<ClassId, Vec<KeyValue>>,
}

impl LedgerBuilder {
    pub fn new(_classes: EquivalenceClasses) -> Self {
        todo!("P4: ledger build phase")
    }

    pub fn observe(&mut self, _class: ClassId, _value: &KeyValue) {
        todo!("P4: domain collection")
    }

    /// Mints one injective mapping per class, then freezes. The parallel
    /// transform only ever sees `&Ledger`.
    pub fn freeze(self, _rng: &mut ChaCha8Rng) -> Result<Ledger, Error> {
        todo!("P4: seeded permutation with collision retry")
    }
}

/// Frozen, injective, per-class key mapping. Keys are minted rather than
/// hashed: hashing a six-figure key domain into a fixed width collides, and a
/// collision silently merges two parent rows.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub mappings: BTreeMap<ClassId, BTreeMap<KeyValue, KeyValue>>,
}

impl Ledger {
    pub fn lookup(&self, _class: ClassId, _value: &KeyValue) -> Option<&KeyValue> {
        todo!("P4: mapping lookup")
    }
}
