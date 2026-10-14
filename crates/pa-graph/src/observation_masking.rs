//! R7 — observation masking: keep recent observations, elide older ones behind
//! a dereferenceable recall id.
//!
//! An agent's message log grows without bound: every tool result is appended
//! and never edited. Sending the whole log to the model on every turn is
//! wasteful, and *summarizing* the old parts is worse than useless — it
//! fabricates content and destroys the natural stopping signals an agent uses
//! to notice it has finished. Masking takes the middle path: the log stays
//! complete and append-only, and only the *projection* sent to the model
//! replaces old observations with a compact placeholder that names the exact
//! log entry the content still lives in.
//!
//! [`ObservationMasker::mask`] keeps the last `keep_last` observations in full
//! and replaces every earlier one with `[recall:<id>]`. Non-observation
//! messages (system, user, assistant reasoning) pass through untouched —
//! masking those would destroy the trail the model needs to stay coherent.
//! [`ObservationMasker::recall`] dereferences an id back to the full content
//! from the untouched log, so nothing is ever lost.
//!
//! Oversized results are truncated once, at append time, with an explicit
//! marker so the model knows output was cut rather than silently ending.
//!
//! ```no_run
//! use pa_graph::{Message, ObservationMasker};
//!
//! let mut masker = ObservationMasker::new();
//! masker.append(Message::tool("call-1", "read_file", "fn main() {}"));
//! masker.append(Message::assistant("done"));
//! let view = masker.mask();
//! assert!(masker.recall("call-1").is_some());
//! # let _ = view;
//! ```

/// Who produced a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// System prompt / rules.
    System,
    /// The human turn.
    User,
    /// The model's reasoning or action.
    Assistant,
    /// A tool result (an *observation*).
    Tool,
}

/// One entry in the append-only message log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Who produced the message.
    pub role: Role,
    /// The message body.
    pub content: String,
    /// For tool observations: the tool's name.
    pub tool_name: Option<String>,
    /// For tool observations: the call id used as the recall handle.
    pub tool_call_id: Option<String>,
}

impl Message {
    /// A system message.
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self::plain(Role::System, content)
    }

    /// A user message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::plain(Role::User, content)
    }

    /// An assistant message (reasoning or action).
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::plain(Role::Assistant, content)
    }

    /// A tool observation, carrying the recall handle `call_id`.
    #[must_use]
    pub fn tool(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_name: Some(tool_name.into()),
            tool_call_id: Some(call_id.into()),
        }
    }

    fn plain(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_name: None,
            tool_call_id: None,
        }
    }

    /// Whether this message is an observation (a tool result).
    #[must_use]
    pub fn is_observation(&self) -> bool {
        self.role == Role::Tool
    }

    /// The recall handle for an observation, if it carries one.
    #[must_use]
    pub fn recall_id(&self) -> Option<&str> {
        self.tool_call_id.as_deref()
    }
}

/// The model-facing projection of the log, with old observations elided.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MaskedMessages {
    /// The projected messages, in log order.
    pub messages: Vec<Message>,
    /// How many observations were replaced by a placeholder.
    pub elided: usize,
}

impl MaskedMessages {
    /// Number of messages in the projection.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the projection is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Total bytes of the projected message bodies (the context cost).
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.messages.iter().map(|m| m.content.len()).sum()
    }
}

/// The full content behind one recall id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    /// The recall id it was fetched by.
    pub recall_id: String,
    /// The complete, untruncated-by-masking observation text.
    pub text: String,
}

/// Masking configuration and the append-only log it projects.
#[derive(Debug, Clone)]
pub struct ObservationMasker {
    keep_last: usize,
    max_result_bytes: usize,
    log: Vec<Message>,
}

impl Default for ObservationMasker {
    fn default() -> Self {
        Self::new()
    }
}

