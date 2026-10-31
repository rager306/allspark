//! Headless session selection: the flag order a session file resolves in, the
//! runtime lease every opened or materialized file takes, and the model and
//! thinking defaults a build starts from.

use std::sync::Arc;

use pa_core::session::discovery::{
    find_most_recent_session_for_cwd, resolve_session_path, ResolvedSession,
};
use pa_types::ai::Model;

use pa_core::session_engine::provider_adapter::map_thinking_level;

use crate::mode::RunOptions;

use super::render_selector_error;

pub(super) async fn select_model(
    registry: &mut pa_core::models::ModelRegistry,
    config: &crate::mode::RuntimeConfig,
    session: Option<&pa_core::session::SessionContext>,
) -> Result<Model, String> {
    // Explicit selectors retain the CLI resolver's provider inference and do
    // not inherit an unrelated saved provider.
    if let Some(model_name) = config.model.as_deref() {
        let mut candidates: Vec<Model> = registry.get_available().into_iter().cloned().collect();
        candidates.extend(
            registry
                .get_all()
                .iter()
                .filter(|model| {
                    !registry.has_configured_auth(model) && registry.has_request_auth(model)
                })
                .cloned(),
        );
        let resolved =
            pa_core::models::resolve_cli_model(config.provider.as_deref(), model_name, &candidates);
        if let Some(error) = resolved.error {
            return Err(error);
        }
        return resolved
            .model
            .ok_or_else(|| "No matching model found.".to_string());
    }
    // A resumed session with messages restores its own model before the
    // defaults chain (TS `createAgentSession`), through the daemon restore's
    // bounded catalog-readiness wait; a miss falls through on the record.
    if let Some((provider, model_id)) = session
        .filter(|context| !context.messages.is_empty())
        .and_then(|context| context.model.as_ref())
    {
        if let Some(model) = pa_core::models::find_session_model_with_readiness_wait(
            registry,
            provider,
            model_id,
            pa_core::models::SESSION_MODEL_RESTORE_READINESS_TIMEOUT_MS,
        )
        .await
        {
            return Ok(model);
        }
        eprintln!("Could not restore model {provider}/{model_id}");
    }
    // A missed restore's readiness window may have refreshed the catalog, so
    // the defaults chain resolves against the refreshed view.
    let settings = pa_core::settings::SettingsManager::create(&config.cwd, &config.agent_dir);
    let available: Vec<Model> = registry.get_available().into_iter().cloned().collect();
    let saved_provider = settings.get_default_provider();
    let flag_provider = config.provider.as_deref();
    let saved_model = settings.get_default_model().filter(|_| {
        flag_provider.is_none_or(|provider| {
            saved_provider.is_some_and(|saved| provider.eq_ignore_ascii_case(saved))
        })
    });
    if let Some(saved_model) = saved_model {
        // Saved IDs must exist in the authenticated catalog. Do not synthesize
        // a fallback model, especially an unauthorized private Prime model.
        // The lookup is scoped like TS findInitialModel's saved default: the
        // flag, when given, has already matched the saved provider above.
        let scope = flag_provider.or(saved_provider);
        return available.iter().find(|model| {
            scope.is_none_or(|provider| model.provider.eq_ignore_ascii_case(provider))
                && (model.id.eq_ignore_ascii_case(saved_model)
                    || format!("{}/{}", model.provider, model.id).eq_ignore_ascii_case(saved_model))
        }).cloned().ok_or_else(|| format!("Configured default model \"{saved_model}\" is unavailable. Check settings.json, models.json, and provider authentication."));
    }
    // A settings defaultProvider without a defaultModel does not scope the
    // no-flag choice (TS reads the two keys together); only the flag does.
    let candidates: Vec<Model> = available
        .into_iter()
        .filter(|model| {
            flag_provider.is_none_or(|provider| model.provider.eq_ignore_ascii_case(provider))
        })
        .collect();
    pa_core::models::find_preferred_default_model(&candidates)
        .or_else(|| candidates.first())
        .cloned()
        .ok_or_else(|| match flag_provider {
            Some(provider) => format!("No available models for provider \"{provider}\". Check models.json and provider authentication."),
            None => "No authenticated models available. Configure a provider or select a model explicitly.".to_string(),
        })
}

