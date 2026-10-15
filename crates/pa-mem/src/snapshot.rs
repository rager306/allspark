//! P7 — block-LZ4 snapshots with a 16-byte header and a CRC32 per block.
//!
//! A port of `save.c` from pogocache. The stream is cut into 1 MiB blocks; each
//! block is LZ4-compressed on its own and prefixed with a 16-byte header:
//!
//! ```text
//! (0-3)  'POGO' tag
//! (4-7)  crc32 of the COMPRESSED bytes
//! (8-11) length of the decompressed data
//! (12-15) length of the compressed data
//! ```
//!
//! Blocking keeps memory O(block) rather than O(stream), lets the writer stream
//! to a file, and — because each block carries its own checksum — lets a reader
//! salvage the good blocks around a corrupt one. The LZ4 codec is `lz4_flex`'s
//! pure-Rust, safe-only block codec (the C original uses `LZ4_compress_default`
//! / `LZ4_decompress_safe`); the checksum is `crc32fast`, the same IEEE
//! polynomial pogocache's hand-rolled `crc32` uses.
//!
//! This crate's file header is its own (pogocache's container is shard-oriented
//! and shard-specific), but the per-block framing is byte-for-byte the C one.

use crc32fast::Hasher as Crc32;

use crate::error::{MemError, MemResult};

/// The per-block size: 1 MiB (`BLOCKSIZE` in `save.c`).
pub const BLOCK_SIZE: usize = 1_048_576;
/// Bytes in a per-block header: 4-byte tag + 3 `u32`s.
pub const BLOCK_HEADER_LEN: usize = 16;
/// Bytes in the file header: magic + block count + total length + header CRC.
pub const FILE_HEADER_LEN: usize = 20;

/// The 4-byte tag on every block (pogocache's `"POGO"`).
pub const BLOCK_MAGIC: [u8; 4] = *b"POGO";
/// The 4-byte tag on the file header.
pub const FILE_MAGIC: [u8; 4] = *b"PMS1";

/// The 16-byte per-block header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    /// The `"POGO"` tag.
    pub magic: [u8; 4],
    /// CRC32 of the compressed payload that follows.
    pub crc: u32,
    /// Length of the block once decompressed.
    pub uncompressed_len: u32,
    /// Length of the compressed payload that follows.
    pub compressed_len: u32,
}

impl BlockHeader {
    /// The header length in bytes.
    pub const LEN: usize = BLOCK_HEADER_LEN;

    /// Encodes the header, little-endian.
    #[must_use]
    pub fn encode(self) -> [u8; BLOCK_HEADER_LEN] {
        let mut out = [0u8; BLOCK_HEADER_LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..8].copy_from_slice(&self.crc.to_le_bytes());
        out[8..12].copy_from_slice(&self.uncompressed_len.to_le_bytes());
        out[12..16].copy_from_slice(&self.compressed_len.to_le_bytes());
        out
    }

    /// Decodes a header from the front of `data`.
    ///
    /// # Errors
    ///
    /// [`MemError::SnapshotTruncated`] when fewer than 16 bytes are available;
    /// [`MemError::BadMagic`] when the tag is wrong.
    pub fn decode(data: &[u8]) -> MemResult<Self> {
        if data.len() < BLOCK_HEADER_LEN {
            return Err(MemError::SnapshotTruncated {
                offset: 0,
                need: BLOCK_HEADER_LEN,
                have: data.len(),
            });
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&data[0..4]);
        if magic != BLOCK_MAGIC {
            return Err(MemError::BadMagic {
                expected: BLOCK_MAGIC,
                found: magic,
            });
        }
        Ok(Self {
            magic,
            crc: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            uncompressed_len: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            compressed_len: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
        })
    }
}

/// The 20-byte file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHeader {
    /// The `"PMS1"` tag.
    pub magic: [u8; 4],
    /// Number of blocks that follow.
    pub block_count: u32,
    /// Total decompressed length of the whole snapshot.
    pub total_len: u64,
    /// CRC32 of the three fields above (magic included).
    pub crc: u32,
}

