//! FORK-ONLY: dev-only one-request bridge for the advisory QA judge.
//! Build explicitly: `cargo build -p pa-ai --example qa_judge_request`

use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pa_ai::types::{
    AssistantContent, Context, Message, SimpleStreamOptions, StopReason, StreamOptions,
    UserMessage, UserMessageContent,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Request {
    model: String,
    api_key: String,
    system_prompt: String,
    state: String,
    timeout_ms: u64,
}

#[tokio::main]
async fn main() {
    let result = run().await;
    println!("{result}");
}

async fn run() -> Value {
    // The judge input is bounded independently of the Python caller.
    let mut input = String::new();
    if io::stdin().take(65_537).read_to_string(&mut input).is_err() || input.len() > 65_536 {
        return json!({"error": "invalid or oversized request"});
    }
    let Ok(request) = serde_json::from_str::<Request>(&input) else {
        return json!({"error": "invalid request"});
    };
    let Some(model) = pa_ai::models_fork::fork_provider_models()
        .into_iter()
        .find(|m| {
            format!("{}/{}", m.provider, m.id) == request.model && m.provider.as_str() == "opencode"
        })
    else {
        return json!({"error": "unsupported judge model"});
    };
    if request.api_key != "public" {
        return json!({"error": "opencode free judge requires explicit public auth"});
    }
    let context = Context {
        system_prompt: Some(request.system_prompt),
        system_prompt_cache_prefix: None,
        messages: vec![Message::User(UserMessage {
            content: UserMessageContent::Text(request.state),
            timestamp: 0,
            rest: serde_json::Map::default(),
        })],
        tools: None,
    };
    let response_meta = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&response_meta);
    let timeout_ms = request.timeout_ms.clamp(1, 120_000);
    let options = SimpleStreamOptions::from_base(StreamOptions {
        api_key: Some(request.api_key),
        max_tokens: Some(512),
        timeout_ms: Some(timeout_ms),
        on_response: Some(Arc::new(move |response, _model| {
            if let Ok(mut meta) = captured.lock() {
                *meta = Some((
                    response.status,
                    response.headers.get("retry-after").cloned(),
                ));
            }
        })),
        ..Default::default()
    });
    let started = Instant::now();
    let completion = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        pa_ai::complete_simple(&model, &context, Some(options)),
    )
    .await;
    let mut output = json!({"model": request.model, "duration_ms": started.elapsed().as_millis()});
    if let Some((status, retry_after)) = response_meta.lock().ok().and_then(|meta| meta.clone()) {
        output["http_status"] = json!(status);
        output["retry_after"] = json!(retry_after);
    }
    match completion {
        Err(_) => output["error"] = json!("judge timeout"),
        Ok(Err(_)) => output["error"] = json!("provider request failed"),
        Ok(Ok(message)) => {
            output["usage"] = json!(message.usage);
            match message.stop_reason {
                StopReason::Stop => {
                    output["text"] = json!(message
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            AssistantContent::Text(text) => Some(text.text.as_str()),
                            AssistantContent::Thinking(_) | AssistantContent::ToolCall(_) => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"));
                }
                StopReason::Length
                | StopReason::ToolUse
                | StopReason::Error
                | StopReason::Aborted => {
                    output["error"] = json!("incomplete or failed provider response");
                }
            }
        }
    }
    output
}
