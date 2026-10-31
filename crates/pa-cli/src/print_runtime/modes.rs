//! The headless mode entrypoints: ACP, RPC, and print. Each owns its runtime
//! and the lease its session holds for the run.

use crate::headless_autonomous::autonomous_runtime_config;
use crate::mode::RunOptions;

use super::driver::run_prompts_and_emit;
use super::engine::select_session_manager_with_lease;
use super::engine::{build_engine, build_headless_engine_parts, HeadlessEmbedding, HeadlessEngine};
use super::faux::faux_script_env;
use super::selection::{
    explicit_cwd_override, open_session_file, replacement_session_dir, select_headless_session,
    session_open_guard, stored_session_cwd, HeadlessSession,
};

/// The ACP headless mode (TS main.ts, `useDaemonClient`): ensure a
/// supervisor is listening (spawning one detached), create the daemon
/// session the CLI session flags select, and serve the ACP surface over it
/// until the client disconnects. Any startup failure is an `Error:` exit 1
/// before the first ACP frame.
pub(super) fn run_acp_mode(options: &RunOptions) -> Result<i32, String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    rt.block_on(acp_mode_main(options))
}

async fn acp_mode_main(options: &RunOptions) -> Result<i32, String> {
    // The disclosure prints on the ACP client's stderr before any
    // transport starts (TS pushes the daemon-created session's diagnostics
    // to the client; the Rust stderr surface prints it here).
    crate::telemetry_notice::print_if_due(&options.config);
    // Flag > env > default: the same `PRIME_AGENT_DAEMON_SOCKET` contract
    // as every other mode (the `prime-agent` launcher written by
    // install-rust.sh pins that env, so the ACP path must honor it or it
    // would target the TypeScript default socket and treat the schema
    // mismatch as a stale daemon).
    let socket_path = crate::config::resolve_daemon_socket_path(options.daemon_socket.as_deref());
    crate::interactive_mode::ensure_daemon_running(&socket_path, &options.config.cwd)
        .await
        .map_err(|error| format!("{error:#}"))?;
    let (actual_cwd, create) = daemon_acp_create(options)?;
    pa_daemon::acp::daemon::run_daemon_attached_acp_mode(pa_daemon::acp::daemon::DaemonAcpOptions {
        socket_path,
        actual_cwd,
        product_version: crate::config::version().to_string(),
        create,
    })
    .await
    .map_err(|error| format!("{error:#}"))
}

/// The ACP daemon session's create (TS main.ts `defaultSessionConfig` +
/// the startup create): the session the flags select, client-owned only
/// for `--no-session`, plus the session's cwd. The worker opens and
/// leases the selected file itself.
fn daemon_acp_create(
    options: &RunOptions,
) -> Result<(std::path::PathBuf, pa_types::daemon::DaemonCommand), String> {
    use pa_types::daemon::DaemonSessionLifecycle;
    let config = &options.config;
    let session_dir = replacement_session_dir(options);
    let (cwd, session_path, lifecycle) = if options.session.no_session {
        (
            config.cwd.clone(),
            None,
            DaemonSessionLifecycle::ClientOwned,
        )
    } else {
        let (cwd, session_path) = match select_headless_session(options)? {
            HeadlessSession::Fork(source) => {
                let fork = pa_core::session::manager::SessionManager::fork_from(
                    &source,
                    &config.cwd,
                    &session_dir,
                )?;
                (
                    config.cwd.clone(),
                    fork.get_session_file().map(std::path::Path::to_path_buf),
                )
            }
            HeadlessSession::Open(path) => (
                stored_session_cwd(&path, &config.cwd, explicit_cwd_override(options))?,
                Some(path),
            ),
            HeadlessSession::Fresh => (config.cwd.clone(), None),
        };
        (cwd, session_path, DaemonSessionLifecycle::Resident)
    };
    // The CLI session flags, under the TS `runtimeConfigFromArgs`
    // names. `--api-key` stays off: the create config is persisted.
    let mut create_config = serde_json::json!({
        "cwd": cwd.display().to_string(),
        "sessionDir": session_dir.display().to_string(),
        // The telemetry execution mode (TS main.ts `executionMode: appMode`).
        "executionMode": "acp",
    });
    if let Some(provider) = &config.provider {
        create_config["provider"] = serde_json::json!(provider);
    }
    if let Some(model) = &config.model {
        create_config["model"] = serde_json::json!(model);
    }
    if let Some(thinking) = config.thinking {
        create_config["thinking"] = serde_json::json!(thinking.wire_name());
    }
    if let Some(system_prompt) = &config.system_prompt {
        create_config["systemPrompt"] = serde_json::json!(system_prompt);
    }
    if !config.append_system_prompt.is_empty() {
        create_config["appendSystemPrompt"] = serde_json::json!(config.append_system_prompt);
    }
    for (key, paths) in [
        ("skills", &config.skills),
        ("promptTemplates", &config.prompt_templates),
    ] {
        if !paths.is_empty() {
            create_config[key] = paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .into();
        }
    }
    if let Some(autonomous) = &config.autonomous {
        create_config["autonomous"] = serde_json::json!(autonomous_runtime_config(autonomous));
    }
    // Verification seam (the interactive mode's contract): a scripted daemon
    // session from a script FILE path. The product never sets it.
    if let Some(script) = std::env::var_os("PRIME_AGENT_FAUX_SCRIPT") {
        create_config["script"] = serde_json::Value::String(script.to_string_lossy().to_string());
    }
    // Verification seam (the TS child runtime inherits the parent's
    // `sessionConfig`): a scripted parent session's children run this script
    // FILE. The product never sets it.
    if let Some(child_script) = std::env::var_os("PRIME_AGENT_FAUX_CHILD_SCRIPT") {
        create_config["childScript"] =
            serde_json::Value::String(child_script.to_string_lossy().to_string());
    }
    // TS main.ts `launchEnv` (:1092 via the daemon-client create): the
    // client's own environment travels on the create so the session's
    // worker runs in the environment of the terminal that invoked it,
    // not the daemon's boot environment.
    let launch_env = {
        let env = pa_types::daemon::launch_env::collect_process_launch_env();
        (!env.is_empty()).then_some(env)
    };
    let create = pa_types::daemon::DaemonCommand::Create {
        id: None,
        session_path: session_path.map(|path| path.display().to_string()),
        continue_recent: None,
        no_session: options.session.no_session.then_some(true),
        name: None,
        config: Some(create_config),
        telemetry_disabled: crate::mode::create_telemetry_disabled(config),
        runtime_metadata: None,
        lifecycle: Some(lifecycle),
        env: None,
        launch_env,
        rest: serde_json::Map::default(),
    };
    Ok((cwd, create))
}

