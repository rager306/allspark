//! P2 — Robin Hood hashing with `dib` and backward-shift deletion.
//!
//! A direct port of `map_insert` / `map_get_bucket` / `delbkt` from
//! pogocache's `pogocache.c`. The table is open-addressed with linear probing;
//! each occupied slot records its *distance to bucket* (`dib`, 1-based, `0`
//! meaning empty). On insert, a slot whose `dib` is smaller than the incoming
//! entry's is displaced — the "Robin Hood" rule that keeps probe lengths even
//! and predictable. Deletion shifts the following cluster back one slot at a
//! time until a slot at its home position is reached, so no tombstones are
//! needed.
//!
//! Keys are hashed with the crate's [`crate::hash::th64`] rather than the
//! standard library's randomly seeded `SipHash`, so placement is reproducible
//! across runs — the property the sharded map and the tests both rely on.
//!
//! The probe counter (`dib`, distance to bucket) is two bytes per slot here,
//! where pogocache packs it into one: a one-byte counter cannot record a
//! cluster deeper than 255, and backward-shift deletion needs the true
//! displacement to keep `dibs` monotone. The wider counter adds one byte per
//! slot (its own `Vec`, parallel to the buckets): `dibs` is 5.9% of a
//! `u64`/`u64` table's footprint, where it was 3.0%. The share holds at every
//! size because the buckets and `dibs` both scale with the bucket count —
//! 64 slots: 2 176 vs 2 112 bytes; 1 024: 34 816 vs 33 792; 65 536: 2 228 224
//! vs 2 162 688. An `Option<Slot<u64, u64>>` is 32 bytes, not 24: 16 of payload
//! plus a discriminant, `u64` having no niche to steal. See [`MAX_DIB`] for the
//! ceiling this buys.

use std::hash::{Hash, Hasher};

use crate::hash::th64;

/// A [`Hasher`] that accumulates bytes and finishes with [`th64`].
///
/// `th64` is a one-shot function over a byte slice, so the incremental
/// [`Hasher`] interface buffers the writes and hashes them at `finish`. The
/// seed is fixed at `0`: placement must be deterministic, not flood-resistant
/// (the caller owns that decision; see [`crate::shard::ShardedMap`]).
#[derive(Default)]
struct Th64Hasher {
    bytes: Vec<u8>,
}

impl Hasher for Th64Hasher {
    fn finish(&self) -> u64 {
        th64(&self.bytes, 0)
    }

    fn write(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
}

/// Hashes `key` with the deterministic `th64` hasher.
fn hash_of<K: Hash>(key: &K) -> u64 {
    let mut hasher = Th64Hasher::default();
    key.hash(&mut hasher);
    hasher.finish()
}

/// One occupied slot: the stored hash, key, and value.
#[derive(Debug)]
struct Slot<K, V> {
    hash: u64,
    key: K,
    value: V,
}

/// The deepest probe a `u16` `dib` can record.
///
/// pogocache packs `dib` into one byte (the 10-byte bucket in [`crate::ptr48`]
/// still does); here it is two, because a one-byte counter cannot describe a
/// cluster deeper than 255 and backward-shift deletion needs the true
/// displacement to keep `dibs` monotone along a cluster. A good hash keeps
/// probes in the low tens, and the load-factor cap keeps them there; a cluster
/// that somehow reaches this ceiling still saturates instead of overflowing.
///
/// Reaching the ceiling would take a 65 536-entry cluster, and every key in it
/// on one home slot — at least 87 000 slots at the 75% load factor, which is an
/// adversarial hash, not a workload.
pub const MAX_DIB: u16 = u16::MAX;

/// An open-addressed hash map with Robin Hood probing.
///
/// The load factor is capped at 75% (`DEFLOADFACTOR` in pogocache) and the
/// bucket count is always a power of two, so the home slot is a mask rather
/// than a modulo.
///
/// `dib` is two bytes wide, one byte more than pogocache's bucket field, so the
/// displacement a 512-bucket-sized cluster needs (hundreds) is recordable and
/// backward-shift deletion keeps working. That bounds the usable probe distance
/// at [`MAX_DIB`], which is unreachable in practice; the load-factor cap keeps
/// real probe lengths in the low tens, and the counter saturates rather than
/// wrapping on pathological input.
#[derive(Debug)]
pub struct RobinHoodMap<K, V> {
    buckets: Vec<Option<Slot<K, V>>>,
    /// `dib` per bucket, parallel to `buckets`; `0` marks an empty slot.
    dibs: Vec<u16>,
    mask: usize,
    len: usize,
}

/// The bucket count a fresh map starts with (`INITCAP` in pogocache).
const INITIAL_CAPACITY: usize = 64;

impl<K: Hash + Eq, V> Default for RobinHoodMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Hash + Eq, V> RobinHoodMap<K, V> {
    /// A map with the default initial capacity (64 buckets).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(INITIAL_CAPACITY)
    }

