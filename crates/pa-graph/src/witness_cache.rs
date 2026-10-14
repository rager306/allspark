//! M11 — witness-keyed cache.
//!
//! A result is valid as long as the *witness* that produced it is unchanged.
//! The key is `(witness_hash, normalized query)`, so two callers asking the
//! same thing against the same witness share a cache entry, and a witness
//! change (new commit, new index generation) transparently invalidates every
//! entry bound to the old witness — no explicit flush needed.

use std::collections::HashMap;

use crate::query_cache::normalize_query;

/// A cache keyed by a witness hash plus the normalized query.
#[derive(Debug, Clone, Default)]
pub struct WitnessKeyedCache {
    entries: HashMap<(String, String), String>,
    hits: usize,
    misses: usize,
}

impl WitnessKeyedCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Cache hit count.
    #[must_use]
    pub fn hits(&self) -> usize {
        self.hits
    }

    /// Cache miss count.
    #[must_use]
    pub fn misses(&self) -> usize {
        self.misses
    }

    fn key(witness_hash: &str, query: &str) -> (String, String) {
        (witness_hash.to_string(), normalize_query(query))
    }

    /// Look up the value cached under `(witness_hash, query)`.
    pub fn get(&mut self, witness_hash: &str, query: &str) -> Option<&str> {
        let key = Self::key(witness_hash, query);
        if let Some(value) = self.entries.get(&key) {
            self.hits += 1;
            Some(value.as_str())
        } else {
            self.misses += 1;
            None
        }
    }

    /// Store `value` under `(witness_hash, query)`.
    pub fn put(&mut self, witness_hash: &str, query: &str, value: impl Into<String>) {
        self.entries
            .insert(Self::key(witness_hash, query), value.into());
    }

    /// Drop every entry bound to `witness_hash`. Returns the number dropped.
    pub fn invalidate_witness(&mut self, witness_hash: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|(witness, _), _| witness != witness_hash);
        before - self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_witness_shares_a_hit() {
        let mut cache = WitnessKeyedCache::new();
        cache.put("w1", "Parse Config", "answer");
        assert_eq!(cache.get("w1", "parse   config"), Some("answer"));
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn different_witness_is_a_miss() {
        let mut cache = WitnessKeyedCache::new();
        cache.put("w1", "q", "answer");
        assert_eq!(cache.get("w2", "q"), None);
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn invalidating_a_witness_drops_only_its_entries() {
        let mut cache = WitnessKeyedCache::new();
        cache.put("w1", "q1", "a1");
        cache.put("w1", "q2", "a2");
        cache.put("w2", "q1", "b1");
        let dropped = cache.invalidate_witness("w1");
        assert_eq!(dropped, 2);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get("w2", "q1"), Some("b1"));
        assert_eq!(cache.get("w1", "q1"), None);
    }
}