pub(super) fn run_rpc_mode(options: &RunOptions) -> Result<i32, String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    rt.block_on(rpc_mode_main(options))
}

async fn rpc_mode_main(options: &RunOptions) -> Result<i32, String> {
    let config = &options.config;
    let mut initial_lease = None;
    let parts = build_headless_engine_parts(options, "rpc", &mut initial_lease).await?;
    if let Some(goal) = &config.initial_goal {
        parts
            .engine
            .seed_initial_goal(&goal.objective, goal.token_budget.map(u64::from))
            .await
            .map_err(|error| format!("{error:#}"))?;
    }
    let factory = rpc_engine_factory(options);
    let mut engine_handle = pa_daemon::rpc::session::RpcEngineHandle::from(parts);
    engine_handle.session_lease = initial_lease;
    let exit_code = pa_daemon::rpc::run_rpc_mode(pa_daemon::rpc::RpcOptions {
        engine: engine_handle,
        engine_factory: Some(factory),
        cwd: config.cwd.clone(),
        agent_dir: config.agent_dir.clone(),
        autonomous_config: options
            .config
            .autonomous
            .as_ref()
            .map(autonomous_runtime_config),
    })
    .await
    .map_err(|error| format!("{error:#}"))?;
    Ok(exit_code)
}

/// The engine-replacement seam the RPC mode's `new_session`/`switch_session`/
/// `fork` commands drive: pa-cli owns the assembly, the mode owns the swap.
fn rpc_engine_factory(options: &RunOptions) -> pa_daemon::rpc::session::RpcEngineFactory {
    let options = options.clone();
    std::sync::Arc::new(move |request| {
        let mut options = options.clone();
        // The replacement sessions ignore the CLI's session-selection flags.
        options.session.resume = None;
        options.session.resume_bare = false;
        options.session.continue_recent = false;
        options.session.fork = None;
        Box::pin(async move {
            let (manager, opened_lease) = match &request {
                pa_daemon::rpc::session::RpcEngineRequest::New {
                    parent_session,
                    cwd,
                } => {
                    let session_dir = replacement_session_dir(&options);
                    // The active session's cwd when the command passed one, else the CLI startup
                    // directory.
                    let cwd = cwd.clone().unwrap_or_else(|| options.config.cwd.clone());
                    let manager = match parent_session {
                        Some(parent) => {
                            let mut manager = pa_core::session::manager::SessionManager::persisted(
                                &cwd,
                                &session_dir,
                            );
                            manager.new_session(&pa_core::session::manager::NewSessionOptions {
                                parent_session: Some(parent.clone()),
                                ..Default::default()
                            });
                            manager
                        }
                        None => {
                            pa_core::session::manager::SessionManager::persisted(&cwd, &session_dir)
                        }
                    };
                    // The fresh session's file is leased BEFORE the replacement can write it.
                    let lease = pa_daemon::lease::acquire_runtime_session_lease(
                        manager
                            .get_session_file()
                            .expect("a fresh session knows its file"),
                        &options.config.agent_dir,
                    )
                    .map_err(|error| format!("{error:#}"))?;
                    (manager, Some(lease))
                }
                pa_daemon::rpc::session::RpcEngineRequest::Open {
                    session_path,
                    reuse_lease,
                } => {
                    let session_dir = replacement_session_dir(&options);
                    let cwd = options.config.cwd.clone();
                    // A same-path reopen skips the guard (the session layer adopted it).
                    let lease = if *reuse_lease {
                        None
                    } else {
                        Some(session_open_guard(
                            options.daemon_socket.as_deref(),
                            session_path,
                        )?)
                    };
                    let manager = open_session_file(session_path, &session_dir, &cwd, None)?;
                    // The replacement ADOPTS the opened session's own cwd: tools,
                    // settings, and file work run against the session's repository.
                    options.config.cwd = manager.get_cwd().to_path_buf();
                    (manager, lease)
                }
            };
            let engine = build_engine(
                &options,
                faux_script_env()?.as_ref(),
                Some(manager),
                "rpc",
                &HeadlessEmbedding::Rpc,
            )
            .await?;
            let mut handle = pa_daemon::rpc::session::RpcEngineHandle::from(engine);
            handle.session_lease = opened_lease;
            Ok(handle)
        })
    })
}

