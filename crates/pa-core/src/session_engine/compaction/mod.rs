//! Context compaction: pure decision logic — token estimation, cut
//! points, summarization prompts. The LLM summarizer call and file-op
//! details land with the provider integration slice.
//!
//! The pass is one module per responsibility:
//!
//! | module | holds |
//! |---|---|
//! | [`tokens`] | the chars/4 estimators: message, entry, whole context |
//! | [`plan`] | [`ContextPlan`], the priced projection of the next request |
//! | [`threshold`] | the threshold formula and the two "is compaction due" questions |
//! | [`cut`] | legal cut positions, the tool-pair predicate, the retention walk |
//! | [`prompt`] | the summarization, split-turn, and retry prompts |
//! | [`tests`] | the tests, grouped by the contract each one pins |
//!
//! The public surface is re-exported here, so every former
//! `session_engine::compaction::NAME` path is unchanged.

mod cut;
mod plan;
mod prompt;
mod threshold;
mod tokens;

pub const DEFAULT_RESERVE_TOKENS: u64 = 16_384;
pub const DEFAULT_KEEP_RECENT_TOKENS: u64 = 20_000;

/// The percentage of the context window past which a threshold compaction
/// always fires (guards estimate drift on large windows).
pub const COMPACT_THRESHOLD_RATIO: f64 = 0.95;
pub const DEFAULT_THRESHOLD_PERCENT: f64 = COMPACT_THRESHOLD_RATIO * 100.0;

/// The smallest headroom kept under the combined input+output ceiling (the
/// chars/4 estimate error); `reserve_tokens` raises it when larger.
pub const COMBINED_LIMIT_HEADROOM_FLOOR: u64 = 4_096;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactionSettings {
    pub enabled: bool,
    /// Requested percentage ceiling; output and reserve headroom can trigger earlier.
    pub threshold_percent: f64,
    /// Headroom under the combined input+output ceiling (a floor of
    /// [`COMBINED_LIMIT_HEADROOM_FLOOR`]): combined-limit providers reject
    /// `input + requested output > contextWindow`.
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_percent: DEFAULT_THRESHOLD_PERCENT,
            reserve_tokens: DEFAULT_RESERVE_TOKENS,
            keep_recent_tokens: DEFAULT_KEEP_RECENT_TOKENS,
        }
    }
}

// The threshold tests live beside the other session-engine test modules; the
// path keeps them there rather than moving them under the split's directory.
#[cfg(test)]
#[path = "../compaction_threshold_tests.rs"]
mod threshold_tests;

#[cfg(test)]
mod tests;

pub use cut::{find_cut_point, find_valid_cut_points, orphans_a_tool_result, CutPointResult};
pub use plan::{estimate_text_tokens, estimate_tool_schema_tokens, ContextPlan};
pub use prompt::{build_summarization_prompt, TURN_PREFIX_SUMMARIZATION_PROMPT};
pub(crate) use prompt::{SHORTER_SUMMARY_INSTRUCTION, SUMMARY_TRUNCATED_MARKER};
pub use threshold::{
    compaction_threshold, plan_compaction_due, request_output_budget, should_compact,
    threshold_compaction_due,
};
pub use tokens::{
    calculate_context_tokens, estimate_context_tokens, estimate_entry_tokens, estimate_tokens,
    get_last_assistant_usage, ContextTokensEstimate,
};
pub(crate) use tokens::message_timestamp;
