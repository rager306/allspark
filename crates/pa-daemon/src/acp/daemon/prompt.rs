//! `session/prompt`: one admitted turn — the wire prompt, the streamed
//! settlement (response boundary, completion envelope, terminal quiescence),
//! and the stop reason (TS `promptAndWait` + `waitForHeadlessCompletion`).

use std::sync::Arc;

use pa_types::daemon::DaemonCommand;
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use super::link::DaemonLink;
use super::DaemonAcpState;
use crate::acp::meta::{self, PrimeAgentEventPhase, PrimeAgentOutcome, PrimeAgentSessionMeta};
use crate::acp::producer::{self, UpdateProducer};
use crate::acp::types;
use crate::acp::{jsonrpc, turn_failure_error};

pub(super) async fn handle_session_prompt(
    id: Value,
    params: Value,
    admission_id: String,
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
    tx: producer::FrameSink,
) {
    let reply = prompt_turn(id, params, &admission_id, link, state).await;
    release_turn_slot(state, &admission_id).await;
    let _ = tx.send(reply);
}

/// Release this prompt's turn slot, after its stop sequence if it was
/// cancelled. A slot holding another prompt's turn (close + new while
/// this prompt ran) is left alone.
async fn release_turn_slot(state: &Arc<Mutex<DaemonAcpState>>, admission_id: &str) {
    let mut cancel_task = {
        let mut guard = state.lock().await;
        let Some(hosted) = guard.session.as_mut() else {
            return;
        };
        let Some(turn) = hosted.turn.as_mut() else {
            return;
        };
        if turn.admission_id != admission_id {
            return;
        }
        hosted.cancel_task.clone()
    };
    if let Some(cancel_task) = cancel_task.as_mut() {
        let _ = cancel_task.changed().await;
    }
    if let Some(hosted) = state.lock().await.session.as_mut() {
        hosted
            .turn
            .take_if(|turn| turn.admission_id == admission_id);
    }
}

/// Whether this prompt lost its turn: cancelled, or its session was closed
/// (close, or close + new, takes the slot - like TS close aborting the turn).
fn turn_cancelled(state: &DaemonAcpState, admission_id: &str) -> bool {
    !state
        .session
        .as_ref()
        .and_then(|hosted| hosted.turn.as_ref())
        .is_some_and(|turn| turn.admission_id == admission_id && !turn.cancelled)
}

fn cancelled_response(id: &Value) -> Value {
    jsonrpc::response(
        id,
        &serde_json::to_value(types::AcpStopReasonResponse {
            stop_reason: types::AcpStopReason::Cancelled,
        })
        .expect("serializes"),
    )
}

/// Render a prompt-block failure as the ACP invalid-params error. The TS
/// SDK validates the request schema; the Rust port validates the blocks it
/// actually reads.
fn prompt_block_error(id: &Value, error: &types::PromptBlockError) -> Value {
    jsonrpc::error_response(
        id,
        jsonrpc::INVALID_PARAMS,
        "Invalid params",
        Some(&json!({ "reason": error.to_string() })),
    )
}

