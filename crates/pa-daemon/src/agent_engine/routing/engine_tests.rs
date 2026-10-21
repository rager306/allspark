//! The router at the live engine seam: the level gate hands the caller's own
//! ladder back unchanged for `off`/`dry-run`, `on` reorders the same ladder,
//! and a hard filter drops a row instead of walking into it.
//!
//! The unit battery beside the module covers the pure mapping (`apply_route_ladder`);
//! this one proves the wiring: settings → level → decision → the vector the
//! retry driver actually walks.

use super::*;
use crate::agent_engine::{AgentEngineConfig, AgentSessionEngine};
use crate::engine::{EngineModelSelection, SessionEngine as _};
use pa_core::settings::ModelRoutingLevel;
use serde_json::json;

/// A three-provider catalog: ONE model id served three ways, in catalog order
/// dear → mid → cheap. The serving provider is `router-dear`, so catalog-order
/// failover walks mid then cheap — while the router's score prefers cheap, then
/// mid. That gap is what makes a reorder observable.
fn write_routing_pair(agent_dir: &std::path::Path) {
    std::fs::create_dir_all(agent_dir).unwrap();
    let provider = |name: &str, key: &str, input: f64| {
        (
            name.to_string(),
            json!({
                "api": "openai-completions",
                "baseUrl": "http://127.0.0.1:9/v1",
                "apiKey": key,
                "models": [{
                    "id": "mock-1", "name": name, "api": "openai-completions",
                    "input": ["text"], "contextWindow": 128_000, "maxTokens": 4096,
                    "cost": {"input": input, "output": input, "cacheRead": 0.0, "cacheWrite": 0.0}
                }]
            }),
        )
    };
    let providers: serde_json::Map<String, serde_json::Value> = [
        provider("router-dear", "sk-dear", 15.0),
        provider("router-mid", "sk-mid", 8.0),
        provider("router-cheap", "sk-cheap", 0.1),
    ]
    .into_iter()
    .collect();
    std::fs::write(
        agent_dir.join("models.json"),
        json!({ "providers": providers }).to_string(),
    )
    .unwrap();
}

fn router_engine(dir: &std::path::Path, mode: &str) -> std::sync::Arc<AgentSessionEngine> {
    let agent_dir = dir.join("agent");
    write_routing_pair(&agent_dir);
    std::fs::write(
        agent_dir.join("settings.json"),
        json!({ "routing": { "mode": mode } }).to_string(),
    )
    .unwrap();
    let engine = AgentSessionEngine::new(AgentEngineConfig {
        admission: crate::agent_engine::provider_admission::AdmissionClient::Disabled,
        cwd: dir.to_path_buf(),
        agent_dir,
        provider: Some("router-dear".to_string()),
        model: Some("mock-1".to_string()),
        api_key: None,
        thinking: None,
        session_dir: None,
        session_file: None,
        faux_script: None,
        supervisor_link: None,
        telemetry_disabled: Some(true),
        cron_store: None,
        queued_steering_probe: None,
    })
    .unwrap();
    let engine = std::sync::Arc::new(engine);
    engine.configure_model(EngineModelSelection {
        provider: Some("router-dear".to_string()),
        model: Some("mock-1".to_string()),
        api_key: None,
        thinking: None,
    });
    engine
}

fn selectors(models: Vec<Model>) -> Vec<String> {
    models
        .into_iter()
        .map(|model| format!("{}/{}", model.provider, model.id))
        .collect()
}

fn facts() -> super::TurnRouteFacts {
    super::TurnRouteFacts {
        needed_tokens: 1_000,
        output_tokens: 4_096,
        carries_images: false,
    }
}

#[test]
fn off_leaves_the_catalog_order_untouched() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = router_engine(dir.path(), "off");
    let serving = engine.resolve_model().unwrap();
    assert_eq!(engine.routing_level(), ModelRoutingLevel::Off);
    let catalog_order = selectors(engine.failover_candidates(&serving));
    // The catalog order this test pins: the serving provider first-position,
    // so the ladder starts one step after it.
    assert_eq!(
        catalog_order,
        vec!["router-mid/mock-1", "router-cheap/mock-1"]
    );
    let routed = selectors(engine.route_turn_candidates(
        &serving,
        engine.failover_candidates(&serving),
        facts(),
    ));
    assert_eq!(routed, catalog_order, "off is the pre-router behaviour");
}

