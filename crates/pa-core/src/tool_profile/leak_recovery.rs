//! O8: tool-call markup leak recovery.
//!
//! Some models occasionally emit their tool-call markup as *plain text*
//! instead of a structured tool-call delta (`OpenAI harmony` `<|call|>` tokens,
//! `DeepSeek DSML` `<|DSML|tool_calls>` blocks). Left alone the loop sees an
//! assistant turn with no tool calls and the work stalls. This module detects
//! the leaked markup, recovers the embedded calls, and (bounded by a cap)
//! nudges the model to re-issue them structurally.

use serde::{Deserialize, Serialize};

/// Maximum number of recovery nudges issued in one session (Serena/omp
/// `MAX_DSML_LEAK_NUDGES`).
pub const MAX_LEAK_NUDGES: u32 = 2;

/// A tool call recovered from leaked markup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveredCall {
    /// The tool name.
    pub name: String,
    /// The call arguments.
    pub arguments: serde_json::Value,
}

/// A recovery attempt's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct LeakRecovery {
    /// The leaked markup that was detected.
    pub kind: LeakKind,
    /// The calls recovered from it.
    pub calls: Vec<RecoveredCall>,
    /// Text with the leaked markup removed.
    pub cleaned_text: String,
}

/// The markup dialect detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakKind {
    /// `OpenAI harmony` tokens (`<|call|> ... <|end|>`).
    Harmony,
    /// `DeepSeek DSML` blocks (`<|DSML|tool_calls> ... </|DSML|tool_calls>`).
    Dsml,
}

impl LeakKind {
    /// A human-facing name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            LeakKind::Harmony => "harmony",
            LeakKind::Dsml => "dsml",
        }
    }
}

/// Detect leaked tool-call markup in assistant text.
#[must_use]
pub fn detect(text: &str) -> Option<LeakKind> {
    if text.contains("<|DSML|") {
        Some(LeakKind::Dsml)
    } else if text.contains("<|call|>") || text.contains("<|tool_call|>") {
        Some(LeakKind::Harmony)
    } else {
        None
    }
}

/// Recover leaked tool calls from assistant text.
///
/// Returns `None` when no markup is present. Recovery is best-effort: a block
/// that does not parse as JSON is dropped, and the cleaned text keeps
/// everything outside the markup.
#[must_use]
pub fn recover(text: &str) -> Option<LeakRecovery> {
    let kind = detect(text)?;
    let (calls, cleaned_text) = match kind {
        LeakKind::Harmony => recover_harmony(text),
        LeakKind::Dsml => recover_dsml(text),
    };
    Some(LeakRecovery {
        kind,
        calls,
        cleaned_text,
    })
}

fn recover_harmony(text: &str) -> (Vec<RecoveredCall>, String) {
    let mut calls = Vec::new();
    let mut cleaned = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<|call|>") {
        cleaned.push_str(&rest[..start]);
        let after = &rest[start + "<|call|>".len()..];
        let end = after
            .find("<|end|>")
            .or_else(|| after.find("<|call|>"))
            .unwrap_or(after.len());
        let body = &after[..end];
        if let Some(call) = parse_call_json(body) {
            calls.push(call);
        }
        rest = if end < after.len() { &after[end..] } else { "" };
        if rest.starts_with("<|end|>") {
            rest = &rest["<|end|>".len()..];
        }
    }
    cleaned.push_str(rest);
    (calls, cleaned.trim().to_string())
}

