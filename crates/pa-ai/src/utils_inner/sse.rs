//! Server-sent event decoding shared by streaming providers.
//!
//! Line grammar matches the TS SSE decoder: \n, \r\n and \r all end a line, `:` starts a comment,
//! `event:`/`data:` are the fields it keeps, multi-line data joins with \n, and a blank line or
//! end of stream flushes.
//!
//! Chunking, however, is this decoder's own contract, and one place the TS version is not
//! invariant: it reads a `\r` that ends a chunk as a finished line, so the `\n` of a split
//! `\r\n` then looks like an empty line and flushes early. Here a chunk boundary can never
//! change the events (see [`SseDecoder`]).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSentEvent {
    pub event: Option<String>,
    pub data: String,
    pub raw: Vec<String>,
}

#[derive(Debug, Default)]
struct DecoderState {
    event: Option<String>,
    data: Vec<String>,
    raw: Vec<String>,
}

impl DecoderState {
    fn flush(&mut self) -> Option<ServerSentEvent> {
        if self.event.is_none() && self.data.is_empty() {
            return None;
        }
        let event = ServerSentEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
            raw: std::mem::take(&mut self.raw),
        };
        Some(event)
    }

    fn decode_line(&mut self, line: &str) -> Option<ServerSentEvent> {
        if line.is_empty() {
            return self.flush();
        }
        self.raw.push(line.to_string());
        if line.starts_with(':') {
            return None;
        }
        let (field_name, value) = match line.find(':') {
            Some(index) => {
                let name = &line[..index];
                let mut value = &line[index + 1..];
                if let Some(stripped) = value.strip_prefix(' ') {
                    value = stripped;
                }
                (name, value)
            }
            None => (line, ""),
        };
        if field_name == "event" {
            self.event = Some(value.to_string());
        } else if field_name == "data" {
            self.data.push(value.to_string());
        }
        None
    }
}

/// Incremental SSE decoder over text chunks.
///
/// `\r\n`, `\r` and `\n` all end a line, and a chunk boundary must not change the result:
/// `push_text(a) + push_text(b)` yields the same events as `push_text(a + b)` for every split.
/// A `\r` that ends a chunk is therefore held until the next byte says whether it is a `\r\n`
/// pair or a lone `\r`. Reading it eagerly as a lone break made the `\n` of a split `\r\n` end
/// an empty line, and an empty line flushes, so one event with two `data:` lines became two
/// events (review #61). The held byte is also never consumed — it is rescanned with the next
/// chunk, which is why `scanned` is not `buffer.len()`.
#[derive(Debug, Default)]
pub struct SseDecoder {
    state: DecoderState,
    /// Decoded text not yet ended by a line break: the current partial line, ending in the
    /// `\r` that is still waiting for its next byte, if there is one.
    buffer: String,
    /// How far into `buffer` the previous scan reached. Never past a byte that is still
    /// undecided, so a held `\r` is examined again; otherwise a resume would re-read a line
    /// that already went through [`DecoderState::decode_line`] and duplicate it in `raw`.
    scanned: usize,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of decoded text; returns complete events in order.
    pub fn push_text(&mut self, text: &str) -> Vec<ServerSentEvent> {
        self.buffer.push_str(text);
        self.drain_lines(/*at_eof*/ false)
    }