#[test]
fn dry_run_records_and_still_hands_back_catalog_order() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = router_engine(dir.path(), "dry-run");
    let serving = engine.resolve_model().unwrap();
    let level = engine.routing_level();
    assert_eq!(level, ModelRoutingLevel::DryRun);
    assert!(level.is_enabled() && !level.applies());
    let routed = selectors(engine.route_turn_candidates(
        &serving,
        engine.failover_candidates(&serving),
        facts(),
    ));
    assert_eq!(
        routed,
        selectors(engine.failover_candidates(&serving)),
        "dry-run applies nothing"
    );
}

#[test]
fn auto_holds_catalog_order_on_the_first_turn_then_reorders_by_score() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = router_engine(dir.path(), "auto");
    let serving = engine.resolve_model().unwrap();
    // Catalog order after `router-dear` is mid → cheap (the providers sort by
    // name); the score prefers cheap → mid by ~0.14, well past the margin.
    assert_eq!(
        selectors(engine.failover_candidates(&serving)),
        vec!["router-mid/mock-1", "router-cheap/mock-1"],
        "the catalog order the router must improve on"
    );
    // The first boundary observes no served turn (dwell 0), so the dwell floor
    // holds catalog order: a session's very first turn never reorders.
    let first = selectors(engine.route_turn_candidates(
        &serving,
        engine.failover_candidates(&serving),
        facts(),
    ));
    assert_eq!(first, vec!["router-mid/mock-1", "router-cheap/mock-1"]);
    assert_eq!(engine.router_state.lock().unwrap().dwell_turns, 0);

    // Two settled turns later the challenger clears both the margin and the
    // floor, and the same two rows come back in score order.
    let mut routed = first;
    for _ in 0..2 {
        routed = selectors(engine.route_turn_candidates(
            &serving,
            engine.failover_candidates(&serving),
            facts(),
        ));
    }
    assert_eq!(
        routed,
        vec!["router-cheap/mock-1", "router-mid/mock-1"],
        "the cheaper provider leads once the dwell allows the move"
    );
}

#[test]
fn a_move_resets_the_dwell_under_auto() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = router_engine(dir.path(), "auto");
    let serving = engine.resolve_model().unwrap();
    for _ in 0..3 {
        let _ =
            engine.route_turn_candidates(&serving, engine.failover_candidates(&serving), facts());
    }
    // The third boundary moved the ladder: the dwell restarted, so the next
    // soft preference has to wait for the floor again.
    assert_eq!(engine.router_state.lock().unwrap().dwell_turns, 0);
}

#[test]
fn a_hard_context_filter_drops_a_row_from_the_live_ladder() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = router_engine(dir.path(), "on");
    let serving = engine.resolve_model().unwrap();
    // Narrow one ladder row below the request: it must leave the ladder
    // instead of being walked into.
    let mut narrow = engine.failover_candidates(&serving);
    assert_eq!(narrow.len(), 2);
    narrow[0].context_window = 64_000;
    let huge = super::TurnRouteFacts {
        needed_tokens: 100_000,
        output_tokens: 4_096,
        carries_images: false,
    };
    assert_eq!(
        selectors(narrow.clone()),
        vec!["router-mid/mock-1", "router-cheap/mock-1"]
    );
    assert_eq!(narrow[0].provider, "router-mid");
    let routed = selectors(engine.route_turn_candidates(&serving, narrow, huge));
    assert_eq!(
        routed,
        vec!["router-cheap/mock-1"],
        "the narrow row is skipped, the roomy one stays"
    );
}

#[test]
fn the_dwell_counts_turns_served_and_gates_only_auto() {
    // `on` orders by score every turn by definition: it runs the counter
    // without gating on it.
    let dir = tempfile::TempDir::new().unwrap();
    let engine = router_engine(dir.path(), "on");
    for expected in 0..3 {
        let serving = engine.resolve_model().unwrap();
        let _ =
            engine.route_turn_candidates(&serving, engine.failover_candidates(&serving), facts());
        assert_eq!(
            engine.router_state.lock().unwrap().dwell_turns,
            expected,
            "`on` runs the counter without gating on it"
        );
    }
}

