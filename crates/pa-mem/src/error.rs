//! Errors for the packed-layout codecs.
//!
//! Every decoder in this crate is bounds-checked, so a corrupt or truncated
//! buffer is a value ([`MemError`]), never a panic or an out-of-bounds read.

/// A packed-layout decode/encode failure.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemError {
    /// A 48-bit index does not fit in the 6-byte pointer field.
    #[error("index {index} does not fit a 48-bit pointer (max {max})")]
    PtrOverflow {
        /// The rejected index.
        index: u64,
        /// The largest index the field can hold.
        max: u64,
    },
    /// An entry blob ran past the end of the arena buffer.
    #[error("entry blob truncated at offset {offset}: need {need} bytes, have {have}")]
    EntryTruncated {
        /// Where the read started.
        offset: usize,
        /// How many bytes the read wanted.
        need: usize,
        /// How many bytes were actually available.
        have: usize,
    },
    /// A varint did not terminate within 10 bytes.
    #[error("malformed varint (no terminator in 10 bytes)")]
    BadVarint,
    /// A snapshot does not start with the expected magic.
    #[error("bad snapshot magic: expected {expected:?}, found {found:?}")]
    BadMagic {
        /// The magic the format requires.
        expected: [u8; 4],
        /// The magic actually found.
        found: [u8; 4],
    },
    /// A block's stored CRC32 does not match the computed one.
    #[error("crc mismatch: stored {stored:#010x}, computed {computed:#010x}")]
    CrcMismatch {
        /// The checksum in the header.
        stored: u32,
        /// The checksum of the bytes present.
        computed: u32,
    },
    /// A snapshot buffer ended before a header or block did.
    #[error("snapshot truncated at offset {offset}: need {need} bytes, have {have}")]
    SnapshotTruncated {
        /// Where the read started.
        offset: usize,
        /// How many bytes the read wanted.
        need: usize,
        /// How many bytes were actually available.
        have: usize,
    },
    /// The decompressed length disagrees with the file header.
    #[error("snapshot length mismatch: header says {expected} bytes, decoded {actual}")]
    LengthMismatch {
        /// The length the file header declares.
        expected: usize,
        /// The length actually decoded.
        actual: usize,
    },
    /// The LZ4 block decoder rejected the payload.
    #[error("lz4 block decode failed: {0}")]
    Lz4(String),
    /// A filesystem operation for the build cache failed.
    #[error("build cache i/o failed: {0}")]
    Io(String),
}

/// A [`MemError`] result.
pub type MemResult<T> = Result<T, MemError>;
