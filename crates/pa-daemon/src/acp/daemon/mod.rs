//! The daemon-attached ACP transport: the same ACP JSON-RPC surface served
//! over a daemon session (TS
//! `runAcpModeWithConnection(DaemonAgentConnection)`).
//!
//! Startup creates and attaches the daemon session the CLI session flags
//! select. `session/new` binds it, admits its MCP servers through the
//! `replace_acp_mcp_servers` wire command, and every prompt runs
//! `prompt_and_wait` while the streamed session events fan out as ACP
//! updates. The turn settlement (response boundary, completion envelope
//! with the live outstanding-subagent count, terminal frame after the RLM
//! family settles, stop reason) mirrors the TS captures, and the stop
//! sequence cancels the outstanding children (TS #1612).
//! The daemon worker's `goal_update` session events surface through the
//! wire mapping (`wire_events.rs`), and the autonomous accounting rides the
//! `wait_for_headless_completion` response into the completion envelope
//! and the stop reason (TS `waitForHeadlessCompletion` + `acpStopReason`).
//!
//! The model and effort pickers (TS #2455) ride the worker's own wire
//! commands: `get_connection_state` for the live model/level,
//! `get_available_models` for discovery, `set_model` /
//! `set_thinking_level` for the applied selection.
//!
//! The transport was one 2 255-line file; it is now one module per
//! responsibility:
//!
//! | module | holds |
//! |---|---|
//! | [`link`] | the supervisor socket link, its frame reader, and the request exchange |
//! | [`ordering`] | the frame prefix: prompt admission, cancel marking, close reservation |
//! | [`stop`] | the stop sequence: input pause, abort, idle wait, child cancels |
//! | [`dispatch`] | one incoming request routed to its handler, plus `initialize` |
//! | [`session_new`] | `session/new`: the hosted session and its MCP servers |
//! | [`prompt`] | `session/prompt`: the turn, its settlement, and the stop reason |
//! | [`close`] | `session/close` and the EOF teardown |
//! | [`tests`] | the tests, grouped by the contract each one pins |
//!
//! The public surface is re-exported here, so
//! `pa_daemon::acp::daemon::run_daemon_attached_acp_mode`,
//! `DaemonAcpOptions`, and the crate-internal `DaemonLink` /
//! `DaemonAcpState` keep their paths for `pa-cli` and `wire_config`.

use std::path::PathBuf;
use std::sync::Arc;

use pa_types::daemon::{DaemonCommand, DaemonSessionLifecycle};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

mod close;
mod dispatch;
mod link;
mod ordering;
mod prompt;
mod session_new;
mod stop;

#[cfg(test)]
mod tests;

use close::{handle_session_close, teardown};
use dispatch::handle_incoming;
pub(crate) use link::DaemonLink;
use link::LinkFrame;
use ordering::{cancel_order, frame_order_prefix, wait_for_session_close, CancelOrder, FrameOrder};
use prompt::handle_session_prompt;
use stop::run_cancel_stop;

use super::jsonrpc::{self, Incoming};
use super::meta::{self, PrimeAgentEventPhase, PrimeAgentSessionMeta};
use super::producer::UpdateProducer;
use super::types;
use super::wire_config::{refresh_wire_config, HostedConfig};
use super::wire_events::{self, WireMappingState};

#[derive(Clone)]
pub struct DaemonAcpOptions {
    pub socket_path: PathBuf,
    pub actual_cwd: PathBuf,
    pub product_version: String,
    /// The startup `create` built from the CLI session flags.
    pub create: DaemonCommand,
}

/// The daemon session this connection created at startup; every
/// `session/new` binds it.
#[derive(Clone)]
pub(super) struct DaemonBinding {
    pub(super) active_session_id: String,
    /// The create's `client_owned` lifecycle (`--no-session`): the
    /// session ends with the connection.
    pub(super) client_owned: bool,
    pub(super) mcp_owner_id: String,
}

