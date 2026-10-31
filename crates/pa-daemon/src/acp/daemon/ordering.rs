//! Prompt admission and cancel marking, in the client's frame order.
//!
//! A frame's synchronous prefix runs before the next frame is read (TS runs
//! a request's synchronous prefix before the next handler): `session/prompt`
//! reserves the turn slot, `session/close` reserves its stop, and
//! `session/cancel` arms the stop. The prefix's decision — [`FrameOrder`] —
//! tells the main loop what to spawn.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use super::{ActiveTurn, DaemonAcpState};
use crate::acp::jsonrpc::Incoming;

pub(super) enum FrameOrder {
    Spawn,
    AdmitPrompt {
        id: Value,
        params: Value,
        admission_id: String,
    },
    Refused {
        id: Value,
        message: String,
    },
    Close {
        id: Value,
        params: Value,
        done: tokio::sync::watch::Sender<bool>,
    },
    Cancel(CancelOrder),
}

pub(super) enum CancelOrder {
    None,
    Armed(CancelStop),
    WaitForClose { session_id: String },
}

pub(super) struct CancelStop {
    pub(super) acp_session_id: String,
    pub(super) daemon_session_id: String,
    pub(super) admission_id: Option<String>,
    pub(super) done: tokio::sync::watch::Sender<bool>,
}

/// Prompt admission and cancel marking, in the client's frame order.
pub(super) async fn frame_order_prefix(
    incoming: &Incoming,
    state: &Arc<Mutex<DaemonAcpState>>,
) -> FrameOrder {
    match incoming {
        Incoming::Request { id, method, params } if method == "session/prompt" => {
            match admit_prompt(params, state).await {
                Ok(admission_id) => FrameOrder::AdmitPrompt {
                    id: id.clone(),
                    params: params.clone(),
                    admission_id,
                },
                Err(message) => FrameOrder::Refused {
                    id: id.clone(),
                    message,
                },
            }
        }
        Incoming::Request { id, method, params } if method == "session/close" => {
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let (done_tx, done_rx) = tokio::sync::watch::channel(false);
            let mut guard = state.lock().await;
            if guard
                .session
                .as_ref()
                .is_none_or(|hosted| hosted.acp_session_id != session_id)
            {
                return FrameOrder::Refused {
                    id: id.clone(),
                    message: format!("Unknown ACP session: {session_id}"),
                };
            }
            if guard.session_close_done.is_some() {
                return FrameOrder::Refused {
                    id: id.clone(),
                    message: format!("ACP session is already closing: {session_id}"),
                };
            }
            guard.session_close_done = Some(done_rx);
            FrameOrder::Close {
                id: id.clone(),
                params: params.clone(),
                done: done_tx,
            }
        }
        Incoming::Notification { method, params } if method == "session/cancel" => {
            FrameOrder::Cancel(cancel_order(params, state).await)
        }
        _ => FrameOrder::Spawn,
    }
}

/// Reserve the turn slot for one prompt. `Err` is the refusal message,
/// checked in TS order: unknown session, closing, cancelling, stop
/// failure, turn running.
pub(super) async fn admit_prompt(
    params: &Value,
    state: &Arc<Mutex<DaemonAcpState>>,
) -> Result<String, String> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut guard = state.lock().await;
    let session_closing = guard.session_close_done.is_some();
    let Some(hosted) = guard
        .session
        .as_mut()
        .filter(|hosted| hosted.acp_session_id == session_id)
    else {
        return Err(format!("Unknown ACP session: {session_id}"));
    };
    if session_closing {
        return Err(format!("ACP session is closing: {session_id}"));
    }
    if hosted.cancelling {
        return Err(format!("ACP session is cancelling: {session_id}"));
    }
    if let Some(stop_failure) = hosted.stop_failure.as_ref() {
        return Err(format!("ACP session stop failed: {stop_failure}"));
    }
    if hosted.turn.is_some() {
        return Err("A prompt turn is already running for this ACP session".to_string());
    }
    let admission_id = format!("prompt-admission:{}", uuid::Uuid::new_v4());
    // The stop reason is per-turn: a turn that runs no model call (a
    // slash command) must not inherit the previous turn's stop reason.
    hosted.assistant_stop_reason = None;
    hosted.turn = Some(ActiveTurn {
        admission_id: admission_id.clone(),
        cancelled: false,
    });
    Ok(admission_id)
}

pub(super) async fn cancel_order(
    params: &Value,
    state: &Arc<Mutex<DaemonAcpState>>,
) -> CancelOrder {
    let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
        return CancelOrder::None;
    };
    let mut guard = state.lock().await;
    if guard.session_close_done.is_some() {
        return CancelOrder::WaitForClose {
            session_id: session_id.to_string(),
        };
    }
    arm_cancel_locked(session_id, &mut guard)
}

pub(super) fn arm_cancel_locked(session_id: &str, guard: &mut DaemonAcpState) -> CancelOrder {
    let Some(hosted) = guard
        .session
        .as_mut()
        .filter(|hosted| hosted.acp_session_id == session_id)
    else {
        return CancelOrder::None;
    };
    if hosted.cancelling {
        return CancelOrder::None;
    }
    let admission_id = match hosted.turn.as_mut() {
        Some(turn) => {
            turn.cancelled = true;
            Some(turn.admission_id.clone())
        }
        None if hosted.stop_failure.is_none() => return CancelOrder::None,
        None => None,
    };
    let (done_tx, done_rx) = tokio::sync::watch::channel(false);
    hosted.cancel_task = Some(done_rx);
    hosted.cancelling = true;
    CancelOrder::Armed(CancelStop {
        acp_session_id: hosted.acp_session_id.clone(),
        daemon_session_id: hosted.daemon_active_session_id.clone(),
        admission_id,
        done: done_tx,
    })
}

pub(super) async fn wait_for_session_close(state: &Arc<Mutex<DaemonAcpState>>) {
    let close_done = { state.lock().await.session_close_done.clone() };
    if let Some(mut done) = close_done {
        let _ = done.wait_for(|finished| *finished).await;
    }
}
