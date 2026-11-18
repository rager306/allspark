//! RLM child lifecycle notices: the durable custom rows a parent session receives when
//! a child run ends without an explicit reply, fails, or is cancelled; this module owns
//! the wire vocabulary, the content text, and the details block.

use super::agent_messaging::sanitize_message_header_value;
use pa_types::ai::UserContent;
use pa_types::session::CustomMessage;
use serde_json::json;

/// A child run that failed (TS `rlm_child_failure`).
pub const RLM_CHILD_FAILURE_CUSTOM_TYPE: &str = "rlm_child_failure";
/// A child run that ended without an explicit reply or was cancelled
/// (TS `rlm_child_terminal_notice`).
pub const RLM_CHILD_TERMINAL_NOTICE_CUSTOM_TYPE: &str = "rlm_child_terminal_notice";

/// How a child run ended without an explicit reply (TS
/// `RlmChildTerminalNoticeDetails`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RlmChildTerminalNotice {
    /// The parent deleted a still-running child.
    Cancelled {
        child_id: String,
        session_name: String,
        reason: Option<String>,
    },
    /// The child finished its task without sending an agent message back.
    ///
    /// `tool_use_count` is the child's observed tool-call count: zero means it
    /// settled without touching anything, which the notice states outright so
    /// a parent does not read an empty answer as a completed task.
    CompletedWithoutReply {
        child_id: String,
        session_name: String,
        last_assistant_text_preview: Option<String>,
        tool_use_count: Option<u64>,
    },
}

/// The terminal-notice row: `[child-exited: cancelled|no-reply child:<name>]`
/// with the reason or the child's last assistant text as the body.
#[must_use]
pub fn create_rlm_child_terminal_notice(
    notice: &RlmChildTerminalNotice,
    timestamp: u64,
) -> CustomMessage {
    let (content, details) = match notice {
        RlmChildTerminalNotice::Cancelled {
            child_id,
            session_name,
            reason,
        } => {
            let name = sanitize_message_header_value(session_name);
            let mut content = format!("[child-exited: cancelled child:{name}]");
            if let Some(reason) = reason.as_deref().filter(|reason| !reason.is_empty()) {
                content.push_str("\n\n");
                content.push_str(reason);
            }
            let details = json!({
                "kind": "cancelled",
                "childId": child_id,
                "sessionName": session_name,
                "reason": reason,
            });
            (content, details)
        }
        RlmChildTerminalNotice::CompletedWithoutReply {
            child_id,
            session_name,
            last_assistant_text_preview,
            tool_use_count,
        } => {
            let name = sanitize_message_header_value(session_name);
            let mut content = format!("[child-exited: no-reply child:{name}]");
            // An observed zero is a fact, not an inference: the child settled
            // without calling any tool, so it cannot have changed the tree.
            // Saying so stops a parent from reading the empty answer as done.
            if *tool_use_count == Some(0) {
                content.push_str(
                    "\n\nThe child called no tool, so it changed nothing; its answer is not a result.",
                );
            }
            if let Some(preview) = last_assistant_text_preview
                .as_deref()
                .filter(|preview| !preview.is_empty())
            {
                content.push_str("\n\nLast assistant text: ");
                content.push_str(preview);
            }
            let details = json!({
                "kind": "completed_without_reply",
                "childId": child_id,
                "sessionName": session_name,
                "lastAssistantTextPreview": last_assistant_text_preview,
                "toolUseCount": tool_use_count,
            });
            (content, details)
        }
    };
    CustomMessage {
        custom_type: RLM_CHILD_TERMINAL_NOTICE_CUSTOM_TYPE.to_string(),
        content: UserContent::Text(content),
        display: true,
        details: Some(details),
        timestamp,
        rest: serde_json::Map::default(),
    }
}

