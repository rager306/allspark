//! P1 — a sharded map: 4096 shards, high hash bits pick the shard.
//!
//! pogocache spreads its cache over 4096 independent shards (`DEFSHARDS`) and
//! routes each key with `(hash >> 32) % nshards`, so the *high* 32 bits choose
//! the shard and the low 32 bits are the in-shard hash. Every shard carries its
//! own lock, so unrelated keys never contend:
//!
//! ```c
//! static int shard_index(struct pogocache *cache, uint64_t hash) {
//!     return (hash>>32) % cache->ctx.nshards;
//! }
//! ```
//!
//! This port stores each shard behind a [`RwLock`] (reads run concurrently,
//! writes take the shard exclusively). The shard count is a power of two, so
//! the modulo is a mask; the default is [`DEFAULT_SHARDS`].
//!
//! A poisoned shard (a thread panicked while holding it) is recovered rather
//! than re-raised: `HashMap` never exposes a torn state — a panic during
//! `insert` leaves the map unchanged — so the map is safe to keep using. This
//! keeps the map's API total, with no panic path for callers to reason about.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::RwLock;

use crate::hash::th64;

/// The default shard count (`DEFSHARDS` in pogocache).
pub const DEFAULT_SHARDS: usize = 4096;

/// The per-process hash seed the sharded map mixes into every key.
///
/// pogocache seeds `th64` with a crypto-random value at startup to blunt
/// hash-flooding. `ShardedMap::new` derives one from the address of a
/// heap allocation plus the clock — cheap, process-unique, and (unlike a
/// fixed seed) not attacker-predictable. `ShardedMap::with_seed` pins it for
/// reproducible tests.
fn default_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    // A fresh allocation's address differs per process/run; mixing it with the
    // clock gives a seed that is not fixed across runs.
    let probe = Box::new(0u8);
    let addr = std::ptr::from_ref(&*probe) as u64;
    crate::hash::mix13(nanos ^ addr.rotate_left(17))
}

/// Hashes `key` with `th64` under `seed`.
fn hash_of<K: Hash>(key: &K, seed: u64) -> u64 {
    let mut hasher = Th64Hasher {
        seed,
        bytes: Vec::new(),
    };
    key.hash(&mut hasher);
    hasher.finish()
}

/// A [`Hasher`] that buffers writes and finishes with `th64` under a seed.
struct Th64Hasher {
    seed: u64,
    bytes: Vec<u8>,
}

impl Hasher for Th64Hasher {
    fn finish(&self) -> u64 {
        th64(&self.bytes, self.seed)
    }

    fn write(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
}

/// A sharded, thread-safe map.
///
/// Keys are routed to a shard by their hash's high 32 bits; each shard is an
/// independent [`RwLock<HashMap>`]. Contention is per-shard, so two keys in
/// different shards never block each other.
#[derive(Debug)]
pub struct ShardedMap<K, V> {
    shards: Box<[RwLock<HashMap<K, V>>]>,
    mask: usize,
    seed: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> ShardedMap<K, V> {
    /// A map with [`DEFAULT_SHARDS`] shards and a process-unique hash seed.
    #[must_use]
    pub fn new() -> Self {
        Self::with_shards(DEFAULT_SHARDS)
    }

    /// A map with `shards` shards, rounded up to a power of two.
    #[must_use]
    pub fn with_shards(shards: usize) -> Self {
        Self::with_seed(shards, default_seed())
    }

    /// A map with `shards` shards and an explicit hash seed.
    ///
    /// A fixed seed makes [`ShardedMap::shard_of`] reproducible across runs;
    /// tests use it, production leaves the default.
    #[must_use]
    pub fn with_seed(shards: usize, seed: u64) -> Self {
        let count = shards.max(1).next_power_of_two();
        let shards = (0..count)
            .map(|_| RwLock::new(HashMap::new()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            shards,
            mask: count - 1,
            seed,
        }
    }

    /// The number of shards (a power of two).
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    /// The shard index for `key`: `(hash >> 32) & mask`.
    ///
    /// This is the exact routing pogocache's `shard_index` performs.
    #[must_use]
    pub fn shard_of(&self, key: &K) -> usize {
        let hash = hash_of(key, self.seed);
        ((hash >> 32) as usize) & self.mask
    }

    /// Inserts `key`/`value`, returning the previous value for `key`.
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let shard = self.shard_of(&key);
        let mut guard = self.shards[shard]
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(key, value)
    }

    /// Returns a clone of the value for `key`.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<V> {
        let shard = self.shard_of(key);
        let guard = self.shards[shard]
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.get(key).cloned()
    }

    /// Removes `key`, returning its value if it was present.
    pub fn remove(&self, key: &K) -> Option<V> {
        let shard = self.shard_of(key);
        let mut guard = self.shards[shard]
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.remove(key)
    }

    /// Whether `key` is present.
    #[must_use]
    pub fn contains_key(&self, key: &K) -> bool {
        let shard = self.shard_of(key);
        let guard = self.shards[shard]
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.contains_key(key)
    }

    /// The total number of entries across every shard.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| {
                shard
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len()
            })
            .sum()
    }

    /// Whether every shard is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shards.iter().all(|shard| {
            shard
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        })
    }

    /// Applies `f` to every shard's map while holding its write lock.
    ///
    /// Shards are visited one at a time, so the callback must not touch this
    /// map (it would deadlock on the shard already held).
    pub fn for_each_shard<F: FnMut(usize, &mut HashMap<K, V>)>(&self, mut f: F) {
        for (index, shard) in self.shards.iter().enumerate() {
            let mut guard = shard
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            f(index, &mut guard);
        }
    }

    /// The per-shard entry counts, in shard order.
    #[must_use]
    pub fn shard_loads(&self) -> Vec<usize> {
        self.shards
            .iter()
            .map(|shard| {
                shard
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len()
            })
            .collect()
    }
}

