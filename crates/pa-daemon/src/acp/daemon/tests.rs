//! The transport's tests, grouped by the contract each one pins: the EOF /
//! close ordering, the MCP-server clear retry, the TS admission refusal
//! order, and the cancel arming rules.
//!
//! Every test is the one that was in the file before the split.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use pa_types::daemon::{
    DaemonCommand, DaemonCommandEnvelope, DaemonResponse, DAEMON_PROTOCOL_VERSION,
};
use serde_json::{json, Map};
use tokio::sync::{mpsc, Mutex};

use super::ordering::{
    admit_prompt, arm_cancel_locked, frame_order_prefix, wait_for_session_close, CancelOrder,
    FrameOrder,
};
use super::session_new::handle_session_new;
use super::wire_events::WireMappingState;
use super::{
    ActiveTurn, DaemonAcpOptions, DaemonAcpState, DaemonBinding, DaemonLink, HostedSession,
};
use crate::acp::jsonrpc::Incoming;
use crate::acp::producer::UpdateProducer;
use crate::acp::wire_config::HostedConfig;

fn hosted_session() -> HostedSession {
    let (sink, _drain) = mpsc::unbounded_channel();
    HostedSession {
        acp_session_id: "acp-1".to_string(),
        daemon_active_session_id: "daemon-1".to_string(),
        producer: UpdateProducer::new("acp-1", sink),
        config: Arc::new(HostedConfig {
            queue: tokio::sync::Mutex::new(()),
            published: tokio::sync::Mutex::new(Vec::new()),
            models: tokio::sync::Mutex::new(Vec::new()),
        }),
        cancelling: false,
        stop_failure: None,
        input_pause_key: None,
        input_pause_id: None,
        cancel_task: None,
        turn: None,
        assistant_stop_reason: None,
        mapping: WireMappingState::default(),
        observed_children: std::collections::HashSet::new(),
    }
}

fn state(session: Option<HostedSession>) -> DaemonAcpState {
    DaemonAcpState {
        session,
        ..DaemonAcpState::default()
    }
}

#[tokio::test]
async fn eof_waits_for_a_reserved_close_before_taking_the_session() {
    let state = Arc::new(Mutex::new(state(Some(hosted_session()))));
    let order = frame_order_prefix(
        &Incoming::Request {
            id: json!(1),
            method: "session/close".to_string(),
            params: json!({ "sessionId": "acp-1" }),
        },
        &state,
    )
    .await;
    let FrameOrder::Close { done, .. } = order else {
        panic!("the close should reserve its stop");
    };
    let eof = tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            wait_for_session_close(&state).await;
            state.lock().await.session.take().is_some()
        }
    });
    tokio::task::yield_now().await;
    assert!(
        !eof.is_finished(),
        "EOF must not take the session before close finishes"
    );
    assert!(state.lock().await.session.is_some());
    done.send(true).unwrap();
    state.lock().await.session_close_done = None;
    assert!(eof.await.unwrap());

    // A close can settle before EOF checks the state. No stale watch
    // should be awaited, and the hosted session is still available.
    let (done, mut done_rx) = tokio::sync::watch::channel(false);
    let settled = Arc::new(Mutex::new(self::state(Some(hosted_session()))));
    done.send(true).unwrap();
    assert!(*done_rx.borrow_and_update());
    settled.lock().await.session_close_done = Some(done_rx);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        wait_for_session_close(&settled),
    )
    .await
    .expect("an already completed close must not strand EOF");
}

#[tokio::test]
async fn lost_replace_and_clear_ack_retries_clear_before_an_empty_session() {
    let (writer, mut commands) = mpsc::unbounded_channel::<String>();
    let (_frames_tx, frames) = mpsc::unbounded_channel();
    let pending = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let link = Arc::new(DaemonLink {
        writer,
        pending: Arc::clone(&pending),
        closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        frames: Mutex::new(frames),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        next_request_id: std::sync::atomic::AtomicU64::new(0),
    });
    let worker = tokio::spawn(async move {
        let mut replacements = Vec::new();
        let mut installed = Vec::<String>::new();
        while let Some(line) = commands.recv().await {
            let envelope: DaemonCommandEnvelope = serde_json::from_str(&line).unwrap();
            let id = envelope.id;
            let (command, data, lose_ack) = match envelope.command {
                DaemonCommand::GetConnectionState { .. } => ("get_connection_state", None, false),
                DaemonCommand::GetAvailableModels { .. } => {
                    ("get_available_models", Some(json!({ "models": [] })), false)
                }
                DaemonCommand::GetRlmChildren { .. } => {
                    ("get_rlm_children", Some(json!({ "children": [] })), false)
                }
                DaemonCommand::ReplaceAcpMcpServers { servers, .. } => {
                    installed = servers
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|server| server.get("name").unwrap().as_str().unwrap().to_string())
                        .collect();
                    replacements.push(installed.clone());
                    ("replace_acp_mcp_servers", None, replacements.len() <= 2)
                }
                other => panic!("unexpected command: {other:?}"),
            };
            let reply = pending.lock().unwrap().remove(&id).unwrap();
            if !lose_ack {
                reply
                    .send(DaemonResponse {
                        id: Some(id),
                        command: command.to_string(),
                        success: true,
                        data,
                        error: None,
                        error_info: None,
                    })
                    .unwrap();
            }
            // Dropping the reply simulates an applied command whose
            // acknowledgement never reaches the ACP connection.
        }
        (replacements, installed)
    });
    let state = Arc::new(Mutex::new(state(None)));
    let binding = DaemonBinding {
        active_session_id: "daemon-1".to_string(),
        client_owned: false,
        mcp_owner_id: "owner-1".to_string(),
    };
    let options = DaemonAcpOptions {
        socket_path: PathBuf::new(),
        actual_cwd: PathBuf::from("/tmp"),
        product_version: "test".to_string(),
        create: DaemonCommand::GetConnectionState {
            id: None,
            active_session_id: "daemon-1".to_string(),
            rest: Map::default(),
        },
    };
    let (tx, mut responses) = mpsc::unbounded_channel();
    handle_session_new(
        json!(1),
        json!({ "mcpServers": [{ "name": "example", "command": "echo", "args": [], "env": [] }] }),
        &link,
        &state,
        &options,
        &binding,
        tx.clone(),
    )
    .await;
    assert!(responses.recv().await.unwrap().get("error").is_some());
    assert_eq!(state.lock().await.mcp_server_names, vec!["example"]);

    handle_session_new(
        json!(2),
        json!({ "mcpServers": [] }),
        &link,
        &state,
        &options,
        &binding,
        tx,
    )
    .await;
    assert!(responses.recv().await.unwrap().get("result").is_some());
    assert_eq!(state.lock().await.mcp_server_names.len(), 0);
    drop(link);
    let (replacements, installed) = worker.await.unwrap();
    assert_eq!(replacements, vec![vec!["example"], vec![], vec![]]);
    assert_eq!(installed.len(), 0);
}