/// Resolve the session thinking level: the CLI flag, then the settings
/// default, then "medium" — always clamped to what the model supports.
pub(super) fn resolve_thinking_level(
    config: &crate::mode::RuntimeConfig,
    model: &Model,
) -> pa_agent::types::ThinkingLevel {
    use pa_types::ai::ModelThinkingLevel;
    let settings = pa_core::settings::SettingsManager::create(&config.cwd, &config.agent_dir);
    let requested = config
        .thinking
        .or_else(|| {
            settings
                .get_default_thinking_level()
                .map(pa_core::settings::ThinkingLevelSetting::model_level)
        })
        .unwrap_or(ModelThinkingLevel::Medium);
    let clamped = pa_ai::models::clamp_thinking_level(model, requested);
    map_thinking_level(clamped)
}

/// The headless session selection, in the flag order of TS
/// `createSessionManager` (noSession -> fork -> resume -> continue ->
/// create). `--no-session` never reaches here: its callers skip the
/// selection.
pub(super) enum HeadlessSession {
    /// Fork this source session into a fresh file.
    Fork(std::path::PathBuf),
    /// Open this existing session file.
    Open(std::path::PathBuf),
    Fresh,
}

/// main.ts `explicitCwdOverride`: with --cwd, the flag's directory wins
/// over the stored session cwd on resume.
pub(super) fn explicit_cwd_override(options: &RunOptions) -> Option<&std::path::Path> {
    options
        .session
        .cwd_from_flag
        .then_some(options.config.cwd.as_path())
}

pub(super) fn select_headless_session(options: &RunOptions) -> Result<HeadlessSession, String> {
    let cwd = &options.config.cwd;
    let session_dir = replacement_session_dir(options);
    // TS `createSessionManager`'s fork arm: every resolution shape forks —
    // a GLOBAL session is exactly what --fork is for (a different
    // project's session copied into this cwd) — with no daemon-active
    // guard: the copy writes a fresh file, never the hosted source.
    if let Some(selector) = &options.session.fork {
        // A leading `~` expands against the home dir (the resume
        // selector's convention; the interactive fork arm matches).
        let expanded = crate::config::expand_tilde_path(selector);
        let selector = expanded.to_string_lossy();
        let resolved = resolve_session_path(&selector, cwd, &session_dir)
            .map_err(|error| render_selector_error(&error))?;
        let source = match resolved {
            ResolvedSession::Path(path)
            | ResolvedSession::Local(path)
            | ResolvedSession::Global { path, .. } => path,
        };
        return Ok(HeadlessSession::Fork(source));
    }
    if let Some(selector) = &options.session.resume {
        let resolved = resolve_session_path(selector, cwd, &session_dir)
            .map_err(|error| render_selector_error(&error))?;
        return match resolved {
            ResolvedSession::Path(path) | ResolvedSession::Local(path) => {
                Ok(HeadlessSession::Open(
                    std::path::absolute(&path).map_err(|error| error.to_string())?,
                ))
            }
            ResolvedSession::Global {
                path: _,
                cwd: session_cwd,
            } => {
                // Headless modes have no fork prompt; mirror the TS non-TTY path.
                Err(format!(
                    "session {selector} belongs to a different project ({}). Pass --fork {selector} to use it here, or run from that project's directory.",
                    session_cwd.display()
                ))
            }
        };
    }
    if options.session.continue_recent {
        // Absolute like the resume arm (TS `setSessionFile` resolves it).
        if let Some(path) = find_most_recent_session_for_cwd(&session_dir, cwd) {
            return Ok(HeadlessSession::Open(
                std::path::absolute(&path).map_err(|error| error.to_string())?,
            ));
        }
    }
    Ok(HeadlessSession::Fresh)
}