impl FileHeader {
    /// The header length in bytes.
    pub const LEN: usize = FILE_HEADER_LEN;

    /// The CRC32 of the header's own leading fields.
    fn checksum(magic: [u8; 4], block_count: u32, total_len: u64) -> u32 {
        let mut hasher = Crc32::new();
        hasher.update(&magic);
        hasher.update(&block_count.to_le_bytes());
        hasher.update(&total_len.to_le_bytes());
        hasher.finalize()
    }

    /// Builds a header with a freshly computed CRC.
    #[must_use]
    pub fn new(block_count: u32, total_len: u64) -> Self {
        Self {
            magic: FILE_MAGIC,
            block_count,
            total_len,
            crc: Self::checksum(FILE_MAGIC, block_count, total_len),
        }
    }

    /// Encodes the header, little-endian.
    #[must_use]
    pub fn encode(self) -> [u8; FILE_HEADER_LEN] {
        let mut out = [0u8; FILE_HEADER_LEN];
        out[0..4].copy_from_slice(&self.magic);
        out[4..8].copy_from_slice(&self.block_count.to_le_bytes());
        out[8..16].copy_from_slice(&self.total_len.to_le_bytes());
        out[16..20].copy_from_slice(&self.crc.to_le_bytes());
        out
    }

    /// Decodes and validates a file header.
    ///
    /// # Errors
    ///
    /// [`MemError::SnapshotTruncated`] on a short buffer,
    /// [`MemError::BadMagic`] on a wrong tag, and [`MemError::CrcMismatch`]
    /// when the header's own checksum does not match.
    pub fn decode(data: &[u8]) -> MemResult<Self> {
        if data.len() < FILE_HEADER_LEN {
            return Err(MemError::SnapshotTruncated {
                offset: 0,
                need: FILE_HEADER_LEN,
                have: data.len(),
            });
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&data[0..4]);
        if magic != FILE_MAGIC {
            return Err(MemError::BadMagic {
                expected: FILE_MAGIC,
                found: magic,
            });
        }
        let block_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let total_len = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);
        let crc = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
        let computed = Self::checksum(magic, block_count, total_len);
        if crc != computed {
            return Err(MemError::CrcMismatch {
                stored: crc,
                computed,
            });
        }
        Ok(Self {
            magic,
            block_count,
            total_len,
            crc,
        })
    }
}

/// A block-oriented LZ4 snapshot codec.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockSnapshot;

impl BlockSnapshot {
    /// Compresses `data` into a snapshot: a [`FileHeader`] then one
    /// [`BlockHeader`] + payload per 1 MiB block.
    ///
    /// # Panics
    ///
    /// Panics only if the internal scratch buffer — sized by
    /// [`lz4_flex::block::get_maximum_output_size`] for exactly [`BLOCK_SIZE`]
    /// — were too small for a block; it cannot be, so this does not fire.
    #[must_use]
    pub fn compress(data: &[u8]) -> Vec<u8> {
        let mut writer = SnapshotWriter::new();
        for block in data.chunks(BLOCK_SIZE) {
            // `compress_block` only fails if the scratch buffer is undersized,
            // which it is not (it is sized by `get_maximum_output_size`).
            writer
                .push_block(block)
                .expect("a correctly sized scratch buffer never overflows");
        }
        writer.finish()
    }

    /// Decompresses a snapshot, verifying every block's CRC32 and the total
    /// length.
    ///
    /// # Errors
    ///
    /// [`MemError::BadMagic`], [`MemError::SnapshotTruncated`],
    /// [`MemError::CrcMismatch`], [`MemError::Lz4`], or
    /// [`MemError::LengthMismatch`] on a corrupt or truncated snapshot.
    pub fn decompress(blob: &[u8]) -> MemResult<Vec<u8>> {
        let header = FileHeader::decode(blob)?;
        let mut out = Vec::with_capacity(header.total_len as usize);
        for block in BlockSnapshot::blocks(blob)? {
            out.extend_from_slice(&block?);
        }
        if out.len() != header.total_len as usize {
            return Err(MemError::LengthMismatch {
                expected: header.total_len as usize,
                actual: out.len(),
            });
        }
        Ok(out)
    }

