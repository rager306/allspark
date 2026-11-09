//! The headless engine assembly: the model a build starts from, the provider
//! target the session stream reads per call, the image-model routing that arms
//! it, and the one `build_engine` seam every headless mode enters.

use std::sync::Arc;

use pa_agent::stream::{LlmContext, StreamFn, StreamRequestOptions};
use pa_agent::types::Model as AgentModel;
use pa_core::session_engine::provider_adapter::{
    stream_once, switchable_stream_fn, to_agent_model, ProviderTarget,
};
use pa_types::ai::Model;
use tokio::sync::Notify;

use crate::mode::RunOptions;

use super::faux::{build_faux_engine_with, faux_script_env, FauxChildQueue};
use super::selection::{
    build_session_manager_with_lease, builtin_tools, resolve_thinking_level, select_model,
};

/// One engine build for every headless embedding: the scripted
/// verification seam when a faux script is set, the real provider
/// otherwise.
pub(crate) async fn build_engine(
    options: &RunOptions,
    faux: Option<&serde_json::Value>,
    session_manager: Option<pa_core::session::manager::SessionManager>,
    execution_mode: &str,
    embedding: &HeadlessEmbedding,
) -> Result<HeadlessEngine, String> {
    // The RPC embedding tells the session what its prompt cannot deliver
    // (review #31); the other headless embeddings keep the CLI's guidelines.
    let rpc_options = matches!(embedding, HeadlessEmbedding::Rpc).then(|| {
        let mut options = options.clone();
        // `load_resources` only discovers `APPEND_SYSTEM.md` when the
        // explicit source list is EMPTY, so the notice cannot simply be
        // pushed: the RPC path must name the discovered file itself or it
        // would silently drop the user's appended prompt.
        let mut sources = if options.config.append_system_prompt.is_empty() {
            pa_core::resources::discover_append_system_prompt_file(
                &options.config.cwd,
                &options.config.agent_dir,
            )
            .map(|path| vec![path.display().to_string()])
            .unwrap_or_default()
        } else {
            options.config.append_system_prompt.clone()
        };
        sources.push(RPC_NO_FAMILY_NOTICE.to_string());
        options.config.append_system_prompt = sources;
        options
    });
    let options = rpc_options.as_ref().unwrap_or(options);
    match faux {
        Some(script) => {
            build_faux_engine_with(options, script, session_manager, execution_mode, embedding)
                .await
        }
        None => {
            build_headless_engine_with(options, session_manager, execution_mode, embedding).await
        }
    }
}

/// The RPC embedding's missing family surface (review #31). The static core
/// layer states the surface unconditionally in a session that has an agent
/// family, and this transport installs no family host handlers — no parent,
/// no siblings, no children, and no daemon link — so the
/// `agent-message`/`agent-observe` skills never register and their kernel
/// host requests are absent. The dynamic session-role block already names
/// the other daemonless gaps (`rlm.spawn`, heartbeats); this line states the
/// family gap for the session at hand, so the model does not plan
/// coordination it cannot run.
const RPC_NO_FAMILY_NOTICE: &str = "## Session limits\n\nThis RPC session has no agent family: no parent, no siblings, no children, and no daemon link. `agent_message.send`, `agent_observe.*`, and the `agent-message`/`agent-observe` skills are not installed here. Do not plan multi-agent coordination in this session — it is single-session work.";

/// The switchable provider target the session's stream reads per call
/// (shared with the RPC mode, whose picker model switches swap it live).
pub type ProviderTargetSlot = std::sync::Arc<std::sync::RwLock<Option<ProviderTarget>>>;

/// What one engine build varies by session: the print root (inline
/// subagents over its node), a spawned inline child (its own model,
/// auth, and session), or the RPC mode (no inline subagents — today's
/// behavior).
pub(crate) enum HeadlessEmbedding {
    Root {
        node: std::sync::Arc<crate::print_rlm::InlineSessionNode>,
    },
    Child(Box<ChildEngineOverrides>),
    Rpc,
}

/// One spawned inline child's engine deltas: its model lane, the
/// thinking level it runs, and the parent-edge identity.
pub(crate) struct ChildEngineOverrides {
    pub node: std::sync::Arc<crate::print_rlm::InlineSessionNode>,
    pub lane: ChildLane,
    pub thinking: pa_types::ai::ModelThinkingLevel,
    pub semantic_parent_session_id: Option<String>,
    pub spawned_by_request_id: Option<String>,
    /// The spawn carried a factory run tag.
    pub factory_child: bool,
}

