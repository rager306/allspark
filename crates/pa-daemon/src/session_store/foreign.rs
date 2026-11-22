//! Foreign (TypeScript-era) session rows: the shape conversions every read
//! path applies before the entry reaches the index.

use serde_json::Value;

/// Fold an assistant row's sibling `toolCalls` array into its typed `content`
/// blocks, mirroring `pa_core::session::normalize_foreign_tool_calls`.
///
/// The TS writer kept tool calls beside `content`; this build reads them as
/// `{"type":"toolCall"}` blocks inside it. A row that keeps them in the sibling
/// field parses with no calls at all, so a cut guard that reads the typed shape
/// treats its `toolResult` as orphaned and walks the cut back to the start —
/// the imported session then refuses to compact ("Session is too short").
///
/// This lives on the daemon's store because the store has its own untyped
/// `SessionEntry`; the core loader normalizes the typed rows it parses.
pub(super) fn normalize_tool_calls(entry: &mut crate::session_store::SessionEntry) {
    if entry.type_ != "message" {
        return;
    }
    let Some(message) = entry.fields.get_mut("message") else {
        return;
    };
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let Some(Value::Array(calls)) = message
        .as_object_mut()
        .and_then(|object| object.remove("toolCalls"))
    else {
        return;
    };
    let content = message
        .as_object_mut()
        .and_then(|object| object.get_mut("content"))
        .and_then(Value::as_array_mut);
    let Some(content) = content else {
        return;
    };
    for call in calls {
        let (Some(id), Some(name), Some(arguments)) = (
            call.get("id").and_then(Value::as_str),
            call.get("name").and_then(Value::as_str),
            call.get("arguments"),
        ) else {
            continue;
        };
        content.push(serde_json::json!({
            "type": "toolCall",
            "id": id,
            "name": name,
            "arguments": arguments,
        }));
    }
}