    /// A map with room for at least `capacity` entries before the first grow.
    ///
    /// The bucket count is rounded up to a power of two.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let count = capacity.max(INITIAL_CAPACITY).next_power_of_two();
        Self {
            buckets: std::iter::repeat_with(|| None).take(count).collect(),
            dibs: vec![0; count],
            mask: count - 1,
            len: 0,
        }
    }

    /// Number of stored entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the map holds no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The current bucket count (a power of two).
    #[must_use]
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// The longest probe distance in the table — the Robin Hood guarantee in
    /// one number. A well-mixed table keeps this small; the ceiling is
    /// [`MAX_DIB`].
    #[must_use]
    pub fn max_probe(&self) -> u16 {
        self.dibs.iter().copied().max().unwrap_or(0)
    }

    /// Inserts `key`/`value`, returning the previous value for `key` if any.
    ///
    /// # Panics
    ///
    /// Panics only on an internal invariant violation (an occupied slot the
    /// probe loop lost track of); the table is maintained by this type, so it
    /// does not fire in practice.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if (self.len + 1) * 4 >= self.buckets.len() * 3 {
            self.grow();
        }
        let hash = hash_of(&key);
        let mut carried = Slot { hash, key, value };
        let mut dib = 1u16;
        let mut i = (hash as usize) & self.mask;
        loop {
            if self.buckets[i].is_none() {
                self.dibs[i] = dib;
                self.buckets[i] = Some(carried);
                self.len += 1;
                return None;
            }
            // SAFETY-equivalent: the `is_none` branch above already returned.
            let occupant = self.buckets[i].take().expect("slot is occupied");
            if occupant.hash == carried.hash && occupant.key == carried.key {
                // Same key: the new value takes the slot, the old one is
                // returned. Keys are `Eq`, so dropping the occupant's copy is
                // lossless.
                let old_value = occupant.value;
                self.buckets[i] = Some(carried);
                return Some(old_value);
            }
            if self.dibs[i] < dib {
                // The occupant is closer to home than we are: swap. We take
                // over the slot and carry the displaced entry onward, one
                // probe further along.
                let occupant_dib = self.dibs[i];
                self.dibs[i] = dib;
                self.buckets[i] = Some(carried);
                carried = occupant;
                dib = occupant_dib;
            } else {
                self.buckets[i] = Some(occupant);
            }
            i = (i + 1) & self.mask;
            // `dib` is a `u16`: a cluster deeper than `MAX_DIB` saturates
            // rather than wrapping. With a real hash (crate::hash::th64) the
            // load-factor cap keeps probes in the low tens, so this only bites
            // on adversarially colliding keys — see the `MAX_DIB` note on the
            // type.
            dib = dib.saturating_add(1);
        }
    }

    /// Returns a reference to the value for `key`.
    ///
    /// # Panics
    ///
    /// Panics only if `get_index` named a slot that is not occupied, which the
    /// lookup contract rules out.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<&V> {
        self.get_index(key).map(|i| {
            // SAFETY-equivalent: `get_index` returns only occupied slots.
            &self.buckets[i]
                .as_ref()
                .expect("get_index returns an occupied slot")
                .value
        })
    }

    /// Whether `key` is present.
    #[must_use]
    pub fn contains_key(&self, key: &K) -> bool {
        self.get_index(key).is_some()
    }

    /// Removes `key`, returning its value if it was present.
    ///
    /// The deletion is backward-shift: the following cluster is pulled back one
    /// slot at a time until an entry sits at its home bucket, so no tombstone
    /// is left behind.
    ///
    /// # Panics
    ///
    /// Panics only if a located slot were not occupied, which `get_index`
    /// rules out.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let i = self.get_index(key)?;
        let slot = self.buckets[i].take().expect("occupied slot");
        self.dibs[i] = 0;
        self.len -= 1;
        self.backward_shift(i);
        Some(slot.value)
    }

    /// The slot index holding `key`, or `None`.
    ///
    /// Probes forward and stops as soon as the occupant's `dib` is smaller than
    /// the current probe distance: under Robin Hood ordering the key cannot
    /// have travelled further than that.
    fn get_index(&self, key: &K) -> Option<usize> {
        let hash = hash_of(key);
        let mut i = (hash as usize) & self.mask;
        let mut dib = 1u16;
        loop {
            let slot = self.buckets[i].as_ref()?;
            if self.dibs[i] < dib {
                return None;
            }
            if slot.hash == hash && slot.key == *key {
                return Some(i);
            }
            i = (i + 1) & self.mask;
            // Saturate exactly as `insert` does. A plain increment overflows past
            // `MAX_DIB`: in debug that is a panic on a *lookup*, and in release it
            // wraps to 0, which makes `dibs[i] < dib` false for every slot and turns
            // a miss into a full-table walk. A cluster deeper than `MAX_DIB` is only
            // reachable with adversarially colliding keys, but a lookup must not be
            // the thing that breaks.
            dib = dib.saturating_add(1);
        }
    }

    /// Pulls the cluster after the now-empty slot `hole` back one position.
    fn backward_shift(&mut self, hole: usize) {
        let mut hole = hole;
        loop {
            let next = (hole + 1) & self.mask;
            if self.dibs[next] <= 1 {
                // `next` is empty or at its home slot: the cluster ends here.
                self.dibs[hole] = 0;
                self.buckets[hole] = None;
                return;
            }
            self.buckets[hole] = self.buckets[next].take();
            self.dibs[hole] = self.dibs[next] - 1;
            hole = next;
        }
    }

    /// Doubles the table and reinserts every entry at its (unchanged) hash.
    fn grow(&mut self) {
        let new_cap = self.buckets.len() * 2;
        let fresh = std::iter::repeat_with(|| None).take(new_cap).collect();
        let old_buckets = std::mem::replace(&mut self.buckets, fresh);
        let old_dibs = std::mem::replace(&mut self.dibs, vec![0; new_cap]);
        self.mask = new_cap - 1;
        let old_len = self.len;
        self.len = 0;
        debug_assert_eq!(old_len, old_dibs.iter().filter(|d| **d != 0).count());
        for (slot, dib) in old_buckets.into_iter().zip(old_dibs) {
            if dib == 0 {
                continue;
            }
            if let Some(slot) = slot {
                // Reinsertion never replaces (the source had unique keys), so
                // the return is always `None`.
                let _ = self.insert(slot.key, slot.value);
            }
        }
    }

    /// Iterates over `(&key, &value)` pairs in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.buckets
            .iter()
            .filter_map(|slot| slot.as_ref().map(|s| (&s.key, &s.value)))
    }
}

