//! Context compaction: pure decision logic — token estimation, cut
//! points, summarization prompts. The LLM summarizer call and file-op
//! details land with the provider integration slice.

use crate::tools::render_utils::{get_image_dimensions_prefix, IMAGE_DIMENSIONS_PREFIX_BYTES};
use pa_types::ai::AssistantContentBlock;
use pa_types::session::{AgentMessage, FileEntry};
use std::fmt::Write as _;

#[cfg(test)]
#[path = "compaction_threshold_tests.rs"]
mod threshold_tests;

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

/// Context tokens from usage: `totalTokens` when present, else the sum.
/// Output counts: the response becomes part of the next prompt.
#[must_use]
pub fn calculate_context_tokens(usage: &pa_types::ai::Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

fn assistant_usage(message: &AgentMessage) -> Option<pa_types::ai::Usage> {
    let AgentMessage::Assistant(assistant) = message else {
        return None;
    };
    match assistant.stop_reason {
        pa_types::ai::StopReason::Aborted | pa_types::ai::StopReason::Error => None,
        _ => Some(assistant.usage),
    }
}

/// Last non-aborted assistant usage from session entries.
#[must_use]
pub fn get_last_assistant_usage(entries: &[FileEntry]) -> Option<pa_types::ai::Usage> {
    for entry in entries.iter().rev() {
        if let FileEntry::Message { message, .. } = entry {
            if let Some(usage) = assistant_usage(message) {
                return Some(usage);
            }
        }
    }
    None
}

/// The cost of ONE entry as the provider will see it.
///
/// `find_cut_point` walks entries but used to charge only
/// `FileEntry::Message`, while the model-visible projection also carries
/// `CustomMessage` (notices, digest) and non-empty `BranchSummary`. Trigger and
/// retention therefore priced the same history differently: the tail could be
/// kept "because it is under `keep_recent_tokens`" while its real cost was larger
/// (review 5, discussion #3779). This is the one entry-level estimator both
/// sides use; it maps an entry to exactly the message the projection builds and
/// sums that.
#[must_use]
pub fn estimate_entry_tokens(entry: &FileEntry) -> u64 {
    match entry {
        FileEntry::Message { message, .. } => estimate_tokens(message),
        // The digest and the kernel notice are regenerable snapshots that the
        // projection keeps only for the newest of each; an older copy costs
        // nothing because it is not sent.
        FileEntry::CustomMessage { payload, .. } => {
            if payload.custom_type == "harness_digest"
                || payload.custom_type == super::ipython_state::IPYTHON_STATE_CUSTOM_TYPE
            {
                return 0;
            }
            estimate_tokens(&AgentMessage::Custom(
                crate::session::create_custom_message(payload, entry),
            ))
        }
        FileEntry::BranchSummary { payload, .. } => {
            if payload.summary.is_empty() {
                return 0;
            }
            estimate_tokens(&AgentMessage::BranchSummary(
                pa_types::session::BranchSummaryMessage {
                    summary: payload.summary.clone(),
                    from_id: payload.from_id.clone(),
                    timestamp: crate::session::timestamp_to_millis(entry.timestamp()),
                },
            ))
        }
        // Everything else (header, compaction, settings changes) is either not
        // model-visible or is the boundary the walk stops at.
        _ => 0,
    }
}

/// Chars/4 heuristic token estimate (conservative; overestimates).
#[must_use]
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let chars = |text: &str| text.chars().count() as u64;
    match message {
        AgentMessage::User(user) => match &user.content {
            pa_types::ai::UserContent::Text(text) => div4(chars(text)),
            pa_types::ai::UserContent::Blocks(blocks) => {
                // Text stays chars/4; images add their own token count (see
                // [`user_image_tokens`]). TS counted user images as zero —
                // deliberate parity-divergence: an image-heavy turn used to
                // under-count and mis-time compaction.
                let text_chars: u64 = blocks
                    .iter()
                    .filter_map(|block| match block {
                        pa_types::ai::UserContentBlock::Text(text) => Some(chars(&text.text)),
                        _ => None,
                    })
                    .sum();
                let image_tokens: u64 = blocks.iter().map(user_image_tokens).sum();
                div4(text_chars) + image_tokens
            }
        },
        AgentMessage::Assistant(assistant) => {
            let mut total = 0u64;
            for block in &assistant.content {
                match block {
                    pa_types::ai::AssistantContentBlock::Text(text) => total += chars(&text.text),
                    pa_types::ai::AssistantContentBlock::Thinking(thinking) => {
                        total += chars(&thinking.thinking);
                    }
                    pa_types::ai::AssistantContentBlock::ToolCall(call) => {
                        total += chars(&call.name)
                            + serde_json::to_string(&call.arguments)
                                .map_or(0, |json| json.chars().count() as u64);
                    }
                }
            }
            div4(total)
        }
        AgentMessage::Custom(custom) => match &custom.content {
            pa_types::ai::UserContent::Text(text) => div4(chars(text)),
            pa_types::ai::UserContent::Blocks(blocks) => div4(
                blocks
                    .iter()
                    .map(|block| match block {
                        pa_types::ai::UserContentBlock::Text(text) => chars(&text.text),
                        // TS parity: custom images stay a flat 4_800 chars
                        // (1_200 tokens) — not geometry-aware like the user path.
                        pa_types::ai::UserContentBlock::Image(_) => 4_800,
                        // Un-modeled blocks have no modeled size; TS sizes
                        // only typed blocks, so estimate zero.
                        pa_types::ai::UserContentBlock::Raw(_) => 0,
                    })
                    .sum(),
            ),
        },
        AgentMessage::ToolResult(result) => div4(
            result
                .content
                .iter()
                .map(|block| match block {
                    pa_types::ai::UserContentBlock::Text(text) => chars(&text.text),
                    // TS parity: tool-result images stay a flat 4_800 chars
                    // (1_200 tokens) — not geometry-aware like the user path.
                    pa_types::ai::UserContentBlock::Image(_) => 4_800,
                    pa_types::ai::UserContentBlock::Raw(_) => 0,
                })
                .sum(),
        ),
        AgentMessage::BashExecution(bash) => div4(chars(&bash.command) + chars(&bash.output)),
        AgentMessage::BranchSummary(summary) => div4(chars(&summary.summary)),
        AgentMessage::CompactionSummary(summary) => div4(chars(&summary.summary)),
    }
    .saturating_add(0) // keep type u64
}

fn div4(chars: u64) -> u64 {
    chars.div_ceil(4)
}

/// The token cost of one user-attached image block: `pa-ai`'s known-geometry
/// rule (`ceil(w * h / 750)`) when the payload's pixel dimensions can be read
/// from a bounded prefix, else the flat default. The daemon's snapshot elision
/// leaves `widthPx`/`heightPx` on elided blocks; a tool-result or raw block
/// without a readable image payload falls back to the flat figure.
fn user_image_tokens(block: &pa_types::ai::UserContentBlock) -> u64 {
    let pa_types::ai::UserContentBlock::Image(image) = block else {
        return 0;
    };
    let dimensions = image
        .rest
        .get("widthPx")
        .and_then(serde_json::Value::as_u64)
        .zip(
            image
                .rest
                .get("heightPx")
                .and_then(serde_json::Value::as_u64),
        )
        .map(|(width, height)| (width as u32, height as u32))
        .or_else(|| {
            get_image_dimensions_prefix(
                &image.data,
                &image.mime_type,
                IMAGE_DIMENSIONS_PREFIX_BYTES,
            )
            .map(|dims| (dims.width_px, dims.height_px))
        });
    match dimensions {
        Some((width, height)) => pa_ai::image_tokens_for_dimensions(width, height),
        None => pa_ai::DEFAULT_IMAGE_TOKENS,
    }
}

