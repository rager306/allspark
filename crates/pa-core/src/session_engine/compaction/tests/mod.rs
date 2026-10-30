//! The compaction tests, grouped by subject.
//!
//! Each subject is its own module because each pins a different contract: what
//! an estimate costs, when the trigger fires, what the wire actually carries,
//! where a cut may land, and what the summarizer is told. The split is of the
//! tests only — every test is the one that was there before, and the fixtures
//! they share live in [`fixtures`].

mod cut;
mod fixtures;
mod plan;
mod prompt;
mod threshold;
mod tokens;
