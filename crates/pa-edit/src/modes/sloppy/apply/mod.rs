//! Sloppy matching and application: pattern parsing, candidate location,
//! rewrite planning, overlap reconciliation, echo recovery, and the atomic
//! apply.
//!
//! Port of `packages/coding-agent/src/edit/sloppy.ts` lines 1651–4258.
//!
//! This module was one 2 867-line file. It is now this facade plus one module
//! per stage of the pipeline:
//!
//! | module | holds |
//! |---|---|
//! | `pattern` | `*** Find` normalization and pattern parsing into the token stream |
//! | `occurrences` | literal and fuzzy occurrence search, and the normalized↔source offset maps |
//! | `candidates` | the match-mode ladder that turns occurrences into candidates |
//! | `diagnostics` | line-numbered previews, closest-fragment search, and the no-match error |
//! | `locate` | candidate location for one operation and its recovery ladder |
//! | `rewrite` | `*** Replace` rendering and inline-selection framing |
//! | `run` | the atomic apply: plan, reconcile overlaps, splice |
//!
//! Dependencies run one way — `pattern` ← `occurrences` ← `candidates` ←
//! `locate` ← `run` — with `rewrite` and `diagnostics` as the two side stages
//! each of those calls into. The public surface is re-exported below, so
//! `apply::apply_sloppy`, `apply::normalize_text` and the rest keep their paths
//! for [`super::parse`] and [`super::SloppyEngine`].

mod candidates;
mod diagnostics;
mod locate;
mod occurrences;
mod pattern;
mod rewrite;
mod run;

use std::path::Path;

use super::types::ATOMICITY_NOTICE;

use crate::{error::EditError, store::EditStore};

use self::run::apply_operations;

pub use self::pattern::normalize_text;
pub(crate) use self::{
    locate::{closest_desired_block, diff_shaped_candidates, is_diff_shaped, locate},
    pattern::parse_pattern,
};

/// State shared by every operation in one file section.
pub struct ApplyContext<'a> {
    pub path: &'a str,
    pub notes: &'a mut Vec<String>,
    pub store: &'a EditStore,
    pub canonical: &'a Path,
    /// The payload is still streaming: a trailing `*** Find` without its
    /// `*** Replace` is dropped instead of run through the recovery ladder.
    pub streaming: bool,
}

/// Parse, locate, and atomically apply one sloppy section.
pub fn apply_sloppy(
    content: &str,
    input: &str,
    mut context: ApplyContext<'_>,
) -> Result<String, EditError> {
    apply_operations(content, input, &mut context).map_err(|error| {
        let message = error.to_string();
        EditError::matched(if message.contains(ATOMICITY_NOTICE) {
            message
        } else {
            format!("{ATOMICITY_NOTICE}\n{message}")
        })
    })
}
