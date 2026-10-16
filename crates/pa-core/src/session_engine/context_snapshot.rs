//! FORK-ONLY (not in upstream): a context snapshot for test/diagnostic
//! assertions over the exact model-facing context a session builds — the
//! system prompt, the conversation messages, and the tool definitions —
//! serialized into a normalized, key-sorted JSON document.
//!
//! Two sessions assembled from the same config must produce byte-identical
//! canonical JSON; a session whose MCP servers (or skills, tools, model, or
//! prompt inputs) differ must produce a different document. Volatile fields —
//! the session id, timestamps, the agent dir, the working directory, and the
//! crate version — are replaced with stable placeholders so only meaningful
//! differences survive.

use serde_json::{Map, Value};

use super::engine::SessionEngine;

/// The placeholder a volatile string field collapses to.
pub const SESSION_PLACEHOLDER: &str = "<SESSION>";
/// The timestamp placeholder.
pub const TIMESTAMP_PLACEHOLDER: &str = "<TIMESTAMP>";
/// The agent-dir placeholder.
pub const AGENT_DIR_PLACEHOLDER: &str = "<AGENT_DIR>";
/// The working-directory placeholder.
pub const CWD_PLACEHOLDER: &str = "<CWD>";
/// The version placeholder.
pub const VERSION_PLACEHOLDER: &str = "<VERSION>";
/// The conversation-log path placeholder.
pub const CONVERSATION_LOG_PLACEHOLDER: &str = "<CONVERSATION_LOG>";

/// The context snapshot surface: capture, normalize, canonicalize.
pub struct ContextSnapshot;

impl ContextSnapshot {
    /// Capture the engine's exact model-facing context as a JSON document.
    ///
    /// The raw volatile values (session id, agent dir, cwd, version) ride the
    /// document under their own keys so [`ContextSnapshot::normalize`] can
    /// rewrite every occurrence elsewhere without extra arguments.
    pub async fn capture(engine: &SessionEngine) -> Value {
        let state = engine.session.agent().state().await;
        let (session_id, cwd) = {
            let persistence = engine.session.shared_persistence();
            let manager = persistence.lock().await;
            (
                manager.get_session_id().to_string(),
                manager.get_cwd().display().to_string(),
            )
        };
        let agent_dir = engine
            .session
            .agent_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let mut tools: Vec<Value> = state
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name(),
                    "label": tool.label(),
                    "description": tool.description(),
                    "parameters": tool.parameters(),
                })
            })
            .collect();
        // Tool order is roster order and already deterministic; sorting keeps
        // the document stable if a future roster gains a nondeterministic
        // insertion point.
        tools.sort_by(|left, right| {
            left.get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    right
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        });
        let messages = serde_json::to_value(&state.messages).unwrap_or(Value::Null);
        let mut skills: Vec<String> = engine
            .skills
            .iter()
            .map(|skill| skill.name.clone())
            .collect();
        skills.sort();
        Value::Object(Map::from_iter([
            ("sessionId".to_string(), Value::String(session_id)),
            ("agentDir".to_string(), Value::String(agent_dir)),
            ("cwd".to_string(), Value::String(cwd)),
            (
                "version".to_string(),
                Value::String(env!("CARGO_PKG_VERSION").to_string()),
            ),
            (
                "systemPrompt".to_string(),
                Value::String(engine.system_prompt.clone()),
            ),
            (
                "model".to_string(),
                serde_json::to_value(&state.model).unwrap_or(Value::Null),
            ),
            (
                "thinkingLevel".to_string(),
                serde_json::to_value(state.thinking_level).unwrap_or(Value::Null),
            ),
            ("messages".to_string(), messages),
            ("tools".to_string(), Value::Array(tools)),
            (
                "skills".to_string(),
                Value::Array(skills.into_iter().map(Value::String).collect()),
            ),
            (
                "nativeTools".to_string(),
                Value::Array(
                    engine
                        .native_tool_names
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
        ]))
    }

    /// Rewrite every volatile field in `value` to its placeholder, in place.
    ///
    /// Keys named like a volatile field collapse regardless of value; string
    /// values elsewhere have embedded raw session-id / agent-dir / cwd text
    /// rewritten (the system prompt's environment section carries the cwd).
    pub fn normalize(value: &mut Value) {
        let context = NormalizationContext {
            session_id: string_field(value, "sessionId"),
            agent_dir: string_field(value, "agentDir"),
            cwd: string_field(value, "cwd"),
            version: string_field(value, "version"),
        };
        normalize_value(value, &context);
    }

    /// The canonical JSON of a snapshot: object keys recursively sorted, no
    /// insignificant whitespace. Two contexts that differ only in volatile
    /// fields serialize to identical bytes.
    #[must_use]
    pub fn to_canonical_json(value: &Value) -> String {
        serde_json::to_string(&canonicalize(value)).unwrap_or_else(|_| "null".to_string())
    }

    /// Capture, normalize, and canonicalize in one step.
    pub async fn capture_canonical(engine: &SessionEngine) -> String {
        let mut snapshot = Self::capture(engine).await;
        Self::normalize(&mut snapshot);
        Self::to_canonical_json(&snapshot)
    }
}