impl From<HeadlessEngine> for pa_daemon::rpc::session::RpcEngineHandle {
    fn from(parts: HeadlessEngine) -> Self {
        Self {
            engine: std::sync::Arc::new(parts.engine),
            model: parts.model,
            api_key: parts.api_key,
            provider_target: parts.provider_target,
            bash_wake: parts
                .rpc_bash_wake
                .expect("the RPC embedding builds its background-bash wake handle"),
            session_lease: None,
        }
    }
}

pub(super) fn run_print_mode(options: &RunOptions) -> Result<i32, String> {
    with_print_runtime(options, |options, lease| {
        Box::pin(print_mode_main(options, lease))
    })
}

/// Own the print lease through runtime shutdown, including errors and unwinding.
/// The operation seam lets the regression exercise a writer pending at shutdown.
pub(super) fn with_print_runtime<Context>(
    context: &Context,
    run: impl for<'a> FnOnce(
        &'a Context,
        &'a mut Option<pa_daemon::lease::SessionLease>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<i32, String>> + 'a>,
    >,
) -> Result<i32, String> {
    // Declared first so unwinding also stops all runtime tasks before release.
    let mut lease = None;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let result = rt.block_on(run(context, &mut lease));
    // Runtime Drop joins blocking writers and cancels async host tasks. Engine
    // Drop alone cannot do that: detached host requests retain session writers.
    drop(rt);
    result
}

async fn print_mode_main(
    options: &RunOptions,
    lease: &mut Option<pa_daemon::lease::SessionLease>,
) -> Result<i32, String> {
    // `print` or `json`: the telemetry execution mode is the app mode (TS
    // main.ts `executionMode: appMode`).
    crate::telemetry_notice::print_if_due(&options.config);
    let (session_manager, opened_lease) = select_session_manager_with_lease(options)?;
    *lease = opened_lease;
    let session_manager = session_manager.unwrap_or_else(|| {
        pa_core::session::manager::SessionManager::in_memory(&options.config.cwd)
    });
    // The print root's inline-subagent node: the quiescence drive, the
    // host, and the messaging controller all hang off it. The faux
    // script (when scripted) rides the recipe down to every child.
    let faux = faux_script_env()?;
    let root_node = crate::print_rlm::root_node(options, &session_manager, faux.clone());
    let headless = build_engine(
        options,
        faux.as_ref(),
        Some(session_manager),
        options.app_mode.as_str(),
        &HeadlessEmbedding::Root {
            node: std::sync::Arc::clone(&root_node),
        },
    )
    .await?;
    let engine = std::sync::Arc::new(headless.engine);
    crate::print_rlm::adopt_built_engine(&root_node, &engine, &headless.model).await;
    // The CLI `--goal` seed: a fresh root branch starts the goal; a resumed
    // branch keeps its persisted goal. Depth 0 only — the print session is a root.
    if let Some(goal) = &options.config.initial_goal {
        engine
            .seed_initial_goal(&goal.objective, goal.token_budget.map(u64::from))
            .await
            .map_err(|error| format!("{error:#}"))?;
    }
    let exit = run_prompts_and_emit(
        &engine,
        &headless.model,
        headless.api_key.clone(),
        options,
        &root_node,
    )
    .await;
    // No inline descendant outlives the print process: kernels get their
    // final snapshot, drivers join, runs abort (every exit path).
    crate::print_rlm::dispose_inline_subagents(&root_node).await;
    exit
}
