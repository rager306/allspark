//! P3 — 48-bit (6-byte) pointers and the 10-byte bucket.
//!
//! x86-64 and ARM64 use only the low 48 bits of a virtual address, so
//! pogocache stores an entry pointer in 6 bytes instead of 8 and its hash in 3
//! (`PTRSIZE 6`, `HASHSIZE 3`), shrinking a bucket from 12 to 10 bytes:
//!
//! ```text
//! struct bucket { uint8_t entry[6]; uint8_t hash[3]; uint8_t dib; };
//! static_assert(sizeof(struct bucket) == PTRSIZE + HASHSIZE + 1);
//! ```
//!
//! We cannot (and must not) dereference a raw address, so the 48-bit field
//! holds an *arena index* instead: the slot number an [`crate::entry::EntryArena`]
//! handed out. Packing and unpacking are the same shift/mask arithmetic the C
//! code does — `load_ptr`/`store_ptr` from `pogocache.c` — with no `unsafe`
//! anywhere, and every packed index is bounds-checked against `2^48`.

use crate::error::{MemError, MemResult};

/// Bytes in a packed 48-bit pointer field.
pub const PTR_BYTES: usize = 6;
/// Bits in a packed pointer field.
pub const PTR_BITS: u32 = 48;
/// The largest index a 6-byte pointer can hold (`2^48 - 1`).
pub const MAX_INDEX: u64 = (1u64 << PTR_BITS) - 1;
/// Bytes in the per-bucket hash prefix.
pub const HASH_BYTES: usize = 3;
/// Bits in the per-bucket hash prefix (`HASHSIZE 3` in pogocache).
pub const HASH_BITS: u32 = 24;
/// The mask selecting the 24-bit hash prefix.
pub const HASH_MASK: u32 = (1u32 << HASH_BITS) - 1;
/// Bytes in a packed bucket: 6-byte pointer + 3-byte hash + 1-byte `dib`.
pub const BUCKET_BYTES: usize = PTR_BYTES + HASH_BYTES + 1;

/// A 48-bit arena index packed into 6 bytes.
///
/// The safe analogue of pogocache's `load_ptr`/`store_ptr`: instead of a
/// machine address it carries the index of a slot inside an arena, which is
/// all the layout needs and all that can be checked without `unsafe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SixBytePtr(u64);

impl SixBytePtr {
    /// Packs `index`, rejecting anything that does not fit 48 bits.
    ///
    /// # Errors
    ///
    /// [`MemError::PtrOverflow`] when `index > MAX_INDEX`.
    pub fn pack(index: u64) -> MemResult<Self> {
        if index > MAX_INDEX {
            return Err(MemError::PtrOverflow {
                index,
                max: MAX_INDEX,
            });
        }
        Ok(Self(index))
    }

    /// The unpacked index.
    #[must_use]
    pub const fn index(self) -> u64 {
        self.0
    }

    /// The little-endian 6-byte encoding.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; PTR_BYTES] {
        let bytes = self.0.to_le_bytes();
        [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]
    }

    /// Decodes a 6-byte little-endian pointer.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; PTR_BYTES]) -> Self {
        Self(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], 0, 0,
        ]))
    }

    /// Decodes a 6-byte pointer straight to its index — the `load_ptr` half.
    #[must_use]
    pub fn unpack(bytes: [u8; PTR_BYTES]) -> u64 {
        Self::from_bytes(bytes).index()
    }

    /// Packs `index`, silently masking it to 48 bits.
    ///
    /// The unchecked variant pogocache's `store_ptr` performs: it truncates
    /// rather than erroring. Prefer [`SixBytePtr::pack`] unless the caller has
    /// already proven the bound.
    #[must_use]
    pub const fn raw_pack(index: u64) -> [u8; PTR_BYTES] {
        Self(index & MAX_INDEX).to_bytes()
    }

    /// Decodes a 6-byte pointer, masking the result to 48 bits.
    #[must_use]
    pub const fn raw_unpack(bytes: [u8; PTR_BYTES]) -> u64 {
        Self::from_bytes(bytes).index() & MAX_INDEX
    }
}

/// Clips a 64-bit hash to the 3-byte (24-bit) prefix a bucket stores.
#[must_use]
pub fn clip_hash(hash: u64) -> [u8; HASH_BYTES] {
    let clipped = (hash as u32) & HASH_MASK;
    [clipped as u8, (clipped >> 8) as u8, (clipped >> 16) as u8]
}

