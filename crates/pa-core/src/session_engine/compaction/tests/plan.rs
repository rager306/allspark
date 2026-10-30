//! The priced projection: what the wire actually carries, and the prefix rule.
//!
//! A plan adds the system prompt and tool schemas the message estimate cannot
//! see, and it must add them EXACTLY once: twice over-counts an anchored
//! estimate, zero under-counts a fresh one.

use super::super::tokens::div4;
use super::super::*;

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
    // A REAL fork tool, not a synthetic schema: the native edit tool's schema is what the
    // model actually receives on a session with `editHybrid.enabled`. (It priced the bash
    // definition before upstream removed the built-in bash tool as dead code.)
    let tool: std::sync::Arc<dyn pa_agent::types::AgentTool> =
        std::sync::Arc::new(pa_edit::tool::EditHybridTool::new(
            std::sync::Arc::new(std::sync::Mutex::new(pa_edit::EditStore::new())),
            None,
            std::path::PathBuf::from("/tmp"),
            pa_edit::EditMode::Replace,
            vec![pa_edit::EditMode::Replace],
        ));
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
    let static_prompt = include_str!("../../../prompts/layers/core.md");
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

/// The prefix rule lives in ONE place, and both halves of it are load-bearing.
///
/// History of getting this wrong, in both directions:
/// - adding the prefix to an ANCHORED estimate double-counted it (the provider's
///   usage already covers the prompt and schemas), inflating the threshold enough to
///   compact a turn early in seven daemon tests;
/// - adding `tool_schemas` unconditionally while gating only `system_prompt` left the
///   schemas double-counted — the same bug at half the size, which is why the rule is
///   a function instead of two inline `if`s.
#[test]
fn the_prefix_is_priced_exactly_once() {
    let anchored = ContextTokensEstimate {
        tokens: 20_000,
        last_usage_index: Some(3),
    };
    let plan = ContextPlan::for_request(anchored, true, (14_197, 3_810), 4_000, 128_000);
    assert_eq!(
        plan.input(),
        20_000,
        "an anchored estimate already carries the prefix: {plan:?}"
    );

    let fresh = ContextTokensEstimate {
        tokens: 500,
        last_usage_index: None,
    };
    let plan = ContextPlan::for_request(fresh, false, (14_197, 3_810), 4_000, 128_000);
    assert_eq!(
        plan.input(),
        500 + 14_197 + 3_810,
        "without an anchor the prefix must be added, or the request is priced as if \
         it carried no prompt: {plan:?}"
    );
    assert_eq!(plan.output_reserve, 4_000);
    assert_eq!(plan.context_window, 128_000);
}