/// A child's model source: a real-provider child carries its resolved
/// model; a scripted child runs its own faux queue.
pub(crate) enum ChildLane {
    Real(Box<Model>),
    Faux(FauxChildQueue),
}

/// The per-embedding fields both headless builders (the real provider
/// and the faux script) share: the inline host + agent-message handlers
/// with the RLM depth, the background-bash wake both engine seams fire
/// (the completion row's admission wake and the kernel's
/// background-settle callback), and the late agent-message receipt sink
/// print sessions record through.
pub(crate) struct HeadlessEmbeddingFields {
    pub(crate) rlm_subagent_host:
        Option<Arc<dyn pa_core::session_engine::rlm_host::RlmSubagentHost>>,
    pub(crate) extra_host_handlers: Option<pa_core::kernel::shared::HostRequestHandlers>,
    pub(crate) rlm_depth: Option<u32>,
    pub(crate) factory_child: bool,
    pub(crate) bash_wake: pa_core::session_engine::bash_notices::BashNoticeWake,
    /// The RPC mode's wake handle, carried onto the engine handle so the
    /// queue-pump listener parks on it; `None` outside the RPC embedding.
    pub(crate) rpc_bash_wake: Option<Arc<Notify>>,
    /// The late fire-and-forget `agent_message.send` receipt sink
    /// (#10-print); `None` for the RPC embedding (no print node).
    pub(crate) on_late_sent_agent_message: Option<pa_core::LateSentAgentMessageHandler>,
}

/// The embedding-owned fields for one headless engine build: print
/// sessions wake on the inline node's activity (every quiescence park
/// listens on it), the RPC mode on the built handle's notify.
pub(crate) fn headless_embedding_fields(embedding: &HeadlessEmbedding) -> HeadlessEmbeddingFields {
    let node = match embedding {
        HeadlessEmbedding::Root { node } => Some(Arc::clone(node)),
        HeadlessEmbedding::Child(overrides) => Some(Arc::clone(&overrides.node)),
        HeadlessEmbedding::Rpc => None,
    };
    let (rlm_subagent_host, extra_host_handlers) = match &node {
        Some(node) => {
            let (host, handlers) = crate::print_rlm::inline_subagent_wiring(node);
            (Some(host), Some(handlers))
        }
        None => (None, None),
    };
    let (bash_wake, rpc_bash_wake) = if let Some(node) = &node {
        let node = Arc::clone(node);
        (
            Arc::new(move || node.activity.notify_waiters())
                as pa_core::session_engine::bash_notices::BashNoticeWake,
            None,
        )
    } else {
        let notify = Arc::new(Notify::new());
        let wake = {
            let notify = Arc::clone(&notify);
            Arc::new(move || notify.notify_waiters())
                as pa_core::session_engine::bash_notices::BashNoticeWake
        };
        (wake, Some(notify))
    };
    let on_late_sent_agent_message = node
        .as_ref()
        .map(crate::print_rlm::late_sent_agent_message_sink);
    HeadlessEmbeddingFields {
        rlm_subagent_host,
        extra_host_handlers,
        rlm_depth: match embedding {
            HeadlessEmbedding::Child(overrides) => Some(overrides.node.depth),
            HeadlessEmbedding::Root { .. } | HeadlessEmbedding::Rpc => None,
        },
        factory_child: matches!(
            embedding,
            HeadlessEmbedding::Child(overrides) if overrides.factory_child
        ),
        bash_wake,
        rpc_bash_wake,
        on_late_sent_agent_message,
    }
}

/// The assembled headless engine, so host transports can drive session-command
/// executors (compact/refine) with the session's own model.
pub(crate) struct HeadlessEngine {
    pub(crate) engine: pa_core::session_engine::engine::SessionEngine,
    pub(crate) model: Model,
    pub(crate) api_key: Option<String>,
    /// The live provider target: the stream reads it per call, and the
    /// RPC mode's picker switches swap it (TS `setModel`'s stream
    /// re-registration; `set_model` swaps it without rebuilding the
    /// session).
    pub(super) provider_target: ProviderTargetSlot,
    /// The RPC mode's background-bash wake handle; `None` for the print
    /// embeddings (their wake is the node's activity notify).
    pub(crate) rpc_bash_wake: Option<Arc<Notify>>,
}

