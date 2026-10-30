//! The chars/4 token estimates: one message, one session entry, one whole
//! context.
//!
//! Every number here is an ESTIMATE the compaction trigger and the retention
//! budget both price against, so the entry-level and message-level estimators
//! must agree on what one model-visible entry costs.

use crate::tools::render_utils::{get_image_dimensions_prefix, IMAGE_DIMENSIONS_PREFIX_BYTES};
use pa_types::session::{AgentMessage, FileEntry};

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
                || payload.custom_type
                    == crate::session_engine::ipython_state::IPYTHON_STATE_CUSTOM_TYPE
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

pub(super) fn div4(chars: u64) -> u64 {
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
pub(super) fn message_timestamp(message: &AgentMessage) -> u64 {
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
