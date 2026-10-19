//! V20 — PDX: a compact, columnar layout for `f32` vectors in a BLOB.
//!
//! Storing vectors row-major wastes cache when a query scans one dimension
//! across many vectors. PDX stores them *column-major*: all first components,
//! then all second components, and so on, so a single dimension is one
//! contiguous run. Everything is little-endian `f32` produced through
//! `to_le_bytes` / `from_le_bytes` — no `unsafe`, no `bytemuck`.
//!
//! Wire format (all integers little-endian):
//!
//! ```text
//! magic  : 4 bytes  = b"PDX1"
//! count  : u32      = number of vectors
//! dim    : u32      = components per vector
//! data   : count*dim f32, column-major (vector j of dimension i at i*count + j)
//! ```

/// The 4-byte magic identifying a PDX blob.
pub const PDX_MAGIC: [u8; 4] = *b"PDX1";
/// Bytes of header before the payload.
pub const PDX_HEADER_LEN: usize = 12;

/// Errors from [`PdxLayout::unpack`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PdxError {
    /// The blob is shorter than the header.
    #[error("pdx blob too short: {0} bytes")]
    TooShort(usize),
    /// The magic bytes are wrong.
    #[error("pdx bad magic")]
    BadMagic,
    /// The declared length does not match the payload.
    #[error("pdx length mismatch: header says {expected} bytes, got {actual}")]
    LengthMismatch {
        /// Bytes the header implies.
        expected: usize,
        /// Bytes actually present.
        actual: usize,
    },
    /// Vectors have inconsistent dimensions.
    #[error("pdx ragged vectors: expected dim {expected}, got {actual}")]
    Ragged {
        /// Dimension of the first vector.
        expected: usize,
        /// Dimension of the offending vector.
        actual: usize,
    },
}

/// Columnar PDX codec for a batch of equal-length `f32` vectors.
#[derive(Debug, Clone, Copy, Default)]
pub struct PdxLayout;

impl PdxLayout {
    /// Pack `vectors` into a column-major BLOB.
    ///
    /// All vectors must share one dimension; a ragged batch packs to an empty
    /// blob (the caller should validate first — see [`PdxLayout::validate`]).
    #[must_use]
    pub fn pack(vectors: &[Vec<f32>]) -> Vec<u8> {
        let count = vectors.len();
        let dim = vectors.first().map_or(0, Vec::len);
        if Self::validate(vectors).is_err() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(PDX_HEADER_LEN + count * dim * 4);
        out.extend_from_slice(&PDX_MAGIC);
        out.extend_from_slice(&(count as u32).to_le_bytes());
        out.extend_from_slice(&(dim as u32).to_le_bytes());
        for column in 0..dim {
            for vector in vectors {
                out.extend_from_slice(&vector[column].to_le_bytes());
            }
        }
        out
    }