/// Transfer the opened lease to its mode before fallible engine assembly.
pub(super) async fn build_headless_engine_parts(
    options: &RunOptions,
    execution_mode: &str,
    lease: &mut Option<pa_daemon::lease::SessionLease>,
) -> Result<HeadlessEngine, String> {
    // Every headless mode discloses immediately (TS main: only the
    // interactive `deferTelemetryNoticeForOnboarding` holds the notice
    // back behind onboarding; `--list-models` never reaches this
    // assembly, matching the TS exit before its diagnostics report).
    crate::telemetry_notice::print_if_due(&options.config);
    let (session_manager, opened_lease) = select_session_manager_with_lease(options)?;
    *lease = opened_lease;
    build_engine(
        options,
        faux_script_env()?.as_ref(),
        session_manager,
        execution_mode,
        &HeadlessEmbedding::Rpc,
    )
    .await
}

/// The session-manager selection every engine build shares, returning the opened session's runtime
/// lease.
pub(super) fn select_session_manager_with_lease(
    options: &RunOptions,
) -> Result<
    (
        Option<pa_core::session::manager::SessionManager>,
        Option<pa_daemon::lease::SessionLease>,
    ),
    String,
> {
    if options.session.no_session {
        return Ok((None, None));
    }
    let (manager, lease) = build_session_manager_with_lease(options)?;
    Ok((Some(manager), lease))
}

