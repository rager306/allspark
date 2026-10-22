// MIRROR OF: crates/pa-mem/src/ptr48.rs (Bucket layout: set_ptr/set_hash non-overlap, empty, dib). Update together with the source.
// Trusted surface: external_body wrappers below — minimal by design.
use vstd::prelude::*;

verus! {

// pa-mem Bucket layout semantics: 6-byte ptr + 3-byte hash + 1-byte dib, packed.
// The Rust struct pins the size with a static_assert; these specs pin the ACCESSOR
// semantics — the property byte-offset bugs actually break: writing one field
// preserves the others.

pub const PTR_BYTES: usize = 6;
pub const HASH_BYTES: usize = 3;
pub const BUCKET_BYTES: usize = PTR_BYTES + HASH_BYTES + 1;

pub open spec fn le6(b: Seq<u8>) -> int {
    b[0] as int + b[1] as int * 256 + b[2] as int * 65536 + b[3] as int * 16777216
        + b[4] as int * 4294967296 + b[5] as int * 1099511627776
}

pub open spec fn le3(b: Seq<u8>) -> int {
    b[0] as int + b[1] as int * 256 + b[2] as int * 65536
}

/// The bucket as ten raw bytes, exactly pogocache's `struct bucket`.
pub struct BucketBytes {
    pub b: [u8; 10],
}

pub open spec fn ptr_of(b: &BucketBytes) -> int {
    le6(Seq::new(6, |i: int| b.b[i]))
}

pub open spec fn hash_of(b: &BucketBytes) -> int {
    le3(Seq::new(3, |i: int| b.b[6 + i]))
}

pub open spec fn dib_of(b: &BucketBytes) -> u8 {
    b.b[9]
}

/// Write the pointer field; hash and dib are preserved byte-for-byte.
pub fn set_ptr(b: &mut BucketBytes, idx: u64)
    requires
        idx < 281474976710656,  // pa-mem: pack rejects > 2^48-1
    ensures
        ptr_of(final(b)) == idx as int,
        // non-overlap: the other two fields are untouched
        hash_of(final(b)) == hash_of(&old(b)),
        dib_of(final(b)) == dib_of(&old(b)),
{
    let old = b.b;
    let src = u64_to_le_bytes6(idx);
    b.b[0] = src[0];
    b.b[1] = src[1];
    b.b[2] = src[2];
    b.b[3] = src[3];
    b.b[4] = src[4];
    b.b[5] = src[5];
    proof {
        let nb = Seq::new(6, |i: int| b.b[i]);
        assert(forall|i: int| 0 <= i < 6 ==> nb[i] == src[i]);
        assert(nb =~= Seq::new(6, |i: int| src[i]));
        let h_old = Seq::new(3, |i: int| old[6 + i]);
        let h_new = Seq::new(3, |i: int| b.b[6 + i]);
        assert(h_new =~= h_old);
        assert(b.b[9] == old[9]);
    }
}

/// Write the hash prefix; pointer and dib are preserved byte-for-byte.
pub fn set_hash(b: &mut BucketBytes, h: [u8; 3])
    ensures
        hash_of(final(b)) == le3(Seq::new(3, |i: int| h[i])),
        ptr_of(final(b)) == ptr_of(&old(b)),
        dib_of(final(b)) == dib_of(&old(b)),
{
    let old = b.b;
    b.b[6] = h[0];
    b.b[7] = h[1];
    b.b[8] = h[2];
    proof {
        let p_old = Seq::new(6, |i: int| old[i]);
        let p_new = Seq::new(6, |i: int| b.b[i]);
        assert(p_new =~= p_old);
        let h_new = Seq::new(3, |i: int| b.b[6 + i]);
        assert(forall|i: int| 0 <= i < 3 ==> h_new[i] == h[i]);
        assert(h_new =~= Seq::new(3, |i: int| h[i]));
        assert(b.b[9] == old[9]);
    }
}

/// The empty bucket: `dib == 0` marks an empty slot; both fields decode to 0.
pub fn empty() -> (out: BucketBytes)
    ensures
        dib_of(&out) == 0,
        ptr_of(&out) == 0,
        hash_of(&out) == 0,
{
    BucketBytes { b: [0; 10] }
}

// trusted wrapper — the declared trust surface (same as pilots 1-2)
#[verifier::external_body]
pub fn u64_to_le_bytes6(v: u64) -> (out: [u8; 6])
    ensures
        le6(Seq::new(6, |i: int| out[i])) == v as int,
{
    let b = v.to_le_bytes();
    [b[0], b[1], b[2], b[3], b[4], b[5]]
}

fn main() {}

}
