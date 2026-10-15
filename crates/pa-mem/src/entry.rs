//! P4 — one allocation per entry, a minimal size prefix, and a bitfield header.
//!
//! pogocache packs an entire cache entry — key, value, expiry, flags, and the
//! length of its own allocation — into a single heap block:
//!
//! ```c
//! struct entry {
//!     int64_t time;           // timestamp
//!     atomic_int rc;          // reference counter
//!     unsigned memszsz:2;     // size-prefix width: 0=1, 1=2, 2=4, 3=8 bytes
//!     unsigned has_expires:1;
//!     unsigned has_flags:1;
//!     unsigned has_sixpack:1;
//!     uint8_t data[];         // size-prefix, then the optional fields, then key+value
//! };
//! ```
//!
//! One block means no per-field allocations and no fragmentation, the
//! `memszsz` width keeps the self-describing length as small as the entry
//! allows, and the three flags cost one byte total. There is no raw pointer to
//! hand out here, so [`EntryRef`] is an `(offset, len)` pair into an
//! [`EntryArena`]'s byte buffer — the arena *is* the allocator, and every read
//! is bounds-checked.
//!
//! The `has_sixpack` bit is carried through ([`EntryOpts::sixpacked`] in,
//! [`EntryView::sixpacked`] out) so callers that store sixpack-compressed keys
//! can record the fact; this crate does not itself re-encode keys.

use std::collections::BTreeMap;

use crate::error::{MemError, MemResult};
use crate::varint;

/// The width of the size prefix, in bytes: `1 << shift`.
///
/// pogocache stores this as `memszsz:2`, so a 2-bit field covers 1, 2, 4, and
/// 8 bytes. The smallest width that holds the entry's total length is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizePrefix {
    /// One byte (lengths 0..=255). The default prefix.
    #[default]
    Byte1,
    /// Two bytes (lengths 0..=65535).
    Byte2,
    /// Four bytes.
    Byte4,
    /// Eight bytes.
    Byte8,
}

impl SizePrefix {
    /// The prefix width in bytes.
    #[must_use]
    pub const fn width(self) -> usize {
        1 << self.shift()
    }

    /// The `memszsz` field value (`0..=3`).
    #[must_use]
    pub const fn shift(self) -> u8 {
        match self {
            Self::Byte1 => 0,
            Self::Byte2 => 1,
            Self::Byte4 => 2,
            Self::Byte8 => 3,
        }
    }

    /// The smallest prefix that holds `memsize`.
    #[must_use]
    pub const fn for_size(memsize: usize) -> Self {
        if memsize <= u8::MAX as usize {
            Self::Byte1
        } else if memsize <= u16::MAX as usize {
            Self::Byte2
        } else if memsize <= u32::MAX as usize {
            Self::Byte4
        } else {
            Self::Byte8
        }
    }

    /// Decodes the `memszsz` field value.
    #[must_use]
    pub const fn from_shift(shift: u8) -> Self {
        match shift & 0b11 {
            0 => Self::Byte1,
            1 => Self::Byte2,
            2 => Self::Byte4,
            _ => Self::Byte8,
        }
    }

    /// Writes `value` into the first `width()` bytes of `out`, little-endian.
    fn write(self, out: &mut Vec<u8>, value: usize) {
        let bytes = (value as u64).to_le_bytes();
        out.extend_from_slice(&bytes[..self.width()]);
    }

    /// Reads the little-endian value from the front of `data`.
    fn read(self, data: &[u8]) -> MemResult<usize> {
        let width = self.width();
        if data.len() < width {
            return Err(MemError::EntryTruncated {
                offset: 0,
                need: width,
                have: data.len(),
            });
        }
        let mut bytes = [0u8; 8];
        bytes[..width].copy_from_slice(&data[..width]);
        Ok(u64::from_le_bytes(bytes) as usize)
    }
}

/// The bitfield header byte, laid out like pogocache's packed `unsigned`.
///
/// `memszsz` occupies the low two bits; `has_expires`, `has_flags`, and
/// `has_sixpack` are the next three. One byte carries all five.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Header {
    size_prefix: SizePrefix,
    has_expires: bool,
    has_flags: bool,
    has_sixpack: bool,
}

impl Header {
    const HAS_EXPIRES: u8 = 1 << 2;
    const HAS_FLAGS: u8 = 1 << 3;
    const HAS_SIXPACK: u8 = 1 << 4;