/// The real-provider engine assembly over one session-manager selection:
/// model resolution, session persistence, and the engine facade.
pub(crate) async fn build_headless_engine_with(
    options: &RunOptions,
    session_manager: Option<pa_core::session::manager::SessionManager>,
    execution_mode: &str,
    embedding: &HeadlessEmbedding,
) -> Result<HeadlessEngine, String> {
    let config = &options.config;

    // Model registry: composed catalog + models.json with real auth. A
    // spawned child carries its already-resolved model and request auth
    // (the spawn resolved them against this same catalog).
    let auth = pa_core::auth::AuthStorage::create(&config.agent_dir);
    let mut registry =
        pa_core::models::ModelRegistry::create(auth, config.agent_dir.join("models.json"));
    registry.load_private_authorization_from_cache();
    let model = match embedding {
        HeadlessEmbedding::Child(overrides) => match &overrides.lane {
            ChildLane::Real(model) => (**model).clone(),
            ChildLane::Faux(_) => {
                return Err("a scripted child builds through the faux builder".to_string());
            }
        },
        HeadlessEmbedding::Root { .. } | HeadlessEmbedding::Rpc => {
            // The opened session's active context joins the selection (TS
            // `createAgentSession`); `--no-session` selects from no session.
            let session = session_manager
                .as_ref()
                .map(pa_core::session::manager::SessionManager::active_context);
            select_model(&mut registry, config, session.as_ref()).await?
        }
    };
    // Resolve request auth once: the stored team / `PRIME_TEAM_ID` reach the wire through these
    // headers (children resolve against the same catalog the spawn used).
    let resolved = registry.get_api_key_and_headers(&model, model.headers.as_ref());

    let provider_target: ProviderTargetSlot =
        std::sync::Arc::new(std::sync::RwLock::new(Some(ProviderTarget {
            api_key: resolved.api_key.clone(),
            model: model.clone(),
            service_tier: None,
            headers: resolved.headers.clone(),
        })));
    // Field-by-field, never a wire-shape round-trip: the loop spells the
    // endpoint `base_url` where pa-ai writes `baseUrl`, and a serde default
    // would fill the mismatch with an empty string.
    let agent_model: AgentModel = to_agent_model(&model);

    // Children serve their own provider target (the route-authoritative
    // stream would serve the parent's model, and children have no
    // image-model routing).
    let (stream_fn, image_model_router) = if matches!(embedding, HeadlessEmbedding::Child(_)) {
        (
            switchable_stream_fn(std::sync::Arc::clone(&provider_target)),
            None,
        )
    } else {
        // While an episode is armed the stream serves THIS target, so a concurrent
        // `set_model` picker write cannot redirect an in-flight routed turn.
        let armed_target: std::sync::Arc<std::sync::Mutex<Option<ProviderTarget>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let stream_fn = route_authoritative_stream_fn(
            std::sync::Arc::clone(&provider_target),
            std::sync::Arc::clone(&armed_target),
        );
        // TS settings.imageModel routing: image-attaching batches on a session model
        // without image input route to the configured image model or refuse.
        (
            stream_fn,
            Some(headless_image_model_router(
                &provider_target,
                armed_target,
                config.cwd.clone(),
                config.agent_dir.clone(),
                model.clone(),
            )),
        )
    };

    // Telemetry: the CLI's env/settings opt-out decides; depth 0 only, enforced by the engine.
    // A spawned child never reports (the engine's depth gate holds anyway).
    let telemetry = (!config.telemetry_disabled
        && matches!(
            embedding,
            HeadlessEmbedding::Root { .. } | HeadlessEmbedding::Rpc
        ))
    .then(|| {
        let settings = pa_core::settings::SettingsManager::create(&config.cwd, &config.agent_dir);
        pa_core::session_engine::telemetry::TelemetryWiring {
            client: pa_core::session_engine::telemetry::build_client(&settings, &config.agent_dir),
            execution_mode: Some(execution_mode.to_string()),
            now: None,
            telemetry_enabled: Some(
                pa_core::session_engine::telemetry::telemetry_enabled_switch(
                    &config.cwd,
                    &config.agent_dir,
                ),
            ),
        }
    });
    let queue_settings = pa_core::settings::SettingsManager::create(&config.cwd, &config.agent_dir);
    let queue_mode = |mode: pa_core::settings::QueueModeSetting| match mode {
        pa_core::settings::QueueModeSetting::All => pa_agent::agent::QueueMode::All,
        pa_core::settings::QueueModeSetting::OneAtATime => pa_agent::agent::QueueMode::OneAtATime,
    };
    let steering_mode = Some(queue_mode(queue_settings.get_steering_mode()));
    let follow_up_mode = Some(queue_mode(queue_settings.get_follow_up_mode()));
    // Auth construction blocks; run it off the async runtime like the engine's own gating does.
    let mcp_manager = {
        let cwd = config.cwd.clone();
        let agent_dir = config.agent_dir.clone();
        let manager = tokio::task::spawn_blocking(move || {
            crate::mcp_login::cli_mcp_manager(&cwd, &agent_dir)
        })
        .await
        .map_err(|error| format!("MCP manager construction failed: {error}"))?;
        std::sync::Arc::new(std::sync::Mutex::new(manager))
    };
    // TS print mode records too (`semanticEdgeLedgerPath`): the session
    // id names the ledger under the session artifact dir; `--no-session`
    // runs on the in-memory manager (no artifact dir), so the identity is
    // ledger-less but the request ids still go on the wire.
    let session_manager = session_manager
        .unwrap_or_else(|| pa_core::session::manager::SessionManager::in_memory(&config.cwd));
    let HeadlessEmbeddingFields {
        rlm_subagent_host,
        extra_host_handlers,
        rlm_depth,
        factory_child,
        bash_wake,
        rpc_bash_wake,
        on_late_sent_agent_message,
    } = headless_embedding_fields(embedding);
    let (semantic_edges, thinking_level) = match embedding {
        HeadlessEmbedding::Child(overrides) => (
            Some(
                pa_core::session_engine::semantic_edges::SemanticEdgeIdentity {
                    session_id: session_manager.get_session_id().to_string(),
                    // A spawned child's ledger lives in its rlm session dir.
                    ledger_path: pa_core::session_engine::semantic_edges::semantic_edge_ledger_path(
                        Some(std::path::Path::new(&overrides.node.session_dir)),
                        session_manager.get_session_artifact_dir().as_deref(),
                    ),
                    parent_session_id: overrides.semantic_parent_session_id.clone(),
                    spawned_by_request_id: overrides.spawned_by_request_id.clone(),
                },
            ),
            Some(pa_core::session_engine::provider_adapter::map_thinking_level(overrides.thinking)),
        ),
        HeadlessEmbedding::Root { .. } | HeadlessEmbedding::Rpc => (
            Some(
                pa_core::session_engine::semantic_edges::SemanticEdgeIdentity {
                    session_id: session_manager.get_session_id().to_string(),
                    ledger_path: pa_core::session_engine::semantic_edges::semantic_edge_ledger_path(
                        None,
                        session_manager.get_session_artifact_dir().as_deref(),
                    ),
                    parent_session_id: None,
                    spawned_by_request_id: None,
                },
            ),
            Some(resolve_thinking_level(config, &model)),
        ),
    };
    let engine = pa_core::session_engine::engine::create_session(
        pa_core::session_engine::engine::SessionEngineConfig {
            on_late_sent_agent_message,
            cron_store: None,
            semantic_edges,
            telemetry,
            steering_mode,
            follow_up_mode,
            cwd: config.cwd.clone(),
            agent_dir: config.agent_dir.clone(),
            mcp_manager: Some(mcp_manager),
            model: Some(agent_model),
            thinking_level,
            stream_fn: Some(stream_fn),
            tools: builtin_tools(&config.cwd),
            custom_system_prompt: config.system_prompt.clone(),
            prompt_guidelines: config.append_system_prompt.clone(),
            generic_mcp_servers: vec![],
            allow_recursion: None,
            session_manager: Some(session_manager),
            extra_host_handlers,
            conversation_log_path: None,
            additional_skill_paths: config
                .skills
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            additional_prompt_paths: config
                .prompt_templates
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            extra_builtin_skill_overrides: vec![],
            rlm_subagent_host,
            rlm_usage_store: None,
            progress_note_emit: None,
            rlm_depth,
            factory_child,
            daemonless: true,
            model_info: Some(model.clone()),
            // The kernel boots in the background at creation; a spawned
            // child boots lazily on first use.
            prewarm_ipython_kernel: (!matches!(embedding, HeadlessEmbedding::Child(_)))
                .then_some(true),
            // The background-bash wake pair: the completion row's
            // admission fires it, and the kernel's background settle does
            // too, so the embedding's parked drive or pump re-checks.
            bash_notice_wake: Some(Arc::clone(&bash_wake)),
            on_background_work_settled: Some(bash_wake),
            queued_goal_context_purge: None,
            queued_steering_probe: None,
            should_stop_after_turn: None,
            image_model_router,
        },
    )
    .await
    .map_err(|error| format!("{error:#}"))?;
    Ok(HeadlessEngine {
        engine,
        model,
        api_key: resolved.api_key,
        provider_target,
        rpc_bash_wake,
    })
}

