//! Anthropic Messages request params assembly.

use serde_json::{json, Map, Value};

use crate::providers::anthropic::convert::{convert_messages, convert_tools};
use crate::providers::anthropic::{
    get_anthropic_compat, is_always_on_adaptive_thinking_model, supports_adaptive_thinking,
    AnthropicOptions, AnthropicThinkingDisplay, CacheControl,
};
use crate::types::{Context, Model};
use crate::utils_inner::sanitize_unicode::sanitize_surrogates;

// Long by design: mirrors the provider's stream shape.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_params(
    model: &Model,
    context: &Context,
    is_oauth_token: bool,
    options: Option<&AnthropicOptions>,
    cache_control: Option<&CacheControl>,
) -> Value {
    let base = options
        .map(|options| options.base.clone())
        .unwrap_or_default();
    let mut params = Map::new();
    params.insert("model".into(), json!(model.id));
    params.insert(
        "messages".into(),
        json!(convert_messages(
            context,
            model,
            is_oauth_token,
            cache_control
        )),
    );
    params.insert(
        "max_tokens".into(),
        json!(base.max_tokens.unwrap_or(model.max_tokens / 3)),
    );
    params.insert("stream".into(), json!(true));

    // For OAuth tokens, we MUST include Claude Code identity.
    if is_oauth_token {
        let mut system = vec![json!({
            "type": "text",
            "text": "You are Claude Code, Anthropic's official CLI for Claude.",
        })];
        // No breakpoint on the identity block: it sits at the very front and is
        // already covered by the breakpoint on the prompt block that follows
        // it. Anthropic allows four breakpoints per request, and a redundant
        // one here spends a slot that the conversation tail needs (oh-my-pi
        // strips exactly this one for the same reason).
        if let Some(system_prompt) = &context.system_prompt {
            let mut entry = json!({
                "type": "text",
                "text": sanitize_surrogates(system_prompt),
            });
            if let Some(cache_control) = cache_control {
                entry
                    .as_object_mut()
                    .expect("system entry is an object")
                    .insert("cache_control".into(), cache_control.to_json());
            }
            system.push(entry);
        }
        params.insert("system".into(), json!(system));
    } else if let Some(system_prompt) = &context.system_prompt {
        // Split at the cache boundary when the caller declared one: the stable
        // block gets its own breakpoint so it survives a change in the volatile
        // tail (the date, the working directory, a file list). Without the
        // split a single block with one breakpoint at its end is re-billed in
        // full the moment any trailing byte changes.
        let split = context
            .system_prompt_cache_prefix
            .filter(|&at| at > 0 && at < system_prompt.len())
            .filter(|&at| system_prompt.is_char_boundary(at));
        let mut system = Vec::new();
        if let Some(at) = split {
            let (stable, volatile) = system_prompt.split_at(at);
            let mut stable_entry = json!({
                "type": "text",
                "text": sanitize_surrogates(stable),
            });
            if let Some(cache_control) = cache_control {
                stable_entry
                    .as_object_mut()
                    .expect("system entry is an object")
                    .insert("cache_control".into(), cache_control.to_json());
            }
            system.push(stable_entry);
            system.push(json!({
                "type": "text",
                "text": sanitize_surrogates(volatile),
            }));
        } else {
            system.push(json!({
                "type": "text",
                "text": sanitize_surrogates(system_prompt),
            }));
        }
        if let Some(cache_control) = cache_control {
            if let Some(last) = system.last_mut() {
                last.as_object_mut()
                    .expect("system entry is an object")
                    .insert("cache_control".into(), cache_control.to_json());
            }
        }
        params.insert("system".into(), json!(system));
    }

    // Temperature is incompatible with extended thinking (adaptive or budget-based), and always-on
    // models reject sampling params outright.
    if let Some(temperature) = base.temperature {
        if options.map(|options| options.thinking_enabled) != Some(Some(true))
            && !is_always_on_adaptive_thinking_model(&model.id)
        {
            params.insert("temperature".into(), json!(temperature));
        }
    }

    if let Some(tools) = &context.tools {
        if !tools.is_empty() {
            params.insert(
                "tools".into(),
                json!(convert_tools(
                    tools,
                    is_oauth_token,
                    get_anthropic_compat(model).supports_eager_tool_input_streaming,
                    cache_control,
                )),
            );
        }
    }

    // Configure thinking mode: adaptive, budget-based, or explicitly disabled.
    if model.reasoning {
        if options.map(|options| options.thinking_enabled) == Some(Some(true)) {
            let display = options
                .and_then(|options| options.thinking_display)
                .unwrap_or(AnthropicThinkingDisplay::Summarized);
            if supports_adaptive_thinking(&model.id) {
                params.insert(
                    "thinking".into(),
                    json!({ "type": "adaptive", "display": display.as_str() }),
                );
                if let Some(effort) = options.and_then(|options| options.effort) {
                    params.insert("output_config".into(), json!({ "effort": effort.as_str() }));
                }
            } else {
                params.insert(
                    "thinking".into(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": options.and_then(|options| options.thinking_budget_tokens).unwrap_or(1024),
                        "display": display.as_str(),
                    }),
                );
            }
        } else if options.map(|options| options.thinking_enabled) == Some(Some(false))
            && !is_always_on_adaptive_thinking_model(&model.id)
        {
            params.insert("thinking".into(), json!({ "type": "disabled" }));
        }
    }

    if let Some(metadata) = &base.metadata {
        if let Some(user_id) = metadata.get("user_id").and_then(|value| value.as_str()) {
            params.insert("metadata".into(), json!({ "user_id": user_id }));
        }
    }

    if let Some(tool_choice) = options.and_then(|options| options.tool_choice.as_ref()) {
        params.insert("tool_choice".into(), tool_choice.to_json());
    }

    Value::Object(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, UserMessage, UserMessageContent};
    use serde_json::json;

    fn model() -> Model {
        serde_json::from_value(json!({
            "id": "claude-test", "name": "Claude Test", "api": "anthropic-messages",
            "provider": "anthropic", "baseUrl": "http://127.0.0.1:1", "reasoning": false,
            "input": ["text"],
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
            "contextWindow": 200_000, "maxTokens": 8192
        }))
        .unwrap()
    }

    fn user(text: &str) -> Message {
        Message::User(UserMessage {
            content: UserMessageContent::Text(text.to_string()),
            timestamp: 0,
            rest: Map::default(),
        })
    }

    fn cache() -> CacheControl {
        CacheControl { ttl: None }
    }

    /// The cached prefix must be byte-identical between consecutive turns.
    ///
    /// A provider matches the cache by the longest common prefix of the
    /// serialized request, so anything that reorders or re-renders an earlier
    /// part of the payload silently drops the hit for every token behind it.
    /// Grok Build asserts exactly this (`assert_prefix_stable` on each step);
    /// we had no such guard.
    #[test]
    fn request_prefix_is_byte_stable_across_turns() {
        let model = model();
        let tools = vec![crate::types::Tool {
            name: "lsp".into(),
            description: "query language servers".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }];

        // Turn N: one user message.
        let turn_n = Context {
            system_prompt: Some("# prime-agent harness\n\nStable prefix.".into()),
            system_prompt_cache_prefix: None,
            messages: vec![user("first question")],
            tools: Some(tools.clone()),
        };
        // Turn N+1: the same conversation plus one more exchange.
        let turn_n1 = Context {
            system_prompt: Some("# prime-agent harness\n\nStable prefix.".into()),
            system_prompt_cache_prefix: None,
            messages: vec![user("first question"), user("second question")],
            tools: Some(tools),
        };

        let a = serde_json::to_string(&build_params(&model, &turn_n, false, None, Some(&cache())))
            .unwrap();
        let b = serde_json::to_string(&build_params(&model, &turn_n1, false, None, Some(&cache())))
            .unwrap();

        // The system block and tool array lead both payloads; the longer one
        // must start with everything the shorter one had, minus its closing
        // bracket, up to the messages array.
        let a_system_end = a.find("\"messages\"").expect("messages key");
        let b_system_end = b.find("\"messages\"").expect("messages key");
        assert_eq!(
            &a[..a_system_end],
            &b[..b_system_end],
            "the pre-messages payload (model, system, tools) must not drift between turns"
        );
    }

    /// The cache breakpoint must ride the system block, so a turn that adds a
    /// message still reuses the cached prefix rather than re-billing it.
    #[test]
    fn system_block_carries_the_cache_breakpoint() {
        let context = Context {
            system_prompt: Some("Stable prefix.".into()),
            system_prompt_cache_prefix: None,
            messages: vec![user("hi")],
            tools: None,
        };
        let params = build_params(&model(), &context, false, None, Some(&cache()));
        let system = params.get("system").expect("system present");
        let first = &system[0];
        assert_eq!(
            first.get("cache_control"),
            Some(&json!({"type": "ephemeral"})),
            "system block must carry cache_control: {system}"
        );
    }
}