/// The effective compaction threshold: a retention-floored percentage capped by model safety.
/// PERCENTAGE: `context_window * threshold_percent / 100` (estimate drift on
/// large windows). COMBINED-LIMIT: `context_window - max_output_tokens -
/// max(reserve_tokens, COMBINED_LIMIT_HEADROOM_FLOOR)`: the output budget
/// must still fit. TS checks only `contextWindow - reserveTokens`; reserving
/// the output budget is a deliberate Rust-side fix. A non-positive threshold
/// disables the trigger.
#[must_use]
pub fn compaction_threshold(
    context_window: u64,
    max_output_tokens: u64,
    settings: &CompactionSettings,
) -> u64 {
    let percent = if pa_types::compaction::is_valid_threshold_percent(settings.threshold_percent) {
        settings.threshold_percent
    } else {
        DEFAULT_THRESHOLD_PERCENT
    };
    let requested = (context_window as f64 * (percent / 100.0)) as u64;
    // Retained history and both summary passes must fit below the trigger,
    // otherwise a low percentage re-summarizes the same retained context on
    // every tool turn. Two reserves cover both summary completion budgets.
    let retention_floor = settings
        .keep_recent_tokens
        .saturating_add(settings.reserve_tokens.saturating_mul(2))
        .saturating_add(1);
    let percentage = requested.max(retention_floor);
    let headroom = settings.reserve_tokens.max(COMBINED_LIMIT_HEADROOM_FLOOR);
    // A DECLARED budget that cannot coexist with any input is not a
    // simultaneous claim: the provider caps generated output to whatever the
    // window still holds. Eight models in the compiled catalog declare exactly
    // that (`gpt-4` 8192/8192, `llama-3.1-8b-instant` 131072/131072,
    // `moonshotai/kimi-k3` 1048576/1048576, and five more). Subtracting the
    // whole declared budget drove the threshold to 0, and 0 means DISABLED:
    // compaction stopped firing for those models and the session grew until the
    // provider refused it, with nothing logged (review 5, discussion #3793).
    //
    // Such a model reserves half the window instead — the most input a response
    // of that size can leave room for — so the trigger keeps firing while the
    // window can still hold the retention floor. A budget BELOW the window is a
    // real claim and is reserved in full, unchanged.
    // A reservation can never exceed half the window either: the other half is
    // the input the session exists to hold. A model declaring 124k output on a
    // 128k window (an explicit `models.json` budget) used to saturate the bound
    // to 0 and stop compaction entirely.
    let declared_output = max_output_tokens.min(context_window / 2);
    let combined = context_window
        .saturating_sub(declared_output)
        .saturating_sub(headroom);
    // Reserves are ADVISORY headroom, not physical claims on the window. When
    // they sum past it — an 8k window (`gpt-4`, 8192/8192) against the 16k
    // headroom floor alone — the difference saturates to 0, and 0 means
    // DISABLED. A window still holds input, so the trigger must still fire:
    // compact at half the window, which leaves the other half for output and
    // estimation drift. Ordinary windows never reach this branch (their
    // combined bound is positive), so their thresholds are unchanged.
    let combined = if combined == 0 && context_window > 0 {
        context_window / 2
    } else {
        combined
    };
    percentage.min(combined)
}

/// The trigger priced from a [`ContextPlan`]: the same threshold, against the
/// tokens the next request will actually claim.
///
/// The message-only entry point below answers "how full is the HISTORY"; this one
/// answers "will the REQUEST fit", which is the question a threshold exists for.
/// They differ by the fixed prefix (system prompt plus tool schemas), measured at
/// ~6 300 tokens for the static layer alone — enough to cross a threshold a
/// message-only estimate says is far away.
#[must_use]
pub fn plan_compaction_due(plan: &ContextPlan, settings: &CompactionSettings) -> bool {
    if !settings.enabled || plan.context_window == 0 {
        return false;
    }
    // The threshold already reserves the output budget; the plan's input is what
    // competes with it.
    let threshold = compaction_threshold(plan.context_window, plan.output_reserve, settings);
    threshold > 0 && plan.input() > threshold
}

#[must_use]
pub fn should_compact(
    context_tokens: u64,
    context_window: u64,
    max_output_tokens: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    if context_window == 0 {
        return false;
    }
    let threshold = compaction_threshold(context_window, max_output_tokens, settings);
    threshold > 0 && context_tokens > threshold
}

/// The effective requested output budget of the session's next model call:
/// the per-request default output budget (an explicitly configured
/// `maxTokens` unchanged; a catalog value `min(model.maxTokens, 32000)`),
/// plus the thinking budget budget-folding providers (Anthropic/Bedrock
/// models without adaptive thinking) add on top of it for the session's
/// reasoning level, capped at the model's declared max output; 0 when the
/// model declares no max output. The combined input+output ceiling reserves
/// what the request will actually claim, or a budget-folded request can
/// overflow while the trigger still says "not due".
#[must_use]
pub fn request_output_budget(
    model: &pa_types::ai::Model,
    thinking: pa_types::ai::ModelThinkingLevel,
) -> u64 {
    pa_ai::effective_request_max_tokens(model, thinking)
}

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

/// The message-anchored context estimate: the last valid assistant usage
/// anchors it, and messages after it add chars/4 estimates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextTokensEstimate {
    pub tokens: u64,
    /// The live-message index the estimate anchored on; `None` when the
    /// context carries no valid assistant usage.
    pub last_usage_index: Option<usize>,
}

/// Estimate the context tokens: the last valid assistant usage plus
/// chars/4 estimates for the messages that trail it.
///
/// # Panics
///
/// The `expect` cannot fire: the index comes from an `rposition` over
/// messages with usage present.
pub fn estimate_context_tokens(messages: &[AgentMessage]) -> ContextTokensEstimate {
    match messages
        .iter()
        .rposition(|message| assistant_usage(message).is_some())
    {
        Some(index) => {
            let usage = assistant_usage(&messages[index]).expect("index from rposition");
            let trailing: u64 = messages[index + 1..].iter().map(estimate_tokens).sum();
            ContextTokensEstimate {
                tokens: calculate_context_tokens(&usage) + trailing,
                last_usage_index: Some(index),
            }
        }
        None => ContextTokensEstimate {
            tokens: messages.iter().map(estimate_tokens).sum(),
            last_usage_index: None,
        },
    }
}

/// A message's timestamp (millis since the epoch; every role carries one).
fn message_timestamp(message: &AgentMessage) -> u64 {
    match message {
        AgentMessage::User(user) => user.timestamp,
        AgentMessage::Assistant(assistant) => assistant.timestamp,
        AgentMessage::Custom(custom) => custom.timestamp,
        AgentMessage::ToolResult(result) => result.timestamp,
        AgentMessage::BashExecution(bash) => bash.timestamp,
        AgentMessage::BranchSummary(summary) => summary.timestamp,
        AgentMessage::CompactionSummary(summary) => summary.timestamp,
    }
}