impl ObservationMasker {
    /// Defaults: keep the last 8 observations, cap a result at 100 kB.
    #[must_use]
    pub fn new() -> Self {
        Self {
            keep_last: 8,
            max_result_bytes: 100_000,
            log: Vec::new(),
        }
    }

    /// Keep the last `keep_last` observations in full; older ones are elided.
    #[must_use]
    pub fn with_keep_last(mut self, keep_last: usize) -> Self {
        self.keep_last = keep_last;
        self
    }

    /// Truncate any observation larger than `bytes` at append time.
    #[must_use]
    pub fn with_max_result_bytes(mut self, bytes: usize) -> Self {
        self.max_result_bytes = bytes;
        self
    }

    /// The configured observation retention window.
    #[must_use]
    pub fn keep_last(&self) -> usize {
        self.keep_last
    }

    /// The configured per-result byte cap.
    #[must_use]
    pub fn max_result_bytes(&self) -> usize {
        self.max_result_bytes
    }

    /// The complete append-only log.
    #[must_use]
    pub fn log(&self) -> &[Message] {
        &self.log
    }

    /// Append one message, truncating an oversized observation first.
    ///
    /// The log is append-only: entries are never edited or removed, which is
    /// what keeps every elided observation addressable by its recall id.
    pub fn append(&mut self, mut message: Message) {
        if message.is_observation() {
            message.content = truncate_result(message.content, self.max_result_bytes);
        }
        self.log.push(message);
    }

    /// Append several messages in order.
    pub fn extend(&mut self, messages: impl IntoIterator<Item = Message>) {
        for message in messages {
            self.append(message);
        }
    }

    /// Project the log for the model: elide all but the last `keep_last`
    /// observations, leaving every non-observation message verbatim.
    #[must_use]
    pub fn mask(&self) -> MaskedMessages {
        let total = self.log.iter().filter(|m| m.is_observation()).count();
        if total <= self.keep_last {
            return MaskedMessages {
                messages: self.log.clone(),
                elided: 0,
            };
        }
        let elide_before = total - self.keep_last;
        let mut seen = 0usize;
        let mut elided = 0usize;
        let messages = self
            .log
            .iter()
            .map(|message| {
                if !message.is_observation() {
                    return message.clone();
                }
                let index = seen;
                seen += 1;
                if index < elide_before {
                    elided += 1;
                    Message {
                        role: Role::Tool,
                        content: placeholder(message),
                        tool_name: message.tool_name.clone(),
                        tool_call_id: message.tool_call_id.clone(),
                    }
                } else {
                    message.clone()
                }
            })
            .collect();
        MaskedMessages { messages, elided }
    }

    /// Dereference a recall id against the full log.
    ///
    /// Returns `None` for an unknown id. Always reads the complete log, never a
    /// masked projection — recalling from a projection would return the
    /// placeholder rather than the content.
    #[must_use]
    pub fn recall(&self, id: &str) -> Option<Content> {
        self.log
            .iter()
            .find(|m| m.is_observation() && m.recall_id() == Some(id))
            .map(|m| Content {
                recall_id: id.to_string(),
                text: m.content.clone(),
            })
    }
}

/// The placeholder substituted for an elided observation, naming its recall id.
fn placeholder(message: &Message) -> String {
    match message.recall_id() {
        Some(id) => format!("[recall:{id}]"),
        None => "[recall:unavailable]".to_string(),
    }
}