/// Run one admitted prompt turn and return its reply frame.
async fn prompt_turn(
    id: Value,
    params: Value,
    admission_id: &str,
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
) -> Value {
    let params = types::PromptParams::parse(&params);
    let admitted = match types::AdmittedPrompt::parse(&params.prompt) {
        Ok(admitted) => admitted,
        Err(error) => return prompt_block_error(&id, &error),
    };
    let (producer, hosted_daemon_session_id) = {
        let guard = state.lock().await;
        match guard.session.as_ref() {
            Some(hosted) if hosted.acp_session_id == params.session_id => (
                Arc::clone(&hosted.producer),
                hosted.daemon_active_session_id.clone(),
            ),
            // Admission checked the session id: a miss is a close since.
            _ => return cancelled_response(&id),
        }
    };
    let turn_id = producer.begin_prompt().await;
    // TS `abort.signal.aborted` before `promptAndWait`.
    let cancelled = turn_cancelled(&*state.lock().await, admission_id);
    if cancelled {
        producer.finish_prompt(turn_id).await;
        return cancelled_response(&id);
    }
    let prompt = DaemonCommand::PromptAndWait {
        id: None,
        active_session_id: hosted_daemon_session_id.clone(),
        message: admitted.text,
        input: pa_types::daemon::PromptInput {
            content: None,
            // The image blocks ride the wire form `parse_prompt_images`
            // reads (`{type, data, mimeType}`); TS forwards `images`
            // only when the prompt carries any.
            images: (!admitted.images.is_empty()).then(|| {
                json!(admitted
                    .images
                    .into_iter()
                    .map(|image| json!({ "type": "image", "data": image.data, "mimeType": image.mime_type }))
                    .collect::<Vec<_>>())
            }),
            // TS sends `followUp` + `queueIfBusy: true` on every ACP
            // prompt (acp-mode.ts): a prompt carrying a streaming
            // behavior is the worker's resume site for the post-abort
            // queued-input suspension, so a prompt after a Stop runs.
            streaming_behavior: Some(pa_types::daemon::StreamingBehavior::FollowUp),
            queue_if_busy: Some(true),
            expand_prompt_templates: None,
            source: None,
            agent_message_id: None,
            custom_message: None,
            queue_key: None,
            prefix_messages: None,
            admission_id: Some(admission_id.to_string()),
            rlm_notice_nonce: None,
        },
        rest: Map::default(),
    };
    let response = match link.request_until_close(prompt).await {
        Ok(response) => response,
        Err(error) => {
            publish_error_boundary(&producer, turn_id).await;
            producer.finish_prompt(turn_id).await;
            // The lost supervisor link (route timeout, worker loss) is
            // classified like every other failed-prompt arm: typed data,
            // never retryable at the boundary — the turn's outcome is
            // unknown, so a host retry may re-run side-effecting tools.
            return turn_failure_error(&id, &error.to_string(), &error.to_string());
        }
    };
    // TS `abort.signal.aborted` after `promptAndWait`: every turn frame
    // is published by now - the worker flushes its session events before
    // the response leaves its socket, and the supervisor's per-client
    // writer writes a queued event ahead of a queued response - so the
    // settle reads the stream directly, with no marker wait.
    let cancelled = {
        let guard = state.lock().await;
        turn_cancelled(&guard, admission_id)
    };
    if cancelled {
        producer.finish_prompt(turn_id).await;
        return cancelled_response(&id);
    }
    // TS settle catch: one error boundary, no terminal-quiescence update.
    if !response.success {
        let failure = response
            .error
            .unwrap_or_else(|| "unknown error".to_string());
        // The failure text rides the internal_error reply, where a test
        // asserting only the stop reason loses it: echo it to stderr so
        // every harness's child-stderr dump shows WHY the turn failed.
        eprintln!("pa-daemon: acp turn failed: {failure}");
        publish_error_boundary(&producer, turn_id).await;
        producer.finish_prompt(turn_id).await;
        // The worker's failure text classifies into the #200 host
        // contract (the raw text still never rides past `details`), so
        // the reply data is machine-readable at the transport boundary.
        return turn_failure_error(
            &id,
            &failure,
            &format!("{} turn failed: {failure}", pa_types::brand::INVOCATION),
        );
    }
    // The autonomous accounting for the completion envelope: the daemon's
    // headless-completion status (TS `waitForHeadlessCompletion`), fetched
    // after the turn settled (the response is the run's end). A failed
    // fetch degrades to no autonomous meta (the envelope still settles
    // without a run).
    let autonomous_status = fetch_autonomous_status(link, &hosted_daemon_session_id, false)
        .await
        .ok();
    // TS `abort.signal.aborted` after `waitForHeadlessCompletion`: queued
    // continuations (goal, post-compaction autonomous) run inside that wait.
    if turn_cancelled(&*state.lock().await, admission_id) {
        producer.finish_prompt(turn_id).await;
        return cancelled_response(&id);
    }
    // The completion observation's roster read (TS `getRlmChildSnapshots`
    // throws): a failed read errors the prompt before the boundary, never
    // publishes a wrong count — with the same typed data as every other
    // failed-prompt arm.
    let children = match fetch_rlm_children(link, &hosted_daemon_session_id).await {
        Ok(children) => children,
        Err(error) => {
            publish_error_boundary(&producer, turn_id).await;
            producer.finish_prompt(turn_id).await;
            return turn_failure_error(&id, &error.to_string(), &error.to_string());
        }
    };
    if turn_cancelled(&*state.lock().await, admission_id) {
        producer.finish_prompt(turn_id).await;
        return cancelled_response(&id);
    }
    let autonomous_meta = autonomous_status
        .as_ref()
        .filter(|status| status.enabled)
        .map(meta::autonomous_meta);
    // The boundary, completion, and terminal quiescence frames match the
    // TS captures.
    let boundary = types::AcpSessionUpdate::SessionInfoUpdate {
        meta: meta::prime_agent_meta(&PrimeAgentSessionMeta {
            terminal_quiescence_expected: Some(true),
            ..Default::default()
        }),
    };
    let published = producer
        .publish(
            &boundary,
            turn_id,
            PrimeAgentEventPhase::ResponseBoundary,
            Some(PrimeAgentOutcome::Result),
        )
        .await;
    // The completion envelope: the turn's own observation, with the
    // live outstanding-subagent count the settle loop then waits down to
    // zero.
    let quiescence = types::AcpSessionUpdate::SessionInfoUpdate {
        meta: meta::prime_agent_meta(&PrimeAgentSessionMeta {
            autonomous: autonomous_meta,
            quiescence: Some(quiescence_meta(autonomous_status.as_ref(), &children)),
            ..Default::default()
        }),
    };
    let completion_published = producer
        .publish(&quiescence, turn_id, PrimeAgentEventPhase::Event, None)
        .await;
    // The settlement loop (TS `finalizePendingTerminal`): the barrier is
    // the event, no timer, and the roster re-read is the response-cut
    // telemetry (a child can publish a terminal status before its result
    // reaches the parent). A failed barrier or read errors the prompt; a
    // degraded status would spin the loop.
    let settled_status = loop {
        let status = match fetch_autonomous_status(link, &hosted_daemon_session_id, true).await {
            Ok(status) => status,
            Err(error) => {
                producer.finish_prompt(turn_id).await;
                return turn_failure_error(
                    &id,
                    &error.to_string(),
                    &format!("ACP lifecycle reconciliation failed: {error}"),
                );
            }
        };
        if turn_cancelled(&*state.lock().await, admission_id) {
            producer.finish_prompt(turn_id).await;
            return cancelled_response(&id);
        }
        let children = match fetch_rlm_children(link, &hosted_daemon_session_id).await {
            Ok(children) => children,
            Err(error) => {
                producer.finish_prompt(turn_id).await;
                return turn_failure_error(
                    &id,
                    &error.to_string(),
                    &format!("ACP lifecycle reconciliation failed: {error}"),
                );
            }
        };
        if turn_cancelled(&*state.lock().await, admission_id) {
            producer.finish_prompt(turn_id).await;
            return cancelled_response(&id);
        }
        let terminal_quiescence = quiescence_meta(Some(&status), &children);
        if terminal_quiescence.outstanding_subagents != 0 {
            continue;
        }
        // TS `sealTerminal`: the terminal frame is the last update stamped
        // with this turn; later events resume turn 0.
        producer.finish_prompt(turn_id).await;
        let terminal = types::AcpSessionUpdate::SessionInfoUpdate {
            meta: meta::prime_agent_meta(&PrimeAgentSessionMeta {
                autonomous: status.enabled.then(|| meta::autonomous_meta(&status)),
                quiescence: Some(terminal_quiescence),
                ..Default::default()
            }),
        };
        let terminal_published = producer
            .publish(
                &terminal,
                turn_id,
                PrimeAgentEventPhase::TerminalQuiescence,
                Some(PrimeAgentOutcome::Result),
            )
            .await;
        if !published || !completion_published || !terminal_published {
            // The typed shape even for a publish failure: a host can tell
            // "classified, not retryable" from an untyped error here too.
            return turn_failure_error(
                &id,
                "Failed to publish ACP completion",
                "Failed to publish ACP completion",
            );
        }
        break status;
    };
    // The stop reason follows the TS mapping (acp-stop-reason.ts) plus the
    // turn's final assistant stop reason (#3363): the abort flag read after
    // the settlement, the settlement's status (`pending.status`), not the
    // first observation's, and the newest assistant stop reason — the link's
    // one frame consumer applies every message_end (the autonomous
    // continuations' included, since they run inside the waits above) before
    // the response that ended each wait resolved, so this read after the
    // settlement sees the run's final one.
    let (cancelled, assistant_stop_reason) = {
        let guard = state.lock().await;
        (
            turn_cancelled(&guard, admission_id),
            guard
                .session
                .as_ref()
                .and_then(|hosted| hosted.assistant_stop_reason),
        )
    };
    let stop_reason =
        meta::acp_stop_reason_for_status(cancelled, Some(&settled_status), assistant_stop_reason);
    jsonrpc::response(
        &id,
        &serde_json::to_value(types::AcpStopReasonResponse { stop_reason }).expect("serializes"),
    )
}