/// The hosted daemon session: the ACP identity, the daemon routing id,
/// the update producer, and the stop state.
pub(crate) struct HostedSession {
    pub(crate) acp_session_id: String,
    pub(crate) daemon_active_session_id: String,
    pub(crate) producer: Arc<UpdateProducer>,
    pub(crate) config: Arc<HostedConfig>,
    pub(super) cancelling: bool,
    pub(super) stop_failure: Option<String>,
    pub(super) input_pause_key: Option<String>,
    pub(super) input_pause_id: Option<String>,
    pub(super) cancel_task: Option<tokio::sync::watch::Receiver<bool>>,
    /// The running prompt turn; one at a time. A cancelled turn keeps
    /// the slot until its cancel stop finishes.
    pub(super) turn: Option<ActiveTurn>,
    /// The newest assistant stop reason observed on the event stream,
    /// cleared when a prompt turn is admitted so a turn never reads a
    /// previous turn's stop reason (read after the settlement for the
    /// stop-reason response).
    pub(super) assistant_stop_reason: Option<pa_types::ai::StopReason>,
    /// The event mapping state lives and dies with the session, like TS.
    pub(super) mapping: WireMappingState,
    pub(super) observed_children: std::collections::HashSet<String>,
}

pub(super) struct ActiveTurn {
    /// TS `admissionId`; the stop sequence cancels the admission by it.
    pub(super) admission_id: String,
    /// TS the per-turn `AbortController`.
    pub(super) cancelled: bool,
}

/// The ACP transport state: one hosted session at most.
#[derive(Default)]
pub(crate) struct DaemonAcpState {
    pub(crate) session: Option<HostedSession>,
    pub(crate) session_new_in_flight: bool,
    pub(crate) session_close_done: Option<tokio::sync::watch::Receiver<bool>>,
    pub(crate) closed_input_pause_id: Option<String>,
    pub(crate) closed_input_pause_key: Option<String>,
    pub(crate) mcp_server_names: Vec<String>,
}

