//! Compact symbol bodies (S15) and the EOF-terminated range convention.
//!
//! FORK-ONLY (allspark). A Rust port of Serena's `SymbolBody`
//! (`solidlsp/ls.py:232-311`). A body is stored as four integer coordinates
//! plus a *shared* reference to the file's line buffer, so many bodies from the
//! same file cost ~40 bytes each (4 `u32` + one `Arc` pointer) instead of a
//! `String` clone per symbol. `SymbolBodyFactory` hands out the shared buffer.
//!
//! EOF convention (Serena parity): a range whose end is `(len(lines), 0)`
//! means "through end of file" and is clamped; *any other* out-of-range
//! coordinate is a hard error ([`BodyError::InvalidTextLocation`]) — the body
//! is never silently guessed, because a wrong body would corrupt an edit.

use std::sync::Arc;

use lsp_types::Range;
use thiserror::Error;

/// A body range that does not address real text.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BodyError {
    /// A coordinate points past the end of the buffer (and is not the
    /// `(len, 0)` EOF sentinel).
    #[error(
        "text location out of range: line {line}, column {column} \
         (buffer has {lines} line(s))"
    )]
    InvalidTextLocation {
        /// The offending (0-based) line.
        line: u32,
        /// The offending (0-based) character column.
        column: u32,
        /// How many lines the shared buffer actually holds.
        lines: usize,
    },
}

/// A lazily-materialized slice of a file: four coordinates into a shared line
/// buffer. Cloning is cheap (an `Arc` bump plus four integers).
#[derive(Debug, Clone)]
pub struct SymbolBody {
    lines: Arc<[String]>,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
}

impl SymbolBody {
    /// Build a body from a shared line buffer and a range. No validation is
    /// done here — [`SymbolBody::get_text`] validates on access, so a body can
    /// be constructed for a range that the EOF convention will later clamp.
    #[must_use]
    pub fn new(lines: Arc<[String]>, range: Range) -> Self {
        Self {
            lines,
            start_line: range.start.line,
            start_col: range.start.character,
            end_line: range.end.line,
            end_col: range.end.character,
        }
    }

    /// Convenience constructor that splits `text` into lines once.
    #[must_use]
    pub fn from_text(text: &str, range: Range) -> Self {
        Self::new(Arc::from(split_lines(text)), range)
    }

    /// The addressed range (0-based, LSP coordinates).
    #[must_use]
    pub fn range(&self) -> Range {
        Range::new(
            lsp_types::Position::new(self.start_line, self.start_col),
            lsp_types::Position::new(self.end_line, self.end_col),
        )
    }

    /// Whether the range uses the `(len(lines), 0)` EOF sentinel.
    #[must_use]
    pub fn is_eof_terminated(&self) -> bool {
        self.end_line as usize == self.lines.len() && self.end_col == 0
    }

    /// The number of lines in the shared buffer.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Materialize the body text.
    ///
    /// # Errors
    /// Returns [`BodyError::InvalidTextLocation`] when a coordinate is out of
    /// range and is not the EOF sentinel.
    pub fn get_text(&self) -> Result<String, BodyError> {
        let len = self.lines.len();
        let start_line = self.start_line as usize;
        let end_line = self.end_line as usize;
        let start_col = self.start_col as usize;
        let end_col = self.end_col as usize;

        if start_line > len {
            return Err(self.invalid(self.start_line, self.start_col));
        }
        let eof = self.is_eof_terminated();

        // The tail of the start line, from `start_col` to the line end (or to
        // `end_col` on a single-line, non-EOF body).
        let start_tail = if start_line == len {
            // Only reachable for the EOF sentinel with an empty tail.
            if !(eof && start_col == 0) {
                return Err(self.invalid(self.start_line, self.start_col));
            }
            String::new()
        } else {
            let line = &self.lines[start_line];
            let chars = line.chars().count();
            if start_col > chars {
                return Err(self.invalid(self.start_line, self.start_col));
            }
            let upper = if eof || start_line != end_line {
                chars
            } else {
                if end_col > chars {
                    return Err(self.invalid(self.end_line, self.end_col));
                }
                end_col
            };
            char_slice(line, start_col, upper)
        };

        if eof {
            if start_line == len {
                return Ok(String::new());
            }
            let mut out = start_tail;
            for line in &self.lines[start_line + 1..] {
                out.push('\n');
                out.push_str(line);
            }
            return Ok(out);
        }

        if end_line >= len {
            return Err(self.invalid(self.end_line, self.end_col));
        }
        if start_line == end_line {
            return Ok(start_tail);
        }
        let end_chars = self.lines[end_line].chars().count();
        if end_col > end_chars {
            return Err(self.invalid(self.end_line, self.end_col));
        }

        let mut out = start_tail;
        for line in &self.lines[start_line + 1..end_line] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&char_slice(&self.lines[end_line], 0, end_col));
        Ok(out)
    }

    fn invalid(&self, line: u32, column: u32) -> BodyError {
        BodyError::InvalidTextLocation {
            line,
            column,
            lines: self.lines.len(),
        }
    }
}