/// One open-addressing slot, laid out exactly as pogocache's `struct bucket`.
///
/// `dib` is the *distance to bucket*: the number of probes the entry has been
/// displaced from its home slot, with `0` meaning empty. The `#[repr(C)]` plus
/// the compile-time assertion below pin the size at
/// [`BUCKET_BYTES`] = 10 — the whole reason the 6-byte pointer exists.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bucket {
    /// The 48-bit arena index, little-endian.
    pub ptr: [u8; PTR_BYTES],
    /// The 24-bit hash prefix, little-endian.
    pub hash: [u8; HASH_BYTES],
    /// Distance to bucket; `0` marks an empty slot.
    pub dib: u8,
}

// The layout invariant pogocache states with `static_assert`: 6 + 3 + 1 = 10.
const _: () = assert!(
    std::mem::size_of::<Bucket>() == BUCKET_BYTES,
    "a packed bucket must be exactly 10 bytes"
);

impl Bucket {
    /// The empty bucket (`dib == 0`).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ptr: [0; PTR_BYTES],
            hash: [0; HASH_BYTES],
            dib: 0,
        }
    }

    /// Whether the slot is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.dib == 0
    }

    /// Builds a bucket from an index, hash, and `dib`.
    ///
    /// # Errors
    ///
    /// [`MemError::PtrOverflow`] when `index` does not fit 48 bits.
    pub fn with(index: u64, hash: u64, dib: u8) -> MemResult<Self> {
        Ok(Self {
            ptr: SixBytePtr::pack(index)?.to_bytes(),
            hash: clip_hash(hash),
            dib,
        })
    }

    /// Overwrites the packed index.
    ///
    /// # Errors
    ///
    /// [`MemError::PtrOverflow`] when `index` does not fit 48 bits.
    pub fn set_index(&mut self, index: u64) -> MemResult<()> {
        self.ptr = SixBytePtr::pack(index)?.to_bytes();
        Ok(())
    }

    /// Overwrites the 24-bit hash prefix.
    pub fn set_hash(&mut self, hash: u64) {
        self.hash = clip_hash(hash);
    }

    /// The unpacked arena index.
    #[must_use]
    pub fn index(self) -> u64 {
        SixBytePtr::from_bytes(self.ptr).index()
    }

    /// The unpacked 24-bit hash prefix.
    #[must_use]
    pub fn hash24(self) -> u32 {
        u32::from_le_bytes([self.hash[0], self.hash[1], self.hash[2], 0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_is_ten_bytes() {
        assert_eq!(std::mem::size_of::<Bucket>(), 10);
        assert_eq!(BUCKET_BYTES, 10);
    }

    #[test]
    fn pointers_round_trip_across_the_range() {
        for index in [0u64, 1, 255, 256, 65_535, 0x00ff_ffff, MAX_INDEX] {
            let packed = SixBytePtr::pack(index).expect("in range");
            assert_eq!(packed.index(), index);
            assert_eq!(SixBytePtr::unpack(packed.to_bytes()), index);
        }
    }

    #[test]
    fn packing_rejects_an_index_above_48_bits() {
        let err = SixBytePtr::pack(MAX_INDEX + 1).expect_err("out of range");
        assert_eq!(
            err,
            MemError::PtrOverflow {
                index: MAX_INDEX + 1,
                max: MAX_INDEX,
            }
        );
    }

    #[test]
    fn the_raw_variants_truncate_instead_of_erroring() {
        let masked = SixBytePtr::raw_pack(MAX_INDEX + 1);
        assert_eq!(SixBytePtr::raw_unpack(masked), 0, "2^48 wraps to zero");
    }

    #[test]
    fn encoding_is_little_endian() {
        let packed = SixBytePtr::pack(0x0000_0102_0304_0506).expect("in range");
        assert_eq!(packed.to_bytes(), [0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn hash_clips_to_24_bits() {
        assert_eq!(clip_hash(0xdead_beef_cafe_babe), [0xbe, 0xba, 0xfe]);
        let bucket = Bucket::with(7, 0xdead_beef_cafe_babe, 3).expect("in range");
        assert_eq!(bucket.hash24(), 0x00fe_babe);
        assert_eq!(bucket.index(), 7);
        assert_eq!(bucket.dib, 3);
    }

    #[test]
    fn an_empty_bucket_has_dib_zero() {
        let empty = Bucket::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.index(), 0);
        assert_eq!(empty, Bucket::default());
    }
}
