//! M3 — 2-random eviction (pogocache, Power of Two Choices).
//!
//! A bounded cache with no LRU bookkeeping: on insert into a full cache, two
//! *deterministic pseudo-random* victims are chosen and the older one is
//! evicted (a new insert is newer than anything already stored, so an existing
//! victim is always evicted). This is O(1) with a timestamp per entry and no
//! recency list to maintain.
//!
//! Determinism matters for tests and reproducibility: the victim choice is a
//! function of the inserted key and the current entry set, not of wall-clock
//! randomness.

use std::collections::HashMap;

/// A bounded cache with 2-random eviction.
#[derive(Debug, Clone)]
pub struct TwoRandomCache {
    entries: HashMap<String, (String, u64)>,
    capacity: usize,
    clock: u64,
    evictions: usize,
}

impl TwoRandomCache {
    /// A cache holding at most `capacity` entries (at least 1).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
            clock: 0,
            evictions: 0,
        }
    }

    /// The capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of stored entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of evictions performed.
    #[must_use]
    pub fn evictions(&self) -> usize {
        self.evictions
    }

    /// Whether `key` is present.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Insert `(key, value)`, evicting a 2-random victim if the cache is full.
    ///
    /// Returns the evicted key, if any. Inserting an existing key refreshes its
    /// value and timestamp without eviction.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        let key = key.into();
        self.clock += 1;
        if self.entries.contains_key(&key) {
            self.entries.insert(key, (value.into(), self.clock));
            return None;
        }
        let mut evicted = None;
        if self.entries.len() >= self.capacity {
            evicted = self.evict_one(&key);
        }
        self.entries.insert(key, (value.into(), self.clock));
        evicted
    }

    /// Look up a value (does not change recency; this cache has no LRU).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|(value, _)| value.as_str())
    }

    /// Choose and remove one victim using the power of two choices.
    fn evict_one(&mut self, incoming: &str) -> Option<String> {
        let mut keys: Vec<&String> = self.entries.keys().collect();
        // Deterministic ordering independent of HashMap iteration order.
        keys.sort();
        if keys.is_empty() {
            return None;
        }
        let seed = fnv1a(incoming);
        let n = keys.len();
        let first = keys[(seed % n as u64) as usize].clone();
        let second = if n == 1 {
            first.clone()
        } else {
            keys[((seed / n as u64 + 1) % n as u64) as usize].clone()
        };
        // Older timestamp loses; ties break on the lexicographically smaller
        // key so the outcome stays deterministic.
        let time_of = |key: &str| self.entries.get(key).map_or(0, |(_, time)| *time);
        let victim = if time_of(&first) < time_of(&second) {
            first
        } else if time_of(&second) < time_of(&first) {
            second
        } else if first <= second {
            first
        } else {
            second
        };
        self.entries.remove(&victim);
        self.evictions += 1;
        Some(victim)
    }
}

/// A small FNV-1a 64-bit hash for deterministic victim selection.
fn fnv1a(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_up_to_capacity() {
        let mut cache = TwoRandomCache::new(2);
        cache.insert("a", "1");
        cache.insert("b", "2");
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("a"), Some("1"));
    }

    #[test]
    fn insert_beyond_capacity_evicts_exactly_one() {
        let mut cache = TwoRandomCache::new(2);
        cache.insert("a", "1");
        cache.insert("b", "2");
        let evicted = cache.insert("c", "3");
        assert!(evicted.is_some(), "a victim was chosen");
        assert_eq!(cache.len(), 2, "capacity is respected");
        assert_eq!(cache.get("c"), Some("3"), "the new key is present");
        assert_eq!(cache.evictions(), 1);
        let victim = evicted.expect("victim");
        assert!(cache.get(&victim).is_none(), "the victim is gone");
    }

    #[test]
    fn eviction_is_deterministic() {
        let mut first = TwoRandomCache::new(3);
        let mut second = TwoRandomCache::new(3);
        for key in ["a", "b", "c", "d", "e"] {
            let evicted_a = first.insert(key, key);
            let evicted_b = second.insert(key, key);
            assert_eq!(evicted_a, evicted_b, "same inputs, same victim");
        }
    }

    #[test]
    fn updating_an_existing_key_does_not_evict() {
        let mut cache = TwoRandomCache::new(2);
        cache.insert("a", "1");
        cache.insert("b", "2");
        assert!(cache.insert("a", "1-updated").is_none());
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("a"), Some("1-updated"));
        assert_eq!(cache.evictions(), 0);
    }

    #[test]
    fn single_slot_cache_evicts_the_only_entry() {
        let mut cache = TwoRandomCache::new(1);
        cache.insert("a", "1");
        let evicted = cache.insert("b", "2");
        assert_eq!(evicted.as_deref(), Some("a"));
        assert_eq!(cache.get("b"), Some("2"));
        assert!(!cache.contains("a"));
    }
}