    /// Decompresses as many blocks as possible, returning the salvaged bytes
    /// and one error per unrecoverable block.
    ///
    /// A corrupt block costs its own bytes, not the whole snapshot — the
    /// property the per-block CRC buys. The file header must still be valid.
    ///
    /// # Errors
    ///
    /// Only when the *file* header is unreadable; per-block failures are
    /// returned in the error vector.
    pub fn decompress_lossy(blob: &[u8]) -> MemResult<(Vec<u8>, Vec<MemError>)> {
        let _header = FileHeader::decode(blob)?;
        let mut out = Vec::new();
        let mut errors = Vec::new();
        for block in BlockSnapshot::blocks(blob)? {
            match block {
                Ok(bytes) => out.extend_from_slice(&bytes),
                Err(err) => errors.push(err),
            }
        }
        Ok((out, errors))
    }

    /// Iterates the blocks of a snapshot, yielding each block's decompressed
    /// bytes (or the error that stopped it).
    ///
    /// # Errors
    ///
    /// [`MemError`] when the file header is unreadable. Per-block results are
    /// yielded, not returned.
    pub fn blocks(blob: &[u8]) -> MemResult<BlockIter<'_>> {
        let header = FileHeader::decode(blob)?;
        Ok(BlockIter {
            data: &blob[FILE_HEADER_LEN..],
            remaining: header.block_count as usize,
        })
    }
}

/// An iterator over the blocks of a snapshot, yielding each block's
/// decompressed bytes.
///
/// Blocks are decoded one at a time, so memory stays O(block) even for a
/// multi-gigabyte snapshot. A block that fails its CRC or does not decompress
/// is yielded as an error and the iterator *continues* with the next block —
/// the framed length in the header still locates it, which is exactly the
/// salvage the per-block checksum buys. Only a broken frame (bad magic, or a
/// truncated header/payload) ends iteration, since past it the next block's
/// position is unknown.
#[derive(Debug)]
pub struct BlockIter<'a> {
    data: &'a [u8],
    remaining: usize,
}

impl Iterator for BlockIter<'_> {
    type Item = MemResult<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let (result, consumed) = decode_block_at(self.data);
        if consumed == 0 {
            // The frame is unreadable: the next block cannot be located.
            self.remaining = 0;
        } else {
            self.data = &self.data[consumed..];
        }
        Some(result)
    }
}

/// An incremental snapshot writer: compresses one block at a time and appends
/// it to the output, so memory stays O(block).
#[derive(Debug)]
pub struct SnapshotWriter {
    out: Vec<u8>,
    scratch: Vec<u8>,
    blocks: u32,
    total_len: u64,
    /// Space reserved for the file header, patched in `finish`.
    header_placeholder: usize,
}

impl Default for SnapshotWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotWriter {
    /// A writer with no blocks yet.
    #[must_use]
    pub fn new() -> Self {
        let out = vec![0u8; FILE_HEADER_LEN];
        let header_placeholder = out.len();
        Self {
            out,
            scratch: vec![0u8; lz4_flex::block::get_maximum_output_size(BLOCK_SIZE)],
            blocks: 0,
            total_len: 0,
            header_placeholder,
        }
    }