    fn encode(self) -> u8 {
        let mut byte = self.size_prefix.shift() & 0b11;
        if self.has_expires {
            byte |= Self::HAS_EXPIRES;
        }
        if self.has_flags {
            byte |= Self::HAS_FLAGS;
        }
        if self.has_sixpack {
            byte |= Self::HAS_SIXPACK;
        }
        byte
    }

    fn decode(byte: u8) -> Self {
        Self {
            size_prefix: SizePrefix::from_shift(byte & 0b11),
            has_expires: byte & Self::HAS_EXPIRES != 0,
            has_flags: byte & Self::HAS_FLAGS != 0,
            has_sixpack: byte & Self::HAS_SIXPACK != 0,
        }
    }
}

/// The optional fields of an entry, mirroring pogocache's `has_expires` /
/// `has_flags` bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntryOpts {
    /// Monotonic timestamp (nanoseconds) the entry was written at.
    pub time: i64,
    /// Absolute expiry (0 = never), stored as 8 bytes only when `Some`.
    pub expires: Option<i64>,
    /// User flags, stored as 4 bytes only when `Some`.
    pub flags: Option<u32>,
    /// Whether the key is sixpack-encoded (the caller encodes it).
    pub sixpacked: bool,
}

impl EntryOpts {
    /// Options with just a timestamp.
    #[must_use]
    pub const fn at(time: i64) -> Self {
        Self {
            time,
            expires: None,
            flags: None,
            sixpacked: false,
        }
    }
}

/// A handle to one entry blob inside an [`EntryArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryRef {
    offset: usize,
    len: usize,
}

impl EntryRef {
    /// The byte offset of the blob in the arena.
    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    /// The blob's total length in bytes.
    #[must_use]
    pub const fn len(self) -> usize {
        self.len
    }

    /// Whether the blob is empty (never true for a real entry).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// A decoded view of an entry: borrowed slices into the arena buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryView<'a> {
    /// The entry's timestamp.
    pub time: i64,
    /// The absolute expiry, if the entry carries one.
    pub expires: Option<i64>,
    /// The user flags, if the entry carries them.
    pub flags: Option<u32>,
    /// Whether the stored key is sixpack-encoded.
    pub sixpacked: bool,
    /// The stored key bytes (still packed when `sixpacked`).
    pub key: &'a [u8],
    /// The value bytes.
    pub value: &'a [u8],
    /// The width of the size prefix that was used.
    pub prefix: SizePrefix,
}

/// A byte-buffer arena holding one contiguous blob per entry.
///
/// [`EntryArena::alloc`] lays the entry out in the buffer and hands back an
/// [`EntryRef`]; [`EntryArena::get`] decodes it; [`EntryArena::free`] returns
/// the block to a free list for reuse. The arena never hands out a raw
/// pointer, so the whole thing stays inside `#![forbid(unsafe_code)]`.
#[derive(Debug, Default)]
pub struct EntryArena {
    buf: Vec<u8>,
    /// Live blobs by offset: their length plus the timestamp, which pogocache
    /// keeps in the fixed `struct entry` header (before the flexible `data[]`)
    /// rather than in the packed bytes — so it lives here, not in the blob.
    live: BTreeMap<usize, LiveEntry>,
    /// Freed blocks `(offset, len)`, best-fit reused on the next alloc.
    free: Vec<(usize, usize)>,
}

/// The fixed-size part of a live entry, held beside the packed blob.
#[derive(Debug, Clone, Copy)]
struct LiveEntry {
    len: usize,
    time: i64,
}

impl EntryArena {
    /// An empty arena.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes currently reserved (live blobs plus the high-water mark of freed
    /// space that has not been reused).
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Number of live entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Whether the arena holds no live entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Bytes of reusable (freed) space.
    #[must_use]
    pub fn free_bytes(&self) -> usize {
        self.free.iter().map(|(_, len)| len).sum()
    }