/// Truncate `content` to at most `max_bytes`, never splitting a UTF-8
/// character, and mark the cut when anything was removed.
#[must_use]
pub fn truncate_result(content: String, max_bytes: usize) -> String {
    const MARKER: &str = "\n... [output truncated]";
    if content.len() <= max_bytes {
        return content;
    }
    if max_bytes < MARKER.len() {
        let end = floor_char_boundary(&content, max_bytes);
        return content[..end].to_string();
    }
    let end = floor_char_boundary(&content, max_bytes - MARKER.len());
    let mut out = String::with_capacity(end + MARKER.len());
    out.push_str(&content[..end]);
    out.push_str(MARKER);
    out
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    let mut n = max.min(s.len());
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, body: &str) -> Message {
        Message::tool(id, "read_file", body)
    }

    #[test]
    fn keeps_everything_under_the_limit() {
        let mut masker = ObservationMasker::new();
        masker.extend([
            Message::assistant("plan"),
            tool("t1", "a"),
            tool("t2", "b"),
        ]);
        let view = masker.mask();
        assert_eq!(view.elided, 0);
        assert_eq!(view.messages, masker.log());
    }

    #[test]
    fn elides_all_but_the_last_n_observations() {
        let mut masker = ObservationMasker::new().with_keep_last(2);
        masker.extend([
            tool("t1", "oldest"),
            tool("t2", "older"),
            tool("t3", "recent"),
            tool("t4", "newest"),
        ]);
        let view = masker.mask();
        assert_eq!(view.elided, 2);
        assert_eq!(view.messages[0].content, "[recall:t1]");
        assert_eq!(view.messages[1].content, "[recall:t2]");
        assert_eq!(view.messages[2].content, "recent");
        assert_eq!(view.messages[3].content, "newest");
    }

    #[test]
    fn never_masks_reasoning_or_actions() {
        let mut masker = ObservationMasker::new().with_keep_last(0);
        masker.extend([
            Message::system("rules"),
            Message::user("do the thing"),
            Message::assistant("here is my plan"),
            tool("t1", "contents"),
        ]);
        let view = masker.mask();
        assert_eq!(view.messages[0], Message::system("rules"));
        assert_eq!(view.messages[1], Message::user("do the thing"));
        assert_eq!(view.messages[2], Message::assistant("here is my plan"));
        assert!(view.messages[3].content.contains("recall:t1"));
    }

    #[test]
    fn recall_returns_full_content_from_the_log() {
        let mut masker = ObservationMasker::new().with_keep_last(0);
        masker.append(tool("call-42", "many matches"));
        // The projection is a placeholder, but the log still holds the body.
        assert_eq!(masker.mask().messages[0].content, "[recall:call-42]");
        let content = masker.recall("call-42").expect("recall");
        assert_eq!(content.text, "many matches");
        assert_eq!(content.recall_id, "call-42");
        assert!(masker.recall("missing").is_none());
    }

    #[test]
    fn masking_shrinks_the_projection() {
        let mut masker = ObservationMasker::new().with_keep_last(1);
        for i in 0..20 {
            masker.append(tool(&format!("t{i}"), &"x".repeat(500)));
        }
        let full: usize = masker.log().iter().map(|m| m.content.len()).sum();
        assert!(masker.mask().byte_len() < full, "projection must be smaller");
    }

    #[test]
    fn oversized_results_are_truncated_at_append() {
        let mut masker = ObservationMasker::new().with_max_result_bytes(100);
        masker.append(tool("big", &"x".repeat(1000)));
        let stored = &masker.log()[0].content;
        assert!(stored.len() <= 100);
        assert!(stored.ends_with("[output truncated]"));
    }

    #[test]
    fn truncation_leaves_short_content_alone() {
        assert_eq!(truncate_result("short".into(), 100), "short");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_char() {
        let content = "\u{1f642}".repeat(100);
        for cap in [33usize, 50, 77, 99] {
            let out = truncate_result(content.clone(), cap);
            assert!(out.len() <= cap, "cap {cap} exceeded: {} bytes", out.len());
        }
    }

    #[test]
    fn masking_is_a_pure_projection() {
        let mut masker = ObservationMasker::new().with_keep_last(1);
        masker.extend([tool("t1", "a"), tool("t2", "b")]);
        let before = masker.log().to_vec();
        let _ = masker.mask();
        let _ = masker.mask();
        assert_eq!(masker.log(), before.as_slice(), "mask must not mutate the log");
    }
}