#[tokio::test]
async fn admit_prompt_matches_the_ts_refusal_order() {
    let none = Arc::new(Mutex::new(state(None)));
    assert_eq!(
        admit_prompt(&json!({ "sessionId": "nope" }), &none)
            .await
            .unwrap_err(),
        "Unknown ACP session: nope"
    );

    let closing = Arc::new(Mutex::new({
        let mut state = state(Some(hosted_session()));
        let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
        state.session_close_done = Some(done_rx);
        state
    }));
    assert_eq!(
        admit_prompt(&json!({ "sessionId": "acp-1" }), &closing)
            .await
            .unwrap_err(),
        "ACP session is closing: acp-1"
    );

    let mut cancelling = state(Some(hosted_session()));
    cancelling.session.as_mut().unwrap().cancelling = true;
    let cancelling = Arc::new(Mutex::new(cancelling));
    assert_eq!(
        admit_prompt(&json!({ "sessionId": "acp-1" }), &cancelling)
            .await
            .unwrap_err(),
        "ACP session is cancelling: acp-1"
    );

    let mut failed = state(Some(hosted_session()));
    failed.session.as_mut().unwrap().stop_failure = Some("boom".to_string());
    failed.session.as_mut().unwrap().turn = Some(ActiveTurn {
        admission_id: "prompt-admission:1".to_string(),
        cancelled: false,
    });
    let failed = Arc::new(Mutex::new(failed));
    assert_eq!(
        admit_prompt(&json!({ "sessionId": "acp-1" }), &failed)
            .await
            .unwrap_err(),
        "ACP session stop failed: boom"
    );

    let clean = Arc::new(Mutex::new(state(Some(hosted_session()))));
    let admitted = admit_prompt(&json!({ "sessionId": "acp-1" }), &clean)
        .await
        .unwrap();
    assert!(admitted.starts_with("prompt-admission:"));
    assert_eq!(
        admit_prompt(&json!({ "sessionId": "acp-1" }), &clean)
            .await
            .unwrap_err(),
        "A prompt turn is already running for this ACP session"
    );
}

#[test]
fn arm_cancel_arms_a_running_turn_a_stop_retry_or_nothing() {
    let mut running = state(Some(hosted_session()));
    running.session.as_mut().unwrap().turn = Some(ActiveTurn {
        admission_id: "prompt-admission:1".to_string(),
        cancelled: false,
    });
    let CancelOrder::Armed(stop) = arm_cancel_locked("acp-1", &mut running) else {
        panic!("a running turn arms its stop");
    };
    assert_eq!(stop.admission_id.as_deref(), Some("prompt-admission:1"));
    let turn = running.session.as_ref().unwrap().turn.as_ref().unwrap();
    assert!(turn.cancelled);
    assert!(running.session.as_ref().unwrap().cancelling);
    assert!(running.session.as_ref().unwrap().cancel_task.is_some());

    let mut idle = state(Some(hosted_session()));
    assert!(matches!(
        arm_cancel_locked("acp-1", &mut idle),
        CancelOrder::None
    ));
    assert!(idle.session.as_ref().unwrap().turn.is_none());

    let mut failed = state(Some(hosted_session()));
    failed.session.as_mut().unwrap().stop_failure = Some("boom".to_string());
    let CancelOrder::Armed(stop) = arm_cancel_locked("acp-1", &mut failed) else {
        panic!("a stop failure arms the retry");
    };
    assert!(stop.admission_id.is_none());
    assert!(failed.session.as_ref().unwrap().turn.is_none());
    assert!(failed.session.as_ref().unwrap().cancelling);
    assert!(failed.session.as_ref().unwrap().cancel_task.is_some());
}

#[test]
fn a_cancel_while_a_stop_runs_never_arms_a_second_stop() {
    let mut stopping = state(Some(hosted_session()));
    stopping.session.as_mut().unwrap().cancelling = true;
    stopping.session.as_mut().unwrap().turn = Some(ActiveTurn {
        admission_id: "prompt-admission:1".to_string(),
        cancelled: true,
    });
    assert!(matches!(
        arm_cancel_locked("acp-1", &mut stopping),
        CancelOrder::None
    ));
}
