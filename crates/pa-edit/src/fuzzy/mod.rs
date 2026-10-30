//! Text/sequence matching primitives shared by `replace`, `patch`, and
//! `sloppy`: Levenshtein similarity, whole-block fuzzy search,
//! line-sequence placement, and context-line placement.
//!
//! The file was one 1 946-line module; it is now one module per
//! responsibility. The call graph is acyclic, and the arrows only point
//! downward:
//!
//! | module | holds |
//! |---|---|
//! | `levenshtein` | the scalar distance and the within-a-bound version |
//! | `distance` | [`PatternDistance`] (bit-parallel) and [`similarity`] |
//! | `matches` | the occurrence tally both ladders count into |
//! | `block` | the exact pass, the window scan, [`find_match`] and its types |
//! | `sequence` | the line-sequence ladder and [`seek_sequence`] |
//! | `context` | the context-line ladder and [`find_context_line`] |
//! | `report` | the two error formatters and the preview window |
//! | `replace` | [`replace_text`] and its outcome types |
//! | `tests` | the tests, grouped by the contract each one pins |
//!
//! ```text
//! replace ──> block, distance
//! sequence ──> block, matches, distance
//! context ──> matches, distance
//! block ──> report, distance
//! report ──> (block's types only)
//! distance ──> levenshtein
//! ```
//!
//! The public surface is re-exported here, so `crate::fuzzy::find_match`,
//! `crate::fuzzy::PatternDistance`, and the threshold constants keep their
//! paths for `modes::{replace, patch}`, `modes::sloppy::apply`, and the
//! parity tests.

mod block;
mod context;
mod distance;
mod levenshtein;
mod matches;
mod replace;
mod report;
mod sequence;

#[cfg(test)]
mod tests;

/// Default similarity threshold for fuzzy matching.
pub const DEFAULT_FUZZY_THRESHOLD: f64 = 0.95;
/// Threshold for sequence-based fuzzy matching.
pub const SEQUENCE_FUZZY_THRESHOLD: f64 = 0.92;
/// Fallback threshold for line-based matching without indentation depth.
pub const FALLBACK_THRESHOLD: f64 = 0.8;
/// Threshold for context-line fuzzy matching.
pub const CONTEXT_FUZZY_THRESHOLD: f64 = 0.8;
/// Minimum normalized pattern length for partial matching.
pub const PARTIAL_MATCH_MIN_LENGTH: usize = 6;
/// Minimum pattern-to-line ratio for ambiguous substring matching.
pub const PARTIAL_MATCH_MIN_RATIO: f64 = 0.3;
/// Number of surrounding lines in occurrence previews.
pub const OCCURRENCE_PREVIEW_CONTEXT: usize = 5;
/// Maximum displayed line length in occurrence previews.
pub const OCCURRENCE_PREVIEW_MAX_LEN: usize = 80;
/// Occurrence previews and indices recorded before truncation.
pub const MAX_RECORDED_MATCHES: usize = 5;
/// A fuzzy hit at or above this confidence can dominate weaker siblings.
pub const DOMINANT_FUZZY_MIN_CONFIDENCE: f64 = 0.97;
/// Minimum confidence gap for a dominant fuzzy hit.
pub const DOMINANT_FUZZY_DELTA: f64 = 0.08;
/// Threshold for the final character-level sequence fallback.
pub const CHARACTER_MATCH_THRESHOLD: f64 = 0.92;

pub use block::{ExcludedRange, FindMatchOptions, FuzzyMatch, MatchOutcome, find_match};
pub use context::{ContextLineResult, ContextMatchStrategy, find_context_line};
pub use distance::{PatternDistance, similarity};
pub use levenshtein::{levenshtein_distance, levenshtein_within};
pub use replace::{ReplaceOutcome, ReplaceResult, replace_text};
pub use report::{format_match_error, format_occurrence_error};
pub use sequence::{
    SequenceMatchStrategy, SequenceSearchResult, find_closest_sequence_match, seek_sequence,
};