#[test]
fn a_real_faux_turn_passes_through_the_router_unharmed() {
    // The whole point of the wiring: a turn runs end to end with the router
    // consulted, and the turn still settles. `dry-run` is the level that must
    // be safe by definition (it applies nothing), so it is the one a real
    // turn can exercise without a network provider behind the ladder.
    let _serialized = crate::agent_engine::tests::FAUX_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempfile::TempDir::new().unwrap();
    let agent_dir = dir.path().join("agent");
    write_routing_pair(&agent_dir);
    std::fs::write(
        agent_dir.join("settings.json"),
        json!({ "routing": { "mode": "dry-run" } }).to_string(),
    )
    .unwrap();
    let engine = AgentSessionEngine::new(AgentEngineConfig {
        admission: crate::agent_engine::provider_admission::AdmissionClient::Disabled,
        cwd: dir.path().to_path_buf(),
        agent_dir,
        provider: None,
        model: None,
        api_key: None,
        thinking: None,
        session_dir: None,
        session_file: None,
        faux_script: Some(
            json!({ "responses": [{
                "role": "assistant",
                "content": [{"type": "text", "text": "settled"}],
                "stopReason": "stop"
            }] })
            .to_string(),
        ),
        supervisor_link: None,
        telemetry_disabled: Some(true),
        cron_store: None,
        queued_steering_probe: None,
    })
    .unwrap();
    // A faux script never fails over (`failover_candidates` returns empty), so
    // the router is asked for a decision over a one-row set and applies
    // nothing; the assertion is that the turn still settles.
    let mut events = Vec::new();
    crate::agent_engine::tests::admit(&engine, "hello".to_string(), &mut events);
    assert_eq!(engine.routing_level(), ModelRoutingLevel::DryRun);
    let settled = events.iter().any(|event| {
        matches!(
            event,
            crate::engine::EngineEvent::AssistantMessage(message)
                if message.get("stopReason").and_then(|value| value.as_str()) == Some("stop")
        )
    });
    assert!(
        settled,
        "the routed turn settles: {:?}",
        events
            .iter()
            .map(|event| format!("{event:?}").chars().take(60).collect::<String>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_local_registry_supplies_a_graded_error_rate() {
    // The registry's retry-budget window is the graded twin of its breaker:
    // the same samples, read as a rate. Prove the seam reads it, that it stays
    // `None` below the window's minimum sample count (a 1-in-2 rate is noise,
    // not evidence), and that a rate UNDER the breaker's 10% budget is still
    // reported — which is the grace period the graded term exists for.
    use crate::agent_engine::provider_admission::{
        ProviderAdmission, ProviderKey, StreamAdmission, StreamOutcome,
    };
    let registry = ProviderAdmission::new(6);
    let key = ProviderKey::new("router-dear", "mock-1");
    let dir = tempfile::TempDir::new().unwrap();
    write_routing_pair(&dir.path().join("agent"));
    let engine = AgentSessionEngine::new(AgentEngineConfig {
        admission: crate::agent_engine::provider_admission::AdmissionClient::local(
            std::sync::Arc::clone(&registry),
        ),
        cwd: dir.path().to_path_buf(),
        agent_dir: dir.path().join("agent"),
        provider: Some("router-dear".to_string()),
        model: Some("mock-1".to_string()),
        api_key: None,
        thinking: None,
        session_dir: None,
        session_file: None,
        faux_script: None,
        supervisor_link: None,
        telemetry_disabled: Some(true),
        cron_store: None,
        queued_steering_probe: None,
    })
    .unwrap();
    let model = engine.resolve_model().unwrap();

    // No samples yet: no fact. The term must stay neutral rather than reading
    // a clean record out of silence.
    assert_eq!(engine.admission_facts(&model).unwrap().error_rate, None);

    let settle = |outcome: StreamOutcome| {
        let admission = engine
            .runtime
            .block_on(registry.acquire_stream(&key, "test-holder"));
        let StreamAdmission::Admitted(lease) = admission else {
            panic!("the registry admits a provider under its budget");
        };
        registry.settle(lease, outcome);
    };

    // Two clean settles: still below the window's minimum, so still no rate.
    for _ in 0..2 {
        settle(StreamOutcome::Settled);
    }
    assert_eq!(
        engine.admission_facts(&model).unwrap().error_rate,
        None,
        "two samples is noise, not a rate"
    );

    // Nine more clean settles cross the minimum: the rate reports, at zero.
    for _ in 0..9 {
        settle(StreamOutcome::Settled);
    }
    let facts = engine.admission_facts(&model).unwrap();
    assert_eq!(facts.error_rate, Some(0.0), "eleven clean settles");

    // One rate limit out of twelve is 8.3% — under the 10% budget, so the
    // breaker stays closed while the graded term already sees the defect.
    settle(StreamOutcome::RateLimited(
        pa_core::session_engine::provider_retry::RateLimitReason::RateLimit,
    ));
    let facts = engine.admission_facts(&model).unwrap();
    let rate = facts.error_rate.expect("the window has samples");
    assert!(
        (rate - 1.0 / 12.0).abs() < 1e-9,
        "one failure in twelve samples: {rate}"
    );
    assert!(
        !facts.breaker_open,
        "8.3% is under the 10% retry budget, so the hard filter has not tripped"
    );
}
