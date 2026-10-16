//! Context-snapshot tests: two sessions from the same config canonicalize
//! identically; a session with different MCP servers (or tools) does not.

use std::sync::Arc;

use pa_agent::scripted::ScriptedProvider;

use super::*;
use crate::session_engine::engine::{create_session, SessionEngineConfig};

fn model() -> pa_agent::types::Model {
    pa_agent::types::Model {
        id: "m".into(),
        name: "m".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 1_000,
        max_tokens: 100,
        max_tokens_explicit: false,
    }
}

fn config(cwd: &std::path::Path, agent_dir: &std::path::Path) -> SessionEngineConfig {
    SessionEngineConfig {
        cwd: cwd.to_path_buf(),
        agent_dir: agent_dir.to_path_buf(),
        model: Some(model()),
        stream_fn: Some(Arc::new(ScriptedProvider::new(model())).stream_fn()),
        tools: vec![],
        daemonless: true,
        ..Default::default()
    }
}

/// A temp project/agent pair seeded with one user message so the message list
/// is non-trivial (and its per-message timestamp is exercised).
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    (tmp, cwd, agent_dir)
}

async fn push_user_message(engine: &SessionEngine, text: &str) {
    engine
        .session
        .agent()
        .append_messages(vec![pa_agent::types::AgentMessage::user(text)])
        .await;
}

#[tokio::test]
async fn identical_configs_canonicalize_identically() {
    let (_tmp, cwd, agent_dir) = fixture();
    let first = create_session(config(&cwd, &agent_dir)).await.unwrap();
    push_user_message(&first, "hello").await;
    let second = create_session(config(&cwd, &agent_dir)).await.unwrap();
    push_user_message(&second, "hello").await;

    // The two sessions have distinct ids; the canonical form must erase
    // that (and every other volatile field).
    assert_ne!(
        first
            .session
            .shared_persistence()
            .lock()
            .await
            .get_session_id(),
        second
            .session
            .shared_persistence()
            .lock()
            .await
            .get_session_id()
    );

    let first_json = ContextSnapshot::capture_canonical(&first).await;
    let second_json = ContextSnapshot::capture_canonical(&second).await;
    assert_eq!(first_json, second_json);

    // The placeholders actually landed: no raw session id survives.
    let raw_id = first
        .session
        .shared_persistence()
        .lock()
        .await
        .get_session_id()
        .to_string();
    assert!(!first_json.contains(&raw_id));
    assert!(first_json.contains(SESSION_PLACEHOLDER));
    assert!(first_json.contains(TIMESTAMP_PLACEHOLDER));
}

#[tokio::test]
async fn different_mcp_servers_diverge() {
    let (_tmp, cwd, agent_dir) = fixture();
    let plain = create_session(config(&cwd, &agent_dir)).await.unwrap();
    let mut with_mcp = config(&cwd, &agent_dir);
    with_mcp.generic_mcp_servers = vec!["fixture-server".to_string()];
    let mcp = create_session(with_mcp).await.unwrap();

    let plain_json = ContextSnapshot::capture_canonical(&plain).await;
    let mcp_json = ContextSnapshot::capture_canonical(&mcp).await;
    assert_ne!(plain_json, mcp_json);
    assert!(mcp_json.contains("fixture-server"));
    assert!(!plain_json.contains("fixture-server"));
}

#[tokio::test]
async fn different_custom_prompts_diverge() {
    let (_tmp, cwd, agent_dir) = fixture();
    let plain = create_session(config(&cwd, &agent_dir)).await.unwrap();
    let mut custom = config(&cwd, &agent_dir);
    custom.custom_system_prompt = Some("Only answer in haiku.".to_string());
    let replaced = create_session(custom).await.unwrap();

    let plain_json = ContextSnapshot::capture_canonical(&plain).await;
    let custom_json = ContextSnapshot::capture_canonical(&replaced).await;
    assert_ne!(plain_json, custom_json);
    assert!(custom_json.contains("Only answer in haiku."));
}

#[tokio::test]
async fn normalize_rewrites_free_text_occurrences() {
    let cwd = "/tmp/volatile-project";
    let agent_dir = "/tmp/volatile-agent";
    let mut value = serde_json::json!({
        "sessionId": "sess-123",
        "agentDir": agent_dir,
        "cwd": cwd,
        "version": "9.9.9",
        "timestamp": 1_750_000_000,
        "systemPrompt": format!("Working directory: {cwd}\nConversation log: not persisted\nAgent dir {agent_dir} v9.9.9"),
        "messages": [{"role": "user", "text": "hi", "timestamp": 1_750_000_000}],
    });
    ContextSnapshot::normalize(&mut value);

    assert_eq!(value["sessionId"], SESSION_PLACEHOLDER);
    assert_eq!(value["timestamp"], TIMESTAMP_PLACEHOLDER);
    let prompt = value["systemPrompt"].as_str().unwrap();
    assert!(!prompt.contains(cwd));
    assert!(prompt.contains(CWD_PLACEHOLDER));
    assert!(prompt.contains(AGENT_DIR_PLACEHOLDER));
    assert!(prompt.contains(VERSION_PLACEHOLDER));
    assert_eq!(value["messages"][0]["timestamp"], TIMESTAMP_PLACEHOLDER);
}

#[test]
fn canonical_json_sorts_object_keys_recursively() {
    let value = serde_json::json!({"z": 1, "a": {"y": 2, "b": 3}, "m": [{"q": 1, "c": 2}]});
    let canonical = ContextSnapshot::to_canonical_json(&value);
    assert_eq!(
        canonical,
        r#"{"a":{"b":3,"y":2},"m":[{"c":2,"q":1}],"z":1}"#
    );
}

#[tokio::test]
async fn snapshot_carries_prompt_messages_and_tools() {
    let (_tmp, cwd, agent_dir) = fixture();
    let engine = create_session(config(&cwd, &agent_dir)).await.unwrap();
    push_user_message(&engine, "ping").await;
    let mut snapshot = ContextSnapshot::capture(&engine).await;
    ContextSnapshot::normalize(&mut snapshot);

    assert!(snapshot["systemPrompt"].as_str().unwrap().len() > 100);
    assert_eq!(snapshot["messages"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["messages"][0]["role"], "user");
    // The ipython tool is always on the roster; its definition rides along.
    let tool_names: Vec<&str> = snapshot["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(tool_names.contains(&pa_types::ai::PYTHON_TOOL_NAME));
    assert!(tool_names.contains(&"lsp"));
}
