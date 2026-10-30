//! The compaction trigger: one threshold formula and the two ways the session
//! asks whether the next request crossed it.

use super::plan::ContextPlan;
use super::tokens::{calculate_context_tokens, estimate_context_tokens, message_timestamp};
use super::{CompactionSettings, COMBINED_LIMIT_HEADROOM_FLOOR, DEFAULT_THRESHOLD_PERCENT};
use pa_types::session::AgentMessage;

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
