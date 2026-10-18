//! The varint codec pogocache's entry layout uses for key lengths.
//!
//! A port of `varint_write_u64` / `varint_read_u64` from `util.c`: 7 bits per
//! byte, little-endian, high bit set on continuation bytes. Values under 128
//! take one byte; the maximum is 10 bytes.

use crate::error::{MemError, MemResult};

/// Appends the varint encoding of `value` to `out` and returns its byte length.
pub fn write_u64(out: &mut Vec<u8>, mut value: u64) -> usize {
    let start = out.len();
    if value < 128 {
        out.push(value as u8);
        return 1;
    }
    loop {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
        if value < 128 {
            break;
        }
    }
    out.push(value as u8);
    out.len() - start
}

/// Decodes a varint from the front of `data`, returning the value and the
/// number of bytes consumed.
///
/// # Errors
///
/// Returns [`MemError::BadVarint`] when no terminator appears within 10 bytes.
pub fn read_u64(data: &[u8]) -> MemResult<(u64, usize)> {
    let mut value = 0u64;
    for (i, &byte) in data.iter().enumerate().take(10) {
        if i == 9 && (byte & 0x7f) > 1 {
            // A tenth byte carries only bit 63; anything beyond silently
            // shifted out of the u64 before, truncating the value. The
            // canonical encoding never produces it, so refuse instead of
            // decoding a different number than the bytes spell.
            return Err(MemError::BadVarint);
        }
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte < 128 {
            return Ok((value, i + 1));
        }
    }
    Err(MemError::BadVarint)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_non_canonical_tenth_byte_is_refused_not_truncated() {
        use super::{read_u64, MemError};
        // Bits above 63 have nowhere to land: before the guard this decoded
        // as Some((1 << 63, 10)), silently dropping the tenth byte's upper
        // bits instead of rejecting the non-canonical encoding.
        let bad = [0x80u8; 9]
            .iter()
            .copied()
            .chain([0x7f])
            .collect::<Vec<_>>();
        assert!(matches!(read_u64(&bad), Err(MemError::BadVarint)));
        let bad_two = [0x80u8; 9]
            .iter()
            .copied()
            .chain([0x02])
            .collect::<Vec<_>>();
        assert!(matches!(read_u64(&bad_two), Err(MemError::BadVarint)));
        // The canonical tenth byte (bit 63 alone) still decodes.
        let canonical = [0x80u8; 9]
            .iter()
            .copied()
            .chain([0x01])
            .collect::<Vec<_>>();
        assert_eq!(
            read_u64(&canonical).expect("canonical decodes"),
            (1u64 << 63, 10)
        );
    }

    use super::*;

    fn round_trip(value: u64) {
        let mut buf = Vec::new();
        let written = write_u64(&mut buf, value);
        assert_eq!(written, buf.len());
        let (decoded, read) = read_u64(&buf).expect("round trip decodes");
        assert_eq!(decoded, value);
        assert_eq!(read, written);
    }

    #[test]
    fn round_trips_across_the_widths() {
        for value in [
            0,
            1,
            127,
            128,
            255,
            16_383,
            16_384,
            u64::from(u32::MAX),
            u64::MAX,
        ] {
            round_trip(value);
        }
    }

    #[test]
    fn small_values_take_one_byte() {
        let mut buf = Vec::new();
        assert_eq!(write_u64(&mut buf, 127), 1);
        assert_eq!(buf, vec![127]);
    }

    #[test]
    fn rejects_an_unterminated_run() {
        let run = [0xffu8; 10];
        assert_eq!(read_u64(&run), Err(MemError::BadVarint));
    }

    #[test]
    fn reads_only_the_first_value() {
        let mut buf = Vec::new();
        write_u64(&mut buf, 300);
        let tail = buf.len();
        buf.extend_from_slice(b"tail");
        let (value, read) = read_u64(&buf).expect("decodes");
        assert_eq!((value, read), (300, tail));
    }
}
