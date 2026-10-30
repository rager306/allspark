//! The priced projection of the next request: messages plus the fixed prefix
//! (system prompt and tool schemas) plus the output reserve.

use super::tokens::{div4, ContextTokensEstimate};

/// The one priced projection of the NEXT request.
///
/// Every input the provider will charge for belongs here, because the three
/// numbers this module used to compute disagreed by construction:
///
/// - `estimate_context_tokens` counts the MESSAGES (anchored on the last
///   provider usage),
/// - `find_cut_point` counts retained ENTRIES,
/// - the wire carries messages PLUS the system prompt PLUS the tool schemas.
///
/// Measured 2026-10-28: the static prompt layer alone is ~6 300 tokens
/// (25 174 chars over `prompts/layers/*.md`) and every tool schema adds its own;
/// neither appeared in any estimate, so a session could reach the provider's
/// limit while the trigger still said "not due".
///
/// This is the donor's `projectUnit` lesson applied one layer up: recommend,
/// explain and apply must share ONE validity rule, or the system offers work it
/// then refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextPlan {
    /// Tokens the retained messages are expected to cost.
    pub messages: u64,
    /// Tokens of the system prompt, static layers plus the dynamic tail.
    pub system_prompt: u64,
    /// Tokens of the tool schemas the request advertises.
    pub tool_schemas: u64,
    /// Budget the request claims for its own output.
    pub output_reserve: u64,
    /// The model's window.
    pub context_window: u64,
}

impl ContextPlan {
    /// Input tokens the request will be billed for and must fit.
    #[must_use]
    pub fn input(&self) -> u64 {
        self.messages
            .saturating_add(self.system_prompt)
            .saturating_add(self.tool_schemas)
    }

    /// Input plus the reserved output: what the window must hold.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.input().saturating_add(self.output_reserve)
    }

    /// Whether this plan cannot fit the window at all.
    ///
    /// Separate from "should compact": a plan can exceed the window while there
    /// is nothing left to cut (a single huge message), and the caller needs to
    /// tell "cut more" from "this cannot be sent".
    #[must_use]
    pub fn overflows(&self) -> bool {
        self.context_window > 0 && self.total() > self.context_window
    }

    /// The same plan with the fixed prefix priced in.
    #[must_use]
    pub fn with_prefix(mut self, system_prompt: u64, tool_schemas: u64) -> Self {
        self.system_prompt = system_prompt;
        self.tool_schemas = tool_schemas;
        self
    }

    /// The plan for one request, with the prefix priced only when it is NOT already
    /// counted.
    ///
    /// This rule is the whole subtlety of pricing a request, and it lives here rather
    /// than at each caller because two callers already got it wrong in opposite
    /// directions: adding the prefix to an ANCHORED estimate double-counts it (the
    /// provider's usage is the count for a request that carried the prompt), while
    /// omitting it on an UNANCHORED one under-counts by the prompt's whole size —
    /// measured here at 18 007 tokens, 14% of a 128k window.
    ///
    /// `anchored` is `estimate.last_usage_index.is_some()`; `prefix` is the prompt plus
    /// the tool schemas.
    #[must_use]
    pub fn for_request(
        estimate: ContextTokensEstimate,
        anchored: bool,
        prefix: (u64, u64),
        output_reserve: u64,
        context_window: u64,
    ) -> Self {
        Self {
            messages: estimate.tokens,
            // With an anchor the prefix is already inside `messages`; without one it
            // must be added, or the trigger prices a request that carries no prompt.
            system_prompt: if anchored { 0 } else { prefix.0 },
            tool_schemas: if anchored { 0 } else { prefix.1 },
            output_reserve,
            context_window,
        }
    }
}

/// Estimate tokens for a text the same way the message estimator does
/// (`chars / 4`), so the prefix and the messages are priced in one unit.
#[must_use]
pub fn estimate_text_tokens(text: &str) -> u64 {
    div4(text.chars().count() as u64)
}

/// Estimate the tokens one tool schema costs on the wire.
///
/// Serialized, because that is what the provider receives: the definition's
/// name, description and JSON schema all travel, and a schema with a long
/// description costs exactly as much as a long message.
#[must_use]
pub fn estimate_tool_schema_tokens(tool: &dyn pa_agent::types::AgentTool) -> u64 {
    let mut total = estimate_text_tokens(tool.name());
    total = total.saturating_add(estimate_text_tokens(tool.description()));
    if let Ok(serialized) = serde_json::to_string(tool.parameters()) {
        total = total.saturating_add(estimate_text_tokens(&serialized));
    }
    total
}