    /// Signal end of stream; flushes the final line and trailing event.
    pub fn finish(&mut self) -> Vec<ServerSentEvent> {
        let mut events = self.drain_lines(/*at_eof*/ true);
        // A last line without a break is still a line.
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.scanned = 0;
            if let Some(event) = self.state.decode_line(&line) {
                events.push(event);
            }
        }
        if let Some(event) = self.state.flush() {
            events.push(event);
        }
        events
    }

    /// Decode every complete line in `buffer`, keeping the unterminated remainder.
    ///
    /// At end of stream a trailing `\r` is a break, because no byte can follow it; while more
    /// chunks are expected it is held instead.
    fn drain_lines(&mut self, at_eof: bool) -> Vec<ServerSentEvent> {
        let mut events = Vec::new();
        let mut line_start = 0;
        let mut index = self.scanned;
        let bytes = self.buffer.as_bytes();
        while index < bytes.len() {
            let break_len = match bytes[index] {
                b'\r' if index + 1 == bytes.len() && !at_eof => break,
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
                b'\r' | b'\n' => 1,
                _ => {
                    index += 1;
                    continue;
                }
            };
            if let Some(event) = self.state.decode_line(&self.buffer[line_start..index]) {
                events.push(event);
            }
            index += break_len;
            line_start = index;
        }
        self.buffer.drain(..line_start);
        self.scanned = index - line_start;
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_basic_events() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push_text("event: message_start\ndata: {\"a\":1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message_start"));
        assert_eq!(events[0].data, "{\"a\":1}");
    }

    #[test]
    fn handles_crlf_and_multiline_data() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push_text("data: line1\r\ndata: line2\r\n\r\n");
        assert_eq!(events[0].data, "line1\nline2");
        assert_eq!(events[0].event, None);
    }

    #[test]
    fn skips_comments_and_unknown_fields() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push_text(": keep-alive\ndata: x\nid: 3\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn decodes_prime_inference_capture() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/testdata/prime_inference_glm53_flash.sse"
        ))
        .unwrap();
        let mut decoder = SseDecoder::new();
        let events = decoder.push_text(&text);
        let extra = decoder.finish();
        assert_eq!(events.len() + extra.len(), 44); // 43 chunks + [DONE]
        let expected_lines: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("data: "))
            .map(|line| &line[6..])
            .collect();
        let actual: Vec<&str> = events
            .iter()
            .chain(extra.iter())
            .map(|event| event.data.as_str())
            .collect();
        assert_eq!(actual, expected_lines);
        let empty: Vec<&str> = events
            .iter()
            .chain(extra.iter())
            .filter(|e| e.data.is_empty())
            .map(|_| "empty")
            .collect();
        assert!(empty.is_empty(), "found empty data events");
        assert!(events[0].data.contains("chat.completion.chunk"));
    }

    #[test]
    fn preserves_fragmented_line_breaks_and_raw_lines() {
        let mut decoder = SseDecoder::new();
        let first = |data: &str| ServerSentEvent {
            event: None,
            data: data.into(),
            raw: vec![format!("data: {data}")],
        };

        // A `\r` ends a chunk: it is still undecided, so nothing is decoded yet. The `\n` that
        // follows pairs with it, and the blank line after THAT flushes — the same lines a whole
        // `"data: first\r\n\n"` produces.
        assert_eq!(decoder.push_text("data: first\r").len(), 0);
        assert_eq!(decoder.push_text("\n").len(), 0);
        assert_eq!(decoder.push_text("\n"), vec![first("first")]);

        assert_eq!(
            decoder.push_text("event: update\rdata: next\n\n"),
            vec![ServerSentEvent {
                event: Some("update".into()),
                data: "next".into(),
                raw: vec!["event: update".into(), "data: next".into()],
            }]
        );

        assert_eq!(decoder.push_text("data: final").len(), 0);
        assert_eq!(decoder.finish(), vec![first("final")]);
    }

    #[test]
    fn flushes_trailing_event_at_eof() {
        let mut decoder = SseDecoder::new();
        decoder.push_text("data: partial");
        let events = decoder.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "partial");
    }

    /// Canvases that between them use every line break the decoder accepts, both inside a line
    /// run and as the blank line that flushes, plus the two shapes a stream can end with.
    const CANVASES: &[&str] = &[
        "data: a\r\ndata: b\r\n\r\n",
        "event: x\r\ndata: {\"k\":1}\r\n\r\ndata: [DONE]\r\n",
        "data: one\rdata: two\r\r",
        "data: a\n\ndata: b\r\n\r\ndata: c\r\r",
        ": keep-alive\r\ndata: x\r\n\r\n",
        "data: tail",
        "data: crlf-tail\r\n",
        "data: dangling\r",
    ];

    fn decode_with_chunks(chunks: &[&str]) -> Vec<ServerSentEvent> {
        let mut decoder = SseDecoder::new();
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(decoder.push_text(chunk));
        }
        events.extend(decoder.finish());
        events
    }

    /// Every place a chunk boundary can fall, as `[before, after]` pieces.
    fn two_piece_splits(text: &str) -> Vec<[&str; 2]> {
        (0..=text.len())
            .map(|at| [&text[..at], &text[at..]])
            .collect()
    }

    fn three_piece_splits(text: &str) -> Vec<[&str; 3]> {
        let mut out = Vec::new();
        for first in 0..=text.len() {
            for second in first..=text.len() {
                out.push([&text[..first], &text[first..second], &text[second..]]);
            }
        }
        out
    }

    /// One chunk per char: the extreme case of a transport that fragments everything.
    fn char_chunks(text: &str) -> Vec<&str> {
        text.char_indices()
            .map(|(start, ch)| &text[start..start + ch.len_utf8()])
            .collect()
    }

    #[test]
    fn chunk_boundaries_never_change_the_decoded_events() {
        for canvas in CANVASES {
            assert!(canvas.is_ascii(), "the split helpers index bytes");
            let whole = decode_with_chunks(&[canvas]);
            for chunks in two_piece_splits(canvas) {
                assert_eq!(
                    decode_with_chunks(&chunks),
                    whole,
                    "two-chunk split {chunks:?} of {canvas:?}"
                );
            }
            for chunks in three_piece_splits(canvas) {
                assert_eq!(
                    decode_with_chunks(&chunks),
                    whole,
                    "three-chunk split {chunks:?} of {canvas:?}"
                );
            }
            assert_eq!(
                decode_with_chunks(&char_chunks(canvas)),
                whole,
                "byte-at-a-time chunks of {canvas:?}"
            );
        }
    }

    #[test]
    fn crlf_split_inside_the_break_keeps_one_event() {
        // "data: a\r\ndata: b\r\n\r\n" is ONE event with data "a\nb".
        let whole = "data: a\r\ndata: b\r\n\r\n";
        let expected = vec![ServerSentEvent {
            event: None,
            data: "a\nb".into(),
            raw: vec!["data: a".into(), "data: b".into()],
        }];
        assert_eq!(decode_with_chunks(&[whole]), expected);
        for chunks in two_piece_splits(whole) {
            assert_eq!(
                decode_with_chunks(&chunks),
                expected,
                "split {chunks:?} must still decode one event"
            );
        }
    }

    #[test]
    fn bare_carriage_return_and_lf_are_line_breaks_too() {
        let expected = vec![ServerSentEvent {
            event: Some("x".into()),
            data: "a".into(),
            raw: vec!["event: x".into(), "data: a".into()],
        }];
        assert_eq!(decode_with_chunks(&["event: x\ndata: a\n\n"]), expected);
        assert_eq!(decode_with_chunks(&["event: x\rdata: a\r\r"]), expected);
        assert_eq!(
            decode_with_chunks(&["event: x\r\ndata: a\r\n\r\n"]),
            expected
        );
    }

    #[test]
    fn a_hanging_carriage_return_is_a_break_at_end_of_stream() {
        let mut hanging = SseDecoder::new();
        assert!(
            hanging.push_text("data: a\r").is_empty(),
            "a trailing \\r waits for the next byte, it does not end a line yet"
        );
        assert_eq!(
            hanging.finish(),
            decode_with_chunks(&["data: a"]),
            "a final \\r is a line break, not part of the line"
        );
    }

    #[test]
    fn no_decoded_line_carries_a_carriage_return() {
        for canvas in CANVASES {
            for chunks in two_piece_splits(canvas) {
                for event in decode_with_chunks(&chunks) {
                    assert!(
                        !event.data.contains('\r'),
                        "data {:?} from {canvas:?} split {chunks:?}",
                        event.data
                    );
                    assert!(
                        event.raw.iter().all(|line| !line.contains('\r')),
                        "raw {:?} from {canvas:?} split {chunks:?}",
                        event.raw
                    );
                }
            }
        }
    }

    #[test]
    fn decodes_a_crlf_capture_split_at_arbitrary_boundaries() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/testdata/prime_inference_glm53_flash.sse"
        ))
        .unwrap();
        let crlf = text.replace('\n', "\r\n");
        let whole = decode_with_chunks(&[&crlf]);
        assert_eq!(whole.len(), 44, "43 chunks + [DONE]");
        assert_eq!(
            whole,
            decode_with_chunks(&[&text]),
            "CRLF breaks must decode exactly like LF ones"
        );

        // Chunk sizes 1..=7 cycling, cut on char boundaries: the shape a transport produces
        // when a carrier splits wherever it likes.
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut size = 1;
        for (offset, ch) in crlf.char_indices() {
            let end = offset + ch.len_utf8();
            if end - start >= size {
                chunks.push(&crlf[start..end]);
                start = end;
                size = if size == 7 { 1 } else { size + 1 };
            }
        }
        chunks.push(&crlf[start..]);
        assert!(chunks.len() > 1000, "the capture is fragmented: {chunks:?}");
        assert_eq!(decode_with_chunks(&chunks), whole);
    }
}