    /// Appends one block (at most [`BLOCK_SIZE`] bytes).
    ///
    /// # Errors
    ///
    /// [`MemError::Lz4`] if the block exceeds [`BLOCK_SIZE`]; the LZ4 encoder
    /// itself cannot fail against the pre-sized scratch buffer.
    pub fn push_block(&mut self, block: &[u8]) -> MemResult<()> {
        if block.len() > BLOCK_SIZE {
            return Err(MemError::Lz4(format!(
                "block is {} bytes, the limit is {BLOCK_SIZE}",
                block.len()
            )));
        }
        let compressed_len = lz4_flex::block::compress_into(block, &mut self.scratch)
            .map_err(|err| MemError::Lz4(err.to_string()))?;
        let compressed = &self.scratch[..compressed_len];
        let mut hasher = Crc32::new();
        hasher.update(compressed);
        let header = BlockHeader {
            magic: BLOCK_MAGIC,
            crc: hasher.finalize(),
            uncompressed_len: block.len() as u32,
            compressed_len: compressed_len as u32,
        };
        self.out.extend_from_slice(&header.encode());
        self.out.extend_from_slice(compressed);
        self.blocks += 1;
        self.total_len += block.len() as u64;
        Ok(())
    }

    /// Finalises the snapshot, patching the file header in front.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        let header = FileHeader::new(self.blocks, self.total_len);
        self.out[..FILE_HEADER_LEN].copy_from_slice(&header.encode());
        self.out
    }

    /// Blocks written so far.
    #[must_use]
    pub const fn block_count(&self) -> u32 {
        self.blocks
    }

    /// The reserved header length (the writer's constant prefix).
    #[must_use]
    pub const fn header_len(&self) -> usize {
        self.header_placeholder
    }
}

/// Decodes one block at `data`, returning the decompressed bytes and how many
/// input bytes it consumed.
///
/// # Errors
///
/// [`MemError::BadMagic`], [`MemError::SnapshotTruncated`],
/// [`MemError::CrcMismatch`], [`MemError::LengthMismatch`], or
/// [`MemError::Lz4`].
pub fn decode_block(data: &[u8]) -> MemResult<(Vec<u8>, usize)> {
    let (result, consumed) = decode_block_at(data);
    result.map(|bytes| (bytes, consumed))
}

