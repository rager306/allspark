//! Streaming UTF-8 decoding for HTTP response bodies.
//!
//! One decoder per byte stream: feed every body chunk to [`StreamingUtf8::decode_chunk`],
//! then [`StreamingUtf8::flush`] once at end of stream. A multi-byte character split
//! across two chunks decodes whole instead of becoming one U+FFFD per partial byte.

/// Streaming UTF-8 decoder matching `TextDecoder` `{ stream: true }`: an incomplete
/// trailing sequence is buffered for the next chunk, and invalid bytes become one U+FFFD
/// per maximal subpart (the `String::from_utf8_lossy` rule), so an overlong, surrogate or
/// out-of-range sequence such as `ED A0 80` yields one U+FFFD per byte.
#[derive(Default)]
pub(crate) struct StreamingUtf8 {
    /// Bytes of a character whose remaining bytes have not arrived yet.
    buffered: Vec<u8>,
}

impl StreamingUtf8 {
    /// Decode the complete characters buffered so far plus `data`; a trailing incomplete
    /// sequence waits for the next chunk.
    pub(crate) fn decode_chunk(&mut self, data: &[u8]) -> String {
        self.buffered.extend_from_slice(data);
        let (text, consumed) = decode_lossy_prefix(&self.buffered);
        self.buffered.drain(..consumed);
        text
    }

    /// End of stream: decode whatever is left, a truncated trailing sequence as one U+FFFD.
    pub(crate) fn finish(&mut self) -> String {
        let buffered = std::mem::take(&mut self.buffered);
        let (mut text, consumed) = decode_lossy_prefix(&buffered);
        if consumed < buffered.len() {
            text.push('\u{FFFD}');
        }
        text
    }
}

/// Lossily decode `bytes` up to a trailing incomplete sequence, returning the text and how
/// many bytes it consumed; the unconsumed tail is a valid prefix of a character that more
/// bytes may complete.
fn decode_lossy_prefix(bytes: &[u8]) -> (String, usize) {
    let mut text = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                return (text, bytes.len());
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                // `valid_up_to` marks the longest valid prefix, so this never fails.
                text.push_str(std::str::from_utf8(valid).unwrap_or_default());
                match error.error_len() {
                    Some(invalid) => {
                        text.push('\u{FFFD}');
                        rest = &after[invalid..];
                    }
                    None => return (text, bytes.len() - after.len()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StreamingUtf8;

    /// A body that is valid UTF-8 with no torn tail must decode to exactly the same string
    /// whatever the chunk boundaries, including every position inside a 4-byte emoji.
    #[test]
    fn every_split_position_of_a_multi_byte_string_decodes_unchanged() {
        let text = "世界🚀";
        let bytes = text.as_bytes();
        for split in 0..=bytes.len() {
            let mut decoder = StreamingUtf8::default();
            let mut whole = decoder.decode_chunk(&bytes[..split]);
            whole.push_str(&decoder.decode_chunk(&bytes[split..]));
            whole.push_str(&decoder.finish());
            assert_eq!(whole, text, "split at byte {split}");
        }
    }

    /// Feeding `bytes` one chunk, one byte at a time, or anywhere in between decodes exactly
    /// what `String::from_utf8_lossy` (the maximal-subpart rule) does for the whole input.
    #[test]
    fn chunking_never_changes_the_decoded_text() {
        for bytes in [
            b"a\xED\xA0\x80\xE0\x80\xAFb".as_slice(),
            "\u{20ac}\u{1F600}".as_bytes(),
            b"x\xE2\x82".as_slice(),
        ] {
            let expected = String::from_utf8_lossy(bytes).into_owned();
            for split in 0..=bytes.len() {
                let mut decoder = StreamingUtf8::default();
                let mut text = decoder.decode_chunk(&bytes[..split]);
                text.push_str(&decoder.decode_chunk(&bytes[split..]));
                text.push_str(&decoder.finish());
                assert_eq!(text, expected, "split {split} of {bytes:02X?}");
            }
            let mut byte_by_byte = StreamingUtf8::default();
            let mut text: String = bytes
                .iter()
                .map(|byte| byte_by_byte.decode_chunk(std::slice::from_ref(byte)))
                .collect();
            text.push_str(&byte_by_byte.finish());
            assert_eq!(text, expected, "byte by byte: {bytes:02X?}");
        }
    }

    /// A sequence still incomplete at end of stream yields exactly one U+FFFD, not a
    /// per-byte run of them.
    #[test]
    fn a_torn_tail_finishes_as_one_replacement_character() {
        let mut decoder = StreamingUtf8::default();
        assert_eq!(decoder.decode_chunk(b"x\xE2\x82"), "x");
        assert_eq!(decoder.finish(), "\u{FFFD}");
    }
}