    /// Encodes `key`/`value` and their optional fields into one blob.
    ///
    /// # Errors
    ///
    /// Returns [`MemError::PtrOverflow`] if the blob would not fit a 48-bit
    /// offset (an arena past 256 TiB — a guard, not a practical limit).
    pub fn alloc(&mut self, opts: EntryOpts, key: &[u8], value: &[u8]) -> MemResult<EntryRef> {
        let blob = Self::encode(opts, key, value);
        let len = blob.len();
        // Best-fit reuse: the smallest freed block that still fits.
        let reuse = self
            .free
            .iter()
            .enumerate()
            .filter(|(_, (_, block_len))| *block_len >= len)
            .min_by_key(|(_, (_, block_len))| *block_len)
            .map(|(i, _)| i);
        let offset = if let Some(index) = reuse {
            let (offset, block_len) = self.free.swap_remove(index);
            self.buf[offset..offset + len].copy_from_slice(&blob);
            // A partially-used block keeps its remainder on the free list.
            if block_len > len {
                self.free.push((offset + len, block_len - len));
            }
            offset
        } else {
            let offset = self.buf.len();
            if (offset + len) as u64 > crate::ptr48::MAX_INDEX {
                return Err(MemError::PtrOverflow {
                    index: (offset + len) as u64,
                    max: crate::ptr48::MAX_INDEX,
                });
            }
            self.buf.extend_from_slice(&blob);
            offset
        };
        self.live.insert(
            offset,
            LiveEntry {
                len,
                time: opts.time,
            },
        );
        Ok(EntryRef { offset, len })
    }