/// Decodes one block, returning the result *and* the number of input bytes the
/// frame occupies — which is known from the header even when the payload fails
/// its checksum, so a caller can skip to the next block.
///
/// A `consumed` of `0` means the frame itself is unreadable (bad magic, or a
/// truncated header/payload) and the stream position is lost.
fn decode_block_at(data: &[u8]) -> (MemResult<Vec<u8>>, usize) {
    let header = match BlockHeader::decode(data) {
        Ok(header) => header,
        Err(err) => return (Err(err), 0),
    };
    let start = BLOCK_HEADER_LEN;
    let end = start + header.compressed_len as usize;
    if data.len() < end {
        return (
            Err(MemError::SnapshotTruncated {
                offset: start,
                need: end,
                have: data.len(),
            }),
            0,
        );
    }
    let consumed = end;
    let compressed = &data[start..end];
    let mut hasher = Crc32::new();
    hasher.update(compressed);
    let computed = hasher.finalize();
    if computed != header.crc {
        return (
            Err(MemError::CrcMismatch {
                stored: header.crc,
                computed,
            }),
            consumed,
        );
    }
    let mut out = vec![0u8; header.uncompressed_len as usize];
    let written = match lz4_flex::block::decompress_into(compressed, &mut out) {
        Ok(written) => written,
        Err(err) => return (Err(MemError::Lz4(err.to_string())), consumed),
    };
    out.truncate(written);
    if written != header.uncompressed_len as usize {
        return (
            Err(MemError::LengthMismatch {
                expected: header.uncompressed_len as usize,
                actual: written,
            }),
            consumed,
        );
    }
    (Ok(out), consumed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(len: usize) -> Vec<u8> {
        // Compressible, non-trivial content: a repeating pattern with a counter.
        (0..len).map(|i| ((i / 7) % 251) as u8).collect()
    }

    #[test]
    fn round_trips_a_small_snapshot() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(10);
        let blob = BlockSnapshot::compress(&data);
        assert_eq!(BlockSnapshot::decompress(&blob).expect("decode"), data);
    }

    #[test]
    fn round_trips_an_empty_snapshot() {
        let blob = BlockSnapshot::compress(&[]);
        assert_eq!(
            BlockSnapshot::decompress(&blob).expect("decode"),
            Vec::<u8>::new()
        );
        let header = FileHeader::decode(&blob).expect("header");
        assert_eq!(header.block_count, 0);
        assert_eq!(header.total_len, 0);
    }

    #[test]
    fn round_trips_across_block_boundaries() {
        for len in [
            BLOCK_SIZE - 1,
            BLOCK_SIZE,
            BLOCK_SIZE + 1,
            BLOCK_SIZE * 3 + 17,
        ] {
            let data = sample(len);
            let blob = BlockSnapshot::compress(&data);
            let header = FileHeader::decode(&blob).expect("header");
            let expected_blocks = len.div_ceil(BLOCK_SIZE);
            assert_eq!(header.block_count as usize, expected_blocks, "len {len}");
            assert_eq!(header.total_len as usize, len);
            assert_eq!(BlockSnapshot::decompress(&blob).expect("decode"), data);
        }
    }

    #[test]
    fn compresses_repetitive_data() {
        let data = vec![0u8; BLOCK_SIZE * 4];
        let blob = BlockSnapshot::compress(&data);
        assert!(
            blob.len() < data.len() / 100,
            "a run of zeros compressed to {} bytes",
            blob.len()
        );
    }

    #[test]
    fn a_flipped_payload_byte_fails_the_crc() {
        let data = sample(BLOCK_SIZE + 100);
        let mut blob = BlockSnapshot::compress(&data);
        // Flip a byte inside the first block's compressed payload.
        let target = FILE_HEADER_LEN + BLOCK_HEADER_LEN + 5;
        blob[target] ^= 0xff;
        let err = BlockSnapshot::decompress(&blob).expect_err("corruption detected");
        assert!(
            matches!(err, MemError::CrcMismatch { .. } | MemError::Lz4(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn a_truncated_snapshot_is_rejected() {
        let data = sample(BLOCK_SIZE * 2);
        let blob = BlockSnapshot::compress(&data);
        let truncated = &blob[..blob.len() / 2];
        assert!(BlockSnapshot::decompress(truncated).is_err());
    }

    #[test]
    fn a_wrong_magic_is_rejected() {
        let mut bad = BlockSnapshot::compress(b"hello");
        bad[0] = b'X';
        assert!(matches!(
            BlockSnapshot::decompress(&bad),
            Err(MemError::BadMagic { .. })
        ));
    }

    #[test]
    fn a_corrupt_file_header_is_rejected() {
        let mut bad = BlockSnapshot::compress(b"hello");
        // Corrupt the block count; the header CRC no longer matches.
        bad[4] ^= 0xff;
        assert!(matches!(
            BlockSnapshot::decompress(&bad),
            Err(MemError::CrcMismatch { .. })
        ));
    }

    #[test]
    fn a_corrupt_block_header_is_rejected() {
        let mut bad = BlockSnapshot::compress(b"hello");
        bad[FILE_HEADER_LEN] = b'X';
        assert!(matches!(
            BlockSnapshot::decompress(&bad),
            Err(MemError::BadMagic { .. })
        ));
    }

    #[test]
    fn lossy_decode_salvages_the_good_blocks() {
        let data = sample(BLOCK_SIZE * 3);
        let mut blob = BlockSnapshot::compress(&data);
        // Walk the framing to find the middle block's compressed payload and
        // flip one byte inside it.
        let mut offset = FILE_HEADER_LEN;
        let mut corrupt_at = None;
        for index in 0..3 {
            let header = BlockHeader::decode(&blob[offset..]).expect("block header");
            if index == 1 {
                corrupt_at = Some(offset + BLOCK_HEADER_LEN + 5);
            }
            offset += BLOCK_HEADER_LEN + header.compressed_len as usize;
        }
        blob[corrupt_at.expect("middle block located")] ^= 0xff;

        let (salvaged, errors) = BlockSnapshot::decompress_lossy(&blob).expect("header ok");
        assert_eq!(errors.len(), 1, "exactly the corrupt block failed");
        // The other two blocks survive intact, in order.
        assert_eq!(salvaged.len(), BLOCK_SIZE * 2);
        assert_eq!(
            salvaged,
            [&data[..BLOCK_SIZE], &data[BLOCK_SIZE * 2..]].concat()
        );
    }

    #[test]
    fn a_corrupt_block_does_not_hide_the_ones_after_it() {
        // The per-block framing means a CRC failure costs one block, not the
        // tail: iteration must resume at the next framed block.
        let data = sample(BLOCK_SIZE * 3);
        let mut blob = BlockSnapshot::compress(&data);
        // Corrupt the FIRST block's payload.
        blob[FILE_HEADER_LEN + BLOCK_HEADER_LEN + 5] ^= 0xff;
        let results: Vec<_> = BlockSnapshot::blocks(&blob).expect("header").collect();
        assert_eq!(results.len(), 3, "all three framed blocks were visited");
        assert!(results[0].is_err(), "the corrupt block reports its error");
        assert_eq!(
            results[1].as_deref().expect("block 1"),
            &data[BLOCK_SIZE..BLOCK_SIZE * 2]
        );
        assert_eq!(
            results[2].as_deref().expect("block 2"),
            &data[BLOCK_SIZE * 2..]
        );
    }

    #[test]
    fn the_block_header_is_sixteen_bytes() {
        assert_eq!(std::mem::size_of::<[u8; BLOCK_HEADER_LEN]>(), 16);
        assert_eq!(BlockHeader::LEN, 16);
        let header = BlockHeader {
            magic: BLOCK_MAGIC,
            crc: 0xdead_beef,
            uncompressed_len: 1024,
            compressed_len: 256,
        };
        assert_eq!(
            BlockHeader::decode(&header.encode()).expect("decode"),
            header
        );
    }

    #[test]
    fn the_file_header_is_twenty_bytes() {
        assert_eq!(FILE_HEADER_LEN, 20);
        let header = FileHeader::new(7, 1_234_567);
        assert_eq!(
            FileHeader::decode(&header.encode()).expect("decode"),
            header
        );
    }

    #[test]
    fn the_writer_matches_the_one_shot_codec() {
        let data = sample(BLOCK_SIZE * 2 + 123);
        let one_shot = BlockSnapshot::compress(&data);
        let mut writer = SnapshotWriter::new();
        for block in data.chunks(BLOCK_SIZE) {
            writer.push_block(block).expect("push");
        }
        assert_eq!(writer.block_count(), 3);
        assert_eq!(writer.finish(), one_shot);
    }

    #[test]
    fn the_writer_rejects_an_oversized_block() {
        let mut writer = SnapshotWriter::new();
        let big = vec![0u8; BLOCK_SIZE + 1];
        assert!(matches!(writer.push_block(&big), Err(MemError::Lz4(_))));
    }

    #[test]
    fn blocks_iterates_every_block_in_order() {
        let data = sample(BLOCK_SIZE + 42);
        let blob = BlockSnapshot::compress(&data);
        let blocks: Vec<Vec<u8>> = BlockSnapshot::blocks(&blob)
            .expect("header")
            .map(|block| block.expect("block"))
            .collect();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].len(), BLOCK_SIZE);
        assert_eq!(blocks[1].len(), 42);
        assert_eq!(blocks.concat(), data);
    }

    #[test]
    fn an_incompressible_block_round_trips() {
        // A pseudo-random, incompressible block must still round-trip.
        let data: Vec<u8> = (0..BLOCK_SIZE)
            .map(|i| {
                let x = (i as u64)
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                (x >> 33) as u8
            })
            .collect();
        let blob = BlockSnapshot::compress(&data);
        assert_eq!(BlockSnapshot::decompress(&blob).expect("decode"), data);
    }
}
