//! The summarization prompts: the two templates, the user instructions, and the
//! outstanding-work section the retained session depends on.

use super::super::*;
use super::fixtures::*;

#[test]
fn summarization_prompt_shapes() {
    let initial = build_summarization_prompt(None, None);
    assert!(initial.contains("Create a structured context checkpoint summary"));
    assert!(initial.contains("the Python kernel keeps running"));
    let update = build_summarization_prompt(None, Some("old summary"));
    assert!(update.contains("NEW conversation messages to incorporate"));
    let custom = build_summarization_prompt(Some("focus on tests"), None);
    assert!(custom.contains("<user-instructions>\nThe user provided these instructions"));
    assert!(custom.contains("focus on tests"));
}

#[test]
fn summarization_request_carries_outstanding_work() {
    let messages = vec![user_message("explore the widget", 0)];
    let request = crate::session_engine::compaction_exec::build_summarization_request(
        &messages, None, None, None, 0,
    );
    let text = match &request[0] {
        AgentMessage::User(user) => user.content.text(),
        _ => panic!("expected a user message"),
    };
    assert!(text.contains("## Outstanding Work"));
    assert!(text.contains("rlm.list_subagents()"));
    assert!(text.contains("the [bash-done] notification"));
    assert!(text.contains("rlm_heartbeat.list()"));

    let update = crate::session_engine::compaction_exec::build_summarization_request(
        &messages,
        None,
        Some("previous summary"),
        None,
        0,
    );
    let update_text = match &update[0] {
        AgentMessage::User(user) => user.content.text(),
        _ => panic!("expected a user message"),
    };
    assert!(update_text.contains("## Outstanding Work"));
    assert!(update_text.contains("PRESERVE the Outstanding Work section"));
}
