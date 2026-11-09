//! The print runtime's engine-level regressions: the RPC prompt's missing
//! family notice, the print lease's lifetime across runtime shutdown, the
//! image router's settle, and the MCP manager the engine build installs.

use super::engine::{build_engine, headless_image_model_router};
use super::modes::with_print_runtime;
use super::HeadlessEmbedding;

fn rpc_run_options(home: &std::path::Path) -> crate::mode::RunOptions {
    crate::mode::RunOptions {
        app_mode: crate::mode::AppMode::Rpc,
        config: crate::mode::RuntimeConfig {
            cwd: home.join("project"),
            agent_dir: home.join("agent"),
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
    }
}

/// The RPC embedding is single-shot (in-process, no daemon link): its
/// session has no agent family, so the messaging skills are not wired.
/// The static core layer names `agent_message.send`/`agent_observe.*`
/// regardless, so the RPC assembly must say the family surface is
/// missing — otherwise the model plans coordination it cannot run
/// (review #31).
#[tokio::test]
async fn rpc_engine_prompt_discloses_the_missing_agent_family() {
    let directory = tempfile::tempdir().unwrap();
    let cli_options = rpc_run_options(directory.path());
    std::fs::create_dir_all(&cli_options.config.cwd).unwrap();
    std::fs::create_dir_all(&cli_options.config.agent_dir).unwrap();
    let script = serde_json::json!({ "responses": ["unused"] });
    let headless = build_engine(
        &cli_options,
        Some(&script),
        None,
        "rpc",
        &HeadlessEmbedding::Rpc,
    )
    .await
    .unwrap();
    let prompt = &headless.engine.system_prompt;
    assert!(
        prompt.contains("agent_message.send"),
        "the static layer advertises agent_message.send in every session: {prompt}"
    );
    assert!(
        !prompt.contains("<name>agent-message</name>"),
        "the RPC embedding installs no messaging skills: {prompt}"
    );
    assert!(
        prompt.contains("has no agent family"),
        "the RPC prompt must name the missing family surface the static \
             layer still advertises: {prompt}"
    );
    headless.engine.dispose_kernel().await;
}

/// The RPC notice must not cost the user their `APPEND_SYSTEM.md`:
/// resource loading only discovers that file when the explicit source
/// list is EMPTY, so a path that only appends the notice silently drops
/// it from every RPC session.
#[tokio::test]
async fn rpc_notice_keeps_the_discovered_append_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let cli_options = rpc_run_options(directory.path());
    std::fs::create_dir_all(&cli_options.config.cwd).unwrap();
    std::fs::create_dir_all(&cli_options.config.agent_dir).unwrap();
    let append_dir = cli_options
        .config
        .cwd
        .join(pa_core::settings::CONFIG_DIR_NAME);
    std::fs::create_dir_all(&append_dir).unwrap();
    std::fs::write(append_dir.join("APPEND_SYSTEM.md"), "PROJECT APPEND MARKER").unwrap();
    let script = serde_json::json!({ "responses": ["unused"] });
    let headless = build_engine(
        &cli_options,
        Some(&script),
        None,
        "rpc",
        &HeadlessEmbedding::Rpc,
    )
    .await
    .unwrap();
    let prompt = &headless.engine.system_prompt;
    assert!(
        prompt.contains("PROJECT APPEND MARKER"),
        "the discovered APPEND_SYSTEM.md still reaches the RPC prompt: {prompt}"
    );
    assert!(
        prompt.contains("has no agent family"),
        "and the notice rides beside it: {prompt}"
    );
    headless.engine.dispose_kernel().await;
}

