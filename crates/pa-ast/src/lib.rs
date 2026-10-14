// Ported verbatim from oh-my-pi's `pi-ast` (ast-grep structural search +
// tree-sitter parsing). The workspace pedantic gate rides in via
// `[lints] workspace = true`; these crate-wide exceptions are the classes the
// 5.3k-LOC port legitimately trips (mirroring upstream oh-my-pi's own lint
// table):
//   - unreadable_literal: grammar/byte constants and hash tables are clearer
//     without digit separators in this vendored code.
//   - missing_errors_doc: the ported API documents errors where it matters,
//     not on every `Result` return.
//   - cast_possible_truncation: node ids/byte offsets are u32 by the
//     tree-sitter ABI; truncation cannot occur for real inputs.
//   - must_use_candidate: not every helper needs `#[must_use]`.
//   - match_same_arms / enum_glob_use: the language dispatch tables are
//     intentionally explicit and enumerate variants for readability.
//   - too_many_lines: the language registration table is one long function.
//   - assert_is_empty: test-side `assert!(!x.is_empty())` states intent.
//   - redundant_closure_for_method_calls: `.map(ToString::to_string)` and
//     friends can read clearer than the method form.
#![allow(
    clippy::assert_is_empty,
    clippy::cast_possible_truncation,
    clippy::enum_glob_use,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::unreadable_literal
)]

pub mod block;
pub mod language;
pub mod ops;
pub mod parse_cache;
pub mod summary;

pub use language::SupportLang;