/// Split source text into lines (without trailing newlines), matching how the
/// EOF sentinel counts `len(lines)`: `"a\nb\n"` has two lines, and `"a\nb"`
/// also has two.
#[must_use]
pub fn split_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

/// The character range `[start, end)` of `line`.
fn char_slice(line: &str, start: usize, end: usize) -> String {
    line.chars().skip(start).take(end.saturating_sub(start)).collect()
}

/// Hands out [`SymbolBody`]s that all share one file's line buffer, so a file's
/// symbols cost one buffer plus a small header each.
#[derive(Debug, Clone)]
pub struct SymbolBodyFactory {
    lines: Arc<[String]>,
}

impl SymbolBodyFactory {
    /// Build a factory over `text`.
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            lines: Arc::from(split_lines(text)),
        }
    }

    /// Build a factory over an already-split, shared buffer.
    #[must_use]
    pub fn from_lines(lines: Arc<[String]>) -> Self {
        Self { lines }
    }

    /// A body addressing `range` in the shared buffer.
    #[must_use]
    pub fn body(&self, range: Range) -> SymbolBody {
        SymbolBody::new(Arc::clone(&self.lines), range)
    }

    /// A shared handle to the underlying buffer.
    #[must_use]
    pub fn lines(&self) -> Arc<[String]> {
        Arc::clone(&self.lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
        Range::new(Position::new(sl, sc), Position::new(el, ec))
    }

    #[test]
    fn single_line_slice_trims_both_ends() {
        let body = SymbolBody::from_text("hello world\nnext\n", range(0, 6, 0, 11));
        assert_eq!(body.get_text().unwrap(), "world");
    }

    #[test]
    fn multi_line_slice_keeps_inner_lines_verbatim() {
        let text = "fn f() {\n    let x = 1;\n    x\n}\n";
        let body = SymbolBody::from_text(text, range(1, 4, 2, 5));
        assert_eq!(body.get_text().unwrap(), "let x = 1;\n    x");
    }

    #[test]
    fn eof_sentinel_clamps_to_end_of_file() {
        // `(len, 0)` means "through EOF": the trailing newline splits into an
        // empty final line, so len == 3 for this text.
        let text = "a\nb\nc";
        let lines = split_lines(text);
        assert_eq!(lines.len(), 3);
        let body = SymbolBody::from_text(text, range(1, 0, 3, 0));
        assert!(body.is_eof_terminated());
        assert_eq!(body.get_text().unwrap(), "b\nc");
    }

    #[test]
    fn non_eof_out_of_range_is_an_error() {
        let body = SymbolBody::from_text("a\nb\n", range(0, 0, 5, 0));
        assert!(!body.is_eof_terminated());
        let err = body.get_text().unwrap_err();
        assert!(matches!(err, BodyError::InvalidTextLocation { line: 5, .. }));
    }

    #[test]
    fn column_past_line_end_is_an_error() {
        let body = SymbolBody::from_text("ab\ncd\n", range(0, 0, 0, 9));
        assert!(matches!(
            body.get_text().unwrap_err(),
            BodyError::InvalidTextLocation { line: 0, column: 9, .. }
        ));
    }

    #[test]
    fn columns_are_characters_not_bytes() {
        // "héllo" — 'é' is two bytes but one character.
        let body = SymbolBody::from_text("héllo\n", range(0, 1, 0, 5));
        assert_eq!(body.get_text().unwrap(), "éllo");
    }

    #[test]
    fn factory_shares_one_buffer_across_bodies() {
        let factory = SymbolBodyFactory::new("one\ntwo\nthree\n");
        let a = factory.body(range(0, 0, 0, 3));
        let b = factory.body(range(1, 0, 1, 3));
        assert_eq!(a.get_text().unwrap(), "one");
        assert_eq!(b.get_text().unwrap(), "two");
        // Both bodies point at the same allocation.
        assert!(Arc::ptr_eq(&factory.lines(), &factory.lines()));
    }
}