/// The armed image route stays AUTHORITATIVE while an episode is armed: a
/// `set_model` picker write to the slot lands only when the settle clears it.
fn route_authoritative_stream_fn(
    provider_target: ProviderTargetSlot,
    armed_target: std::sync::Arc<std::sync::Mutex<Option<ProviderTarget>>>,
) -> StreamFn {
    std::sync::Arc::new(
        move |_requested: AgentModel, context: LlmContext, options: StreamRequestOptions| {
            let armed = armed_target
                .lock()
                .expect("armed image target lock")
                .clone();
            let target = armed
                .or_else(|| {
                    provider_target
                        .read()
                        .expect("provider target lock")
                        .clone()
                })
                .expect("provider target set before the first stream");
            let ProviderTarget {
                api_key,
                model,
                service_tier,
                headers,
            } = target;
            Box::pin(async move {
                stream_once(&model, api_key, service_tier, headers, context, options)
            })
        },
    )
}

/// The routing decision for one dispatched batch (`Err` fails the turn with the
/// actionable refusal) and the serving-target swap (`None` restores the target).
pub(super) fn headless_image_model_router(
    provider_target: &std::sync::Arc<
        std::sync::RwLock<Option<pa_core::session_engine::provider_adapter::ProviderTarget>>,
    >,
    armed_target: std::sync::Arc<
        std::sync::Mutex<Option<pa_core::session_engine::provider_adapter::ProviderTarget>>,
    >,
    cwd: std::path::PathBuf,
    agent_dir: std::path::PathBuf,
    session_model: pa_types::ai::Model,
) -> pa_core::session_engine::image_model_routing::ImageModelRouter {
    // The pre-route session target, captured at the FIRST arm (not at build):
    // the restore must return the SWITCHED-TO target, never the build-time one.
    let armed_from = std::sync::Arc::new(std::sync::Mutex::new(None));
    // Holds the routed target the arm wrote, so the settle can tell a slot
    // that still holds the route from one a mid-run `/model` switch rewrote.
    let armed_to = armed_target;
    let decide_agent_dir = agent_dir.clone();
    let decide_provider_target = std::sync::Arc::clone(provider_target);
    let decide_armed_from = std::sync::Arc::clone(&armed_from);
    let decide = std::sync::Arc::new(
        move |carries_images: bool,
              thinking_level: pa_types::ai::ModelThinkingLevel|
              -> Result<Option<pa_core::models::ResolvedImageModel>, String> {
            if !carries_images {
                return Ok(None);
            }
            // The routing decision needs the SESSION model: during an armed episode
            // the live slot holds the ROUTED target; un-armed, it is the live slot.
            let armed_capture = decide_armed_from
                .lock()
                .expect("armed-from lock")
                .as_ref()
                .map(
                    |target: &pa_core::session_engine::provider_adapter::ProviderTarget| {
                        target.model.clone()
                    },
                );
            let session_model = armed_capture
                .or_else(|| {
                    decide_provider_target
                        .read()
                        .expect("provider target lock")
                        .as_ref()
                        .map(|target| target.model.clone())
                })
                .unwrap_or_else(|| session_model.clone());
            let settings = pa_core::settings::SettingsManager::create(&cwd, &decide_agent_dir);
            let image_model_reference = settings.get_image_model();
            let block_images = settings.get_block_images();
            let auth = pa_core::auth::AuthStorage::create(&decide_agent_dir);
            let mut registry =
                pa_core::models::ModelRegistry::create(auth, decide_agent_dir.join("models.json"));
            registry.load_private_authorization_from_cache();
            let available: Vec<pa_types::ai::Model> =
                registry.get_available().into_iter().cloned().collect();
            // Route acceptance uses the same resolved-auth result the arm path
            // installs: a provider can be signed in while its key resolution still fails.
            let resolvable_auth: std::collections::HashSet<(String, String)> = available
                .iter()
                .filter(|model| {
                    registry
                        .get_api_key_and_headers(model, model.headers.as_ref())
                        .ok
                })
                .map(|model| (model.provider.clone(), model.id.clone()))
                .collect();
            pa_core::models::resolve_image_model_override(
                &pa_core::models::ImageModelRoutingInputs {
                    session_model: &session_model,
                    thinking_level,
                    service_tier: None,
                    image_model_reference: image_model_reference.as_deref(),
                    available_models: &available,
                    // Keyed (provider, id): one provider's authenticated row must not vouch for
                    // another provider's same-id model.
                    has_configured_auth: &|model| {
                        resolvable_auth.contains(&(model.provider.clone(), model.id.clone()))
                    },
                    block_images,
                },
            )
        },
    );
    let swap_target = {
        let provider_target = std::sync::Arc::clone(provider_target);
        let armed_to = std::sync::Arc::clone(&armed_to);
        std::sync::Arc::new(move |route: Option<&pa_core::models::ResolvedImageModel>| {
            if let Some(resolved) = route {
                // The first swap of the episode captures the session target it replaces.
                let mut armed_from = armed_from.lock().expect("armed-from lock");
                if armed_from.is_none() {
                    armed_from.clone_from(&provider_target.read().expect("provider target lock"));
                }
                let auth = pa_core::auth::AuthStorage::create(&agent_dir);
                let mut registry =
                    pa_core::models::ModelRegistry::create(auth, agent_dir.join("models.json"));
                registry.load_private_authorization_from_cache();
                let resolved_auth = registry
                    .get_api_key_and_headers(&resolved.model, resolved.model.headers.as_ref());
                let target = pa_core::session_engine::provider_adapter::ProviderTarget {
                    api_key: resolved_auth.api_key,
                    headers: resolved_auth.headers,
                    model: resolved.model.clone(),
                    service_tier: resolved.service_tier,
                };
                *armed_to.lock().expect("armed-to lock") = Some(target.clone());
                *provider_target.write().expect("provider target lock") = Some(target);
            } else {
                // Restore the captured session target ONLY when the slot still holds the routed
                // target the arm wrote.
                let captured = armed_from.lock().expect("armed-from lock").take();
                let routed = armed_to.lock().expect("armed-to lock").take();
                let current = provider_target
                    .read()
                    .expect("provider target lock")
                    .clone();
                // The full serving target, credentials included: a switch may keep the same
                // id while rotating its api key; the guard treats that slot as switched.
                let still_routed = match (&current, &routed) {
                    (Some(current), Some(routed)) => {
                        current.model.id == routed.model.id
                            && current.service_tier == routed.service_tier
                            && current.api_key == routed.api_key
                            && current.headers == routed.headers
                    }
                    _ => true,
                };
                if still_routed {
                    if let Some(target) = captured.or(current) {
                        *provider_target.write().expect("provider target lock") = Some(target);
                    }
                }
            }
        })
    };
    pa_core::session_engine::image_model_routing::ImageModelRouter {
        decide,
        swap_target,
    }
}

