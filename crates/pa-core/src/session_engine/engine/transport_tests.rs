//! The `transport` setting is wired, not decorative: a session created while
//! the user's settings say `websocket` asks every provider request for that
//! transport, and one created on the default leaves the choice to the provider.
//! The property is the wiring itself — the provider's own transport handling is
//! proven in `pa-ai`.
use super::*;
use pa_agent::scripted::ScriptedProvider;

fn model() -> Model {
    Model {
        id: "m".into(),
        name: "m".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 1_000,
        max_tokens: 100,
        max_tokens_explicit: false,
    }
}

/// Run two turns in a session created under `setting` and return the transport
/// each request carried.
async fn requested_transports(setting: Option<&str>) -> Vec<Option<pa_agent::types::Transport>> {
    let dir = tempfile::TempDir::new().unwrap();
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let agent_dir = dir.path().to_path_buf();
    if let Some(setting) = setting {
        std::fs::write(
            agent_dir.join("settings.json"),
            serde_json::json!({ "transport": setting }).to_string(),
        )
        .unwrap();
        // Deliberately "websocket" and not "web-socket": the spelling the menu
        // writes and the TS product reads is the wire name, not a derivation of
        // the variant name.
        if setting == "websocket" {
            assert!(
                crate::settings::SettingsManager::create(&cwd, &agent_dir)
                    .errors()
                    .is_empty(),
                "the persisted \"websocket\" default must parse"
            );
        }
        // The fixture must actually reach the setting, not sit beside it.
        assert_eq!(
            crate::settings::SettingsManager::create(&cwd, &agent_dir)
                .settings()
                .transport,
            Some(match setting {
                "sse" => crate::settings::TransportSetting::Sse,
                "websocket" => crate::settings::TransportSetting::WebSocket,
                _ => crate::settings::TransportSetting::WebSocketCached,
            })
        );
    }
    let provider = Arc::new(ScriptedProvider::new(model()));
    provider.push_text_turn("first answer");
    provider.push_text_turn("second answer");
    let observed: Arc<std::sync::Mutex<Vec<Option<pa_agent::types::Transport>>>> = Arc::default();
    let capture = Arc::clone(&observed);
    let inner = provider.stream_fn();
    let stream: StreamFn = Arc::new(move |requested, context, options| {
        capture.lock().unwrap().push(options.transport);
        inner(requested, context, options)
    });
    let engine = create_session(SessionEngineConfig {
        cwd: cwd.clone(),
        agent_dir,
        model: Some(model()),
        stream_fn: Some(stream),
        daemonless: true,
        ..Default::default()
    })
    .await
    .unwrap();
    for prompt in ["first", "second"] {
        engine
            .prompt(prompt, PromptOptions::default())
            .await
            .unwrap();
        engine.session.agent().wait_for_idle().await;
    }
    engine.dispose_kernel().await;
    let observed = observed.lock().unwrap().clone();
    observed
}

#[tokio::test]
async fn the_transport_setting_reaches_every_request() {
    assert_eq!(
        requested_transports(Some("websocket")).await,
        vec![Some(pa_agent::types::Transport::Websocket); 2],
        "the `transport` setting must reach every provider request: a settings \
         row that only persists its value promises a choice it never applies"
    );
    assert_eq!(
        requested_transports(Some("websocket-cached")).await,
        vec![Some(pa_agent::types::Transport::WebsocketCached); 2],
        "`websocket-cached` is its own choice; collapsing it into `websocket` \
         silently drops the cached continuation the user selected"
    );
    assert_eq!(
        requested_transports(Some("sse")).await,
        vec![Some(pa_agent::types::Transport::Sse); 2]
    );
    assert_eq!(
        requested_transports(None).await,
        vec![Some(pa_agent::types::Transport::Auto); 2],
        "an unset setting reads as `auto`, the TS `getTransport()` default"
    );
}
