//! `session/new`: admit the hosted session over the startup daemon session,
//! admit its MCP servers through the wire command, publish the picker state
//! and the inherited child roster, and answer.

use std::path::Path;
use std::sync::Arc;

use pa_types::daemon::DaemonCommand;
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use super::link::DaemonLink;
use super::prompt::fetch_rlm_children;
use super::stop::release_session_input_pause;
use super::{DaemonAcpOptions, DaemonAcpState, DaemonBinding, HostedSession};
use crate::acp::meta::{self, PrimeAgentEventPhase, PrimeAgentSessionMeta};
use crate::acp::producer::{self, UpdateProducer};
use crate::acp::types;
use crate::acp::wire_config::{
    fetch_available_models, fetch_connection_state, picker_options_from_state, HostedConfig,
};
use crate::acp::wire_events::{self, WireMappingState};
use crate::acp::{internal_error, jsonrpc, mcp, same_cwd};

/// Admit one session over the startup daemon session and admit its MCP
/// servers through the wire command.
pub(super) async fn handle_session_new(
    id: Value,
    params: Value,
    link: &Arc<DaemonLink>,
    state: &Arc<Mutex<DaemonAcpState>>,
    options: &DaemonAcpOptions,
    binding: &DaemonBinding,
    tx: producer::FrameSink,
) {
    {
        let mut guard = state.lock().await;
        if guard.session.is_some()
            || guard.session_new_in_flight
            || guard.session_close_done.is_some()
        {
            let _ = tx.send(internal_error(
                &id,
                &format!("{} ACP mode hosts one session per connection; start another {} process for a second session", pa_types::brand::PRODUCT_LOWER, pa_types::brand::INVOCATION),
            ));
            return;
        }
        guard.session_new_in_flight = true;
    }
    let params = types::NewSessionParams::parse(&params);
    if !state.lock().await.mcp_server_names.is_empty() {
        if let Err(error) = clear_connection_servers(
            link,
            &binding.active_session_id,
            &binding.mcp_owner_id,
            state,
        )
        .await
        {
            state.lock().await.session_new_in_flight = false;
            let _ = tx.send(internal_error(&id, &error.to_string()));
            return;
        }
    }
    // MCP admission runs after the pending-clear retry: a rejected list
    // fails the request with the same error payloads.
    let resolved = match mcp::resolve_acp_mcp_servers(&params.mcp_servers, &options.actual_cwd) {
        Ok(resolved) => resolved,
        Err(reason) => {
            state.lock().await.session_new_in_flight = false;
            let _ = tx.send(jsonrpc::error_response(
                &id,
                jsonrpc::INVALID_PARAMS,
                "Invalid params",
                Some(&json!({ "reason": reason })),
            ));
            return;
        }
    };
    if let Err(details) = mcp::acp_mcp_tool_names(&resolved) {
        state.lock().await.session_new_in_flight = false;
        let _ = tx.send(internal_error(&id, &details));
        return;
    }

    let acp_session_id = uuid::Uuid::new_v4().to_string();
    let producer = UpdateProducer::new(acp_session_id.clone(), tx.clone());
    // Neither picker fetch may fail the admission: discovery failures
    // catch to an empty list, a state fetch failure degrades to no
    // options.
    let (state_value, models) = (
        fetch_connection_state(link, &binding.active_session_id).await,
        fetch_available_models(link, &binding.active_session_id)
            .await
            .unwrap_or_default(),
    );
    let published = picker_options_from_state(state_value.as_ref(), &models);
    let config = Arc::new(HostedConfig {
        queue: tokio::sync::Mutex::new(()),
        published: tokio::sync::Mutex::new(published),
        models: tokio::sync::Mutex::new(models),
    });
    let mut hosted = HostedSession {
        acp_session_id: acp_session_id.clone(),
        daemon_active_session_id: binding.active_session_id.clone(),
        producer,
        config,
        cancelling: false,
        stop_failure: None,
        input_pause_key: None,
        input_pause_id: None,
        cancel_task: None,
        turn: None,
        assistant_stop_reason: None,
        mapping: WireMappingState::default(),
        observed_children: std::collections::HashSet::new(),
    };
    // The ACP MCP servers ride the wire command, not a local manager.
    let replace_skipped = resolved.is_empty() && state.lock().await.mcp_server_names.is_empty();
    if !replace_skipped {
        // A failed response does not prove the worker rejected this list.
        // Keep the names until a clear is acknowledged, even if the
        // best-effort clear below also loses its acknowledgement.
        if !resolved.is_empty() {
            state.lock().await.mcp_server_names = resolved
                .iter()
                .map(pa_core::mcp::AcpMcpServerConfig::name)
                .map(str::to_string)
                .collect();
        }
        if let Err(error) = replace_connection_servers(
            link,
            &binding.active_session_id,
            &binding.mcp_owner_id,
            &resolved,
        )
        .await
        {
            // The worker may have applied the list before this failed (a
            // lost acknowledgement); the clear is best-effort, like TS.
            let _ = clear_connection_servers(
                link,
                &binding.active_session_id,
                &binding.mcp_owner_id,
                state,
            )
            .await;
            state.lock().await.session_new_in_flight = false;
            let _ = tx.send(internal_error(&id, &error.to_string()));
            return;
        }
        if resolved.is_empty() {
            state.lock().await.mcp_server_names.clear();
        }
    }

    let mut result = json!({
        "sessionId": acp_session_id,
        "configOptions": *hosted.config.published.lock().await,
    });
    if let Some(requested) = params.cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
        let actual = options.actual_cwd.display().to_string();
        if !same_cwd(Path::new(requested), &options.actual_cwd) {
            result["_meta"] = meta::prime_agent_meta(&PrimeAgentSessionMeta {
                cwd: Some(meta::PrimeAgentCwdMeta {
                    requested: requested.to_string(),
                    actual,
                }),
                ..Default::default()
            });
        }
    }
    // Install the hosted session before the admission response leaves
    // (an immediate `session/set_config_option` must resolve against
    // it). The producer gate opens only after the response is queued,
    // so no held update can precede it.
    let producer = Arc::clone(&hosted.producer);
    let inherited_pause = {
        let mut guard = state.lock().await;
        guard.session_new_in_flight = false;
        if let Some(pause_id) = guard.closed_input_pause_id.clone() {
            hosted.input_pause_id = Some(pause_id);
            hosted.input_pause_key = guard.closed_input_pause_key.clone();
        }
        guard.session = Some(hosted);
        guard
            .closed_input_pause_id
            .clone()
            .zip(guard.closed_input_pause_key.clone())
    };
    let children = match fetch_rlm_children(link, &binding.active_session_id).await {
        Ok(children) => children,
        Err(error) => {
            let _ = clear_connection_servers(
                link,
                &binding.active_session_id,
                &binding.mcp_owner_id,
                state,
            )
            .await;
            let mut guard = state.lock().await;
            guard.session = None;
            guard.session_new_in_flight = false;
            drop(guard);
            let _ = tx.send(internal_error(&id, &error.to_string()));
            return;
        }
    };
    {
        let mut guard = state.lock().await;
        if let Some(current) = guard.session.as_mut() {
            for child in children {
                let id = child.get("id").and_then(Value::as_str).unwrap_or_default();
                if !current.observed_children.insert(id.to_string()) {
                    continue;
                }
                let event = json!({ "type": "rlm_child_update", "child": child });
                for update in wire_events::wire_updates(&event, &mut current.mapping) {
                    let _ = current
                        .producer
                        .publish(&update, 0, PrimeAgentEventPhase::Event, None)
                        .await;
                }
            }
        }
    }
    let _ = tx.send(jsonrpc::response(&id, &result));
    if let Some((pause_id, lease_key)) = inherited_pause {
        match release_session_input_pause(link, &binding.active_session_id, &pause_id).await {
            Ok(()) => {
                let mut guard = state.lock().await;
                if guard.closed_input_pause_id.as_deref() == Some(&pause_id) {
                    guard.closed_input_pause_id = None;
                    guard.closed_input_pause_key = None;
                }
                if let Some(hosted) = guard
                    .session
                    .as_mut()
                    .filter(|hosted| hosted.input_pause_id.as_deref() == Some(&pause_id))
                {
                    hosted.input_pause_id = None;
                    if hosted.input_pause_key.as_deref() == Some(&lease_key) {
                        hosted.input_pause_key = None;
                    }
                }
            }
            Err(error) => {
                if let Some(hosted) = state
                    .lock()
                    .await
                    .session
                    .as_mut()
                    .filter(|hosted| hosted.input_pause_id.as_deref() == Some(&pause_id))
                {
                    hosted.stop_failure = Some(error.to_string());
                }
            }
        }
    }
    producer.commit_session_new_response().await;
}

async fn replace_connection_servers(
    link: &Arc<DaemonLink>,
    daemon_session_id: &str,
    owner_id: &str,
    resolved: &[pa_core::mcp::AcpMcpServerConfig],
) -> anyhow::Result<()> {
    link.request_ok(DaemonCommand::ReplaceAcpMcpServers {
        id: None,
        active_session_id: daemon_session_id.to_string(),
        owner_id: owner_id.to_string(),
        servers: serde_json::to_value(resolved)?,
        rest: Map::default(),
    })
    .await
}

pub(super) async fn clear_connection_servers(
    link: &Arc<DaemonLink>,
    daemon_session_id: &str,
    owner_id: &str,
    state: &Arc<Mutex<DaemonAcpState>>,
) -> anyhow::Result<()> {
    replace_connection_servers(link, daemon_session_id, owner_id, &[]).await?;
    state.lock().await.mcp_server_names.clear();
    Ok(())
}
