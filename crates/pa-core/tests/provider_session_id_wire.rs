//! Wire-level regression: the session engine must feed the session's id
//! onto the provider request surface. Providers key prompt-cache and
//! transport affinity by it (`prompt_cache_key` in the body plus the
//! `session_id`/`x-client-request-id` headers for the `OpenAI` responses
//! family), and the whole consumer chain (`AgentOptions.session_id` ->
//! `AgentLoopConfig` -> `StreamRequestOptions` -> `pa_ai::StreamOptions`)
//! existed in the port while the engine built the agent with the default
//! `None` — every provider request left without it (the codex regression
//! class: cache hits fell 96.9% -> 22.5% in upstream discussion #3618).
//!
//! A subagent's requests key by the ROOT session's durable id, so a spawn
//! tree shares one cache routing group. The engine proves it end to end:
//! a real `openai-responses` provider stream against the wire-level mock
//! inference server, one turn through `create_session`'s own wiring.

use std::path::Path;
use std::sync::{Arc, RwLock};

use pa_ai::testing::MockInferenceServer;
use pa_core::session_engine::engine::{create_session, SessionEngineConfig};
use pa_core::session_engine::provider_adapter::{switchable_stream_fn, ProviderTarget};
use pa_core::session_engine::semantic_edges::SemanticEdgeIdentity;
use pa_core::session_engine::{PromptOptions, PromptOutcome};
use serde_json::json;

/// An `openai-responses` model bound to the mock server, as a provider
/// target (the wire-facing model) plus the agent-loop view of it.
fn responses_model(server: &MockInferenceServer) -> (ProviderTarget, pa_agent::types::Model) {
    let wire: pa_types::ai::Model = serde_json::from_value(json!({
        "id": "mock-responses",
        "name": "Mock Responses",
        "api": "openai-responses",
        "provider": "mock",
        "baseUrl": server.base_url(),
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 128_000,
        "maxTokens": 4096,
    }))
    .expect("mock model");
    let target = ProviderTarget {
        api_key: Some("sk-mock".to_string()),
        model: wire.clone(),
        service_tier: None,
        headers: None,
    };
    let agent_model = pa_core::session_engine::provider_adapter::json_round_trip(&wire)
        .expect("model crosses the crate boundary");
    (target, agent_model)
}

/// One scripted `openai-responses` turn: a text answer, then the terminal
/// `response.completed` frame.
fn queue_response_turn(server: &MockInferenceServer) {
    server.queue_sse(vec![
        &json!({"type": "response.created", "response": {"id": "resp_mock"}}).to_string(),
        &json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "message", "id": "msg_1"},
        })
        .to_string(),
        &json!({
            "type": "response.output_text.delta",
            "item_id": "msg_1",
            "output_index": 0,
            "content_index": 0,
            "delta": "done",
        })
        .to_string(),
        &json!({
            "type": "response.completed",
            "response": {
                "id": "resp_mock",
                "usage": {"input_tokens": 3, "output_tokens": 1, "total_tokens": 4},
            },
        })
        .to_string(),
    ]);
}

/// Build the engine over the mock provider and run one turn; the server has
/// recorded the request when the agent goes idle.
async fn engine_turn(
    target: ProviderTarget,
    model: pa_agent::types::Model,
    session_file: Option<&Path>,
    durable_session_id: &str,
) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let slot = Arc::new(RwLock::new(Some(target)));
    let engine = Box::pin(create_session(SessionEngineConfig {
        cwd,
        agent_dir: tmp.path().join("agent"),
        model: Some(model),
        stream_fn: Some(switchable_stream_fn(slot)),
        conversation_log_path: session_file.map(Path::to_path_buf),
        semantic_edges: Some(SemanticEdgeIdentity {
            session_id: durable_session_id.to_string(),
            ledger_path: None,
            parent_session_id: None,
            spawned_by_request_id: None,
        }),
        prewarm_ipython_kernel: Some(false),
        ..SessionEngineConfig::default()
    }))
    .await
    .unwrap();

    let outcome = engine
        .prompt("say done", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(outcome, PromptOutcome::Prompt);
    engine.session.agent().wait_for_idle().await;
}

/// A session file header line (id, optional parent) — the durable JSONL the
/// trace upload and the cache-key walk read.
fn session_header_line(id: &str, parent: Option<&Path>) -> String {
    let mut header = json!({
        "type": "session",
        "version": 3,
        "id": id,
        "timestamp": "2026-10-20T00:00:00Z",
        "cwd": "/tmp",
        "rlmDepth": 0,
    });
    if let Some(parent) = parent {
        header["parentSession"] = json!(parent.display().to_string());
    }
    header.to_string()
}

fn write_session_file(path: &Path, id: &str, parent: Option<&Path>) {
    if let Some(parent_dir) = path.parent() {
        std::fs::create_dir_all(parent_dir).unwrap();
    }
    std::fs::write(path, session_header_line(id, parent)).unwrap();
}

#[tokio::test]
async fn a_top_level_session_keys_its_provider_requests_by_its_durable_id() {
    let server = MockInferenceServer::new();
    queue_response_turn(&server);
    let (target, model) = responses_model(&server);

    Box::pin(engine_turn(target, model, None, "durable-root-session-id")).await;

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "one turn, one provider request");
    let body = requests[0].body_json().expect("completions body is JSON");
    assert_eq!(
        body.get("prompt_cache_key"),
        Some(&json!("durable-root-session-id")),
        "the session id must reach the wire as prompt_cache_key"
    );
    assert_eq!(
        requests[0].header("session_id"),
        Some("durable-root-session-id"),
        "the session id must reach the wire as the session_id header"
    );
    assert_eq!(
        requests[0].header("x-client-request-id"),
        Some("durable-root-session-id")
    );
}

#[tokio::test]
async fn a_subagent_keys_its_provider_requests_by_the_root_session_id() {
    let server = MockInferenceServer::new();
    queue_response_turn(&server);
    let (target, model) = responses_model(&server);

    // A three-level spawn tree on disk: root -> mid -> child. The child's
    // engine must walk past its immediate parent to the ROOT's id.
    let tmp = tempfile::tempdir().unwrap();
    let sessions = tmp.path().join("sessions");
    let root_file = sessions.join("root-session-id.jsonl");
    let mid_file = sessions.join("mid-session-id.jsonl");
    let child_file = tmp.path().join("child-session-id.jsonl");
    write_session_file(&root_file, "root-session-id", None);
    write_session_file(&mid_file, "mid-session-id", Some(&root_file));
    write_session_file(&child_file, "child-durable-id", Some(&mid_file));

    Box::pin(engine_turn(
        target,
        model,
        Some(&child_file),
        "child-durable-id",
    ))
    .await;

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "one turn, one provider request");
    let body = requests[0].body_json().expect("completions body is JSON");
    assert_eq!(
        body.get("prompt_cache_key"),
        Some(&json!("root-session-id")),
        "a subagent's requests carry the ROOT session's id, not its own"
    );
}
