//! Fast hashing primitives lifted from pogocache's `util.c`.
//!
//! [`th64`] (tidwall/th64) is the per-key hash the cache seeds its shard and
//! bucket selection from; [`mix13`] is the Murmur-style finalizer used for
//! mixing a timestamp or an index into a well-distributed word. Neither is
//! cryptographic — a cache does not need that — but both spread short keys
//! cheaply and deterministically.

/// The 64-bit multiplier both the `th64` body and finalizer use.
const R: u64 = 0x1402_0a57_acce_d8b7;

/// `th64` — a fast, non-cryptographic 64-bit hash.
///
/// Processes 8 bytes at a time with a multiply/rotate mix, then the tail a
/// byte at a time, and finalizes with three `x ^= x >> 31; x *= R` rounds.
/// `seed` is the per-process random value pogocache picks at startup to blunt
/// hash-flooding; pass a fixed seed for reproducible shard placement.
#[must_use]
pub fn th64(data: &[u8], seed: u64) -> u64 {
    let mut h = seed;
    let (words, tail) = data.as_chunks::<8>();
    for word in words {
        let x = u64::from_le_bytes(*word).wrapping_mul(R).rotate_left(31);
        h = (h.wrapping_mul(R) ^ x).rotate_left(31);
    }
    for &byte in tail {
        h = h.wrapping_mul(R) ^ u64::from(byte);
    }
    h = h.wrapping_mul(R).wrapping_add(data.len() as u64);
    h ^= h >> 31;
    h = h.wrapping_mul(R);
    h ^= h >> 31;
    h = h.wrapping_mul(R);
    h ^= h >> 31;
    h.wrapping_mul(R)
}

/// `mix13` — the splitmix64-style finalizer pogocache uses to spread a value
/// (a timestamp, a shard index) across the 64-bit space.
#[must_use]
pub fn mix13(mut key: u64) -> u64 {
    key ^= key >> 30;
    key = key.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    key ^= key >> 27;
    key = key.wrapping_mul(0x94d0_49bb_1331_11eb);
    key ^= key >> 31;
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference vectors computed from pogocache's C `th64` (src/pogocache.c)
    // and `mix13` (src/util.c) by a line-for-line port; they pin the Rust
    // arithmetic (wrapping multiply, rotate_left(31), logical shifts).
    #[test]
    fn th64_matches_the_c_reference_vectors() {
        assert_eq!(th64(b"", 0), 0x0000_0000_0000_0000);
        assert_eq!(th64(b"a", 0), 0xbc02_fc57_c97c_3a15);
        assert_eq!(th64(b"hello", 0), 0x6741_0da9_be6b_d0c1);
        assert_eq!(th64(b"pogocache", 0x3039), 0x570f_aa20_1e6c_5104);
        assert_eq!(
            th64(b"hello world, this is a 32-byte key!!", 0xdead_beef),
            0xfa5a_b385_62c6_6d95
        );
        let ramp: Vec<u8> = (0u8..64).collect();
        assert_eq!(th64(&ramp, 1), 0x3d36_e51f_c83d_d40e);
    }

    #[test]
    fn th64_is_seed_sensitive_and_length_aware() {
        assert_ne!(th64(b"key", 0), th64(b"key", 1));
        // A trailing byte changes both the stream and the folded length.
        assert_ne!(th64(b"key", 7), th64(b"key!", 7));
    }

    #[test]
    fn mix13_matches_the_c_reference_vectors() {
        assert_eq!(mix13(0), 0);
        assert_eq!(mix13(1), 0x5692_161d_100b_05e5);
        assert_eq!(mix13(u64::MAX), 0xb4d0_55fc_f2cb_bd7b);
        assert_eq!(mix13(0x0123_4567_89ab_cdef), 0xb2c0_58e4_ebb5_112c);
    }

    #[test]
    fn mix13_spreads_neighbouring_inputs() {
        // Adjacent timestamps must not land in adjacent buckets.
        let a = mix13(1_000_000);
        let b = mix13(1_000_001);
        assert_ne!(a >> 32, b >> 32);
    }
}