/// The error boundary a failed turn publishes (TS
/// `terminalQuiescenceExpected: false`).
async fn publish_error_boundary(producer: &Arc<UpdateProducer>, turn_id: u64) {
    let boundary = types::AcpSessionUpdate::SessionInfoUpdate {
        meta: meta::prime_agent_meta(&PrimeAgentSessionMeta {
            terminal_quiescence_expected: Some(false),
            ..Default::default()
        }),
    };
    let _ = producer
        .publish(
            &boundary,
            turn_id,
            PrimeAgentEventPhase::ResponseBoundary,
            Some(PrimeAgentOutcome::Error),
        )
        .await;
}

/// Fetch the session's autonomous-run status (`wait_for_headless_completion`
/// on the daemon wire; TS `waitForHeadlessCompletion`), once the daemon's
/// headless run settled. With `wait_for_rlm_quiescence`, the answer comes
/// from the quiescence barrier instead: it holds past the parent's idle
/// until every tracked child run settled, so it is the RLM-quiescence
/// event the settlement loop waits on. A failure is the caller's policy
/// (TS throws for both callers): the completion observation degrades, the
/// settlement loop errors the prompt.
///
/// # Errors
///
/// Returns an error when the daemon connection fails or closes mid-request,
/// when the response reports a failure, or when the status payload does not
/// deserialize.
async fn fetch_autonomous_status(
    link: &Arc<DaemonLink>,
    active_session_id: &str,
    wait_for_rlm_quiescence: bool,
) -> anyhow::Result<pa_core::autonomous::AgentAutonomousStatus> {
    let response = link
        .request_until_close(DaemonCommand::WaitForHeadlessCompletion {
            id: None,
            active_session_id: active_session_id.to_string(),
            wait_for_rlm_quiescence: wait_for_rlm_quiescence.then_some(true),
            rest: Map::default(),
        })
        .await?;
    if !response.success {
        anyhow::bail!(response
            .error
            .unwrap_or_else(|| "unknown error".to_string()));
    }
    Ok(serde_json::from_value(
        response.data.unwrap_or(Value::Null),
    )?)
}

