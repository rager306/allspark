//! Byte-offset <-> LSP-position conversion (UTF-16 columns), plus small
//! source-text helpers shared by the symbol layer and the client.
//!
//! FORK-ONLY (allspark). tree-sitter addresses text by byte offset; LSP
//! addresses it by 0-based line and *UTF-16 code-unit* column. Everything that
//! crosses between the two goes through here so the conversion lives in one
//! place.

use lsp_types::Position;

/// Byte offsets of the start of every line in `text`.
#[derive(Debug, Clone)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    /// Build the index for `text`.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(i + 1);
            }
        }
        Self { starts }
    }

    /// The 0-based line containing `byte`.
    #[must_use]
    pub fn line_of(&self, byte: usize) -> usize {
        match self.starts.binary_search(&byte) {
            Ok(line) => line,
            Err(next) => next.saturating_sub(1),
        }
    }

    /// The byte offset where `line` begins, clamped to the buffer length.
    #[must_use]
    pub fn line_start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or_else(|| {
            self.starts.last().copied().unwrap_or(0)
        })
    }
}

/// The LSP position (UTF-16 column) of byte offset `byte` within `text`.
#[must_use]
pub fn position_at(text: &str, index: &LineIndex, byte: usize) -> Position {
    let byte = byte.min(text.len());
    let line = index.line_of(byte);
    let start = index.line_start(line);
    let column = text[start..byte].chars().map(char::len_utf16).sum::<usize>();
    Position::new(u32::try_from(line).unwrap_or(u32::MAX), u32::try_from(column).unwrap_or(u32::MAX))
}

/// The byte offset of an LSP position within `text`.
///
/// A column past the end of its line clamps to the line's end (LSP servers do
/// this too when a range overshoots).
#[must_use]
pub fn byte_offset(text: &str, index: &LineIndex, position: Position) -> usize {
    let line = position.line as usize;
    if line >= index.starts.len() {
        return text.len();
    }
    let start = index.line_start(line);
    let line_end = text[start..]
        .find('\n')
        .map_or(text.len(), |rel| start + rel);
    let mut utf16 = 0u32;
    for (byte_idx, ch) in text[start..line_end].char_indices() {
        if utf16 >= position.character {
            return start + byte_idx;
        }
        utf16 += u32::try_from(ch.len_utf16()).unwrap_or(1);
    }
    line_end
}

/// The byte range `[start, end)` addressed by an LSP range.
#[must_use]
pub fn byte_range(text: &str, index: &LineIndex, range: &lsp_types::Range) -> (usize, usize) {
    (
        byte_offset(text, index, range.start),
        byte_offset(text, index, range.end),
    )
}

/// Build an LSP range from byte offsets.
#[must_use]
pub fn range_from_bytes(text: &str, index: &LineIndex, start: usize, end: usize) -> lsp_types::Range {
    lsp_types::Range::new(position_at(text, index, start), position_at(text, index, end))
}

/// Count the lines in `text` the way [`crate::symbols_body::split_lines`] does
/// (a trailing newline does not add a line).
#[must_use]
pub fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_ascii_positions() {
        let text = "alpha\nbeta\ngamma\n";
        let index = LineIndex::new(text);
        assert_eq!(byte_offset(text, &index, Position::new(1, 0)), 6);
        assert_eq!(position_at(text, &index, 6), Position::new(1, 0));
        assert_eq!(byte_offset(text, &index, Position::new(2, 3)), 14);
        assert_eq!(position_at(text, &index, 14), Position::new(2, 3));
    }

    #[test]
    fn columns_are_utf16_units() {
        // '😀' is one char but two UTF-16 units, four UTF-8 bytes.
        let text = "a😀b\n";
        let index = LineIndex::new(text);
        assert_eq!(byte_offset(text, &index, Position::new(0, 0)), 0);
        assert_eq!(byte_offset(text, &index, Position::new(0, 1)), 1);
        assert_eq!(byte_offset(text, &index, Position::new(0, 3)), 5);
        assert_eq!(position_at(text, &index, 5), Position::new(0, 3));
    }

    #[test]
    fn out_of_range_column_clamps_to_line_end() {
        let text = "ab\ncd\n";
        let index = LineIndex::new(text);
        assert_eq!(byte_offset(text, &index, Position::new(0, 99)), 2);
        assert_eq!(byte_offset(text, &index, Position::new(9, 0)), text.len());
    }

    #[test]
    fn byte_range_handles_multiline() {
        let text = "one\ntwo\nthree\n";
        let index = LineIndex::new(text);
        let (s, e) = byte_range(text, &index, &lsp_types::Range::new(Position::new(1, 0), Position::new(2, 5)));
        assert_eq!(&text[s..e], "two\nthree");
    }
}