/// The failure row: `[child-failed child:<name>]` with the error text.
#[must_use]
pub fn create_rlm_child_failure_message(
    child_id: &str,
    session_name: &str,
    error: &str,
    timestamp: u64,
) -> CustomMessage {
    let name = sanitize_message_header_value(session_name);
    let content = format!("[child-failed child:{name}]\n\n{error}");
    CustomMessage {
        custom_type: RLM_CHILD_FAILURE_CUSTOM_TYPE.to_string(),
        content: UserContent::Text(content),
        display: true,
        details: Some(json!({
            "childId": child_id,
            "sessionName": session_name,
            "error": error,
        })),
        timestamp,
        rest: serde_json::Map::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reply_notice_matches_the_ts_shape() {
        let notice = RlmChildTerminalNotice::CompletedWithoutReply {
            child_id: "sub-8fb5284a".to_string(),
            session_name: "f20-worker".to_string(),
            last_assistant_text_preview: Some("done with the task".to_string()),
            tool_use_count: Some(4),
        };
        let message = create_rlm_child_terminal_notice(&notice, 1_000);
        assert_eq!(message.custom_type, "rlm_child_terminal_notice");
        assert!(message.display);
        assert_eq!(
            message.content,
            UserContent::Text(
                "[child-exited: no-reply child:f20-worker]\n\nLast assistant text: done with the task"
                    .to_string()
            )
        );
        let details = message.details.unwrap();
        assert_eq!(details["kind"], "completed_without_reply");
        assert_eq!(details["childId"], "sub-8fb5284a");
        assert_eq!(details["sessionName"], "f20-worker");
        assert_eq!(details["lastAssistantTextPreview"], "done with the task");
    }

    /// A child that settled without calling a tool changed nothing. The notice
    /// says so, because an observed zero is the one signal that separates "it
    /// finished the task" from "it produced text and touched no file".
    #[test]
    fn a_child_that_called_no_tool_says_it_changed_nothing() {
        let notice = RlmChildTerminalNotice::CompletedWithoutReply {
            child_id: "sub-2".to_string(),
            session_name: "lane".to_string(),
            last_assistant_text_preview: Some("I would start by reading...".to_string()),
            tool_use_count: Some(0),
        };
        let message = create_rlm_child_terminal_notice(&notice, 5_000);
        let UserContent::Text(text) = &message.content else {
            panic!("the notice is text");
        };
        assert!(
            text.contains("called no tool, so it changed nothing"),
            "the zero count must be stated: {text}"
        );
        assert!(text.contains("its answer is not a result"), "{text}");
        let details = message.details.unwrap();
        assert_eq!(details["toolUseCount"], serde_json::json!(0));
    }

    /// An unobserved count must not be reported as zero: the inline host does
    /// not count tool uses, and reading a missing count as "changed nothing"
    /// would be a fabricated fact.
    #[test]
    fn an_unobserved_tool_count_makes_no_claim() {
        let notice = RlmChildTerminalNotice::CompletedWithoutReply {
            child_id: "sub-3".to_string(),
            session_name: "lane".to_string(),
            last_assistant_text_preview: None,
            tool_use_count: None,
        };
        let message = create_rlm_child_terminal_notice(&notice, 6_000);
        let UserContent::Text(text) = &message.content else {
            panic!("the notice is text");
        };
        assert!(
            !text.contains("changed nothing"),
            "an unknown count is not evidence of inaction: {text}"
        );
        assert_eq!(
            message.details.unwrap()["toolUseCount"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn cancelled_notice_without_reason_has_no_body() {
        let notice = RlmChildTerminalNotice::Cancelled {
            child_id: "sub-1".to_string(),
            session_name: "worker [x]".to_string(),
            reason: None,
        };
        let message = create_rlm_child_terminal_notice(&notice, 2_000);
        assert_eq!(
            message.content,
            UserContent::Text("[child-exited: cancelled child:worker x]".to_string())
        );
        let details = message.details.unwrap();
        assert_eq!(details["kind"], "cancelled");
        assert_eq!(details["reason"], serde_json::Value::Null);
    }

    #[test]
    fn cancelled_notice_carries_the_reason_body() {
        let notice = RlmChildTerminalNotice::Cancelled {
            child_id: "sub-1".to_string(),
            session_name: "worker".to_string(),
            reason: Some("Deleted by parent orchestrator".to_string()),
        };
        let message = create_rlm_child_terminal_notice(&notice, 3_000);
        assert_eq!(
            message.content,
            UserContent::Text(
                "[child-exited: cancelled child:worker]\n\nDeleted by parent orchestrator"
                    .to_string()
            )
        );
    }

    #[test]
    fn failure_notice_matches_the_ts_shape() {
        let message = create_rlm_child_failure_message("sub-9", "lane", "boom", 4_000);
        assert_eq!(message.custom_type, "rlm_child_failure");
        assert_eq!(
            message.content,
            UserContent::Text("[child-failed child:lane]\n\nboom".to_string())
        );
        let details = message.details.unwrap();
        assert_eq!(details["childId"], "sub-9");
        assert_eq!(details["error"], "boom");
    }
}
