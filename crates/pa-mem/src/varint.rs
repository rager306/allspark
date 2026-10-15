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
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte < 128 {
            return Ok((value, i + 1));
        }
    }
    Err(MemError::BadVarint)
}

#[cfg(test)]
mod tests {
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