impl<'a, K: Hash + Eq, V> IntoIterator for &'a RobinHoodMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Box<dyn Iterator<Item = (&'a K, &'a V)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

#[cfg(test)]
mod tests {
    /// Deleting from a deep cluster must not make other stored keys unreachable.
    ///
    /// Regression: `dib` used to be a `u8`, so a cluster deeper than 255 could not
    /// record its true displacements and `dibs` saturated at 255. `backward_shift`
    /// then set `dibs[hole] = dibs[next] - 1`, under-reporting the shifted entry's
    /// displacement, and `dibs` stopped being monotone along the cluster. Once that
    /// happened `get_index`'s early stop (`dibs[i] < dib`) fired on a slot that
    /// still held the key: 45 of 295 keys were stored and unreachable. `dib` is
    /// `u16` now, so the displacement of a cluster this size fits and the shifted
    /// `dibs` stay monotone.
    ///
    /// This is reachable through the public API alone — no unsafe, no test hooks:
    /// 300 keys sharing one home slot under `th64` at 512 buckets (58% load, so no
    /// resize), then five removals.
    ///
    /// The fixture must actually build a cluster deeper than the old one-byte
    /// ceiling, or it proves nothing: peak `dib` here is 300 (with every key on a
    /// single home slot the n-th inserted key sits n−1 steps from home, and `dib`
    /// is 1-based, so n keys record 300 — whatever the table size), which the old
    /// `u8` `dib` could not record. The new ceiling `MAX_DIB` is
    /// 65 535 — a cluster that deep needs every key on one home slot of a table
    /// with at least 87 000 slots at the 75% load factor, so the ceiling is
    /// unreachable in practice and this fixture sits far below it.
    #[test]
    fn deleting_from_a_saturated_cluster_loses_keys() {
        const COLLIDING: [u64; 300] = [
            327, 1_878, 1_967, 2_143, 2_193, 2_396, 2_952, 2_954, 3_016, 3_770, 4_650, 5_808,
            5_897, 6_244, 6_484, 7_166, 7_357, 8_022, 8_368, 8_507, 8_610, 8_925, 9_451, 9_657,
            9_931, 10_389, 10_583, 10_632, 11_321, 11_805, 12_149, 13_056, 15_163, 15_240, 15_445,
            16_853, 17_035, 17_337, 17_361, 17_817, 17_845, 17_889, 18_072, 18_701, 18_849, 19_227,
            19_701, 20_154, 21_451, 22_644, 23_568, 23_945, 24_411, 25_592, 25_717, 25_729, 26_614,
            27_144, 27_513, 27_598, 27_649, 27_684, 28_500, 29_212, 29_383, 29_678, 29_919, 30_348,
            30_699, 31_143, 31_683, 31_700, 32_016, 32_289, 34_133, 34_159, 34_322, 34_488, 34_530,
            34_943, 35_462, 35_632, 35_750, 35_771, 35_885, 36_671, 37_289, 37_365, 37_695, 38_316,
            38_431, 38_534, 39_175, 40_079, 40_229, 40_307, 41_327, 41_443, 41_764, 42_582, 42_924,
            44_915, 45_481, 45_669, 45_812, 46_455, 46_966, 46_975, 47_157, 47_159, 47_462, 47_750,
            48_751, 48_804, 49_331, 49_333, 50_383, 50_517, 51_512, 52_006, 52_624, 52_717, 53_141,
            53_478, 53_498, 53_565, 53_873, 53_921, 54_026, 54_471, 54_558, 54_777, 56_592, 56_720,
            57_955, 58_003, 58_092, 58_101, 58_201, 58_301, 58_361, 58_889, 59_123, 59_424, 60_176,
            61_071, 61_168, 61_577, 61_643, 61_762, 61_865, 61_923, 62_129, 62_249, 62_980, 64_167,
            64_176, 64_332, 64_671, 64_913, 65_071, 65_076, 66_747, 68_172, 68_282, 69_133, 69_346,
            69_581, 69_912, 70_112, 70_815, 71_449, 71_804, 72_107, 72_216, 72_572, 72_734, 73_643,
            73_829, 74_436, 74_625, 75_145, 75_270, 75_372, 75_520, 76_159, 76_957, 77_240, 77_578,
            77_768, 78_610, 79_640, 80_170, 80_823, 81_213, 81_341, 81_791, 82_097, 82_406, 83_037,
            83_329, 84_046, 84_335, 84_689, 85_010, 85_172, 85_411, 85_979, 86_874, 86_933, 87_821,
            87_945, 88_056, 88_653, 88_813, 89_197, 89_903, 89_956, 90_759, 91_117, 92_292, 92_358,
            94_640, 95_002, 95_388, 95_881, 95_942, 96_308, 96_368, 96_381, 96_384, 97_474, 97_492,
            98_371, 99_180, 99_439, 99_598, 99_777, 99_802, 99_852, 100_272, 100_424, 101_384,
            101_511, 102_041, 103_259, 103_321, 103_358, 103_694, 104_708, 104_733, 104_810,
            104_874, 105_176, 105_288, 107_491, 107_714, 109_304, 109_382, 109_459, 110_095,
            110_302, 110_567, 111_381, 111_540, 112_237, 113_002, 113_004, 113_549, 113_627,
            113_632, 114_051, 114_284, 114_302, 114_502, 114_530, 114_743, 115_121, 115_707,
            116_232, 117_551, 117_570, 117_640, 117_843, 118_953, 119_786, 119_960, 120_004,
            120_783, 120_995, 121_176, 121_307, 122_817, 123_682, 123_826, 124_241, 125_559,
            125_652, 126_464, 127_076,
        ];
        let mut m: crate::robin_hood::RobinHoodMap<u64, u64> =
            crate::robin_hood::RobinHoodMap::with_capacity(512);
        for k in COLLIDING {
            m.insert(k, k);
        }
        let peak = m.max_probe();
        for k in &COLLIDING[..5] {
            m.remove(k);
        }
        let lost: Vec<u64> = COLLIDING[5..]
            .iter()
            .copied()
            .filter(|k| m.get(k).is_none())
            .collect();
        assert!(
            lost.is_empty(),
            "{} of the remaining 295 keys are unreachable after deleting five: {:?}",
            lost.len(),
            &lost[..lost.len().min(8)]
        );
        // The property comes first so that a regression shows up as the message
        // that names it; the fixture checks run after, and hold only because the
        // property does.
        //
        // 300 keys on one home slot: `dib` is 1-based, so the last sits at
        // displacement 300 (299 probe steps past home). The old one-byte `dib`
        // saturated at 255, so this cluster is past it.
        assert_eq!(
            peak, 300,
            "the fixture must build a cluster deeper than the old u8 ceiling (255)"
        );
        assert!(
            peak < crate::robin_hood::MAX_DIB,
            "the fixture must stay below the current ceiling, MAX_DIB = {}",
            crate::robin_hood::MAX_DIB
        );
    }