/// The in-process session manager for the selected session. The opened
/// session's runtime lease returns alongside: the driving mode owns it for
/// its run's lifetime.
pub(super) fn build_session_manager_with_lease(
    options: &RunOptions,
) -> Result<
    (
        pa_core::session::manager::SessionManager,
        Option<pa_daemon::lease::SessionLease>,
    ),
    String,
> {
    let cwd = options.config.cwd.clone();
    let session_dir = replacement_session_dir(options);
    match select_headless_session(options)? {
        HeadlessSession::Fork(source) => {
            let manager =
                pa_core::session::manager::SessionManager::fork_from(&source, &cwd, &session_dir)?;
            // The materialized fork leases its own file before the engine
            // writes it (the fresh-session rule): another process resuming
            // the new file can never become a second writer while this
            // engine appends — the source was only read, never leased.
            Ok(lease_fresh_manager(manager))
        }
        HeadlessSession::Open(path) => {
            let lease = session_open_guard(options.daemon_socket.as_deref(), &path)?;
            // A failed open's early return drops the lease (released),
            // never leaving an orphaned hold behind.
            let manager =
                open_session_file(&path, &session_dir, &cwd, explicit_cwd_override(options))?;
            Ok((manager, Some(lease)))
        }
        HeadlessSession::Fresh => Ok(fresh_session_with_lease(&cwd, &session_dir)),
    }
}

pub(super) fn replacement_session_dir(options: &RunOptions) -> std::path::PathBuf {
    options
        .session
        .session_dir
        .clone()
        .unwrap_or_else(|| options.config.agent_dir.join("sessions"))
}

/// Build a FRESH persisted manager and lease its eagerly selected file before the engine can write
/// it.
fn fresh_session_with_lease(
    cwd: &std::path::Path,
    session_dir: &std::path::Path,
) -> (
    pa_core::session::manager::SessionManager,
    Option<pa_daemon::lease::SessionLease>,
) {
    let manager = pa_core::session::manager::SessionManager::persisted(cwd, session_dir);
    lease_fresh_manager(manager)
}

/// Lease a freshly materialized session file before the engine can write it: the
/// ungated runtime acquire (`Ok(None)` whenever the env gate is unset).
fn lease_fresh_manager(
    manager: pa_core::session::manager::SessionManager,
) -> (
    pa_core::session::manager::SessionManager,
    Option<pa_daemon::lease::SessionLease>,
) {
    let lease = match manager.get_session_file() {
        Some(path) => {
            match pa_daemon::lease::acquire_runtime_session_lease(
                path,
                &crate::config::get_agent_dir(),
            ) {
                Ok(lease) => Some(lease),
                Err(error) => {
                    eprintln!("prime-agent: could not lease the fresh session file: {error:#}");
                    None
                }
            }
        }
        None => None,
    };
    (manager, lease)
}