/// Serve the daemon-attached ACP mode until stdin closes. The caller
/// guarantees the socket answers (the composition spawns a supervisor
/// when none is listening).
///
/// # Errors
///
/// Returns an error, before any ACP frame is written, when the daemon
/// socket connect, the startup create, or its attach fails; a daemon that
/// drops mid-session fails the hosted session's requests instead, exactly
/// like the TS daemon connection.
///
/// # Panics
///
/// The frame-consumer task panics when the link's pending-response map
/// lock is poisoned (a holder panicked while holding it).
pub async fn run_daemon_attached_acp_mode(options: DaemonAcpOptions) -> anyhow::Result<i32> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(frame) = rx.recv().await {
            let Ok(mut line) = serde_json::to_string(&frame) else {
                continue;
            };
            line.push('\n');
            if stdout.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let link = Arc::new(DaemonLink::connect(&options.socket_path).await?);
    let state = Arc::new(Mutex::new(DaemonAcpState::default()));

    {
        let link = Arc::clone(&link);
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut frames = link.frames.lock().await;
            while let Some(frame) = frames.recv().await {
                match frame {
                    LinkFrame::Event(frame) => {
                        let event = frame.get("event").cloned().unwrap_or(Value::Null);
                        // The refresh triggers: runs that can restore a
                        // failover model or clamp a level. Captured under
                        // the guard, spawned once released.
                        let mut refresh: Option<(Arc<HostedConfig>, Arc<UpdateProducer>, String)> =
                            None;
                        {
                            let mut guard = state.lock().await;
                            let Some(current) = guard.session.as_mut() else {
                                continue;
                            };
                            if let Some(stop) = wire_events::assistant_stop(&event) {
                                current.assistant_stop_reason = stop.stop_reason;
                            }
                            if event.get("type").and_then(Value::as_str) == Some("rlm_child_update")
                            {
                                if let Some(id) = event
                                    .get("child")
                                    .and_then(|child| child.get("id"))
                                    .and_then(Value::as_str)
                                {
                                    current.observed_children.insert(id.to_string());
                                }
                            }
                            let turn_id = current.producer.active_prompt_turn().await;
                            for update in wire_events::wire_updates(&event, &mut current.mapping) {
                                let _ = current
                                    .producer
                                    .publish(&update, turn_id, PrimeAgentEventPhase::Event, None)
                                    .await;
                            }
                            if matches!(
                                event.get("type").and_then(Value::as_str),
                                Some("agent_end" | "auto_retry_start" | "auto_retry_end")
                            ) {
                                refresh = Some((
                                    Arc::clone(&current.config),
                                    Arc::clone(&current.producer),
                                    current.daemon_active_session_id.clone(),
                                ));
                            }
                        }
                        if let Some((config, producer, daemon_session_id)) = refresh {
                            let link = Arc::clone(&link);
                            tokio::spawn(async move {
                                let _guard = config.queue.lock().await;
                                // The background trigger drops refresh
                                // failures (TS `.catch(() => undefined)`).
                                let _ = refresh_wire_config(
                                    &link,
                                    &daemon_session_id,
                                    &config,
                                    &producer,
                                )
                                .await;
                            });
                        }
                    }
                    LinkFrame::Response(response) => {
                        let id = response.id.clone().unwrap_or_default();
                        if let Some(tx) = link.pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(response);
                        }
                    }
                    // Heartbeats are connection-scoped, not session events,
                    // so the consumer maps the broadcast directly instead of
                    // `wire_updates`: TS publishes the change at origin turn 0
                    // even while a prompt runs.
                    LinkFrame::HeartbeatsChanged => {
                        let guard = state.lock().await;
                        if let Some(current) = guard.session.as_ref() {
                            let _ = current
                                .producer
                                .publish(
                                    &types::AcpSessionUpdate::SessionInfoUpdate {
                                        meta: meta::prime_agent_meta(&PrimeAgentSessionMeta {
                                            heartbeats_changed: Some(true),
                                            ..Default::default()
                                        }),
                                    },
                                    0,
                                    PrimeAgentEventPhase::Event,
                                    None,
                                )
                                .await;
                        }
                    }
                }
            }
            // The reader hit the socket close and every frame it read
            // has been delivered: fail the still-pending requests
            // (dropping a sender answers with the mid-request error)
            // and fail every later request fast. The flag goes up
            // before the clear; `request` covers the race.
            link.closed.store(true, std::sync::atomic::Ordering::SeqCst);
            link.pending.lock().unwrap().clear();
        });
    }
    let binding = bind_daemon_session(&link, &state, options.create.clone()).await?;

    // Only the prompt handlers stay owned: EOF aborts them (TS aborts the
    // controller and exits), so the process never waits on a prompt still
    // settling its subagents. The other handlers spawn detached: a close
    // in flight at EOF finishes — its `tx` clone holds `writer.await` — so
    // its session still stops and releases its servers.
    let mut handlers = tokio::task::JoinSet::new();
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut input_line = String::new();
    loop {
        input_line.clear();
        match stdin.read_line(&mut input_line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if input_line.trim().is_empty() {
            continue;
        }
        let incoming = match jsonrpc::parse_line(&input_line) {
            Ok(incoming) => incoming,
            Err(error_response) => {
                let _ = tx.send(error_response);
                continue;
            }
        };
        // Handlers run concurrently (like the TS acp agent's request
        // handling): a prompt turn may span minutes, and `session/cancel`
        // must reach the daemon while it is in flight. The state machine
        // (one hosted session, one in-flight close) keeps the concurrency
        // bounded. Prompt admission and cancel marking run here, in frame
        // order (TS runs a request's synchronous prefix before the next
        // frame's handler).
        match frame_order_prefix(&incoming, &state).await {
            FrameOrder::Spawn => {
                // `session/new` settles before the next frame is read, so
                // EOF's teardown always sees the session it installs.
                if matches!(&incoming, Incoming::Request { method, .. } if method == "session/new")
                {
                    let options_tx = tx.clone();
                    handle_incoming(incoming, &link, &state, &options, &binding, options_tx).await;
                } else {
                    let link = Arc::clone(&link);
                    let state = Arc::clone(&state);
                    let options = options.clone();
                    let binding = binding.clone();
                    let options_tx = tx.clone();
                    tokio::spawn(async move {
                        handle_incoming(incoming, &link, &state, &options, &binding, options_tx)
                            .await;
                    });
                }
            }
            FrameOrder::AdmitPrompt {
                id,
                params,
                admission_id,
            } => {
                let link = Arc::clone(&link);
                let state = Arc::clone(&state);
                let options_tx = tx.clone();
                handlers.spawn(async move {
                    handle_session_prompt(id, params, admission_id, &link, &state, options_tx)
                        .await;
                });
            }
            // A refused prompt or close answers from the prefix; the
            // running turn is untouched.
            FrameOrder::Refused { id, message } => {
                let _ = tx.send(super::internal_error(&id, &message));
            }
            FrameOrder::Close { id, params, done } => {
                let link = Arc::clone(&link);
                let state = Arc::clone(&state);
                let options_tx = tx.clone();
                let owner_id = binding.mcp_owner_id.clone();
                tokio::spawn(async move {
                    handle_session_close(id, params, &link, &state, options_tx, done, owner_id)
                        .await;
                });
            }
            FrameOrder::Cancel(order) => {
                let link = Arc::clone(&link);
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let mut order = order;
                    loop {
                        order = match order {
                            CancelOrder::None => return,
                            CancelOrder::Armed(stop) => {
                                run_cancel_stop(stop, &link, &state).await;
                                return;
                            }
                            CancelOrder::WaitForClose { session_id } => {
                                wait_for_session_close(&state).await;
                                cancel_order(&json!({ "sessionId": session_id }), &state).await
                            }
                        };
                    }
                });
            }
        }
        // Reap finished prompt handlers.
        while handlers.try_join_next().is_some() {}
    }

    handlers.shutdown().await;
    // A close frame is reserved synchronously but its stop runs detached.
    // Let it finish before EOF teardown takes the hosted session away.
    wait_for_session_close(&state).await;
    teardown(&link, &state, &binding).await;
    drop(tx);
    let _ = writer.await;
    Ok(0)
}