    /// A lookup must not overflow the probe counter on a deep cluster.
    ///
    /// Regression: `get_index` advanced `dib` with a plain `+=` while `insert`
    /// saturated. Past the old one-byte ceiling (255) that overflowed: a panic in
    /// debug, and a wrap to 0 in release, where `dibs[i] < dib` then held for no
    /// slot and a miss degraded into a full-table walk. Both sides saturate now.
    ///
    /// These 300 keys share one home slot under `th64` at 512 buckets — found by
    /// evaluating the hash over two million candidates — so they build a
    /// 300-deep cluster without needing a different hasher. That is past the old
    /// one-byte ceiling and far below [`MAX_DIB`]; the table stays at 512 buckets
    /// because 300 entries is below the 75% load factor.
    #[test]
    fn a_lookup_survives_a_cluster_deeper_than_the_old_probe_ceiling() {
        const COLLIDING: [u64; 300] = [
            327, 1_878, 1_967, 2_143, 2_193, 2_396, 2_952, 2_954, 3_016, 3_770, 4_650, 5_808,
            5_897, 6_244, 6_484, 7_166, 7_357, 8_022, 8_368, 8_507, 8_610, 8_925, 9_451, 9_657,
            9_931, 10_389, 10_583, 10_632, 11_321, 11_805, 12_149, 13_056, 15_163, 15_240, 15_445,
            16_853, 17_035, 17_337, 17_361, 17_817, 17_845, 17_889, 18_072, 18_701, 18_849, 19_227,
            19_701, 20_154, 21_451, 22_644, 23_568, 23_945, 24_411, 25_592, 25_717, 25_729, 26_614,
            27_144, 27_513, 27_598, 27_649, 27_684, 28_500, 29_212, 29_383, 29_678, 29_919, 30_348,
            30_699, 31_143, 31_683, 31_700, 32_016, 32_289, 34_133, 34_159, 34_322, 34_488, 34_530,
            34_943, 35_462, 35_632, 35_750, 35_771, 35_885, 36_671, 37_289, 37_365, 37_695, 38_316,
            38_431, 38_534, 39_175, 40_079, 40_229, 40_307, 41_327, 41_443, 41_764, 42_582, 42_924,
            44_915, 45_481, 45_669, 45_812, 46_455, 46_966, 46_975, 47_157, 47_159, 47_462, 47_750,
            48_751, 48_804, 49_331, 49_333, 50_383, 50_517, 51_512, 52_006, 52_624, 52_717, 53_141,
            53_478, 53_498, 53_565, 53_873, 53_921, 54_026, 54_471, 54_558, 54_777, 56_592, 56_720,
            57_955, 58_003, 58_092, 58_101, 58_201, 58_301, 58_361, 58_889, 59_123, 59_424, 60_176,
            61_071, 61_168, 61_577, 61_643, 61_762, 61_865, 61_923, 62_129, 62_249, 62_980, 64_167,
            64_176, 64_332, 64_671, 64_913, 65_071, 65_076, 66_747, 68_172, 68_282, 69_133, 69_346,
            69_581, 69_912, 70_112, 70_815, 71_449, 71_804, 72_107, 72_216, 72_572, 72_734, 73_643,
            73_829, 74_436, 74_625, 75_145, 75_270, 75_372, 75_520, 76_159, 76_957, 77_240, 77_578,
            77_768, 78_610, 79_640, 80_170, 80_823, 81_213, 81_341, 81_791, 82_097, 82_406, 83_037,
            83_329, 84_046, 84_335, 84_689, 85_010, 85_172, 85_411, 85_979, 86_874, 86_933, 87_821,
            87_945, 88_056, 88_653, 88_813, 89_197, 89_903, 89_956, 90_759, 91_117, 92_292, 92_358,
            94_640, 95_002, 95_388, 95_881, 95_942, 96_308, 96_368, 96_381, 96_384, 97_474, 97_492,
            98_371, 99_180, 99_439, 99_598, 99_777, 99_802, 99_852, 100_272, 100_424, 101_384,
            101_511, 102_041, 103_259, 103_321, 103_358, 103_694, 104_708, 104_733, 104_810,
            104_874, 105_176, 105_288, 107_491, 107_714, 109_304, 109_382, 109_459, 110_095,
            110_302, 110_567, 111_381, 111_540, 112_237, 113_002, 113_004, 113_549, 113_627,
            113_632, 114_051, 114_284, 114_302, 114_502, 114_530, 114_743, 115_121, 115_707,
            116_232, 117_551, 117_570, 117_640, 117_843, 118_953, 119_786, 119_960, 120_004,
            120_783, 120_995, 121_176, 121_307, 122_817, 123_682, 123_826, 124_241, 125_559,
            125_652, 126_464, 127_076,
        ];
        let mut m: crate::robin_hood::RobinHoodMap<u64, u64> =
            crate::robin_hood::RobinHoodMap::with_capacity(512);
        for k in COLLIDING {
            m.insert(k, k);
        }
        assert_eq!(
            m.bucket_count(),
            512,
            "the fixture must not trigger a resize"
        );
        assert_eq!(
            m.max_probe(),
            300,
            "the fixture must build a cluster past the old u8 ceiling (255)"
        );
        let lost: Vec<u64> = COLLIDING
            .iter()
            .copied()
            .filter(|k| m.get(k).is_none())
            .collect();
        assert!(
            lost.is_empty(),
            "{} of 300 keys are unreachable; max_probe={}; first few: {:?}",
            lost.len(),
            m.max_probe(),
            &lost[..lost.len().min(8)]
        );
    }