    /// Unpack a PDX BLOB back into row-major vectors.
    ///
    /// # Errors
    /// Returns [`PdxError`] for a short blob, bad magic, or a length mismatch.
    pub fn unpack(blob: &[u8]) -> Result<Vec<Vec<f32>>, PdxError> {
        if blob.len() < PDX_HEADER_LEN {
            return Err(PdxError::TooShort(blob.len()));
        }
        if blob[0..4] != PDX_MAGIC {
            return Err(PdxError::BadMagic);
        }
        let count = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]) as usize;
        let dim = u32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]) as usize;
        let expected = PDX_HEADER_LEN + count * dim * 4;
        if blob.len() != expected {
            return Err(PdxError::LengthMismatch {
                expected,
                actual: blob.len(),
            });
        }
        let mut vectors = vec![vec![0.0_f32; dim]; count];
        let mut offset = PDX_HEADER_LEN;
        for column in 0..dim {
            for vector in vectors.iter_mut().take(count) {
                vector[column] = f32::from_le_bytes([
                    blob[offset],
                    blob[offset + 1],
                    blob[offset + 2],
                    blob[offset + 3],
                ]);
                offset += 4;
            }
        }
        Ok(vectors)
    }

    /// Check that every vector has the same dimension (and that an empty batch
    /// is trivially valid).
    ///
    /// # Errors
    /// Returns [`PdxError::Ragged`] naming the first mismatching vector.
    pub fn validate(vectors: &[Vec<f32>]) -> Result<(), PdxError> {
        let Some(first) = vectors.first() else {
            return Ok(());
        };
        let dim = first.len();
        for vector in vectors {
            if vector.len() != dim {
                return Err(PdxError::Ragged {
                    expected: dim,
                    actual: vector.len(),
                });
            }
        }
        Ok(())
    }

    /// Number of vectors a blob encodes (`None` when malformed).
    #[must_use]
    pub fn count(blob: &[u8]) -> Option<usize> {
        if blob.len() < PDX_HEADER_LEN || blob[0..4] != PDX_MAGIC {
            return None;
        }
        Some(u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]) as usize)
    }

    /// Dimension a blob encodes (`None` when malformed).
    #[must_use]
    pub fn dim(blob: &[u8]) -> Option<usize> {
        if blob.len() < PDX_HEADER_LEN || blob[0..4] != PDX_MAGIC {
            return None;
        }
        Some(u32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Vec<f32>> {
        vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0],
        ]
    }

    #[test]
    fn round_trips_exactly() {
        let vectors = sample();
        let blob = PdxLayout::pack(&vectors);
        let restored = PdxLayout::unpack(&blob).expect("unpack");
        assert_eq!(restored, vectors);
    }

    #[test]
    fn layout_is_column_major() {
        let blob = PdxLayout::pack(&sample());
        // First dimension column: 1.0, 4.0, 7.0, contiguous.
        let read = |i: usize| {
            let o = PDX_HEADER_LEN + i * 4;
            f32::from_le_bytes([blob[o], blob[o + 1], blob[o + 2], blob[o + 3]])
        };
        assert_eq!(read(0).to_bits(), 1.0_f32.to_bits());
        assert_eq!(read(1).to_bits(), 4.0_f32.to_bits());
        assert_eq!(read(2).to_bits(), 7.0_f32.to_bits());
        assert_eq!(
            read(3).to_bits(),
            2.0_f32.to_bits(),
            "second column starts after the first"
        );
    }

    #[test]
    fn header_reports_count_and_dim() {
        let blob = PdxLayout::pack(&sample());
        assert_eq!(PdxLayout::count(&blob), Some(3));
        assert_eq!(PdxLayout::dim(&blob), Some(3));
        assert_eq!(blob.len(), PDX_HEADER_LEN + 3 * 3 * 4);
    }

    #[test]
    fn empty_batch_round_trips() {
        let blob = PdxLayout::pack(&[]);
        assert_eq!(PdxLayout::count(&blob), Some(0));
        assert_eq!(PdxLayout::dim(&blob), Some(0));
        assert_eq!(
            PdxLayout::unpack(&blob).expect("unpack"),
            Vec::<Vec<f32>>::new()
        );
    }

    #[test]
    fn ragged_batch_is_rejected() {
        let ragged = vec![vec![1.0, 2.0], vec![3.0]];
        assert!(matches!(
            PdxLayout::validate(&ragged),
            Err(PdxError::Ragged { .. })
        ));
        assert!(
            PdxLayout::pack(&ragged).is_empty(),
            "ragged packs to nothing"
        );
    }

    #[test]
    fn malformed_blobs_are_rejected() {
        assert_eq!(PdxLayout::unpack(b"ab"), Err(PdxError::TooShort(2)));
        assert_eq!(PdxLayout::unpack(b"XXXX________"), Err(PdxError::BadMagic));
        let mut blob = PdxLayout::pack(&sample());
        blob.push(0);
        assert!(matches!(
            PdxLayout::unpack(&blob),
            Err(PdxError::LengthMismatch { .. })
        ));
    }
}
