// MIRROR OF: crates/pa-mem/src/ptr48.rs (SixBytePtr encode/decode + clip_hash + raw_pack/raw_unpack). Update together with the source.
// Trusted surface: external_body wrappers below — minimal by design.
use vstd::prelude::*;

verus! {

pub const MAX_INDEX: u64 = 0xFFFF_FFFF_FFFF;

pub open spec fn max_index() -> u64 {
    0xFFFF_FFFF_FFFF
}

// Fixed-width little-endian value, concrete coefficients — no recursion, no pow2.
pub open spec fn le6(b: Seq<u8>) -> int {
    b[0] as int + b[1] as int * 256 + b[2] as int * 65536 + b[3] as int * 16777216
        + b[4] as int * 4294967296 + b[5] as int * 1099511627776
}

pub open spec fn le8(b: Seq<u8>) -> int {
    b[0] as int + b[1] as int * 256 + b[2] as int * 65536 + b[3] as int * 16777216
        + b[4] as int * 4294967296 + b[5] as int * 1099511627776
        + b[6] as int * 281474976710656 + b[7] as int * 72057594037927936
}

// vstd has no spec for std byte conversions — trusted wrappers (external_body:
// Verus assumes the ensures without checking the body; the body is compiler-checked
// Rust only). This IS the declared trust surface of the pilot.
#[verifier::external_body]
pub fn u64_from_le(b: [u8; 8]) -> (out: u64)
    ensures
        out == le8(Seq::new(8, |i: int| b[i])) as u64,
{
    u64::from_le_bytes(b)
}

#[verifier::external_body]
pub fn u64_to_le(v: u64) -> (out: [u8; 8])
    ensures
        le8(Seq::new(8, |i: int| out[i])) == v as int,
{
    v.to_le_bytes()
}

/// The packed pointer, copied from pa-mem SixBytePtr (logic only).
pub struct SixBytePtr {
    pub idx: u64,
}

pub open spec fn from_bytes_fn(bytes: [u8; 6]) -> u64 {
    le6(Seq::new(6, |i: int| bytes[i])) as u64
}

pub fn to_bytes(p: &SixBytePtr) -> (out: [u8; 6])
    requires
        // pa-mem invariant: SixBytePtr exists only via pack(), which rejects > 2^48-1
        p.idx <= max_index(),
    ensures
        // decode ∘ encode == id (value level)
        from_bytes_fn(out) == p.idx,
{
    let bytes = u64_to_le(p.idx);
    let out = [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]];
    proof {
        let b8 = Seq::new(8, |i: int| bytes[i]);
        let b6 = Seq::new(6, |i: int| out[i]);
        // le8 over (6 payload bytes + two zero high bytes) == le6 over the payload
        assert(b8[6] == 0 && b8[7] == 0);
        assert(le8(b8) == p.idx as int);
        // v < 2^48 forces the two high bytes to zero
        assert(bytes[6] as int * 281474976710656 <= le8(b8) && bytes[7] as int * 72057594037927936 <= le8(b8));
        assert(bytes[6] == 0 && bytes[7] == 0);
        assert(b8[6] == 0 && b8[7] == 0);
        assert(le8(b8) == le6(b6)) by (compute);
        assert(le6(b6) == from_bytes_fn(out));
    }
    out
}

pub fn from_bytes(bytes: [u8; 6]) -> (out: u64)
    ensures
        out <= max_index(),
        out == from_bytes_fn(bytes),
        // NOTE: byte-level roundtrip to_bytes(from_bytes(b)) == b additionally
        // requires an injectivity lemma (uniqueness of the 6-byte encoding) —
        // measured pilot cost of going further, deferred.
{
    let out = u64_from_le([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], 0, 0,
    ]);
    proof {
        let b8 = Seq::new(8, |i: int| [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], 0u8, 0u8][i]);
        let b6 = Seq::new(6, |i: int| bytes[i]);
        // le8 == le6 when the high bytes are zero
        assert(b8[6] == 0 && b8[7] == 0);
        assert(b8.subrange(0, 6) =~= b6);
        assert(le8(b8) == le6(b6)) by (compute);
        assert(out == le8(b8));
        // each byte <= 255, so the value stays under 2^48
        assert(forall|i: int| 0 <= i < 6 ==> bytes[i] as u64 <= 255u64);
        assert(bytes[5] as int * 1099511627776 <= 255 * 1099511627776);
        assert(255 * 1099511627776 + 1099511627775 <= 0xFFFF_FFFF_FFFF) by (compute);
    }
    out
}

fn main() {}

}
