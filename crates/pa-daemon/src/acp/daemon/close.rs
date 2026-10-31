//! `session/close` and the connection teardown at EOF (TS dispose): stop the
//! session's work, fence the producer, release the MCP servers, and — for a
//! client-owned session — end the daemon session.

use std::sync::Arc;

use pa_types::daemon::DaemonCommand;
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use super::link::DaemonLink;
use super::session_new::clear_connection_servers;
use super::stop::{
    acquire_stop_input_pause, cancel_owned_admission, release_session_input_pause,
    stop_session_work,
};
use super::{DaemonAcpState, DaemonBinding};
use crate::acp::producer;
use crate::acp::{internal_error, jsonrpc};

pub(super) async fn handle_session_close(
    id: Value,
    params: Value,
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
    tx: producer::FrameSink,
    close_done: tokio::sync::watch::Sender<bool>,
    owner_id: String,
) {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let cancel_task = {
        let guard = state.lock().await;
        guard
            .session
            .as_ref()
            .filter(|hosted| hosted.acp_session_id == session_id)
            .and_then(|hosted| hosted.cancel_task.clone())
    };
    if let Some(mut cancel_task) = cancel_task {
        let _ = cancel_task.changed().await;
    }
    let (acp_session_id, daemon_session_id, config, producer, admission_id) = {
        let mut guard = state.lock().await;
        let Some(hosted) = guard
            .session
            .as_mut()
            .filter(|hosted| hosted.acp_session_id == session_id)
        else {
            drop(guard);
            state.lock().await.session_close_done = None;
            let _ = close_done.send(true);
            return;
        };
        hosted.cancelling = true;
        if let Some(turn) = hosted.turn.as_mut() {
            turn.cancelled = true;
        }
        let admission_id = hosted.turn.as_ref().map(|turn| turn.admission_id.clone());
        (
            hosted.acp_session_id.clone(),
            hosted.daemon_active_session_id.clone(),
            Arc::clone(&hosted.config),
            Arc::clone(&hosted.producer),
            admission_id,
        )
    };
    let close_result = match acquire_stop_input_pause(link, state, &acp_session_id).await {
        Ok(lease) => {
            match stop_session_work(link, &daemon_session_id, admission_id.as_deref()).await {
                Ok(()) => {
                    // The serialized config work settles before the
                    // producer fences (TS `await configTask`).
                    let _ = config.queue.lock().await;
                    producer.close().await;
                    let _ =
                        clear_connection_servers(link, &daemon_session_id, &owner_id, state).await;
                    let mut guard = state.lock().await;
                    guard.closed_input_pause_id = Some(lease.pause_id.clone());
                    guard.closed_input_pause_key = Some(lease.lease_key.clone());
                    if let Some(hosted) = guard
                        .session
                        .as_mut()
                        .filter(|hosted| hosted.acp_session_id == session_id)
                    {
                        if hosted.input_pause_id.as_deref() == Some(&lease.pause_id) {
                            hosted.input_pause_id = None;
                        }
                        if hosted.input_pause_key.as_deref() == Some(&lease.lease_key) {
                            hosted.input_pause_key = None;
                        }
                        hosted.stop_failure = None;
                    }
                    guard
                        .session
                        .take_if(|hosted| hosted.acp_session_id == session_id);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    {
        let mut guard = state.lock().await;
        if let Some(hosted) = guard
            .session
            .as_mut()
            .filter(|hosted| hosted.acp_session_id == acp_session_id)
        {
            hosted.cancelling = false;
            if let Err(error) = &close_result {
                hosted.stop_failure = Some(error.to_string());
            }
        }
        guard.session_close_done = None;
    }
    let _ = close_done.send(true);
    match close_result {
        Ok(()) => {
            let _ = tx.send(jsonrpc::response(&id, &json!({})));
        }
        Err(error) => {
            let _ = tx.send(internal_error(&id, &error.to_string()));
        }
    }
}

/// Release the connection's hold after stdin closes (TS dispose): the
/// running prompt is cancelled (TS EOF abort → `cancel_prompt_admission`
/// with `cancelOwned`), the MCP servers go and the producer fences. A
/// client-owned session ends with the connection; a resident one stays
/// and is detached when the link drops.
pub(super) async fn teardown(
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
    binding: &DaemonBinding,
) {
    let hosted = state.lock().await.session.take();
    if let Some(hosted) = hosted.as_ref() {
        if let Some(turn) = hosted.turn.as_ref() {
            cancel_owned_admission(link, &binding.active_session_id, &turn.admission_id).await;
        }
        if let Some(pause_id) = hosted.input_pause_id.as_ref() {
            let _ = release_session_input_pause(link, &binding.active_session_id, pause_id).await;
        }
    }
    let closed_pause = {
        let mut guard = state.lock().await;
        guard.closed_input_pause_key = None;
        guard.closed_input_pause_id.take()
    };
    if let Some(pause_id) = closed_pause {
        let _ = release_session_input_pause(link, &binding.active_session_id, &pause_id).await;
    }
    let _ = clear_connection_servers(
        link,
        &binding.active_session_id,
        &binding.mcp_owner_id,
        state,
    )
    .await;
    // The owned worker stops before the config queue drains: a stalled
    // config operation holding the queue on a wire request fails fast.
    if binding.client_owned {
        let _ = link
            .request(DaemonCommand::CompleteOwnedSession {
                id: None,
                active_session_id: binding.active_session_id.clone(),
                rest: Map::default(),
            })
            .await;
    }
    if let Some(hosted) = hosted {
        let _ = hosted.config.queue.lock().await;
        hosted.producer.close().await;
    }
}
