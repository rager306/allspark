//! M10 — semantic query cache with TTL and generation invalidation.
//!
//! A query cache keys on the *normalized* query text plus an embedding hash
//! (a stub here: the caller supplies whatever digest it wants). Entries carry a
//! TTL and the index generation they were computed under, so a bump of the
//! generation invalidates everything computed against the old index even
//! before the TTL expires.

use std::collections::HashMap;

/// One cached query result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedResult {
    /// The cached payload (serialized result, etc.).
    pub value: String,
    /// Embedding digest the entry was stored under.
    pub embedding_hash: String,
    /// Index generation the entry was computed against.
    pub generation: u64,
    /// Absolute time (in the caller's clock units) at which the entry expires.
    pub expires_at: u64,
}

/// Normalize a query: trim, collapse internal whitespace, lowercase.
#[must_use]
pub fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A semantic query cache.
#[derive(Debug, Clone, Default)]
pub struct QueryCache {
    entries: HashMap<(String, String), CachedResult>,
    generation: u64,
    ttl: u64,
    hits: usize,
    misses: usize,
}

impl QueryCache {
    /// A cache with a default TTL (in the caller's clock units).
    #[must_use]
    pub fn new(ttl: u64) -> Self {
        Self {
            entries: HashMap::new(),
            generation: 0,
            ttl,
            hits: 0,
            misses: 0,
        }
    }

    /// The current index generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of cached entries (including expired ones not yet swept).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no entries.
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

    /// The key for `(query, embedding_hash)`.
    #[must_use]
    pub fn key(query: &str, embedding_hash: &str) -> (String, String) {
        (normalize_query(query), embedding_hash.to_string())
    }

    /// Look up a cached result. `now` is the caller's clock; an entry is a hit
    /// only if it is unexpired and its generation matches the cache's current
    /// generation.
    pub fn get(&mut self, query: &str, embedding_hash: &str, now: u64) -> Option<CachedResult> {
        let key = Self::key(query, embedding_hash);
        match self.entries.get(&key) {
            Some(entry) if entry.expires_at > now && entry.generation == self.generation => {
                self.hits += 1;
                Some(entry.clone())
            }
            _ => {
                self.misses += 1;
                None
            }
        }
    }

    /// Store `value` under `(query, embedding_hash)`, expiring at
    /// `now + ttl` and tagged with the current generation.
    pub fn put(&mut self, query: &str, embedding_hash: &str, value: impl Into<String>, now: u64) {
        let key = Self::key(query, embedding_hash);
        self.entries.insert(
            key,
            CachedResult {
                value: value.into(),
                embedding_hash: embedding_hash.to_string(),
                generation: self.generation,
                expires_at: now.saturating_add(self.ttl),
            },
        );
    }

    /// Bump the generation to `generation`, invalidating every entry computed
    /// against an older index. Returns the number of entries dropped.
    pub fn invalidate(&mut self, generation: u64) -> usize {
        self.generation = generation;
        let before = self.entries.len();
        self.entries.retain(|_, entry| entry.generation == generation);
        before - self.entries.len()
    }

    /// Drop expired entries at `now`.
    pub fn sweep(&mut self, now: u64) {
        self.entries.retain(|_, entry| entry.expires_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_collapses_whitespace_and_case() {
        assert_eq!(normalize_query("  Parse   Config "), "parse config");
        assert_eq!(normalize_query("A\tB\nC"), "a b c");
    }

    #[test]
    fn round_trip_hit() {
        let mut cache = QueryCache::new(100);
        cache.put("Parse Config", "emb", "result", 0);
        let hit = cache.get("  parse config ", "emb", 10).expect("hit");
        assert_eq!(hit.value, "result");
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn ttl_expiry_is_a_miss() {
        let mut cache = QueryCache::new(100);
        cache.put("q", "e", "v", 0);
        assert!(cache.get("q", "e", 99).is_some());
        assert!(cache.get("q", "e", 100).is_none(), "expiry is exclusive");
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn generation_bump_invalidates() {
        let mut cache = QueryCache::new(1000);
        cache.put("q1", "e", "v1", 0);
        cache.put("q2", "e", "v2", 0);
        assert!(cache.get("q1", "e", 1).is_some());
        let dropped = cache.invalidate(1);
        assert_eq!(dropped, 2, "both entries were on the old generation");
        assert!(cache.get("q1", "e", 1).is_none());
        // A fresh put under the new generation is a hit.
        cache.put("q1", "e", "v1-new", 1);
        assert_eq!(cache.get("q1", "e", 2).expect("hit").value, "v1-new");
    }

    #[test]
    fn sweep_removes_expired_entries() {
        let mut cache = QueryCache::new(10);
        cache.put("q", "e", "v", 0);
        assert_eq!(cache.len(), 1);
        cache.sweep(10);
        assert!(cache.is_empty());
    }
}