    use super::*;

    #[test]
    fn inserts_and_reads_back() {
        let mut map = RobinHoodMap::new();
        for i in 0..1000u32 {
            assert_eq!(map.insert(i, i * 2), None);
        }
        assert_eq!(map.len(), 1000);
        for i in 0..1000u32 {
            assert_eq!(map.get(&i), Some(&(i * 2)));
        }
        assert_eq!(map.get(&1000), None);
    }

    #[test]
    fn replacing_a_key_returns_the_old_value() {
        let mut map = RobinHoodMap::new();
        assert_eq!(map.insert("k", 1), None);
        assert_eq!(map.insert("k", 2), Some(1));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&"k"), Some(&2));
    }

    #[test]
    fn growth_preserves_every_entry() {
        let mut map = RobinHoodMap::new();
        let n = 5000u32;
        for i in 0..n {
            map.insert(i, i);
        }
        assert!(map.bucket_count() > INITIAL_CAPACITY);
        assert_eq!(map.len() as u32, n);
        for i in 0..n {
            assert_eq!(map.get(&i), Some(&i), "entry {i} survived the grow");
        }
    }

    #[test]
    fn backward_shift_deletion_keeps_the_cluster_reachable() {
        let mut map = RobinHoodMap::new();
        // Force a long cluster: keys that hash into a narrow band.
        for i in 0..500u32 {
            map.insert(i, i);
        }
        let probe_before = map.max_probe();
        // Remove the middle of each run and confirm the survivors stay findable.
        for i in (0..500u32).step_by(2) {
            assert_eq!(map.remove(&i), Some(i));
        }
        for i in (0..500u32).step_by(2) {
            assert_eq!(map.get(&i), None, "removed key {i} is gone");
        }
        for i in (1..500u32).step_by(2) {
            assert_eq!(map.get(&i), Some(&i), "survivor {i} is still reachable");
        }
        assert_eq!(map.len(), 250);
        // Deleting must not lengthen the table's worst probe distance.
        assert!(map.max_probe() <= probe_before.max(1));
    }

    #[test]
    fn deletes_every_entry_back_to_empty() {
        let mut map = RobinHoodMap::new();
        for i in 0..777u32 {
            map.insert(i, i);
        }
        for i in 0..777u32 {
            assert_eq!(map.remove(&i), Some(i));
        }
        assert!(map.is_empty());
        assert_eq!(map.remove(&0), None);
        assert_eq!(map.max_probe(), 0);
    }

    #[test]
    fn colliding_keys_stay_correct() {
        // `Collide` hashes to a constant, so every key fights for one home slot.
        #[derive(Debug, Clone, PartialEq, Eq)]
        struct Collide(u32);
        impl Hash for Collide {
            fn hash<H: Hasher>(&self, state: &mut H) {
                state.write_u8(0);
            }
        }
        // One cluster deep enough to exercise the probe path. 200 keys on one
        // home slot leave the last at displacement 199, below even the old
        // one-byte `dib` ceiling; the ceiling itself is exercised by
        // `a_lookup_survives_a_cluster_deeper_than_the_old_probe_ceiling` and
        // `deleting_from_a_saturated_cluster_loses_keys`.
        const N: u32 = 200;
        let mut map = RobinHoodMap::new();
        for i in 0..N {
            map.insert(Collide(i), i);
        }
        assert_eq!(map.len(), N as usize);
        for i in 0..N {
            assert_eq!(map.get(&Collide(i)), Some(&i));
        }
        // With every key on one home slot the worst probe is deep by
        // construction.
        assert!(map.max_probe() >= 100, "probe was {}", map.max_probe());
        for i in (0..N).step_by(3) {
            assert_eq!(map.remove(&Collide(i)), Some(i));
        }
        for i in 0..N {
            let expected = if i % 3 == 0 { None } else { Some(&i) };
            assert_eq!(map.get(&Collide(i)), expected);
        }
    }

    #[test]
    fn iteration_visits_every_entry_once() {
        let mut map = RobinHoodMap::new();
        for i in 0..200u32 {
            map.insert(i, i);
        }
        let mut seen: Vec<u32> = map.iter().map(|(k, _)| *k).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..200u32).collect::<Vec<_>>());
    }
}
