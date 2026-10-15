//! V19 — LSM tiers with a Bloom filter.
//!
//! A write-heavy key/value index is cheapest to *write* into a small mutable
//! "hot" tier (a `HashMap`) and cheapest to *read* in bulk from an immutable
//! sorted "cold" tier (a `Vec` with binary search). [`LsmIndex`] keeps both:
//! inserts land in the hot tier, [`compact`](LsmIndex::compact) merges hot into
//! cold, and every lookup checks hot first, then cold. A Bloom filter over the
//! cold tier answers "definitely not here" in constant time, so the common
//! miss never touches the sorted vec.
//!
//! The Bloom filter uses two independent FNV-1a hashes and Kirsch–Mitzenmacher
//! double hashing (`h_i = h1 + i·h2`) — no external dependency. It has no
//! false negatives: a key present in the cold tier always probes positive.

use std::collections::HashMap;

/// FNV-1a 64-bit over `bytes`, seeded with `seed` so two independent hashes can
/// be derived from one function.
fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET ^ seed;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// A fixed-capacity Bloom filter over `u64` keys.
#[derive(Debug, Clone)]
struct Bloom {
    bits: Vec<u64>,
    hashes: u32,
}

impl Bloom {
    /// A filter sized for `capacity` keys at a ~1% false-positive rate.
    fn with_capacity(capacity: usize) -> Self {
        // ~10 bits/key ≈ 1% FP at 7 hashes; round up to whole words.
        let bits_needed = capacity.saturating_mul(10).max(64);
        let words = bits_needed.div_ceil(64);
        Self {
            bits: vec![0u64; words],
            hashes: 7,
        }
    }

    fn bit_count(&self) -> usize {
        self.bits.len() * 64
    }

    /// The two base hashes for `key`; combined per-bit via double hashing.
    fn base_hashes(key: u64) -> (u64, u64) {
        let bytes = key.to_le_bytes();
        let h1 = fnv1a(&bytes, 0x9e37_79b9_7f4a_7c15);
        let h2 = fnv1a(&bytes, 0xc2b2_ae3d_27d4_eb4f) | 1; // odd → coprime with 2^k
        (h1, h2)
    }

    fn insert(&mut self, key: u64) {
        let (h1, h2) = Self::base_hashes(key);
        let nbits = self.bit_count() as u64;
        for i in 0..u64::from(self.hashes) {
            let idx = h1.wrapping_add(i.wrapping_mul(h2)) % nbits;
            self.bits[(idx / 64) as usize] |= 1u64 << (idx % 64);
        }
    }

    fn contains(&self, key: u64) -> bool {
        let (h1, h2) = Self::base_hashes(key);
        let nbits = self.bit_count() as u64;
        for i in 0..u64::from(self.hashes) {
            let idx = h1.wrapping_add(i.wrapping_mul(h2)) % nbits;
            if self.bits[(idx / 64) as usize] & (1u64 << (idx % 64)) == 0 {
                return false;
            }
        }
        true
    }
}

/// Which tier a key lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// The mutable `HashMap` tier.
    Hot,
    /// The immutable sorted-vec tier.
    Cold,
    /// Not present.
    Absent,
}

/// A two-tier LSM index over `u64` keys with `i64` values.
#[derive(Debug, Clone, Default)]
pub struct LsmIndex {
    hot: HashMap<u64, i64>,
    cold: Vec<(u64, i64)>,
    bloom: Option<Bloom>,
}

impl LsmIndex {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or overwrite a key. Always lands in the hot tier.
    pub fn insert(&mut self, key: u64, value: i64) {
        self.hot.insert(key, value);
    }

    /// Look up a key across both tiers (hot shadows cold).
    #[must_use]
    pub fn get(&self, key: u64) -> Option<i64> {
        if let Some(&value) = self.hot.get(&key) {
            return Some(value);
        }
        if self.bloom.as_ref().is_some_and(|b| !b.contains(key)) {
            return None;
        }
        self.cold
            .binary_search_by_key(&key, |&(k, _)| k)
            .ok()
            .map(|idx| self.cold[idx].1)
    }