/// The live child roster (`get_rlm_children` on the daemon wire; TS
/// `getRlmChildSnapshots` throws).
///
/// # Errors
///
/// Returns an error when the daemon connection fails or closes mid-request,
/// when the response reports a failure, or when the answer carries no
/// children array.
pub(super) async fn fetch_rlm_children(
    link: &Arc<DaemonLink>,
    active_session_id: &str,
) -> anyhow::Result<Vec<Value>> {
    let response = link
        .request(DaemonCommand::GetRlmChildren {
            id: None,
            active_session_id: active_session_id.to_string(),
            rest: Map::default(),
        })
        .await?;
    if !response.success {
        anyhow::bail!(response
            .error
            .unwrap_or_else(|| "unknown error".to_string()));
    }
    response
        .data
        .unwrap_or(Value::Null)
        .get("children")
        .cloned()
        .and_then(|children| children.as_array().cloned())
        .ok_or_else(|| anyhow::anyhow!("get_rlm_children answered no children array"))
}

/// TS `outstandingSubagentCount`: the roster's live statuses, verbatim.
fn outstanding_subagents(children: &[Value]) -> u64 {
    children
        .iter()
        .filter(|child| {
            matches!(
                child.get("status").and_then(Value::as_str),
                Some("queued" | "running")
            )
        })
        .count() as u64
}

/// TS `quiescenceMeta`: the outstanding-subagent count plus the run's
/// remaining continuation slots, observed together at one completion point.
fn quiescence_meta(
    status: Option<&pa_core::autonomous::AgentAutonomousStatus>,
    children: &[Value],
) -> meta::PrimeAgentQuiescenceMeta {
    meta::PrimeAgentQuiescenceMeta {
        outstanding_subagents: outstanding_subagents(children),
        remaining_autonomous_continuations: status.filter(|status| status.enabled).map_or(
            0,
            |status| {
                status
                    .limits
                    .max_continuations
                    .saturating_sub(status.continuations_used)
            },
        ),
    }
}