struct NormalizationContext {
    session_id: String,
    agent_dir: String,
    cwd: String,
    version: String,
}

impl NormalizationContext {
    /// Raw strings to rewrite inside free text, longest first so a path that
    /// contains another is rewritten before its substring.
    fn replacements(&self) -> Vec<(&str, &'static str)> {
        let mut pairs: Vec<(&str, &'static str)> = Vec::new();
        if !self.agent_dir.is_empty() {
            pairs.push((self.agent_dir.as_str(), AGENT_DIR_PLACEHOLDER));
        }
        if !self.cwd.is_empty() {
            pairs.push((self.cwd.as_str(), CWD_PLACEHOLDER));
        }
        if !self.version.is_empty() {
            pairs.push((self.version.as_str(), VERSION_PLACEHOLDER));
        }
        if !self.session_id.is_empty() {
            pairs.push((self.session_id.as_str(), SESSION_PLACEHOLDER));
        }
        // Longest raw string first, so a path containing another is rewritten
        // before its substring.
        pairs.sort_by_key(|(raw, _)| std::cmp::Reverse(raw.len()));
        pairs
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The placeholder a volatile key collapses to, if the key is volatile.
fn volatile_placeholder(key: &str) -> Option<&'static str> {
    match key {
        "sessionId" | "session_id" => Some(SESSION_PLACEHOLDER),
        "timestamp" | "createdAt" | "created_at" | "updatedAt" => Some(TIMESTAMP_PLACEHOLDER),
        "agentDir" | "agent_dir" => Some(AGENT_DIR_PLACEHOLDER),
        "cwd" | "workingDirectory" | "working_directory" => Some(CWD_PLACEHOLDER),
        "version" => Some(VERSION_PLACEHOLDER),
        "messagesPath" | "messages_path" | "conversationLog" | "conversation_log_path" => {
            Some(CONVERSATION_LOG_PLACEHOLDER)
        }
        _ => None,
    }
}

fn normalize_value(value: &mut Value, context: &NormalizationContext) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if let Some(placeholder) = volatile_placeholder(key) {
                    *child = Value::String(placeholder.to_string());
                } else {
                    normalize_value(child, context);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_value(item, context);
            }
        }
        Value::String(text) => {
            for (raw, placeholder) in context.replacements() {
                if text.contains(raw) {
                    *text = text.replace(raw, placeholder);
                }
            }
        }
        _ => {}
    }
}

/// Rebuild `value` with every object's keys in sorted order. With
/// `serde_json`'s `preserve_order` feature the map is an `IndexMap`, so
/// inserting keys in sorted order yields sorted serialization.
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonicalize(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests;