/// Guard an in-process open: probe the daemon's live roster, then acquire the
/// runtime lease. Returns the HELD lease — the caller owns its lifetime.
pub(super) fn session_open_guard(
    socket_path: Option<&str>,
    session_path: &std::path::Path,
) -> Result<pa_daemon::lease::SessionLease, String> {
    let socket = crate::interactive_mode::resolve_socket_path(socket_path);
    if let Ok(mut client) = crate::daemon_client::DaemonClient::connect(&socket) {
        let list = client
            .request(pa_types::daemon::DaemonCommand::List {
                id: None,
                all: None,
                cwd: None,
                session_dir: None,
                include_client_owned: None,
                rest: serde_json::Map::default(),
            })
            .map_err(|error| format!("Could not check active sessions: {error:#}"))?;
        if list.success {
            let target = pa_daemon::lease::canonical_session_path(session_path);
            for row in list
                .data
                .and_then(|data| data.get("sessions").cloned())
                .and_then(|sessions| sessions.as_array().cloned())
                .unwrap_or_default()
            {
                let Some(file) = row.get("sessionFile").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if pa_daemon::lease::canonical_session_path(std::path::Path::new(file)) != target {
                    continue;
                }
                let active_session_id = row
                    .get("activeSessionId")
                    .or_else(|| row.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                // The descriptive refusal (operator-directed): attach to the live session instead
                // of reopening its file.
                let message = match pa_tui::session_open_error::holder_from_roster(
                    std::slice::from_ref(&row),
                    &target,
                ) {
                    Some(holder) => {
                        pa_tui::session_open_error::already_active_error(&holder, &target)
                    }
                    None => format!(
                        "Session is already active in {active_session_id}: {}",
                        target.display()
                    ),
                };
                return Err(message);
            }
        }
    }
    // The daemon's roster covers only its own sessions; the session store is
    // shared, so the lease table is the one cross-daemon ownership record.
    let agent_dir = crate::config::get_agent_dir();
    // Acquire, not observe: a probe leaves a window where another process acquires
    // between the check and this open — two writers on one file.
    match pa_daemon::lease::acquire_runtime_session_lease(session_path, &agent_dir) {
        Ok(lease) => Ok(lease),
        Err(error) => {
            let Some(active) = error.downcast_ref::<pa_daemon::lease::SessionAlreadyActiveError>()
            else {
                // The lease table itself failed (io, permissions): never silently proceed over an
                // undeterminable ownership record.
                return Err(format!(
                    "could not verify the session file is not held: {error:#}"
                ));
            };
            Err(pa_daemon::hold_refusal::refusal_message(
                &pa_daemon::hold_refusal::HoldIdentity {
                    pid: active.holder_pid,
                    active_session_id: active.active_session_id.clone(),
                },
                Some(session_path),
            ))
        }
    }
}

pub(super) fn open_session_file(
    path: &std::path::Path,
    session_dir: &std::path::Path,
    fallback_cwd: &std::path::Path,
    explicit_cwd_override: Option<&std::path::Path>,
) -> Result<pa_core::session::manager::SessionManager, String> {
    let session_cwd = stored_session_cwd(path, fallback_cwd, explicit_cwd_override)?;
    Ok(pa_core::session::manager::SessionManager::open(
        &session_cwd,
        session_dir,
        path,
    ))
}

/// The cwd an opened session runs in: the explicit override (main.ts
/// `explicitCwdOverride`), else the stored session cwd, else the
/// fallback. A session stored against a deleted directory must not
/// silently continue somewhere else (main.ts `getMissingSessionCwdIssue`).
pub(super) fn stored_session_cwd(
    path: &std::path::Path,
    fallback_cwd: &std::path::Path,
    explicit_cwd_override: Option<&std::path::Path>,
) -> Result<std::path::PathBuf, String> {
    let session_cwd = explicit_cwd_override.map_or_else(
        || {
            let header = pa_core::session::manager::read_session_header(path);
            header.filter(|header| !header.cwd.is_empty()).map_or_else(
                || fallback_cwd.to_path_buf(),
                |header| std::path::PathBuf::from(&header.cwd),
            )
        },
        std::path::Path::to_path_buf,
    );
    if !session_cwd.exists() {
        return Err(format!(
            "Stored session working directory does not exist: {}\nSession file: {}\nCurrent working directory: {}",
            session_cwd.display(),
            path.display(),
            fallback_cwd.display()
        ));
    }
    Ok(session_cwd)
}

/// Model tools for the print runtime: `python_repl` only — the engine adds the kernel-backed tool
/// itself.
pub(super) fn builtin_tools(_cwd: &std::path::Path) -> Vec<Arc<dyn pa_agent::types::AgentTool>> {
    Vec::new()
}