fn recover_dsml(text: &str) -> (Vec<RecoveredCall>, String) {
    let mut calls = Vec::new();
    let mut cleaned = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<|DSML|") {
        cleaned.push_str(&rest[..start]);
        let after = &rest[start..];
        // Each block is `<|DSML|invoke name="...">...</|DSML|invoke>` or a
        // bare JSON payload inside a `tool_calls` wrapper.
        if let Some(invoke_start) = after.find("<|DSML|invoke") {
            let name = extract_attr(&after[invoke_start..], "name");
            let body_start = after[invoke_start..]
                .find('>')
                .map_or(invoke_start, |offset| invoke_start + offset + 1);
            let body_end = after[body_start..]
                .find("</|DSML|invoke>")
                .map_or(after.len(), |offset| body_start + offset);
            let body = &after[body_start..body_end];
            if let (Some(name), Ok(arguments)) =
                (name, serde_json::from_str::<serde_json::Value>(body.trim()))
            {
                calls.push(RecoveredCall { name, arguments });
            }
            rest = &after[body_end..];
            if rest.starts_with("</|DSML|invoke>") {
                rest = &rest["</|DSML|invoke>".len()..];
            }
            if rest.starts_with("</|DSML|tool_calls>") {
                rest = &rest["</|DSML|tool_calls>".len()..];
            }
        } else if let Some(close) = after.find('>') {
            // `<|DSML|tool_calls>` wrapper with a JSON body.
            let body_end = after.find("</|DSML|").unwrap_or(after.len());
            let body = &after[close + 1..body_end];
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(body.trim()) {
                collect_json_calls(&value, &mut calls);
            }
            rest = &after[body_end..];
        } else {
            break;
        }
    }
    cleaned.push_str(rest);
    (calls, cleaned.trim().to_string())
}

fn parse_call_json(body: &str) -> Option<RecoveredCall> {
    let value: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    parse_call_object(value.as_object()?)
}

fn parse_call_object(object: &serde_json::Map<String, serde_json::Value>) -> Option<RecoveredCall> {
    let name = object
        .get("name")
        .or_else(|| object.get("tool"))
        .or_else(|| object.get("recipient"))?
        .as_str()?
        .to_string();
    let arguments = object
        .get("arguments")
        .or_else(|| object.get("parameters"))
        .or_else(|| object.get("args"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let arguments = match arguments {
        serde_json::Value::String(raw) => {
            serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw))
        }
        other => other,
    };
    Some(RecoveredCall { name, arguments })
}

fn collect_json_calls(value: &serde_json::Value, out: &mut Vec<RecoveredCall>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_calls(item, out);
            }
        }
        serde_json::Value::Object(object) => {
            if let Some(call) = parse_call_object(object) {
                out.push(call);
            } else if let Some(nested) = object.get("tool_calls") {
                collect_json_calls(nested, out);
            }
        }
        _ => {}
    }
}

fn extract_attr(text: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &rest[quote.len_utf8()..];
    let end = body.find(quote)?;
    Some(body[..end].to_string())
}

/// The leak-recovery nudge budget.
#[derive(Debug, Clone, Default)]
pub struct LeakNudgeBudget {
    used: u32,
    cap: u32,
}

impl LeakNudgeBudget {
    /// A budget capped at [`MAX_LEAK_NUDGES`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            used: 0,
            cap: MAX_LEAK_NUDGES,
        }
    }

    /// A budget with an explicit cap.
    #[must_use]
    pub fn with_cap(cap: u32) -> Self {
        Self { used: 0, cap }
    }

    /// Consume one nudge; returns `false` once the cap is exhausted.
    pub fn try_nudge(&mut self) -> bool {
        if self.used >= self.cap {
            return false;
        }
        self.used += 1;
        true
    }

    /// How many nudges have been issued.
    #[must_use]
    pub fn used(&self) -> u32 {
        self.used
    }

    /// Whether the budget is exhausted.
    #[must_use]
    pub fn exhausted(&self) -> bool {
        self.used >= self.cap
    }
}

/// The nudge prompt sent when a leak is recovered.
#[must_use]
pub fn nudge_prompt(kind: LeakKind, calls: &[RecoveredCall]) -> String {
    let names = calls
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Your previous turn leaked {} tool-call markup as plain text ({}). \
         Re-issue {} as a structured tool call, not as text.",
        kind.name(),
        names,
        if calls.len() == 1 { "it" } else { "them" }
    )
}
