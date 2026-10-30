//! Minimal JSON string escaping shared by the hand-built surface payloads.
//!
//! `surface.rs` and `lsp_surface.rs` both emit JSON by string concatenation (a deliberate
//! choice: the payloads are large and highly regular, and a `serde_json::Value` build was
//! measurably slower). Each carried its OWN copy of this escaper — two places for the one
//! rule "a quote or a control character must not break the payload", and a fix in one left
//! the other wrong (review: duplicate-body scan).

/// Escape `value` for embedding inside a JSON string.
///
/// Handles the two characters that break the payload (`"`, `\\`) and the control
/// characters, which JSON forbids raw.
pub(crate) fn escape(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one escaper both payloads use. Asserting the exact bytes is the point: a
    /// silent divergence between two copies was the defect, so the output is pinned.
    #[test]
    fn escapes_the_characters_that_break_a_json_string() {
        assert_eq!(escape("plain"), "plain");
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("a\nb\rc\td"), "a\\nb\\rc\\td");
        assert_eq!(escape("\u{1}"), "\\u0001");
        // A non-ASCII character is passed through, not escaped: the payloads are UTF-8
        // and JSON allows it raw.
        assert_eq!(escape("é"), "é");
    }
}