/// Runtime cancellation unblocks a pending writer; ownership must remain
/// held until that blocking writer finishes, even on errors or unwinding.
#[test]
fn print_lease_outlives_runtime_writers_on_every_return_path() {
    #[derive(Clone, Copy)]
    enum Outcome {
        Success,
        Error,
        Panic,
    }
    struct Fixture {
        session_path: std::path::PathBuf,
        agent_dir: std::path::PathBuf,
        outcome: Outcome,
    }
    struct UnblockWriter(std::sync::mpsc::Sender<()>);
    impl Drop for UnblockWriter {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    for outcome in [Outcome::Success, Outcome::Error, Outcome::Panic] {
        let home = tempfile::TempDir::new().unwrap();
        let fixture = Fixture {
            session_path: home.path().join("session.jsonl"),
            agent_dir: home.path().join("agent"),
            outcome,
        };
        let held_at_write = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer_observation = std::sync::Arc::clone(&held_at_write);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_print_runtime(&fixture, |fixture, lease| {
                Box::pin(async move {
                    *lease = Some(
                        pa_daemon::lease::acquire_runtime_session_lease(
                            &fixture.session_path,
                            &fixture.agent_dir,
                        )
                        .unwrap(),
                    );
                    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
                    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
                    tokio::spawn(async move {
                        let _unblock = UnblockWriter(cancel_tx);
                        let _ = ready_tx.send(());
                        std::future::pending::<()>().await;
                    });
                    ready_rx.await.unwrap();
                    let session_path = fixture.session_path.clone();
                    let agent_dir = fixture.agent_dir.clone();
                    let (writer_ready_tx, writer_ready_rx) = tokio::sync::oneshot::channel();
                    tokio::task::spawn_blocking(move || {
                        let _ = writer_ready_tx.send(());
                        cancel_rx.recv().unwrap();
                        writer_observation.store(
                            pa_daemon::lease::live_lease_owner(&agent_dir, &session_path).is_some(),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        std::fs::write(&session_path, "writer settled\n").unwrap();
                    });
                    writer_ready_rx.await.unwrap();
                    match fixture.outcome {
                        Outcome::Success => Ok(0),
                        Outcome::Error => Err("failed after lease acquisition".to_string()),
                        Outcome::Panic => panic!("unwind after lease acquisition"),
                    }
                })
            })
        }));
        match outcome {
            Outcome::Success => assert_eq!(result.unwrap(), Ok(0)),
            Outcome::Error => assert_eq!(
                result.unwrap(),
                Err("failed after lease acquisition".to_string())
            ),
            Outcome::Panic => assert!(result.is_err()),
        }
        assert!(held_at_write.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            std::fs::read_to_string(&fixture.session_path).unwrap(),
            "writer settled\n"
        );
        assert!(
            pa_daemon::lease::live_lease_owner(&fixture.agent_dir, &fixture.session_path).is_none()
        );
    }
}

/// The settle restores the captured session target only while the slot still holds the route;
/// a mid-run `/model` switch rewrote the slot, and stays.
#[test]
fn headless_image_router_settle_preserves_a_mid_run_model_switch() {
    fn fixture_model(id: &str) -> pa_types::ai::Model {
        pa_types::ai::Model {
            id: id.to_string(),
            name: id.to_string(),
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            base_url: "https://x".to_string(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![
                pa_types::ai::ModelInput::Text,
                pa_types::ai::ModelInput::Image,
            ],
            cost: pa_types::ai::ModelCost {
                input: 1.0.into(),
                output: 2.0.into(),
                cache_read: 0.0.into(),
                cache_write: 0.0.into(),
            },
            context_window: 200_000,
            max_tokens: 8192,
            max_tokens_explicit: false,
            featured: None,
            serving: None,
            headers: None,
            compat: None,
        }
    }
    fn target(
        model: pa_types::ai::Model,
    ) -> pa_core::session_engine::provider_adapter::ProviderTarget {
        pa_core::session_engine::provider_adapter::ProviderTarget {
            api_key: None,
            headers: None,
            model,
            service_tier: None,
        }
    }
    let home = tempfile::TempDir::new().unwrap();
    let agent_dir = home.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let session_model = fixture_model("session-model");
    let provider_target =
        std::sync::Arc::new(std::sync::RwLock::new(Some(target(session_model.clone()))));
    let armed_target: std::sync::Arc<
        std::sync::Mutex<Option<pa_core::session_engine::provider_adapter::ProviderTarget>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(None));
    let router = headless_image_model_router(
        &provider_target,
        std::sync::Arc::clone(&armed_target),
        home.path().to_path_buf(),
        agent_dir,
        session_model,
    );
    let expected_route = pa_core::models::ResolvedImageModel {
        model: fixture_model("image-model"),
        thinking_level: pa_types::ai::ModelThinkingLevel::High,
        service_tier: None,
    };
    (router.swap_target)(Some(&expected_route));
    assert_eq!(
        provider_target.read().unwrap().as_ref().unwrap().model.id,
        "image-model"
    );
    let switched_to = target(fixture_model("switched-model"));
    *provider_target.write().unwrap() = Some(switched_to);
    (router.swap_target)(None);
    assert_eq!(
        provider_target.read().unwrap().as_ref().unwrap().model.id,
        "switched-model"
    );
    // The next episode captures the live slot at ITS first arm: its baseline is the post-switch
    // session model.
    (router.swap_target)(Some(&expected_route));
    (router.swap_target)(None);
    assert_eq!(
        provider_target.read().unwrap().as_ref().unwrap().model.id,
        "switched-model"
    );
}

