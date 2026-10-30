//! Fixtures shared by the compaction tests: one builder per entry shape.

use pa_types::session::EntryBase;
// Re-exported so a test module's one `use super::fixtures::*;` brings the entry
// vocabulary with the builders that construct it.
pub(super) use pa_types::session::{AgentMessage, FileEntry};

pub(super) fn message_entry(id: &str, parent: &str, message: AgentMessage) -> FileEntry {
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

pub(super) fn user_entry(id: &str, parent: &str, text: &str) -> FileEntry {
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

pub(super) fn tool_call_assistant(id: &str, tool: &str) -> AgentMessage {
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

pub(super) fn tool_result_message(call_id: &str, text: &str) -> AgentMessage {
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

pub(super) fn assistant_entry(id: &str, parent: &str) -> FileEntry {
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

pub(super) fn user_message(text: &str, timestamp: u64) -> AgentMessage {
    AgentMessage::User(pa_types::ai::UserMessage {
        content: pa_types::ai::UserContent::Text(text.to_string()),
        timestamp,
        rest: serde_json::Map::default(),
    })
}

pub(super) fn assistant_message(usage_total: u64, timestamp: u64) -> AgentMessage {
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

pub(super) fn compaction_summary(timestamp: u64) -> AgentMessage {
    AgentMessage::CompactionSummary(pa_types::session::CompactionSummaryMessage {
        summary: "the story so far".to_string(),
        tokens_before: 100,
        retained_message_count: None,
        custom_instructions: None,
        harness_digest: None,
        task_state: None,
        harness_state_fingerprint: None,
        timestamp,
    })
}

/// A minimal valid PNG header (24 bytes) whose IHDR declares `width`x`height`;
/// the bounded-prefix decoder reads the geometry without a full image.
pub(super) fn png_base64(width: u32, height: u32) -> String {
    use base64::Engine as _;
    let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10];
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(super) fn user_image_message(
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

pub(super) fn entries_for(pattern: &str) -> Vec<FileEntry> {
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