    /// Which tier currently holds `key`.
    #[must_use]
    pub fn tier_of(&self, key: u64) -> Tier {
        if self.hot.contains_key(&key) {
            Tier::Hot
        } else if self.get(key).is_some() {
            Tier::Cold
        } else {
            Tier::Absent
        }
    }

    /// Whether the cold-tier Bloom filter *might* contain `key`. Never returns
    /// `false` for a key actually present in the cold tier (no false negatives).
    #[must_use]
    pub fn bloom_probably_contains(&self, key: u64) -> bool {
        self.bloom.as_ref().is_none_or(|b| b.contains(key))
    }

    /// Merge the hot tier into the cold tier, rebuilding the Bloom filter.
    /// Hot values overwrite cold ones. After this the hot tier is empty.
    pub fn compact(&mut self) {
        for (key, value) in self.hot.drain() {
            match self.cold.binary_search_by_key(&key, |&(k, _)| k) {
                Ok(idx) => self.cold[idx].1 = value,
                Err(idx) => self.cold.insert(idx, (key, value)),
            }
        }
        let mut bloom = Bloom::with_capacity(self.cold.len());
        for &(key, _) in &self.cold {
            bloom.insert(key);
        }
        self.bloom = Some(bloom);
    }

    /// Number of live keys across both tiers.
    #[must_use]
    pub fn len(&self) -> usize {
        let cold_only = self
            .cold
            .iter()
            .filter(|(k, _)| !self.hot.contains_key(k))
            .count();
        self.hot.len() + cold_only
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_survives_compaction() {
        let mut index = LsmIndex::new();
        for i in 0..50u64 {
            index.insert(i, (i * 10) as i64);
        }
        assert_eq!(index.tier_of(7), Tier::Hot, "fresh writes are hot");
        index.compact();
        assert_eq!(index.tier_of(7), Tier::Cold, "compaction moves to cold");
        for i in 0..50u64 {
            assert_eq!(index.get(i), Some((i * 10) as i64), "key {i} survived");
        }
    }

    #[test]
    fn bloom_has_no_false_negatives() {
        let mut index = LsmIndex::new();
        for i in 0..200u64 {
            index.insert(i, 1);
        }
        index.compact();
        for i in 0..200u64 {
            assert!(
                index.bloom_probably_contains(i),
                "present key {i} must probe positive"
            );
        }
    }

    #[test]
    fn bloom_rejects_most_absent_keys() {
        let mut index = LsmIndex::new();
        for i in 0..100u64 {
            index.insert(i, 1);
        }
        index.compact();
        // Sample absent keys; the ~1% FP rate means the overwhelming majority
        // are correctly rejected.
        let rejected = (1_000u64..2_000)
            .filter(|&k| !index.bloom_probably_contains(k))
            .count();
        assert!(
            rejected > 900,
            "bloom should reject most absent keys, rejected {rejected}/1000"
        );
    }

    #[test]
    fn hot_shadows_cold_after_reinsert() {
        let mut index = LsmIndex::new();
        index.insert(5, 100);
        index.compact();
        assert_eq!(index.get(5), Some(100));
        index.insert(5, 999);
        assert_eq!(index.get(5), Some(999), "hot write shadows the cold value");
        assert_eq!(index.tier_of(5), Tier::Hot);
    }

    #[test]
    fn missing_key_is_absent() {
        let mut index = LsmIndex::new();
        index.insert(1, 1);
        index.compact();
        assert_eq!(index.get(42), None);
        assert_eq!(index.tier_of(42), Tier::Absent);
    }

    #[test]
    fn len_counts_unique_keys() {
        let mut index = LsmIndex::new();
        index.insert(1, 1);
        index.insert(2, 2);
        index.compact();
        index.insert(2, 20);
        index.insert(3, 3);
        assert_eq!(index.len(), 3, "key 2 spans both tiers but counts once");
    }
}