/// Serves a settings-declared server through `mcp.config` and resolves settings LIVE: a
/// rewrite reaches `refresh()`.
#[tokio::test]
async fn print_mode_mcp_manager_serves_settings_servers_and_resolves_live() {
    let home = tempfile::TempDir::new().unwrap();
    let cwd = home.path().to_path_buf();
    let agent_dir = home.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("settings.json"),
        serde_json::json!({
            "mcpServers": {
                "fixture-echo": {
                    "type": "stdio",
                    "command": "python3",
                    "args": ["echo.py"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let built_manager = crate::mcp_login::cli_mcp_manager(&cwd, &agent_dir);
    let manager = std::sync::Arc::new(std::sync::Mutex::new(built_manager));
    let gating_manager = std::sync::Arc::clone(&manager);
    let enabled = tokio::task::spawn_blocking(move || {
        gating_manager
            .lock()
            .unwrap()
            .get_enabled_persistent_generic_servers()
    })
    .await
    .unwrap();
    assert_eq!(enabled, vec!["fixture-echo".to_string()]);
    let mut handlers = pa_core::kernel::shared::HostRequestHandlers::default();
    pa_core::mcp::McpManager::register_host_handlers(&manager, &mut handlers);
    let config = handlers.get("mcp.config").unwrap().clone();
    let result = config(pa_core::kernel::shared::HostRequestPayload {
        data: serde_json::json!({ "server": "fixture-echo" }),
    })
    .await
    .unwrap();
    assert_eq!(result["type"], "stdio");
    assert_eq!(result["command"], "python3");
    assert_eq!(result["args"], serde_json::json!(["echo.py"]));
    std::fs::write(
        agent_dir.join("settings.json"),
        serde_json::json!({
            "mcpServers": {
                "second-echo": {
                    "type": "stdio",
                    "command": "node",
                    "args": ["echo.mjs"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let gating_manager = std::sync::Arc::clone(&manager);
    tokio::task::spawn_blocking(move || {
        let mut manager = gating_manager.lock().unwrap();
        manager.refresh();
        assert_eq!(
            manager.get_enabled_persistent_generic_servers(),
            vec!["second-echo".to_string()]
        );
    })
    .await
    .unwrap();
    let result = config(pa_core::kernel::shared::HostRequestPayload {
        data: serde_json::json!({ "server": "second-echo" }),
    })
    .await
    .unwrap();
    assert_eq!(
        result["command"], "node",
        "the already-registered handler serves the live integrations"
    );
    let withdrawn = config(pa_core::kernel::shared::HostRequestPayload {
        data: serde_json::json!({ "server": "fixture-echo" }),
    })
    .await
    .unwrap();
    assert!(
        withdrawn.as_object().unwrap().is_empty(),
        "the rewrite withdrew the old server from the live integrations"
    );
}

/// Local service-catalog sources reach the catalog resolution; dropping the declaration
/// withdraws the entry on the next resolve.
#[test]
fn print_mode_mcp_manager_resolves_declared_catalog_sources() {
    let home = tempfile::TempDir::new().unwrap();
    let agent_dir = home.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let catalog = home.path().join("local-catalog.json");
    std::fs::write(
        &catalog,
        serde_json::json!({
            "version": 1,
            "entries": [{
                "server": "my-local", "service": "my-local", "label": "My Local",
                "url": "https://my-local.example/mcp", "aliases": [],
                "transport": { "type": "http", "url": "https://my-local.example/mcp" },
                "auth": { "strategy": "oauth", "clientRegistration": "dynamic" },
                "setup": { "status": "ready" },
                "verification": { "status": "unverified" },
                "legacyBuiltin": false,
                "provenance": [{ "source": "user" }]
            }]
        })
        .to_string(),
    )
    .unwrap();
    let settings = |sources: &[&str]| {
        serde_json::json!({
            "mcpCatalogSources": sources
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        })
        .to_string()
    };
    std::fs::write(
        agent_dir.join("settings.json"),
        settings(&[&catalog.display().to_string()]),
    )
    .unwrap();
    let mut manager = crate::mcp_login::cli_mcp_manager(home.path(), &agent_dir);
    let my_local = manager
        .service_descriptors()
        .iter()
        .find(|service| service.service_id == "my-local")
        .expect("declared source entry resolved");
    assert!(my_local.local_source);
    std::fs::write(agent_dir.join("settings.json"), settings(&[])).unwrap();
    manager.refresh();
    assert!(
        !manager
            .service_descriptors()
            .iter()
            .any(|service| service.service_id == "my-local"),
        "the dropped source no longer resolves"
    );
}