    /// Decodes the entry `reference` points at.
    ///
    /// # Errors
    ///
    /// [`MemError::EntryTruncated`] when the handle is stale (the blob was
    /// freed) or its bytes do not decode; every read is bounds-checked.
    pub fn get(&self, reference: EntryRef) -> MemResult<EntryView<'_>> {
        let live = self
            .live
            .get(&reference.offset)
            .copied()
            .filter(|live| live.len == reference.len);
        let Some(live) = live else {
            return Err(MemError::EntryTruncated {
                offset: reference.offset,
                need: reference.len,
                have: 0,
            });
        };
        let mut view = Self::decode(&self.buf[reference.offset..reference.offset + reference.len])?;
        view.time = live.time;
        Ok(view)
    }

    /// Frees `reference`, returning its block to the reuse list.
    ///
    /// Returns `false` for a stale handle (already freed, or never handed out).
    pub fn free(&mut self, reference: EntryRef) -> bool {
        match self.live.remove(&reference.offset) {
            Some(live) if live.len == reference.len => {}
            _ => return false,
        }
        self.free.push((reference.offset, reference.len));
        true
    }

    /// The encoded blob for one entry.
    fn encode(opts: EntryOpts, key: &[u8], value: &[u8]) -> Vec<u8> {
        // Everything after the header byte: the size prefix, the optional
        // fields, the key length, then key and value.
        let mut body = Vec::with_capacity(key.len() + value.len() + 16);
        if let Some(expires) = opts.expires {
            body.extend_from_slice(&expires.to_le_bytes());
        }
        if let Some(flags) = opts.flags {
            body.extend_from_slice(&flags.to_le_bytes());
        }
        varint::write_u64(&mut body, key.len() as u64);
        body.extend_from_slice(key);
        body.extend_from_slice(value);

        // The size prefix's width depends on the total length, which includes
        // the prefix itself — resolve the fixed point by growing once if the
        // chosen width turns out to be one byte too small.
        let mut prefix = SizePrefix::for_size(body.len() + 1 + 1);
        loop {
            let total = 1 + prefix.width() + body.len();
            let needed = SizePrefix::for_size(total);
            if needed.width() == prefix.width() {
                let mut blob = Vec::with_capacity(total);
                blob.push(
                    Header {
                        size_prefix: prefix,
                        has_expires: opts.expires.is_some(),
                        has_flags: opts.flags.is_some(),
                        has_sixpack: opts.sixpacked,
                    }
                    .encode(),
                );
                prefix.write(&mut blob, total);
                blob.extend_from_slice(&body);
                return blob;
            }
            prefix = needed;
        }
    }

    /// Decodes a blob into its borrowed view.
    fn decode(blob: &[u8]) -> MemResult<EntryView<'_>> {
        let header_byte = *blob.first().ok_or(MemError::EntryTruncated {
            offset: 0,
            need: 1,
            have: 0,
        })?;
        let header = Header::decode(header_byte);
        let prefix = header.size_prefix;
        let body_start = 1 + prefix.width();
        if blob.len() < body_start {
            return Err(MemError::EntryTruncated {
                offset: 0,
                need: body_start,
                have: blob.len(),
            });
        }
        let memsize = prefix.read(&blob[1..])?;
        if memsize > blob.len() {
            return Err(MemError::EntryTruncated {
                offset: 0,
                need: memsize,
                have: blob.len(),
            });
        }
        let mut rest = &blob[body_start..memsize];

        let expires = if header.has_expires {
            let (value, consumed) = Self::take_i64(rest, 0)?;
            rest = &rest[consumed..];
            Some(value)
        } else {
            None
        };
        let flags = if header.has_flags {
            let (value, consumed) = Self::take_u32(rest, 0)?;
            rest = &rest[consumed..];
            Some(value)
        } else {
            None
        };
        let (key_len, key_len_bytes) = varint::read_u64(rest)?;
        let key_len = key_len as usize;
        let key_start = key_len_bytes;
        let key_end = key_start + key_len;
        if rest.len() < key_end {
            return Err(MemError::EntryTruncated {
                offset: key_start,
                need: key_end,
                have: rest.len(),
            });
        }
        let key = &rest[key_start..key_end];
        let value = &rest[key_end..];
        // `time` lives in the arena's side table (pogocache's fixed header);
        // `get` fills it in after decoding.
        Ok(EntryView {
            time: 0,
            expires,
            flags,
            sixpacked: header.has_sixpack,
            key,
            value,
            prefix,
        })
    }

    fn take_i64(data: &[u8], offset: usize) -> MemResult<(i64, usize)> {
        let bytes: [u8; 8] =
            data.get(..8)
                .and_then(|s| s.try_into().ok())
                .ok_or(MemError::EntryTruncated {
                    offset,
                    need: 8,
                    have: data.len(),
                })?;
        Ok((i64::from_le_bytes(bytes), 8))
    }

    fn take_u32(data: &[u8], offset: usize) -> MemResult<(u32, usize)> {
        let bytes: [u8; 4] =
            data.get(..4)
                .and_then(|s| s.try_into().ok())
                .ok_or(MemError::EntryTruncated {
                    offset,
                    need: 4,
                    have: data.len(),
                })?;
        Ok((u32::from_le_bytes(bytes), 4))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_bare_entry() {
        let mut arena = EntryArena::new();
        let reference = arena
            .alloc(EntryOpts::at(1_234), b"user:1", b"alice")
            .expect("alloc");
        let view = arena.get(reference).expect("get");
        assert_eq!(view.time, 1_234, "the timestamp survives the side table");
        assert_eq!(view.key, b"user:1");
        assert_eq!(view.value, b"alice");
        assert_eq!(view.expires, None);
        assert_eq!(view.flags, None);
        assert!(!view.sixpacked);
        assert_eq!(arena.len(), 1);
    }

    #[test]
    fn round_trips_every_optional_field() {
        let mut arena = EntryArena::new();
        let opts = EntryOpts {
            time: 42,
            expires: Some(1_700_000_000),
            flags: Some(0xdead_beef),
            sixpacked: true,
        };
        let reference = arena.alloc(opts, b"k", b"v").expect("alloc");
        let view = arena.get(reference).expect("get");
        assert_eq!(view.expires, Some(1_700_000_000));
        assert_eq!(view.flags, Some(0xdead_beef));
        assert!(view.sixpacked);
        assert_eq!(view.key, b"k");
        assert_eq!(view.value, b"v");
    }

    #[test]
    fn round_trips_many_entries_of_uneven_size() {
        let mut arena = EntryArena::new();
        let mut refs = Vec::new();
        for i in 0..500usize {
            let key = format!("key-{i}");
            let value = vec![b'x'; i % 300];
            let reference = arena
                .alloc(EntryOpts::at(i as i64), key.as_bytes(), &value)
                .expect("alloc");
            refs.push((reference, key, value));
        }
        for (reference, key, value) in &refs {
            let view = arena.get(*reference).expect("get");
            assert_eq!(view.key, key.as_bytes());
            assert_eq!(view.value, value.as_slice());
        }
        assert_eq!(arena.len(), 500);
    }

    #[test]
    fn the_size_prefix_uses_the_smallest_width() {
        assert_eq!(SizePrefix::for_size(255), SizePrefix::Byte1);
        assert_eq!(SizePrefix::for_size(256), SizePrefix::Byte2);
        assert_eq!(SizePrefix::for_size(65_535), SizePrefix::Byte2);
        assert_eq!(SizePrefix::for_size(65_536), SizePrefix::Byte4);
        assert_eq!(SizePrefix::for_size(u32::MAX as usize), SizePrefix::Byte4);
        assert_eq!(
            SizePrefix::for_size(u32::MAX as usize + 1),
            SizePrefix::Byte8
        );
    }

    #[test]
    fn a_small_entry_uses_a_one_byte_prefix() {
        let mut arena = EntryArena::new();
        let reference = arena
            .alloc(EntryOpts::default(), b"k", b"v")
            .expect("alloc");
        assert_eq!(arena.get(reference).expect("get").prefix, SizePrefix::Byte1);
        // header(1) + 1-byte prefix(1) + varint(1) + "k"(1) + "v"(1) = 5.
        assert_eq!(reference.len(), 5);
    }

    #[test]
    fn a_long_value_uses_a_two_byte_prefix() {
        let mut arena = EntryArena::new();
        let value = vec![0u8; 400];
        let reference = arena
            .alloc(EntryOpts::default(), b"k", &value)
            .expect("alloc");
        assert_eq!(arena.get(reference).expect("get").prefix, SizePrefix::Byte2);
        assert!(reference.len() > 255);
    }

    #[test]
    fn the_stored_length_matches_the_blob() {
        // The blob is self-describing: the size prefix must equal its own
        // length, which is the property `free`/`get` lean on.
        let mut arena = EntryArena::new();
        for len in [0usize, 1, 200, 254, 255, 256, 1000] {
            let value = vec![b'z'; len];
            let reference = arena
                .alloc(EntryOpts::default(), b"key", &value)
                .expect("alloc");
            let view = arena.get(reference).expect("get");
            assert_eq!(view.value.len(), len);
            let stored = view
                .prefix
                .read(&arena.buf[reference.offset + 1..])
                .expect("prefix");
            assert_eq!(stored, reference.len());
        }
    }

    #[test]
    fn free_returns_the_block_for_reuse() {
        let mut arena = EntryArena::new();
        let first = arena
            .alloc(EntryOpts::default(), b"a", b"1")
            .expect("alloc");
        let high_water = arena.capacity();
        assert!(arena.free(first));
        assert_eq!(arena.len(), 0);
        assert_eq!(arena.free_bytes(), first.len());
        // A same-size alloc reuses the freed block instead of growing the arena.
        let second = arena
            .alloc(EntryOpts::default(), b"b", b"2")
            .expect("alloc");
        assert_eq!(second.offset(), first.offset());
        assert_eq!(arena.capacity(), high_water, "the arena did not grow");
        assert_eq!(arena.get(second).expect("get").value, b"2");
    }

    #[test]
    fn freeing_twice_is_rejected() {
        let mut arena = EntryArena::new();
        let reference = arena
            .alloc(EntryOpts::default(), b"k", b"v")
            .expect("alloc");
        assert!(arena.free(reference));
        assert!(!arena.free(reference), "a stale handle is refused");
    }

    #[test]
    fn reading_a_freed_handle_is_an_error_not_a_panic() {
        let mut arena = EntryArena::new();
        let reference = arena
            .alloc(EntryOpts::default(), b"k", b"v")
            .expect("alloc");
        arena.free(reference);
        assert!(matches!(
            arena.get(reference),
            Err(MemError::EntryTruncated { .. })
        ));
    }

    #[test]
    fn a_corrupt_blob_is_rejected() {
        let mut arena = EntryArena::new();
        let reference = arena
            .alloc(EntryOpts::default(), b"key", b"value")
            .expect("alloc");
        // Overwrite the varint key length with a value past the end.
        arena.buf[reference.offset + 2] = 0x7f;
        assert!(matches!(
            arena.get(reference),
            Err(MemError::EntryTruncated { .. })
        ));
    }

    #[test]
    fn bitfields_pack_into_one_byte() {
        let header = Header {
            size_prefix: SizePrefix::Byte4,
            has_expires: true,
            has_flags: false,
            has_sixpack: true,
        };
        let byte = header.encode();
        assert_eq!(byte & 0b11, 2, "memszsz occupies the low two bits");
        assert_eq!(byte & Header::HAS_EXPIRES, Header::HAS_EXPIRES);
        assert_eq!(byte & Header::HAS_FLAGS, 0);
        assert_eq!(header, Header::decode(byte));
    }
}