/// Whether the live context crossed the effective threshold
/// ([`compaction_threshold`]) and an automatic compaction should run. Usage
/// from before the newest `CompactionSummary` never re-triggers.
#[must_use]
pub fn threshold_compaction_due(
    messages: &[AgentMessage],
    context_window: u64,
    max_output_tokens: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled || context_window == 0 {
        return false;
    }
    let compaction_timestamp = messages.iter().rev().find_map(|message| match message {
        AgentMessage::CompactionSummary(summary) => Some(summary.timestamp),
        _ => None,
    });
    let estimate = estimate_context_tokens(messages);
    let context_tokens = if let Some(index) = estimate.last_usage_index {
        // The usage anchor must postdate the latest compaction.
        if compaction_timestamp
            .is_some_and(|timestamp| message_timestamp(&messages[index]) <= timestamp)
        {
            return false;
        }
        estimate.tokens
    }
    // No valid usage: the last assistant's raw usage decides; error
    // turns never trigger.
    else {
        let Some(AgentMessage::Assistant(assistant)) = messages
            .iter()
            .rev()
            .find(|message| matches!(message, AgentMessage::Assistant(_)))
        else {
            return false;
        };
        if assistant.stop_reason == pa_types::ai::StopReason::Error {
            return false;
        }
        if compaction_timestamp.is_some_and(|timestamp| assistant.timestamp <= timestamp) {
            return false;
        }
        calculate_context_tokens(&assistant.usage)
    };
    should_compact(context_tokens, context_window, max_output_tokens, settings)
}

/// Valid cut positions: every message EXCEPT `ToolResult`, plus branch and
/// compaction boundaries.
///
/// # The tool-pair invariant is STRUCTURAL, not checked
///
/// Excluding `ToolResult` is what guarantees a cut never separates a result from the
/// assistant message that requested it: no valid boundary can fall between a call and
/// its result, so dropping the call always drops its result and keeping the call
/// always keeps it. A provider rejects a request whose `toolResult` names an absent
/// `toolCallId`, and this set makes that state unreachable from `find_cut_point`.
///
/// That is necessary but NOT sufficient, and the measurement says so: the set
/// excludes `ToolResult` as a cut POSITION, while `find_cut_point`'s walk-back can
/// still leave the boundary ON a result, because `cut_index` starts at
/// `cut_points[0]` and the back-walk stops at the previous message. With the pair
/// repair in `prepare_compaction` disabled, the exhaustive probe finds **17 401**
/// stranded cases out of 60 976 (shape, budget) pairs; with the `start_index` fix
/// below (a fit-history cut no longer lands on the first message) that count drops to
/// 1 759 — and with the repair enabled, zero. Both are load-bearing.
///
/// So the invariant is enforced in two places on purpose — this set says which
/// positions are legal, and the repair makes the published boundary respect the
/// pair — and `probe_every_shape_and_budget_for_a_stranded_result` is the guard over
/// both. Removing either one makes that test fail.
#[must_use]
pub fn find_valid_cut_points(
    entries: &[FileEntry],
    start_index: usize,
    end_index: usize,
) -> Vec<usize> {
    let mut cut_points = Vec::new();
    for (i, entry) in entries
        .iter()
        .enumerate()
        .take(end_index.min(entries.len()))
        .skip(start_index)
    {
        match entry {
            FileEntry::Message { message, .. } => match message {
                AgentMessage::User(_)
                | AgentMessage::Assistant(_)
                | AgentMessage::Custom(_)
                | AgentMessage::BashExecution(_)
                | AgentMessage::BranchSummary(_)
                | AgentMessage::CompactionSummary(_) => cut_points.push(i),
                AgentMessage::ToolResult(_) => {}
            },
            FileEntry::BranchSummary { .. } | FileEntry::CustomMessage { .. } => {
                cut_points.push(i);
            }
            _ => {}
        }
    }
    cut_points
}

