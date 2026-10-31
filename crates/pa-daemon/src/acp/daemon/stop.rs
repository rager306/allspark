//! The stop sequence: pause the worker's input, abort its work, cancel the
//! owned admission, wait the idle, cancel the RLM children, release the
//! pause (TS `stopSessionWork`, #1612).

use std::sync::Arc;

use pa_types::daemon::DaemonCommand;
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use super::link::DaemonLink;
use super::ordering::CancelStop;
use super::prompt::fetch_rlm_children;
use super::DaemonAcpState;

pub(super) async fn run_cancel_stop(
    stop: CancelStop,
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
) {
    let outcome = match acquire_stop_input_pause(link, state, &stop.acp_session_id).await {
        Ok(lease) => {
            match stop_session_work(link, &stop.daemon_session_id, stop.admission_id.as_deref())
                .await
            {
                Ok(()) => {
                    release_session_input_pause(link, &stop.daemon_session_id, &lease.pause_id)
                        .await
                        .map(|()| lease)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    {
        let mut guard = state.lock().await;
        let released_closed = matches!(
            &outcome,
            Ok(lease) if guard.closed_input_pause_id.as_deref() == Some(&lease.pause_id)
        );
        if let Some(hosted) = guard
            .session
            .as_mut()
            .filter(|hosted| hosted.acp_session_id == stop.acp_session_id)
        {
            hosted.cancelling = false;
            hosted.cancel_task = None;
            match &outcome {
                Ok(lease) => {
                    if hosted.input_pause_id.as_deref() == Some(&lease.pause_id) {
                        hosted.input_pause_id = None;
                    }
                    if hosted.input_pause_key.as_deref() == Some(&lease.lease_key) {
                        hosted.input_pause_key = None;
                    }
                    hosted.stop_failure = None;
                }
                Err(error) => hosted.stop_failure = Some(error.to_string()),
            }
        }
        if released_closed {
            guard.closed_input_pause_id = None;
            guard.closed_input_pause_key = None;
        }
    }
    let _ = stop.done.send(true);
}

/// TS `stopSessionWork`: abort the worker's running and queued work, stop
/// the owned admission (a prompt sent before the stop but committed after
/// the abort), wait the idle, then cancel the RLM children.
pub(super) async fn stop_session_work(
    link: &Arc<DaemonLink>,
    daemon_session_id: &str,
    admission_id: Option<&str>,
) -> anyhow::Result<()> {
    link.request_ok(DaemonCommand::AbortAndClearQueue {
        id: None,
        active_session_id: daemon_session_id.to_string(),
        rest: Map::default(),
    })
    .await?;
    if let Some(admission_id) = admission_id {
        cancel_owned_admission(link, daemon_session_id, admission_id).await;
    }
    link.request_ok(DaemonCommand::WaitForIdle {
        id: None,
        active_session_id: daemon_session_id.to_string(),
        wait_for_rlm_quiescence: None,
        rest: Map::default(),
    })
    .await?;
    cancel_outstanding_rlm_children(link, daemon_session_id).await
}

/// TS `cancelOutstandingRlmChildren`: cancel every roster row, no status
/// filter — a row already settling keeps its own settle, and the cancels
/// run one after another (TS `Promise.allSettled` is the declared
/// divergence). A failed roster fetch or the first failed cancel fails the
/// stop; every row still gets its cancel.
async fn cancel_outstanding_rlm_children(
    link: &Arc<DaemonLink>,
    daemon_session_id: &str,
) -> anyhow::Result<()> {
    let children = fetch_rlm_children(link, daemon_session_id).await?;
    let mut failure = None;
    for child in children {
        let child_id = child
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Err(error) = link
            .request_ok(DaemonCommand::CancelRlmChild {
                id: None,
                active_session_id: daemon_session_id.to_string(),
                child_id,
                rest: Map::default(),
            })
            .await
        {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

/// TS `cancel_prompt_admission` with `cancelOwned`: a committed prompt's
/// running turn aborts.
pub(super) async fn cancel_owned_admission(
    link: &Arc<DaemonLink>,
    daemon_session_id: &str,
    admission_id: &str,
) {
    let _ = link
        .request(DaemonCommand::CancelPromptAdmission {
            id: None,
            active_session_id: daemon_session_id.to_string(),
            admission_id: admission_id.to_string(),
            cancel_owned: Some(true),
            rest: Map::default(),
        })
        .await;
}

pub(super) struct InputPauseLease {
    pub(super) pause_id: String,
    pub(super) lease_key: String,
}

pub(super) async fn acquire_stop_input_pause(
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
    acp_session_id: &str,
) -> anyhow::Result<InputPauseLease> {
    let (daemon_session_id, lease_key) = {
        let mut guard = state.lock().await;
        let Some(hosted) = guard
            .session
            .as_mut()
            .filter(|hosted| hosted.acp_session_id == acp_session_id)
        else {
            anyhow::bail!("Unknown ACP session: {acp_session_id}");
        };
        let lease_key = hosted
            .input_pause_key
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        hosted.input_pause_key = Some(lease_key.clone());
        (hosted.daemon_active_session_id.clone(), lease_key)
    };
    let response = link
        .request(DaemonCommand::AcquireSessionInputPause {
            id: None,
            active_session_id: daemon_session_id.clone(),
            lease_key: lease_key.clone(),
            rest: Map::default(),
        })
        .await?;
    if !response.success {
        anyhow::bail!(response
            .error
            .unwrap_or_else(|| "unknown error".to_string()));
    }
    let pause_id = response
        .data
        .unwrap_or(Value::Null)
        .get("pauseId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("the daemon returned no session input pause id"))?;
    {
        let mut guard = state.lock().await;
        if let Some(hosted) = guard
            .session
            .as_mut()
            .filter(|hosted| hosted.acp_session_id == acp_session_id)
        {
            hosted.input_pause_id = Some(pause_id.clone());
        }
    }
    Ok(InputPauseLease {
        pause_id,
        lease_key,
    })
}

pub(super) async fn release_session_input_pause(
    link: &Arc<DaemonLink>,
    daemon_session_id: &str,
    pause_id: &str,
) -> anyhow::Result<()> {
    link.request_ok(DaemonCommand::ReleaseSessionInputPause {
        id: None,
        active_session_id: daemon_session_id.to_string(),
        pause_id: pause_id.to_string(),
        rest: Map::default(),
    })
    .await
}
