//! Sloppy payload parsing: section headers → internal op stream →
//! [`super::types::Operation`]s, including every parse-time recovery.
//! Port of `packages/coding-agent/src/edit/sloppy.ts` lines 1–1650.
//!
//! This module was one 2 218-line file. It is now this facade plus one module
//! per half of the payload's own shape:
//!
//! | module | holds |
//! |---|---|
//! | [`sections`] | envelope stripping, section splitting, the normalize/recover ladder, and the payload renderers |
//! | [`operations`] | the op stream's per-operation half: marker lines, the recovery ladder one operation can need, and operation assembly |
//!
//! [`sections::parse_section`] walks the stream and calls into `operations` for
//! every `*** Find`/`*** Replace` it closes. The reverse edge is small and
//! named: `operations`' two finishing passes render copy-ready payloads through
//! [`sections::ir_to_payload`] and normalize through
//! [`sections::normalize_block`].
//!
//! The marker alphabet and every pattern are one table, declared here so
//! neither module repeats a constant. `macro_rules!` is textually scoped, which
//! is what lets both modules invoke `regex!` without importing it; the macro
//! spells `std`/`regex` paths itself so it needs no import of its own.
//!
//! The public surface is re-exported below, so `parse::parse_operations`,
//! `parse::split_sloppy_sections`, `parse::marker_line_content` and the rest
//! keep their paths for [`super::apply`], [`super::SloppyEngine`], and the
//! parity tests.

macro_rules! regex {
    ($name:ident, $pattern:literal) => {
        static $name: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new($pattern).expect(concat!("valid ", stringify!($name), " regex"))
        });
    };
}

mod operations;
mod sections;

use super::types::markers;
use crate::error::EditError;

const OPENER: &str = markers::OPEN;
const REWRITE_HEADER: &str = markers::PUT;
// JSON keeps literal insertion lines out of legacy rewrite recovery and control
// parsing.
const AFTER_HEADER: &str = "\0AFTER ";
const BEFORE_HEADER: &str = "\0BEFORE ";
const SELECT_OPEN: &str = markers::SELECT_OPEN;
const SELECT_CLOSE: &str = markers::SELECT_CLOSE;
const SELECT_DIVIDER: &str = markers::SELECT_DIVIDER;
const GAP: &str = markers::GAP;
const ADD_LINE: &str = markers::ADD;
const REMOVE_LINE: &str = markers::REMOVE;

const LITERAL_OPEN: &str = "\0V8LITOPEN\0";
const LITERAL_CLOSE: &str = "\0V8LITCLOSE\0";
const LITERAL_DIVIDER: &str = "\0V8LITDIV\0";

regex!(
    HEADER_RE,
    r"(?iu)^\*{3}[ \t]+(Edit[ \t]+File|Find|Replace|Insert[ \t]+After|Insert[ \t]+Before)(?::|[ \t]|$)[ \t]*(.*)$"
);

regex!(
    ENVELOPE_WORDS_RE,
    r"(?iu)^\s*(?:Begin|End)(?:\s+of)?\s+(?:patch|edits?|file)\b"
);

regex!(
    ENVELOPE_LINE_RE,
    r"(?iu)^\*{3}\s*(?:(?:Begin|End)(?:\s+of)?\s+(?:patch|edits?|file)|Abort|Update File:|Add File:|Delete File:)"
);

regex!(ENVELOPE_END_RE, r"(?iu)^\*{3}\s*End\b");

regex!(FENCE_LINE_RE, r"^\s*(?:```|~~~)");

regex!(ORDINAL_OPENER_RE, r"^«[1-9]\d*$");

regex!(
    OUTER_FENCE_RE,
    r"(?iu)^```(?:text|xml|html|typescript|ts|tsx|javascript|js)?\s*$"
);

regex!(GLUED_CONTROL_RE, r"^[ \t]*(«\*?|»)([ \t]+\S.*)$");

regex!(
    SHOWING_LINES_RE,
    r"(?iu)^\[(?:Showing lines\b|(?:…|\.\.\.)\d+ln elided\b).*\]$"
);

regex!(ELIDED_LINE_RE, r"^\d+(?:-\d+)?:\s*(?:…|\.\.\.)\s*$");

regex!(
    READ_MORE_NOTICE_RE,
    r"(?iu)^\[\s*(?:\d+\s+)?more\s+lines?\s+in\s+.+\.\s+use\s+.+\s+to\s+continue\]$"
);

regex!(NUMBERED_LINE_RE, r"^\s*\d+\s*[:|]");

regex!(NUMBERED_LITERAL_RE, r#"^\s*\d+\s*[:|]\s*(?:\d|[\"'`])"#);

regex!(
    SELECTION_ONLY_RE,
    r"^\x{27EA}([^\x{27EA}\x{27EB}]*)\x{27EB}$"
);

regex!(REFERENCE_RE, r"^»([1-9]\d*)$");

regex!(
    BARE_DESIRED_RE,
    r"\x{27EA}[^\x{27EA}\x{27EB}\x{2502}\n…]+\x{27EB}"
);

regex!(
    BARE_DESIRED_CAPTURE_RE,
    r"\x{27EA}([^\x{27EA}\x{27EB}\x{2502}\n…]+)\x{27EB}"
);

regex!(UNICODE_TOKEN_RE, r"[\p{L}\p{N}_$]+");

regex!(
    STRAY_DIVIDER_RE,
    r"\x{27EA}([^\x{27EA}\x{27EB}\x{2502}\n]*)\x{27EB}([^\x{27EA}\x{27EB}\x{2502}\n]*)\x{27EB}"
);

regex!(
    DIRECTIVE_RE,
    r"^\x{27EA}([^\x{27EA}\x{27EB}\x{2502}]+)\x{2502}([^\x{27EA}\x{27EB}\x{2502}]*)\x{27EB}$"
);

regex!(
    EMPTY_SELECTION_TAIL_RE,
    r"(?:^|\n)[ \t]*\x{27EA}\x{27EB}[ \t]*$"
);

fn parse_error(message: impl Into<String>) -> EditError {
    EditError::parse(message)
}

fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub(super) use self::{operations::decode_literal_markers, sections::parse_section};
pub(crate) use self::{operations::missing_unmarked_lines, sections::edit_header};
pub use self::{
    operations::{create_operation, has_marker_lines, marker_line_content},
    sections::{
        extract_inline_sloppy_regions, ir_to_payload, normalize_input, operation_payload,
        parse_operations, split_sloppy_sections,
    },
};

#[cfg(test)]
mod tests {
    use super::{
        ADD_LINE, has_marker_lines, marker_line_content, operation_payload, parse_operations,
    };

    #[test]
    fn marker_lines_keep_indentation() {
        assert_eq!(
            marker_line_content("  ＋value", ADD_LINE).as_deref(),
            Some("  value")
        );
        assert!(has_marker_lines("anchor\n  －old"));
    }

    #[test]
    fn operation_payload_restores_inline_desired_text() {
        let operation = parse_operations(
            "«\nconst \u{27ea}old\u{2502}new\u{27eb};",
            "const old;\n",
            "a.ts",
        )
        .unwrap()
        .remove(0);
        let payload = operation_payload(&operation, "a.ts", true, None);
        let round_trip = parse_operations(&payload, "const old;\n", "a.ts").unwrap();
        assert_eq!(round_trip[0].rewrite, operation.rewrite);
        assert!(round_trip[0].all);
    }
}