#[cfg(test)]
mod model_projection_tests {
    //! The headless engine hands the loop its model descriptor in
    //! `build_headless_engine_with`. The JSON round-trip that used to sit
    //! there dropped `base_url`: pa-ai writes the wire key `baseUrl`, the
    //! loop reads `base_url`, and serde's `#[serde(default)]` filled the
    //! mismatch with an empty string — so the provider endpoint reached
    //! neither the loop's `on_payload` hooks nor the daemon's `rpc get_state`
    //! row, which projects the loop's model back out.

    use super::super::ChildLane;
    use super::*;

    fn probe_model() -> Model {
        Model {
            id: "probe/model".into(),
            name: "Probe Model".into(),
            api: "openai-completions".into(),
            provider: "probe".into(),
            base_url: "https://probe.invalid/v1".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![pa_types::ai::ModelInput::Text],
            cost: pa_types::ai::ModelCost {
                input: 1.5.into(),
                output: 9.0.into(),
                cache_read: 0.5.into(),
                cache_write: 2.0.into(),
            },
            context_window: 200_000,
            max_tokens: 64_000,
            max_tokens_explicit: true,
            featured: None,
            serving: None,
            headers: None,
            compat: None,
        }
    }

    /// A spawned child embedding prewarms no Python kernel, so this build
    /// needs neither the network nor a kernel: it reaches the model
    /// conversion and stops there.
    #[tokio::test]
    async fn a_headless_child_build_hands_the_loop_the_provider_base_url() {
        let directory = tempfile::tempdir().unwrap();
        let options = crate::mode::RunOptions {
            app_mode: crate::mode::AppMode::Rpc,
            config: crate::mode::RuntimeConfig {
                cwd: directory.path().join("project"),
                agent_dir: directory.path().join("agent"),
                ..Default::default()
            },
            session: crate::mode::SessionOptions::default(),
            messages: Vec::new(),
            file_args: Vec::new(),
            daemon_socket: None,
            list_models: None,
            initial_message: None,
            initial_images: Vec::new(),
            verbose: false,
            offline: true,
            agents_view_requested: false,
            attach_agent: None,
        };
        std::fs::create_dir_all(&options.config.cwd).unwrap();
        std::fs::create_dir_all(&options.config.agent_dir).unwrap();
        let session_manager =
            pa_core::session::manager::SessionManager::in_memory(&options.config.cwd);
        let node = crate::print_rlm::root_node(&options, &session_manager, None);
        let embedding = HeadlessEmbedding::Child(Box::new(ChildEngineOverrides {
            node,
            lane: ChildLane::Real(Box::new(probe_model())),
            thinking: pa_types::ai::ModelThinkingLevel::Medium,
            semantic_parent_session_id: None,
            spawned_by_request_id: None,
            factory_child: false,
        }));
        let headless = build_engine(
            &options,
            None,
            Some(session_manager),
            "subagent",
            &embedding,
        )
        .await
        .unwrap();
        let state = headless.engine.session.agent().state().await;
        assert_eq!(
            state.model,
            AgentModel {
                id: "probe/model".into(),
                name: "Probe Model".into(),
                api: "openai-completions".into(),
                provider: "probe".into(),
                base_url: "https://probe.invalid/v1".into(),
                reasoning: true,
                cost: pa_agent::types::UsageCost {
                    input: 1.5,
                    output: 9.0,
                    cache_read: 0.5,
                    cache_write: 2.0,
                    total: 0.0,
                },
                context_window: 200_000,
                max_tokens: 64_000,
                max_tokens_explicit: true,
            }
        );
        headless.engine.dispose_kernel().await;
    }
}
