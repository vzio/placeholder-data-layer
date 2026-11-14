//! SplitMix64, the generators' only source of randomness.
//!
//! The corpus keeps its own generator so a dataset never depends on the
//! engine's RNG, nor on a `rand` release: a committed digest is only worth
//! anything while the bytes behind it can be regenerated forever.

/// A SplitMix64 stream. Single-threaded by construction: a generator draws
/// in one fixed order, and work that wants an independent stream forks one.
#[derive(Debug, Clone)]
pub struct Rng {
    origin: u64,
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng {
            origin: seed,
            state: seed,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` by rejection, so no residue is favoured. `below(0)`
    /// is `0`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let threshold = n.wrapping_neg() % n;
        loop {
            let draw = self.next_u64();
            if draw >= threshold {
                return draw % n;
            }
        }
    }

    pub fn index(&mut self, len: usize) -> usize {
        self.below(len as u64) as usize
    }

    /// Inclusive range.
    pub fn between(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high.saturating_sub(low) + 1)
    }

    pub fn chance(&mut self, per_mille: u64) -> bool {
        self.below(1000) < per_mille
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        items.get(self.index(items.len()))
    }

    /// Picks an item with probability proportional to its weight.
    pub fn weighted<'a, T>(&mut self, items: &'a [(T, u64)]) -> Option<&'a T> {
        let total: u64 = items.iter().map(|(_, weight)| weight).sum();
        let mut draw = self.below(total);
        for (item, weight) in items {
            if draw < *weight {
                return Some(item);
            }
            draw -= weight;
        }
        None
    }

    /// An independent stream keyed by `label`, a function of this stream's
    /// seed and the label only — never of how much has been drawn.
    pub fn fork(&self, label: &str) -> Rng {
        Rng::new(self.origin ^ fnv1a(label))
    }
}

fn fnv1a(label: &str) -> u64 {
    label.bytes().fold(0xCBF2_9CE4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_its_reference_outputs_for_seed_zero() {
        let mut rng = Rng::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn below_stays_in_bounds_and_reaches_every_value() {
        let mut rng = Rng::new(7);
        let mut seen = [false; 7];
        for _ in 0..1000 {
            let value = rng.below(7);
            assert!(value < 7);
            seen[value as usize] = true;
        }
        assert!(seen.iter().all(|hit| *hit));
        assert_eq!(rng.below(0), 0);
        assert_eq!(rng.below(1), 0);
    }

    #[test]
    fn fork_depends_on_the_label_and_not_on_draws_taken() {
        let mut drawn = Rng::new(42);
        let fresh = Rng::new(42);
        drawn.next_u64();
        assert_eq!(drawn.fork("a").next_u64(), fresh.fork("a").next_u64());
        assert_ne!(fresh.fork("a").next_u64(), fresh.fork("b").next_u64());
    }

    #[test]
    fn weighted_never_picks_a_zero_weight_item() {
        let mut rng = Rng::new(3);
        let items = [("never", 0), ("always", 5)];
        for _ in 0..200 {
            assert_eq!(rng.weighted(&items), Some(&"always"));
        }
    }
}