impl<K: Hash + Eq + Clone, V: Clone> Default for ShardedMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn inserts_reads_and_removes() {
        let map: ShardedMap<String, u32> = ShardedMap::new();
        for i in 0..1000u32 {
            assert_eq!(map.insert(format!("key{i}"), i), None);
        }
        assert_eq!(map.len(), 1000);
        for i in 0..1000u32 {
            assert_eq!(map.get(&format!("key{i}")), Some(i));
        }
        assert_eq!(map.remove(&"key500".to_string()), Some(500));
        assert_eq!(map.get(&"key500".to_string()), None);
        assert_eq!(map.len(), 999);
        assert!(!map.is_empty());
    }

    #[test]
    fn defaults_to_4096_shards() {
        let map: ShardedMap<u64, u64> = ShardedMap::new();
        assert_eq!(map.shard_count(), DEFAULT_SHARDS);
        assert_eq!(DEFAULT_SHARDS, 4096);
    }

    #[test]
    fn shard_of_is_stable_for_a_fixed_seed() {
        let a: ShardedMap<String, ()> = ShardedMap::with_seed(4096, 0xdead_beef);
        let b: ShardedMap<String, ()> = ShardedMap::with_seed(4096, 0xdead_beef);
        for i in 0..500 {
            let key = format!("path/to/file{i}.rs");
            assert_eq!(a.shard_of(&key), b.shard_of(&key));
        }
    }

    #[test]
    fn shard_of_routes_on_the_high_hash_bits() {
        // The routing is `(hash >> 32) & mask`; verify it against a direct
        // computation with the same seed and hasher.
        let map: ShardedMap<String, ()> = ShardedMap::with_seed(64, 12345);
        for i in 0..200 {
            let key = format!("k{i}");
            let expected = {
                let mut h = Th64Hasher {
                    seed: 12345,
                    bytes: Vec::new(),
                };
                key.hash(&mut h);
                ((h.finish() >> 32) as usize) & 63
            };
            assert_eq!(map.shard_of(&key), expected);
        }
    }

    #[test]
    fn keys_spread_across_shards() {
        let map: ShardedMap<u64, u64> = ShardedMap::with_seed(DEFAULT_SHARDS, 0x5eed);
        for i in 0..20_000u64 {
            map.insert(i, i);
        }
        let loads = map.shard_loads();
        let non_empty = loads.iter().filter(|n| **n > 0).count();
        // 20k keys over 4096 shards: the overwhelming majority of shards must
        // be touched (a bad router would pile into a handful).
        assert!(
            non_empty > DEFAULT_SHARDS * 9 / 10,
            "only {non_empty} of {DEFAULT_SHARDS} shards were used"
        );
        // And no single shard may hold an outsized share.
        let max = loads.iter().copied().max().unwrap_or(0);
        assert!(max < 40, "worst shard held {max} of 20000 keys");
    }

    #[test]
    fn concurrent_writers_do_not_lose_updates() {
        let map: Arc<ShardedMap<u64, u64>> = Arc::new(ShardedMap::new());
        let threads: Vec<_> = (0..8u64)
            .map(|t| {
                let map = Arc::clone(&map);
                thread::spawn(move || {
                    for i in 0..2000u64 {
                        let key = t * 2000 + i;
                        map.insert(key, key);
                    }
                })
            })
            .collect();
        for handle in threads {
            handle.join().expect("writer thread");
        }
        assert_eq!(map.len(), 16_000);
        for t in 0..8u64 {
            for i in 0..2000u64 {
                let key = t * 2000 + i;
                assert_eq!(map.get(&key), Some(key));
            }
        }
    }

    #[test]
    fn concurrent_readers_and_writers_agree() {
        let map: Arc<ShardedMap<u64, u64>> = Arc::new(ShardedMap::new());
        for i in 0..1000u64 {
            map.insert(i, i);
        }
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let map = Arc::clone(&map);
                thread::spawn(move || {
                    let mut hits = 0usize;
                    for round in 0..1000u64 {
                        for i in 0..1000u64 {
                            if map.get(&i) == Some(i) {
                                hits += 1;
                            }
                        }
                        let _ = round;
                    }
                    hits
                })
            })
            .collect();
        let writers: Vec<_> = (0..2)
            .map(|t| {
                let map = Arc::clone(&map);
                thread::spawn(move || {
                    for i in 1000..2000u64 {
                        let key = i + t * 1000;
                        map.insert(key, key);
                    }
                })
            })
            .collect();
        for handle in readers {
            let hits = handle.join().expect("reader thread");
            assert_eq!(hits, 1_000_000, "reads never observed a wrong value");
        }
        for handle in writers {
            handle.join().expect("writer thread");
        }
    }

    #[test]
    fn for_each_shard_sees_every_entry_exactly_once() {
        let map: ShardedMap<u64, u64> = ShardedMap::new();
        for i in 0..5000u64 {
            map.insert(i, i);
        }
        let mut total = 0usize;
        let mut seen = std::collections::HashSet::new();
        map.for_each_shard(|_, shard| {
            total += shard.len();
            seen.extend(shard.keys().copied());
        });
        assert_eq!(total, 5000);
        assert_eq!(seen.len(), 5000);
    }
}
