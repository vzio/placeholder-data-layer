//! Deterministic RNG derivation.
//!
//! Every random stream the engine draws on traces back to the run seed
//! through `blake3::keyed_hash`, keyed on a label that identifies the stream
//! uniquely. Two calls with the same seed and label always produce the same
//! stream, in any process, on any thread, in any order — which is what lets
//! rayon split work across columns and rows without the result depending on
//! how it happened to split.
//!
//! [`ChaCha8Rng`] only. `StdRng` is explicitly not reproducible across
//! releases, which would silently break the determinism contract on the next
//! toolchain bump.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Expands the 64-bit run seed into a 32-byte key for [`blake3::keyed_hash`].
fn expand_seed(seed: u64) -> [u8; 32] {
    blake3::hash(&seed.to_le_bytes()).into()
}

/// Derives an independent, reproducible RNG from `seed` and `label`.
///
/// `label` should identify the stream uniquely for the lifetime of the run —
/// a stage name, a column's qualified name, or a column name combined with a
/// row index. A composite label's parts must be joined so that no two
/// different compositions can ever land on the same bytes.
/// [`derive_for_row_position`] does this by giving every part but the first
/// a fixed width, so the label's total length alone determines where the
/// first part ends. A single separator byte does not achieve this on its
/// own: a caller-supplied part, such as a `String` decoded from JSON, may
/// itself contain that byte.
pub fn derive(seed: u64, label: &[u8]) -> ChaCha8Rng {
    let key = expand_seed(seed);
    let hash = blake3::keyed_hash(&key, label);
    ChaCha8Rng::from_seed(hash.into())
}

/// Derives the RNG for one row's position within `column` under `variant`:
/// keyed on the run seed, the column, the row index, and `variant`. Never a
/// stream advanced sequentially across the column — that would make a
/// value's randomness depend on the order rayon happens to visit rows in,
/// which breaks the moment the column is split differently across threads.
///
/// `variant` distinguishes callers that otherwise share a `(column,
/// row_index)` pair and must not share a stream — the escalation
/// [`crate::transform::Strength`] a retry attempt runs at, say, so that
/// escalating draws an independent stream instead of replaying the same one
/// into a different strategy parameter.
///
/// Collision-free by construction: the row index and `variant` together are
/// encoded as a fixed 9-byte suffix, so the label's total length alone fixes
/// where `column` ends, and two different `(column, row_index, variant)`
/// triples can never compose to the same bytes.
pub fn derive_for_row_position(
    seed: u64,
    column: &str,
    row_index: usize,
    variant: u8,
) -> ChaCha8Rng {
    let mut label = Vec::with_capacity(column.len() + 10);
    label.extend_from_slice(column.as_bytes());
    label.push(0);
    label.extend_from_slice(&(row_index as u64).to_le_bytes());
    label.push(variant);
    derive(seed, &label)
}

#[cfg(test)]
mod tests {
    use rand::RngExt;

    use super::*;

    #[test]
    fn same_seed_and_label_always_derive_the_same_stream() {
        let mut a = derive(42, b"customers.email");
        let mut b = derive(42, b"customers.email");
        assert_eq!(a.random::<u64>(), b.random::<u64>());
    }

    #[test]
    fn different_labels_derive_independent_streams() {
        let mut a = derive(42, b"customers.email");
        let mut b = derive(42, b"customers.phone");
        assert_ne!(a.random::<u64>(), b.random::<u64>());
    }

    #[test]
    fn different_seeds_derive_independent_streams() {
        let mut a = derive(1, b"customers.email");
        let mut b = derive(2, b"customers.email");
        assert_ne!(a.random::<u64>(), b.random::<u64>());
    }

    #[test]
    fn per_value_derivation_does_not_depend_on_visiting_order() {
        let seed = 7;
        let column = "customers.email";

        let in_order: Vec<u64> = (0..16)
            .map(|row| derive_for_row_position(seed, column, row, 0).random())
            .collect();

        let mut out_of_order: Vec<(usize, u64)> = (0..16)
            .rev()
            .map(|row| (row, derive_for_row_position(seed, column, row, 0).random()))
            .collect();
        out_of_order.sort_by_key(|(row, _)| *row);
        let out_of_order: Vec<u64> = out_of_order.into_iter().map(|(_, value)| value).collect();

        assert_eq!(in_order, out_of_order);
    }

    #[test]
    fn distinct_column_and_row_pairs_derive_distinct_streams() {
        let mut a = derive_for_row_position(42, "t.a", 23, 0);
        let mut b = derive_for_row_position(42, "t.a2", 3, 0);
        assert_ne!(a.random::<u64>(), b.random::<u64>());
    }

    #[test]
    fn distinct_variants_derive_distinct_streams_for_the_same_column_and_row() {
        let mut a = derive_for_row_position(42, "t.a", 23, 0);
        let mut b = derive_for_row_position(42, "t.a", 23, 1);
        assert_ne!(a.random::<u64>(), b.random::<u64>());
    }
}