/// Create and attach the startup daemon session (TS main.ts, before the
/// ACP connection serves). A failed attach releases the created session
/// the way EOF does.
async fn bind_daemon_session(
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
    create: DaemonCommand,
) -> anyhow::Result<DaemonBinding> {
    let client_owned = matches!(
        create,
        DaemonCommand::Create {
            lifecycle: Some(DaemonSessionLifecycle::ClientOwned),
            ..
        }
    );
    let created = link.request(create).await?;
    if !created.success {
        anyhow::bail!(created.error.unwrap_or_else(|| "unknown error".to_string()));
    }
    let binding = DaemonBinding {
        active_session_id: created
            .data
            .as_ref()
            .and_then(|summary| summary.get("activeSessionId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        client_owned,
        mcp_owner_id: uuid::Uuid::new_v4().to_string(),
    };
    let attached = link
        .request(DaemonCommand::Attach {
            id: None,
            active_session_id: binding.active_session_id.clone(),
            client_id: None,
            capabilities: None,
            resume_cursor: None,
            telemetry_disabled: None,
            recovery_config: None,
            env: None,
            launch_env: None,
            rest: Map::default(),
        })
        .await
        .and_then(|response| {
            if response.success {
                Ok(())
            } else {
                Err(anyhow::anyhow!(response
                    .error
                    .unwrap_or_else(|| "unknown error".to_string())))
            }
        });
    if let Err(error) = attached {
        teardown(link, state, &binding).await;
        return Err(error);
    }
    Ok(binding)
}