/// Whether keeping `entries[keep_from..]` would strand a tool result: a result
/// whose matching call sits on the DROPPED side.
///
/// This is the provider's own precondition, not a heuristic: a `toolResult`
/// whose `toolCallId` does not appear in a preceding assistant message is
/// rejected at request time, so a plan that produces one advertises a split it
/// cannot apply. The donor's `projectUnit` exists for the same class of defect
/// (recommend, explain and apply sharing ONE validity rule) and names the
/// symptom: "Range would split N turn(s)".
///
/// The check spans the WHOLE kept range, so it also catches a result kept by a
/// later cut while its call was dropped by an earlier one.
#[must_use]
pub fn orphans_a_tool_result(entries: &[FileEntry], keep_from: usize) -> bool {
    let kept = entries.get(keep_from..).unwrap_or(&[]);
    let mut calls: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for entry in kept {
        let FileEntry::Message { message, .. } = entry else {
            continue;
        };
        match message {
            AgentMessage::Assistant(assistant) => {
                for block in &assistant.content {
                    if let AssistantContentBlock::ToolCall(call) = block {
                        calls.insert(call.id.as_str());
                    }
                }
            }
            // A result whose call is not in the kept range: the provider would
            // reject the request, so the cut is not applicable.
            AgentMessage::ToolResult(result) if !calls.contains(result.tool_call_id.as_str()) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// The user/bashExecution message (or branch boundary) starting the turn
/// containing `entry_index`; None when no turn start exists before it.
fn find_turn_start_index(
    entries: &[FileEntry],
    entry_index: usize,
    start_index: usize,
) -> Option<usize> {
    for i in (start_index..=entry_index).rev() {
        let entry = &entries[i];
        match entry {
            FileEntry::BranchSummary { .. }
            | FileEntry::CustomMessage { .. }
            | FileEntry::Message {
                message: AgentMessage::User(_) | AgentMessage::BashExecution(_),
                ..
            } => return Some(i),
            _ => {}
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutPointResult {
    pub first_kept_entry_index: usize,
    /// Turn start when splitting mid-turn (None otherwise).
    pub turn_start_index: Option<usize>,
    pub is_split_turn: bool,
}

/// Find the cut point keeping approximately `keep_recent_tokens`.
///
/// # Panics
///
/// The `unwrap` on the last cut point cannot fire: the cut-point list was
/// checked non-empty above.
#[must_use]
pub fn find_cut_point(
    entries: &[FileEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: u64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);
    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: None,
            is_split_turn: false,
        };
    }
    // A cut exists to DROP something. `cut_index` starting at `cut_points[0]` meant a
    // small budget cut at the first valid message — i.e. summarized away the whole
    // history — and left the boundary where an unrelated result sits. Starting at
    // `start_index` means "keep everything" until the budget proves otherwise; the
    // loop below moves it only when the retained tail actually exceeds the budget.
    let mut accumulated = 0u64;
    let mut cut_index = start_index;
    for (i, entry) in entries
        .iter()
        .enumerate()
        .take(end_index.min(entries.len()))
        .skip(start_index)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        accumulated = accumulated.saturating_add(estimate_entry_tokens(entry));
        if accumulated >= keep_recent_tokens {
            cut_index = *cut_points.last().unwrap();
            for &point in &cut_points {
                if point >= i {
                    cut_index = point;
                    break;
                }
            }
            break;
        }
    }
    // Walk back to a message or compaction boundary.
    while cut_index > start_index {
        match &entries[cut_index - 1] {
            FileEntry::Compaction { .. } | FileEntry::Message { .. } => break,
            _ => cut_index -= 1,
        }
    }
    let is_user_message = matches!(
        &entries[cut_index],
        FileEntry::Message {
            message: AgentMessage::User(_),
            ..
        }
    );
    let turn_start_index = if is_user_message {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };
    CutPointResult {
        first_kept_entry_index: cut_index,
        is_split_turn: !is_user_message && turn_start_index.is_some(),
        turn_start_index,
    }
}

const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.\n\nUse this EXACT format:\n\n## Goal\n[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned by user]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Current work]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Outstanding Work\n- [Running subagents: name / task / status (recovery pointer: rlm.list_subagents())]\n- [Background bash handles: command / status (recovery: the [bash-done] notification; a lost handle cannot be re-awaited)]\n- [Active heartbeats and the goal: schedule / objective (recovery pointers: rlm_heartbeat.list() and goal.get())]\n- [Or \"(none)\" if nothing is in flight]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Any data, examples, or references needed to continue]\n- [Or \"(none)\" if not applicable]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

const KERNEL_PERSIST_SUMMARY_NOTE: &str = "Note: the Python kernel keeps running after this summary — every Python variable, import, and helper you defined stays available. The cells that defined them won't appear above, so record in the summary any names worth remembering so you reuse them instead of redefining them.";

/// A split turn keeps its suffix; this prompt summarizes the cut turn's
/// prefix for the retained recent work.
pub const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.\n\nSummarize the prefix to provide context for the retained suffix:\n\n## Original Request\n[What did the user ask for in this turn?]\n\n## Early Progress\n- [Key decisions and work done in the prefix]\n\n## Outstanding Work\n- [Running subagents, background bash handles, active heartbeats/goal started in the prefix and still in flight; or \"(none)\"]\n\n## Context for Suffix\n- [Information needed to understand the retained recent work]\n\nBe concise. Focus on what's needed to understand the kept suffix.";

const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.\n\nUpdate the existing structured summary with new information. RULES:\n- PRESERVE all existing information from the previous summary\n- ADD new progress, decisions, and context from the new messages\n- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed\n- UPDATE \"Next Steps\" based on what was accomplished\n- PRESERVE exact file paths, function names, and error messages\n- PRESERVE the Outstanding Work section: update in-flight subagents, background bash handles, heartbeats, and the goal from the new messages\n- If something is no longer relevant, you may remove it\n\nUse this EXACT format:\n\n## Goal\n[Preserve existing goals, add new ones if the task expanded]\n\n## Constraints & Preferences\n- [Preserve existing, add new ones discovered]\n\n## Progress\n### Done\n- [x] [Include previously done items AND newly completed items]\n\n### In Progress\n- [ ] [Current work - update based on progress]\n\n### Blocked\n- [Current blockers - remove if resolved]\n\n## Outstanding Work\n- [Preserve in-flight subagents, background bash handles, heartbeats, and the goal; update statuses from the new messages]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale] (preserve all previous, add new)\n\n## Next Steps\n1. [Update based on current state]\n\n## Critical Context\n- [Preserve important context, add new if needed]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// The retry instruction for a summary the output cap cut off: the retry
/// wire call appends it as one more text block on the summarization request.
pub(crate) const SHORTER_SUMMARY_INSTRUCTION: &str = "Your previous summary was cut off at the output limit. Write a shorter summary that fits: keep every required section and compress the detail.";

/// The line a twice-capped summary commits behind: the retry's text still
/// replaces the transcript, so it rides in front of this disclosure.
pub(crate) const SUMMARY_TRUNCATED_MARKER: &str =
    "[summary truncated: earlier detail may be missing]";

/// The instruction part of the summarization prompt (initial or update
/// template, user instructions, kernel persistence note).
#[must_use]
pub fn build_summarization_prompt(
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
) -> String {
    let mut base = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    }
    .to_string();
    if let Some(custom_instructions) = custom_instructions {
        let _ = write!(base,
            "\n\n<user-instructions>\nThe user provided these instructions for this summary. Follow them with high priority while keeping the section format above: emphasize what they ask to focus on, and preserve verbatim anything they ask to remember.\n{custom_instructions}\n</user-instructions>"
        );
    }
    format!("{base}\n\n{KERNEL_PERSIST_SUMMARY_NOTE}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_types::session::EntryBase;

    fn message_entry(id: &str, parent: &str, message: AgentMessage) -> FileEntry {
        FileEntry::Message {
            message,
            base: EntryBase {
                id: Some(id.to_string()),
                parent_id: Some(parent.to_string()),
                timestamp: Some("2024-01-01T00:00:00.000Z".to_string()),
                rest: serde_json::Map::default(),
            },
        }
    }

    fn user_entry(id: &str, parent: &str, text: &str) -> FileEntry {
        message_entry(
            id,
            parent,
            AgentMessage::User(pa_types::ai::UserMessage {
                content: pa_types::ai::UserContent::Text(text.to_string()),
                timestamp: 0,
                rest: serde_json::Map::default(),
            }),
        )
    }

    fn tool_call_assistant(id: &str, tool: &str) -> AgentMessage {
        AgentMessage::Assistant(pa_types::ai::AssistantMessage {
            content: vec![pa_types::ai::AssistantContentBlock::ToolCall(
                pa_types::ai::ToolCall {
                    id: id.to_string(),
                    name: tool.to_string(),
                    arguments: serde_json::Map::default(),
                    thought_signature: None,
                    rest: serde_json::Map::default(),
                },
            )],
            api: "openai-completions".to_string(),
            provider: "p".to_string(),
            model: "m".to_string(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: pa_types::ai::Usage {
                input: 100,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 100,
                cost: pa_types::ai::UsageCost::default(),
            },
            stop_reason: pa_types::ai::StopReason::ToolUse,
            stop_reason_raw: None,
            error_message: None,
            timestamp: 0,
            rest: serde_json::Map::default(),
        })
    }

    fn tool_result_message(call_id: &str, text: &str) -> AgentMessage {
        AgentMessage::ToolResult(pa_types::ai::ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: "bash".to_string(),
            content: vec![pa_types::ai::UserContentBlock::Text(
                pa_types::ai::TextContent {
                    text: text.to_string(),
                    text_signature: None,
                    rest: serde_json::Map::default(),
                },
            )],
            details: None,
            is_error: false,
            timestamp: 0,
            rest: serde_json::Map::default(),
        })
    }

    fn assistant_entry(id: &str, parent: &str) -> FileEntry {
        message_entry(
            id,
            parent,
            AgentMessage::Assistant(pa_types::ai::AssistantMessage {
                content: vec![pa_types::ai::AssistantContentBlock::Text(
                    pa_types::ai::TextContent {
                        text: "ok".to_string(),
                        text_signature: None,
                        rest: serde_json::Map::default(),
                    },
                )],
                api: "openai-completions".to_string(),
                provider: "p".to_string(),
                model: "m".to_string(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: pa_types::ai::Usage {
                    input: 100,
                    output: 50,
                    cache_read: 0,
                    cache_write: 0,
                    total_tokens: 150,
                    cost: pa_types::ai::UsageCost::default(),
                },
                stop_reason: pa_types::ai::StopReason::Stop,
                stop_reason_raw: None,
                error_message: None,
                timestamp: 0,
                rest: serde_json::Map::default(),
            }),
        )
    }

    fn user_message(text: &str, timestamp: u64) -> AgentMessage {
        AgentMessage::User(pa_types::ai::UserMessage {
            content: pa_types::ai::UserContent::Text(text.to_string()),
            timestamp,
            rest: serde_json::Map::default(),
        })
    }

    fn assistant_message(usage_total: u64, timestamp: u64) -> AgentMessage {
        AgentMessage::Assistant(pa_types::ai::AssistantMessage {
            content: vec![pa_types::ai::AssistantContentBlock::Text(
                pa_types::ai::TextContent {
                    text: "ok".to_string(),
                    text_signature: None,
                    rest: serde_json::Map::default(),
                },
            )],
            api: "openai-completions".to_string(),
            provider: "p".to_string(),
            model: "m".to_string(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: pa_types::ai::Usage {
                input: usage_total,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                total_tokens: usage_total,
                cost: pa_types::ai::UsageCost::default(),
            },
            stop_reason: pa_types::ai::StopReason::Stop,
            stop_reason_raw: None,
            error_message: None,
            timestamp,
            rest: serde_json::Map::default(),
        })
    }

    fn compaction_summary(timestamp: u64) -> AgentMessage {
        AgentMessage::CompactionSummary(pa_types::session::CompactionSummaryMessage {
            summary: "the story so far".to_string(),
            tokens_before: 100,
            retained_message_count: None,
            custom_instructions: None,
            harness_digest: None,
            harness_state_fingerprint: None,
            timestamp,
        })
    }

    #[test]
    fn estimate_context_tokens_anchors_on_the_last_valid_usage() {
        let messages = vec![
            user_message("turn one", 1),
            assistant_message(1_000, 2),
            user_message("12345678", 3), // 8 chars -> 2 trailing tokens
        ];
        let estimate = estimate_context_tokens(&messages);
        assert_eq!(estimate.last_usage_index, Some(1));
        assert_eq!(estimate.tokens, 1_002);
        let messages = vec![user_message("12345678", 1)];
        let estimate = estimate_context_tokens(&messages);
        assert_eq!(estimate.last_usage_index, None);
        assert_eq!(estimate.tokens, 2);
    }

    #[test]
    fn threshold_crosses_the_reserve_headroom() {
        // 126_010 tokens on a 128k window with a 127_500 reserve leaves
        // a 500-token combined ceiling — the crossing fires.
        let settings = CompactionSettings {
            threshold_percent: 95.0,
            enabled: true,
            reserve_tokens: 127_500,
            keep_recent_tokens: 10,
        };
        let messages = vec![
            user_message("f14 auto seed turn", 1),
            assistant_message(110, 2),
            user_message("f14 threshold crossing turn", 3),
            assistant_message(126_010, 4),
        ];
        assert!(threshold_compaction_due(&messages, 128_000, 0, &settings));
        let messages = vec![
            user_message("f14 auto seed turn", 1),
            assistant_message(110, 2),
        ];
        assert!(!threshold_compaction_due(&messages, 128_000, 0, &settings));
        let disabled = CompactionSettings {
            threshold_percent: 95.0,
            enabled: false,
            reserve_tokens: 127_500,
            keep_recent_tokens: 10,
        };
        assert!(!threshold_compaction_due(&messages, 128_000, 0, &disabled));
        assert!(!threshold_compaction_due(&messages, 0, 0, &settings));
    }

    #[test]
    fn threshold_fires_before_a_combined_limit_overflow() {
        // The live 400 shape: 1_017_457 input tokens under the old trigger
        // (window - reserve = 1_032_192) but over the combined ceiling once the
        // 32_000 requested output no longer fits — the new trigger fires first.
        let settings = CompactionSettings::default();
        let messages = vec![
            user_message("live session turn", 1),
            assistant_message(1_017_457, 2),
        ];
        assert!(threshold_compaction_due(
            &messages, 1_048_576, 32_000, &settings
        ));
        let messages = vec![
            user_message("live session turn", 1),
            assistant_message(990_000, 2),
        ];
        assert!(!threshold_compaction_due(
            &messages, 1_048_576, 32_000, &settings
        ));
        // A 64k output budget pushes the combined ceiling below the percentage
        // one (1_048_576 - 65_536 - 16_384 = 966_656): 970_000 fires on the
        // combined ceiling alone — under the old reserve line AND 95% of the window.
        let messages = vec![
            user_message("live session turn", 1),
            assistant_message(970_000, 2),
        ];
        assert!(threshold_compaction_due(
            &messages, 1_048_576, 65_536, &settings
        ));
        assert!(!threshold_compaction_due(
            &messages, 1_048_576, 0, &settings
        ));
    }

    #[test]
    fn threshold_percentage_ceiling_guards_large_windows() {
        // Past 95% of a large window the percentage ceiling fires before
        // the combined ceiling (1_048_576 * 0.95 = 996_147).
        let settings = CompactionSettings::default();
        assert!(should_compact(996_148, 1_048_576, 32_000, &settings));
        assert!(!should_compact(996_000, 1_048_576, 32_000, &settings));
    }

    #[test]
    fn threshold_combined_ceiling_guards_small_windows() {
        // A 128k window with a 32k output budget: pure 95% (124_518) would under-reserve
        // — the combined ceiling (131_072 - 32_768 - 16_384 = 81_920) comes first.
        let settings = CompactionSettings::default();
        assert_eq!(compaction_threshold(131_072, 32_768, &settings), 81_920);
        assert!(should_compact(82_000, 131_072, 32_768, &settings));
        assert!(!should_compact(81_000, 131_072, 32_768, &settings));
    }

    #[test]
    fn threshold_headroom_floor_covers_estimate_error() {
        // A tiny reserve still keeps the 4_096 estimate-error floor:
        // 131_072 - 32_768 - 4_096 = 94_208 < the 95% ceiling.
        let settings = CompactionSettings {
            threshold_percent: 95.0,
            enabled: true,
            reserve_tokens: 1,
            keep_recent_tokens: 10,
        };
        assert_eq!(compaction_threshold(131_072, 32_768, &settings), 94_208);
        // An output budget larger than any context still leaves a LIVE trigger
        // (see `a_declared_output_budget_at_the_window_never_disables_the_trigger`):
        // a reservation never exceeds half the window, because the other half is
        // the input the session exists to hold. Returning 0 here used to read as
        // "disabled" and stopped compaction for the model entirely.
        let wide = compaction_threshold(128_000, 124_000, &CompactionSettings::default());
        assert!(wide > 0, "a live trigger is the contract: {wide}");
        assert_eq!(wide, 128_000 / 2 - 16_384);
        assert!(should_compact(
            wide + 1,
            128_000,
            124_000,
            &CompactionSettings::default()
        ));
        assert!(!should_compact(
            wide,
            128_000,
            124_000,
            &CompactionSettings::default()
        ));
    }

    #[test]
    fn stale_pre_compaction_usage_never_retriggers() {
        let settings = CompactionSettings {
            threshold_percent: 95.0,
            enabled: true,
            reserve_tokens: 127_500,
            keep_recent_tokens: 10,
        };
        let messages = vec![
            compaction_summary(10),
            user_message("kept turn", 5),
            assistant_message(126_010, 6),
        ];
        assert!(!threshold_compaction_due(&messages, 128_000, 0, &settings));
        let messages = vec![
            compaction_summary(10),
            user_message("new turn", 11),
            assistant_message(126_010, 12),
        ];
        assert!(threshold_compaction_due(&messages, 128_000, 0, &settings));
    }

    #[test]
    fn token_estimation() {
        let user = AgentMessage::User(pa_types::ai::UserMessage {
            content: pa_types::ai::UserContent::Text("12345678".to_string()), // 8 chars -> 2 tokens
            timestamp: 0,
            rest: serde_json::Map::default(),
        });
        assert_eq!(estimate_tokens(&user), 2);
        let usage = pa_types::ai::Usage {
            input: 10,
            output: 5,
            cache_read: 1,
            cache_write: 1,
            total_tokens: 100,
            cost: pa_types::ai::UsageCost::default(),
        };
        assert_eq!(calculate_context_tokens(&usage), 100);
    }

    /// A minimal valid PNG header (24 bytes) whose IHDR declares `width`x`height`;
    /// the bounded-prefix decoder reads the geometry without a full image.
    fn png_base64(width: u32, height: u32) -> String {
        use base64::Engine as _;
        let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10];
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn user_image_message(
        text: Option<&str>,
        images: &[(u32, u32)],
        timestamp: u64,
    ) -> AgentMessage {
        let mut blocks = Vec::new();
        if let Some(text) = text {
            blocks.push(pa_types::ai::UserContentBlock::Text(
                pa_types::ai::TextContent {
                    text: text.to_string(),
                    text_signature: None,
                    rest: serde_json::Map::default(),
                },
            ));
        }
        for &(width, height) in images {
            blocks.push(pa_types::ai::UserContentBlock::Image(
                pa_types::ai::ImageContent {
                    data: png_base64(width, height),
                    mime_type: "image/png".to_string(),
                    rest: serde_json::Map::default(),
                },
            ));
        }
        AgentMessage::User(pa_types::ai::UserMessage {
            content: pa_types::ai::UserContent::Blocks(blocks),
            timestamp,
            rest: serde_json::Map::default(),
        })
    }

    #[test]
    fn user_images_estimate_by_geometry_not_zero() {
        // TS counted user images as 0; the honest estimate adds ceil(w*h/750).
        let single = user_image_message(None, &[(100, 50)], 1);
        assert_eq!(estimate_tokens(&single), 7);

        // Three images: 7 + ceil(1_000*1_000/750)=1_334 + ceil(1920*1080/750)=2_765.
        let several = user_image_message(None, &[(100, 50), (1_000, 1_000), (1920, 1080)], 1);
        assert_eq!(estimate_tokens(&several), 7 + 1_334 + 2_765);

        // Text still counts chars/4 on top of the image tokens.
        let with_text = user_image_message(Some("12345678"), &[(100, 50)], 1);
        assert_eq!(estimate_tokens(&with_text), 2 + 7);
    }

    #[test]
    fn user_image_without_geometry_falls_back_to_the_flat_default() {
        // A non-image payload (undecodable prefix) keeps the flat 1_200-token
        // default, matching the tool/custom path.
        let message = AgentMessage::User(pa_types::ai::UserMessage {
            content: pa_types::ai::UserContent::Blocks(vec![
                pa_types::ai::UserContentBlock::Image(pa_types::ai::ImageContent {
                    data: "not-an-image".to_string(),
                    mime_type: "image/png".to_string(),
                    rest: serde_json::Map::default(),
                }),
            ]),
            timestamp: 1,
            rest: serde_json::Map::default(),
        });
        assert_eq!(estimate_tokens(&message), pa_ai::DEFAULT_IMAGE_TOKENS);
    }

    #[test]
    fn user_image_geometry_reads_the_elision_marker() {
        // The daemon's snapshot elision leaves widthPx/heightPx with the payload
        // stripped; the estimate still reads the geometry.
        let mut rest = serde_json::Map::default();
        rest.insert("widthPx".to_string(), serde_json::json!(640));
        rest.insert("heightPx".to_string(), serde_json::json!(480));
        let message = AgentMessage::User(pa_types::ai::UserMessage {
            content: pa_types::ai::UserContent::Blocks(vec![
                pa_types::ai::UserContentBlock::Image(pa_types::ai::ImageContent {
                    data: String::new(),
                    mime_type: "image/png".to_string(),
                    rest,
                }),
            ]),
            timestamp: 1,
            rest: serde_json::Map::default(),
        });
        // ceil(640 * 480 / 750) = 410.
        assert_eq!(estimate_tokens(&message), 410);
    }

    #[test]
    fn user_images_shift_the_context_estimate_before_any_usage() {
        // No assistant usage yet (the trailing-window / pre-first-assistant
        // case): an image turn must not read as zero.
        let messages = vec![user_image_message(
            Some("describe this"),
            &[(1_000, 1_000)],
            1,
        )];
        let estimate = estimate_context_tokens(&messages);
        assert_eq!(estimate.last_usage_index, None);
        // "describe this" = 13 chars -> ceil(13/4) = 4; the 1_000x1_000 image adds 1_334.
        assert_eq!(estimate.tokens, 4 + 1_334);
    }

    /// S5: the trigger must price the REQUEST, not only the history.
    ///
    /// The fixed prefix (system prompt + tool schemas) is billed on every call
    /// and appeared in NO estimate: the static prompt layer alone is ~6 300
    /// tokens (25 174 chars over `prompts/layers/*.md`). A session could sit at
    /// "comfortably below threshold" by the message count while the request was
    /// already past it.
    #[test]
    fn the_plan_prices_the_prefix_the_message_estimate_ignores() {
        let settings = CompactionSettings::default();
        // 20k window, 4k output reserve: threshold is 80% of what remains.
        let window = 20_000;
        let output = 4_000;
        let threshold = compaction_threshold(window, output, &settings);
        assert!(threshold > 0, "the fixture must have a live threshold");

        // Messages just under the threshold: the message-only trigger says no.
        let messages = threshold - 100;
        assert!(!should_compact(messages, window, output, &settings));

        // The same session, priced with what the wire actually carries.
        let plan = ContextPlan {
            messages,
            system_prompt: 6_300,
            tool_schemas: 1_500,
            output_reserve: output,
            context_window: window,
        };
        assert_eq!(
            plan.input(),
            messages + 6_300 + 1_500,
            "input is messages plus the prefix"
        );
        assert!(
            plan_compaction_due(&plan, &settings),
            "7 800 tokens of prefix must move the trigger: threshold {threshold}, \
             plan input {}",
            plan.input()
        );
    }

    /// The prefix is measured, not assumed: the schema estimator must see a real
    /// tool's serialized cost, or `ContextPlan::tool_schemas` is decoration.
    #[test]
    fn a_tool_schema_costs_the_bytes_it_actually_sends() {
        let definition = crate::tools::bash::create_bash_tool_definition("/tmp");
        let tool: std::sync::Arc<dyn pa_agent::types::AgentTool> = std::sync::Arc::new(
            crate::session_engine::tool_bridge::ToolDefinitionBridge::new(definition),
        );
        let tool = tool.as_ref();
        // What the wire carries: name + description + the JSON schema.
        let sent = serde_json::to_string(tool.parameters()).expect("schema serializes");
        let expected = div4(tool.name().chars().count() as u64)
            + div4(tool.description().chars().count() as u64)
            + div4(sent.chars().count() as u64);
        assert_eq!(estimate_tool_schema_tokens(tool), expected);
        assert!(
            expected > 100,
            "a real bash schema must cost more than a rounding error: {expected}"
        );
    }

    /// The static prompt layer is a real cost, not a token of politeness: if this
    /// number were zero the plan would still under-count the request.
    #[test]
    fn the_static_prompt_layer_is_a_measurable_share_of_a_small_window() {
        let static_prompt = include_str!("../prompts/layers/core.md");
        let tokens = estimate_text_tokens(static_prompt);
        assert!(
            tokens > 3_000,
            "the core layer alone measured ~4k tokens; got {tokens}"
        );
        // Against an 8k window that is most of the budget.
        let plan = ContextPlan {
            messages: 0,
            system_prompt: tokens,
            tool_schemas: 0,
            output_reserve: 0,
            context_window: 8_192,
        };
        assert!(
            plan.input() as f64 / 8_192.0 > 0.4,
            "the static layer must be a large share of a small window: {}",
            plan.input()
        );
    }

    /// A plan with no prefix must agree with the message-only trigger, so the two
    /// numbers are differently-scoped rather than differently-wrong.
    #[test]
    fn a_plan_without_a_prefix_agrees_with_should_compact() {
        let settings = CompactionSettings::default();
        for window in [8_192u64, 32_768, 200_000] {
            for output in [1_024u64, 4_096] {
                let threshold = compaction_threshold(window, output, &settings);
                for messages in [0, threshold.saturating_sub(1), threshold, threshold + 1] {
                    let plan = ContextPlan {
                        messages,
                        output_reserve: output,
                        context_window: window,
                        ..ContextPlan::default()
                    };
                    assert_eq!(
                        plan_compaction_due(&plan, &settings),
                        should_compact(messages, window, output, &settings),
                        "window {window} output {output} messages {messages}"
                    );
                }
            }
        }
    }

    /// Overflow is a separate question from "should compact": a plan can exceed
    /// the window with nothing left to cut, and a caller that conflates them
    /// compacts forever without fixing the request.
    #[test]
    fn overflow_is_not_the_same_question_as_compaction_due() {
        let plan = ContextPlan {
            messages: 200_000,
            system_prompt: 6_300,
            tool_schemas: 1_500,
            output_reserve: 4_000,
            context_window: 200_000,
        };
        assert!(plan.overflows());
        let empty_window = ContextPlan {
            context_window: 0,
            ..plan
        };
        assert!(
            !empty_window.overflows(),
            "an unknown window is not an overflow"
        );
    }

    #[test]
    fn should_compact_threshold() {
        let settings = CompactionSettings::default();
        assert!(should_compact(120_000, 128_000, 0, &settings)); // > 128k - 16k
        assert!(!should_compact(100_000, 128_000, 0, &settings));
        let disabled = CompactionSettings {
            enabled: false,
            ..settings
        };
        assert!(!should_compact(200_000, 128_000, 0, &disabled));
        assert!(!should_compact(1, 0, 0, &settings));
    }

    #[test]
    fn cut_points_never_tool_results() {
        let entries = vec![
            user_entry("u1", "root", "hello there"),
            assistant_entry("a1", "u1"),
            message_entry(
                "t1",
                "a1",
                AgentMessage::ToolResult(pa_types::ai::ToolResultMessage {
                    tool_call_id: "c".to_string(),
                    tool_name: "bash".to_string(),
                    content: vec![pa_types::ai::UserContentBlock::Text(
                        pa_types::ai::TextContent {
                            text: "result".to_string(),
                            text_signature: None,
                            rest: serde_json::Map::default(),
                        },
                    )],
                    details: None,
                    is_error: false,
                    timestamp: 0,
                    rest: serde_json::Map::default(),
                }),
            ),
        ];
        let points = find_valid_cut_points(&entries, 0, 3);
        assert_eq!(points, vec![0, 1]);
    }

    /// Review 5 (#3779): the cut walk must price every model-visible entry, not
    /// only `Message`.
    ///
    /// A `CustomMessage` notice is in the projection the provider receives, yet
    /// the walk charged it zero. So a tail made of notices looked free: the cut
    /// point stayed far left, keeping a tail whose real cost already exceeded
    /// `keep_recent_tokens` — and the next trigger saw a prompt larger than the
    /// retention calculation promised.
    #[test]
    fn a_custom_notice_costs_the_same_in_the_walk_as_a_message_of_equal_size() {
        let big = "x".repeat(4_000); // ~1_000 tokens
        let notice = |id: &str, parent: &str| FileEntry::CustomMessage {
            payload: pa_types::session::CustomMessageEntry {
                custom_type: "the_user_asked_for_this".to_string(),
                content: pa_types::ai::UserContent::Text(big.clone()),
                details: None,
                display: true,
                rest: serde_json::Map::default(),
            },
            base: pa_types::session::EntryBase {
                id: Some(id.to_string()),
                parent_id: Some(parent.to_string()),
                timestamp: Some("2024-01-01T00:00:00.000Z".to_string()),
                rest: serde_json::Map::default(),
            },
        };
        let with_notice = vec![
            user_entry("u1", "root", "turn one"),
            assistant_entry("a1", "u1"),
            notice("n1", "a1"),
        ];
        let with_message = vec![
            user_entry("u1", "root", "turn one"),
            assistant_entry("a1", "u1"),
            user_entry("u2", "a1", &big),
        ];

        assert_eq!(
            estimate_entry_tokens(&with_notice[2]),
            estimate_entry_tokens(&with_message[2]),
            "a notice of the same text must cost what a message of that text costs"
        );
        // Both walk to the same cut point under the same budget: the notice is
        // as expensive as the user message, so neither may be kept "for free".
        let keep = 900;
        assert_eq!(
            find_cut_point(&with_notice, 0, 3, keep).first_kept_entry_index,
            find_cut_point(&with_message, 0, 3, keep).first_kept_entry_index,
        );
        let only_notice = find_cut_point(&with_notice, 0, 3, keep).first_kept_entry_index;
        assert!(
            only_notice > 0,
            "the expensive tail must be cut, not kept at index 0"
        );
    }

    /// A regenerable snapshot is NOT sent, so it must not consume budget.
    #[test]
    fn a_stale_digest_entry_costs_nothing_in_the_walk() {
        let digest = FileEntry::CustomMessage {
            payload: pa_types::session::CustomMessageEntry {
                custom_type: "harness_digest".to_string(),
                content: pa_types::ai::UserContent::Text("x".repeat(40_000)),
                details: None,
                display: false,
                rest: serde_json::Map::default(),
            },
            base: pa_types::session::EntryBase {
                id: Some("d1".to_string()),
                parent_id: Some("root".to_string()),
                timestamp: Some("2024-01-01T00:00:00.000Z".to_string()),
                rest: serde_json::Map::default(),
            },
        };
        assert_eq!(estimate_entry_tokens(&digest), 0);
    }

    #[test]
    fn cut_point_keeps_recent_tokens() {
        let entries = vec![
            user_entry("u1", "root", "turn one"),
            assistant_entry("a1", "u1"),
            user_entry("u2", "a1", "turn two"),
            assistant_entry("a2", "u2"),
            user_entry("u3", "a2", "turn three"),
        ];
        let keep_all = find_cut_point(&entries, 0, 5, 10_000);
        assert_eq!(keep_all.first_kept_entry_index, 0);
        assert!(!keep_all.is_split_turn);
        let keep_little = find_cut_point(&entries, 0, 5, 1);
        assert!(keep_little.first_kept_entry_index > 0);
        assert!(!keep_little.is_split_turn);

        // An assistant-final session cuts mid-turn.
        let assistant_final = vec![
            user_entry("u1", "root", "turn one"),
            assistant_entry("a1", "u1"),
            user_entry("u2", "a1", "turn two"),
            assistant_entry("a2", "u2"),
        ];
        let keep_small = find_cut_point(&assistant_final, 0, 4, 1);
        assert!(keep_small.is_split_turn);
        assert_eq!(keep_small.turn_start_index, Some(2));
    }

    #[test]
    fn summarization_prompt_shapes() {
        let initial = build_summarization_prompt(None, None);
        assert!(initial.contains("Create a structured context checkpoint summary"));
        assert!(initial.contains("the Python kernel keeps running"));
        let update = build_summarization_prompt(None, Some("old summary"));
        assert!(update.contains("NEW conversation messages to incorporate"));
        let custom = build_summarization_prompt(Some("focus on tests"), None);
        assert!(custom.contains("<user-instructions>\nThe user provided these instructions"));
        assert!(custom.contains("focus on tests"));
    }

    #[test]
    fn summarization_request_carries_outstanding_work() {
        let messages = vec![user_message("explore the widget", 0)];
        let request = crate::session_engine::compaction_exec::build_summarization_request(
            &messages, None, None, None, 0,
        );
        let text = match &request[0] {
            AgentMessage::User(user) => user.content.text(),
            _ => panic!("expected a user message"),
        };
        assert!(text.contains("## Outstanding Work"));
        assert!(text.contains("rlm.list_subagents()"));
        assert!(text.contains("the [bash-done] notification"));
        assert!(text.contains("rlm_heartbeat.list()"));

        let update = crate::session_engine::compaction_exec::build_summarization_request(
            &messages,
            None,
            Some("previous summary"),
            None,
            0,
        );
        let update_text = match &update[0] {
            AgentMessage::User(user) => user.content.text(),
            _ => panic!("expected a user message"),
        };
        assert!(update_text.contains("## Outstanding Work"));
        assert!(update_text.contains("PRESERVE the Outstanding Work section"));
    }

    #[test]
    fn last_assistant_usage_skips_aborted() {
        let entries = vec![
            assistant_entry("a1", "root"),
            message_entry(
                "a2",
                "a1",
                AgentMessage::Assistant(pa_types::ai::AssistantMessage {
                    content: vec![],
                    api: "openai-completions".to_string(),
                    provider: "p".to_string(),
                    model: "m".to_string(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: pa_types::ai::Usage {
                        input: 1,
                        output: 1,
                        cache_read: 0,
                        cache_write: 0,
                        total_tokens: 2,
                        cost: pa_types::ai::UsageCost::default(),
                    },
                    stop_reason: pa_types::ai::StopReason::Aborted,
                    stop_reason_raw: None,
                    error_message: None,
                    timestamp: 0,
                    rest: serde_json::Map::default(),
                }),
            ),
        ];
        let usage = get_last_assistant_usage(&entries).unwrap();
        assert_eq!(usage.total_tokens, 150);
    }

    /// S2 (the donor's `projectUnit` lesson): a cut must never separate a tool
    /// RESULT from the ASSISTANT message that requested it.
    ///
    /// `find_valid_cut_points` refuses `ToolResult` entries as cut positions,
    /// which covers the adjacent shape. The shape it cannot see is a result
    /// whose matching call is on the CUT side: the provider then rejects the
    /// request (`tool_call_id` not found), so the plan advertised a split it
    /// could not apply — the donor's "Range would split N turn(s)" symptom.
    #[test]
    fn a_cut_never_separates_a_tool_result_from_its_call() {
        let entries = vec![
            user_entry("m1", "", "start"),
            message_entry("m2", "m1", tool_call_assistant("call-1", "bash")),
            message_entry("m3", "m2", tool_result_message("call-1", "done")),
            user_entry("m4", "m3", "next"),
            user_entry("m5", "m4", "last"),
        ];
        let cut = find_cut_point(&entries, 0, entries.len(), 0);
        assert!(
            !orphans_a_tool_result(&entries, cut.first_kept_entry_index),
            "cut {} separated a result from its call",
            cut.first_kept_entry_index
        );

        // The shape that CAN happen: the result survives while its call is cut.
        let split = vec![
            user_entry("m1", "", "start"),
            message_entry("m2", "m1", tool_call_assistant("call-1", "bash")),
            user_entry("m3", "m2", "interrupt"),
            message_entry("m4", "m3", tool_result_message("call-1", "done")),
        ];
        assert!(
            orphans_a_tool_result(&split, 2),
            "the predicate must FIRE on the split shape, or it proves nothing"
        );
        assert!(
            !orphans_a_tool_result(&split, 1),
            "keeping the call with its result is legal"
        );
        assert!(
            orphans_a_tool_result(&split, 3),
            "a result with no call before it is orphaned wherever the cut lands"
        );
    }

    /// S2 exhaustive probe: is there ANY transcript shape and budget where the cut
    /// orphans a result? If none, the walk-back loop in `prepare_compaction` is dead
    /// code and the invariant belongs in a test, not in a runtime loop.
    ///
    /// Enumerates every arrangement of the three shapes that can interleave a call
    /// with its result: assistant-with-call, tool-result, and user/assistant text.
    #[test]
    fn probe_every_shape_and_budget_for_a_stranded_result() {
        // Shapes: C = assistant with a tool call, R = its result, T = plain text turn.
        let shapes: &[&str] = &["C", "R", "T"];
        let mut checked = 0usize;
        let mut stranded: Vec<String> = Vec::new();
        for len in 1..=6usize {
            let combos = 3usize.pow(len as u32);
            for combo in 0..combos {
                let mut pattern = String::new();
                let mut n = combo;
                for _ in 0..len {
                    pattern.push_str(shapes[n % 3]);
                    n /= 3;
                }
                // A result with no preceding call is malformed input, not a cut bug.
                let entries = entries_for(&pattern);
                for keep in 0..200u64 {
                    let Ok(prep) =
                        crate::session_engine::compact_session::prepare_compaction(&entries, keep)
                    else {
                        continue;
                    };
                    checked += 1;
                    if orphans_a_tool_result(&entries, prep.cut.first_kept_entry_index) {
                        stranded.push(format!("{pattern} @ keep={keep}"));
                    }
                }
            }
        }
        assert!(
            stranded.is_empty(),
            "{} shape(s) strand a result: {:?}",
            stranded.len(),
            &stranded[..stranded.len().min(5)]
        );
        assert!(checked > 1_000, "the probe must actually run: {checked}");
        eprintln!("checked {checked} (pattern, budget) pairs, none stranded");
    }

    fn entries_for(pattern: &str) -> Vec<FileEntry> {
        let mut out = Vec::new();
        let mut call_id = 0usize;
        for (index, kind) in pattern.chars().enumerate() {
            let id = format!("e{index}");
            let parent = if index == 0 {
                String::new()
            } else {
                format!("e{}", index - 1)
            };
            let message = match kind {
                'C' => {
                    call_id += 1;
                    tool_call_assistant(&format!("call-{call_id}"), "bash")
                }
                'R' => {
                    // The result names the MOST RECENT call, which is the only
                    // well-formed arrangement.
                    tool_result_message(&format!("call-{call_id}"), "out")
                }
                _ => user_message(&format!("text {index}"), 0),
            };
            out.push(message_entry(&id, &parent, message));
        }
        out
    }

    /// The defect the exhaustive probe found: when the whole history FITS the
    /// retention budget, the cut still lands on the first valid message, so
    /// everything before it is summarized away and — when that message is a
    /// tool result's position — a result is stranded from its call.
    ///
    /// A cut is only needed when something must be dropped. A budget larger than the
    /// history means "keep it all", and the honest cut then is `start_index`.
    #[test]
    fn a_history_that_fits_the_budget_is_kept_whole() {
        let entries = vec![
            user_entry("m1", "", "start"),
            message_entry("m2", "m1", tool_call_assistant("call-1", "bash")),
            message_entry("m3", "m2", tool_result_message("call-1", "done")),
            user_entry("m4", "m3", "next"),
        ];
        let total: u64 = entries.iter().map(estimate_entry_tokens).sum();
        // A budget past the whole history: nothing needs to be dropped.
        let cut = find_cut_point(&entries, 0, entries.len(), total + 1_000);
        assert_eq!(
            cut.first_kept_entry_index, 0,
            "a history that fits must be kept whole, not cut at its first message"
        );
        assert!(
            !orphans_a_tool_result(&entries, cut.first_kept_entry_index),
            "and nothing is stranded"
        );
    }
}
