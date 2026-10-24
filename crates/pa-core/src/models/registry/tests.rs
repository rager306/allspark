//! The registry unit battery: catalog, auth filters, cache merges, headers.
use super::*;
use crate::auth::manager::{AuthStorage, NoOAuth};
use crate::auth::types::AuthStorageData;
use std::sync::Arc;

fn auth_with(data: &serde_json::Value) -> AuthStorage {
    let data = AuthStorageData(data.as_object().cloned().unwrap_or_default());
    AuthStorage::in_memory(&data, Arc::new(NoOAuth))
}

/// Like [`auth_with`] but with ambient `PRIME_API_KEY`/`PRIME_TEAM_ID`
/// ignored: the stored credential is the only source, so credential
/// tests are hermetic on boxes that carry Prime Inference env vars.
fn auth_without_env(data: &serde_json::Value) -> AuthStorage {
    let data = AuthStorageData(data.as_object().cloned().unwrap_or_default());
    AuthStorage::in_memory_without_env(&data, Arc::new(NoOAuth))
}

fn model(id: &str, provider: &str) -> Model {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "api": "openai-completions", "provider": provider,
        "baseUrl": "https://example.com", "reasoning": false, "input": ["text"],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 1000, "maxTokens": 100
    }))
    .unwrap()
}

#[test]
fn in_memory_loads_built_in_catalog() {
    let registry = ModelRegistry::in_memory(auth_with(&serde_json::json!({})));
    assert!(registry.get_error().is_none());
    assert_ne!(registry.get_all().len(), 0);
    assert!(registry
        .get_all()
        .iter()
        .any(|m| m.id == "internal/glm-5.2-fast"));
}

/// TS `755-configured-max-tokens.test.ts`: built-in catalog models stay
/// unmarked when no override configures `maxTokens` — the default 32000
/// ceiling keeps applying to them (901 of the catalog's models declare
/// more than 32000 and their request size must not change).
#[test]
fn built_in_catalog_models_stay_unmarked() {
    let registry = ModelRegistry::in_memory(auth_with(&serde_json::json!({})));
    assert_ne!(registry.get_all().len(), 0);
    assert!(registry
        .get_all()
        .iter()
        .all(|model| !model.max_tokens_explicit));
}

#[test]
fn available_filters_by_configured_auth() {
    let auth = auth_with(&serde_json::json!({
        "anthropic": { "type": "api_key", "key": "sk-ant" }
    }));
    let registry = ModelRegistry::in_memory(auth);
    assert!(registry.has_configured_auth(&model("m", "anthropic")));
    // An unconfigured provider is auth-gated out; the name is deliberately
    // obscure so ambient environment variables cannot authorize it.
    assert!(!registry.has_configured_auth(&model("m", "zz-no-provider")));
    let all = registry.get_all();
    let available = registry.get_available();
    assert!(available.iter().all(|available_model| {
        all.iter().any(|model| {
            model.id == available_model.id && model.provider == available_model.provider
        })
    }));
}

#[test]
fn native_free_models_do_not_configure_a_provider_or_enter_default_candidates() {
    let registry = ModelRegistry::in_memory(auth_without_env(&serde_json::json!({})));
    let free = registry
        .get_all()
        .iter()
        .find(|model| model.provider == "opencode" && model.id == "deepseek-v4.1-flash-free")
        .expect("native free model");
    assert!(!registry.auth.has_auth("opencode"));
    assert_eq!(
        registry.get_provider_auth_status("opencode"),
        crate::auth::types::AuthStatus::default()
    );
    assert!(!registry.has_configured_auth(free));
    assert_eq!(registry.get_available().len(), 0);
    // Discovery is the other question (issue #9): the model sends its own public
    // credential, so a picker should offer it even though it is not a failover
    // candidate. Keeping these two apart is what lets the free tier be visible
    // without becoming a default.
    assert!(
        registry
            .get_discoverable_models()
            .iter()
            .any(|model| model.provider == "opencode" && model.id == "deepseek-v4.1-flash-free"),
        "the native free model must be discoverable: {:?}",
        registry
            .get_discoverable_models()
            .iter()
            .map(|m| (m.provider.as_str(), m.id.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn explicit_native_free_selection_resolves_public_request_auth_only_on_its_endpoint() {
    let mut registry = ModelRegistry::in_memory(auth_without_env(&serde_json::json!({})));
    let free = registry
        .resolve_set_model_selection("opencode", "deepseek-v4.1-flash-free")
        .expect("explicit native free selection")
        .clone();
    assert_eq!(
        registry.get_api_key_and_headers(&free, None),
        ResolvedRequestAuth {
            ok: true,
            api_key: Some("public".into()),
            headers: None,
            error: None,
        }
    );
}

/// The deprecate half of the discovery contract: a retirement list is
/// reported only for ids the table actually carries, and the tuple shape
/// (id, date) is what the picker and `model list` render.
#[test]
fn deprecated_opencode_ids_reports_only_table_rows() {
    let registry = ModelRegistry::in_memory(auth_without_env(&serde_json::json!({})));
    // No snapshot in an in-memory registry: empty, never a panic.
    assert_eq!(
        registry.deprecated_opencode_ids(),
        Vec::<(String, String)>::new()
    );
}

#[test]
fn a_native_free_row_never_learns_other_endpoints_request_auth() {
    let mut registry = ModelRegistry::in_memory(auth_without_env(&serde_json::json!({})));
    let free = registry
        .resolve_set_model_selection("opencode", "deepseek-v4.1-flash-free")
        .expect("explicit native free selection")
        .clone();
    assert!(registry.has_request_auth(&free));
    for base_url in [
        "https://example.com/v1",
        "https://opencode.ai.example.com/inference/v1",
        "https://opencode.ai@evil.example/inference/v1",
        "https://opencode.ai:444/inference/v1",
        "https://opencode.ai/zen/v1",
        "http://opencode.ai/inference/v1",
    ] {
        let mut custom_endpoint = free.clone();
        custom_endpoint.base_url = base_url.into();
        assert!(!registry.has_request_auth(&custom_endpoint));
        assert_eq!(
            registry
                .get_api_key_and_headers(&custom_endpoint, None)
                .api_key,
            None
        );
    }
    assert!(matches!(
        registry.resolve_set_model_selection("opencode", "gpt-5"),
        Err(SetModelSelectionError::ProviderUnauthenticated { .. })
    ));
}

#[test]
fn native_free_request_auth_preserves_configured_credentials() {
    let auth = auth_without_env(&serde_json::json!({
        "opencode": {"type": "api_key", "key": "explicit-test-key"}
    }));
    let mut registry = ModelRegistry::in_memory(auth);
    let free = registry
        .resolve_set_model_selection("opencode", "deepseek-v4.1-flash-free")
        .unwrap()
        .clone();
    assert_eq!(
        registry
            .get_api_key_and_headers(&free, None)
            .api_key
            .as_deref(),
        Some("explicit-test-key")
    );
}

#[test]
fn rlm_searchable_models_gate_one_stale_provider_across_its_models() {
    // The per-provider status memo must answer every model of a provider
    // with the same probe result: a stale provider's whole model list stays
    // gated while a second authed provider keeps its models searchable.
    let mut auth = auth_without_env(&serde_json::json!({
        "anthropic": { "type": "api_key", "key": "sk-ant" },
        "openai": { "type": "api_key", "key": "sk-oai" }
    }));
    assert!(auth.mark_auth_stale("anthropic"));
    let registry = ModelRegistry::in_memory(auth);
    let searchable = registry.get_rlm_searchable_models();
    assert!(searchable.iter().all(|model| model.provider != "anthropic"));
    assert!(searchable.iter().any(|model| model.provider == "openai"));
    // `has_auth` is stale-aware (a marked credential is not
    // "configured"), so the available set gates the stale provider's
    // models too - the same semantics at the tip and with the memo.
    let available = registry.get_available();
    assert!(!available.iter().any(|model| model.provider == "anthropic"));
    assert!(available.iter().any(|model| model.provider == "openai"));
    assert!(available
        .iter()
        .all(|model| model.provider != "zz-no-provider"));
}

#[test]
fn models_json_custom_models_and_auth_header() {
    let dir = tempfile::tempdir().unwrap();
    let models_path = dir.path().join("models.json");
    std::fs::write(
        &models_path,
        r#"{ "providers": { "custom": {
            "baseUrl": "https://custom.example", "apiKey": "custom-key",
            "api": "openai-completions", "authHeader": true,
            "models": [ { "id": "my-model" } ]
        } } }"#,
    )
    .unwrap();
    let auth = auth_with(&serde_json::json!({}));
    let mut registry = ModelRegistry::create(auth, &models_path);
    assert!(registry.get_error().is_none());
    let custom = registry
        .get_all()
        .iter()
        .find(|m| m.provider == "custom" && m.id == "my-model")
        .expect("custom model merged")
        .clone();
    assert_eq!(custom.base_url, "https://custom.example");
    assert!(registry.get_available().iter().any(|m| m.id == "my-model"));
    let auth_result = registry.get_api_key_and_headers(&custom, None);
    assert!(auth_result.ok);
    assert_eq!(auth_result.api_key.as_deref(), Some("custom-key"));
    assert_eq!(
        auth_result.headers.as_ref().unwrap().get("Authorization"),
        Some(&"Bearer custom-key".to_string())
    );
}

#[test]
fn fork_providers_join_the_built_in_catalog() {
    let registry = ModelRegistry::in_memory(auth_with(&serde_json::json!({})));
    let all = registry.get_all();
    assert!(all
        .iter()
        .any(|m| m.provider == "muse" && m.id == "muse-spark-1.3"));
    assert!(all
        .iter()
        .any(|m| m.provider == "codebuddy" && m.id == "glm-5.2"));
}

#[test]
fn models_json_provider_overrides_reach_fork_providers() {
    let dir = tempfile::tempdir().unwrap();
    let models_path = dir.path().join("models.json");
    std::fs::write(
        &models_path,
        r#"{ "providers": { "codebuddy": { "baseUrl": "http://127.0.0.1:20128/v1" } } }"#,
    )
    .unwrap();
    let registry = ModelRegistry::create(auth_with(&serde_json::json!({})), &models_path);
    assert!(registry.get_error().is_none());
    let glm = registry
        .get_all()
        .iter()
        .find(|m| m.provider == "codebuddy" && m.id == "glm-5.2")
        .expect("fork codebuddy model present")
        .clone();
    assert_eq!(glm.base_url, "http://127.0.0.1:20128/v1");
}

#[test]
fn models_json_error_keeps_built_ins() {
    let dir = tempfile::tempdir().unwrap();
    let models_path = dir.path().join("models.json");
    std::fs::write(&models_path, "{ not json").unwrap();
    let registry = ModelRegistry::create(auth_with(&serde_json::json!({})), &models_path);
    assert!(registry.get_error().is_some());
    assert_ne!(registry.get_all().len(), 0);
}

#[test]
fn header_precedence_model_over_provider() {
    let mut registry = ModelRegistry::in_memory(auth_with(&serde_json::json!({
        "anthropic": { "type": "api_key", "key": "sk-ant" }
    })));
    let mut m = model("m", "anthropic");
    m.headers = Some(BTreeMap::from([(
        "X-Model".to_string(),
        "model".to_string(),
    )]));
    let result = registry.get_api_key_and_headers(&m, None);
    assert_eq!(result.api_key.as_deref(), Some("sk-ant"));
    assert_eq!(
        result.headers.as_ref().unwrap().get("X-Model").unwrap(),
        "model"
    );
    // Request headers win over everything.
    let request = BTreeMap::from([("X-Model".to_string(), "request".to_string())]);
    let result = registry.get_api_key_and_headers(&m, Some(&request));
    assert_eq!(
        result.headers.as_ref().unwrap().get("X-Model").unwrap(),
        "request"
    );
}

#[test]
fn live_scope_keyed_cache_merges_over_the_compiled_prime_inference_models() {
    let dir = tempfile::tempdir().unwrap();
    let models_path = dir.path().join("models.json");
    std::fs::write(
        &models_path,
        r#"{ "providers": { "custom": {
            "baseUrl": "https://custom.example", "apiKey": "custom-key",
            "api": "openai-completions", "authHeader": true,
            "models": [ { "id": "my-model" } ]
        } } }"#,
    )
    .unwrap();
    // A live-catalog snapshot scoped to these credentials: one compiled
    // model repriced, one new entry, well past the coverage floor so
    // the build accepts it. The scope-keyed cache serves this scope's
    // snapshot only; another credential never sees it.
    let auth = auth_without_env(&serde_json::json!({
        "prime-inference": { "type": "api_key", "key": "live-key",
            "primeTeam": { "teamId": "team-1", "name": "Team 1" } }
    }));
    let compiled = pa_models::transports::prime_inference_offline_entries();
    let repriced = &compiled[0];
    let mut entries = Vec::new();
    for model in &compiled {
        entries.push(serde_json::json!({
            "id": model.id,
            "display_name": model.name,
            "pricing": {
                "input_usd_per_mtok": if model.id == repriced.id { 7.0 } else { model.cost.input.as_f64() },
                "output_usd_per_mtok": model.cost.output.as_f64(),
            },
            "specs": {
                "context_window": model.context_window,
                "max_output_tokens": model.max_tokens,
                "supports_reasoning": model.reasoning,
                "modalities": { "input": ["text"], "output": ["text"] },
            },
        }));
    }
    entries.push(serde_json::json!({
        "id": "anthropic/live-only-model",
        "display_name": "Live Only Model",
        "pricing": { "input_usd_per_mtok": 1.0, "output_usd_per_mtok": 2.0 },
        "specs": {
            "context_window": 64000, "max_output_tokens": 8192,
            "supports_reasoning": false,
            "modalities": { "input": ["text"], "output": ["text"] },
        },
    }));
    let scope = pa_models::prime_inference::scope_key("live-key", "team-1");
    let snapshot = serde_json::json!({
        "url": format!("{}/models", super::super::prime_inference::PRIME_INFERENCE_BASE_URL),
        "scope": scope,
        "fetchedAt": 1,
        "payload": { "data": entries },
    });
    std::fs::create_dir_all(dir.path().join("models")).unwrap();
    std::fs::write(
        dir.path().join("models/prime-inference-models-cache.json"),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    let mut registry = ModelRegistry::create(auth, &models_path);
    let all = registry.get_all().to_vec();
    // The live repriced model replaced its compiled template (other
    // providers may serve the same id; match the provider too).
    let repriced_model = all
        .iter()
        .find(|model| model.id == repriced.id && model.provider == "prime-inference")
        .expect("repriced model");
    assert_eq!(repriced_model.cost.input.as_f64(), 7.0);
    assert_eq!(
        repriced_model.base_url,
        super::super::prime_inference::PRIME_INFERENCE_BASE_URL
    );
    assert!(all
        .iter()
        .any(|model| model.id == "anthropic/live-only-model"));
    assert!(all.iter().any(|model| model.id == "my-model"));
    assert!(all
        .iter()
        .any(|model| model.provider == "anthropic" && model.id != repriced.id));
    let other_auth = auth_without_env(&serde_json::json!({
        "prime-inference": { "type": "api_key", "key": "other-key",
            "primeTeam": { "teamId": "team-2", "name": "Team 2" } }
    }));
    registry.auth = other_auth;
    registry.refresh();
    let all = registry.get_all().to_vec();
    let compiled_price = compiled
        .iter()
        .find(|model| model.id == repriced.id)
        .unwrap();
    let still_compiled = all
        .iter()
        .find(|model| model.id == repriced.id && model.provider == "prime-inference")
        .expect("compiled fallback for the other scope");
    assert_eq!(
        still_compiled.cost.input.as_f64(),
        compiled_price.cost.input.as_f64()
    );
    assert!(!all
        .iter()
        .any(|model| model.id == "anthropic/live-only-model"));
}

#[test]
fn a_missing_or_corrupt_cache_falls_back_to_the_bundled_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let models_path = dir.path().join("models.json");
    std::fs::write(&models_path, "{ not json").unwrap();
    std::fs::write(
        dir.path().join("prime-inference-models-cache.json"),
        "{ not json",
    )
    .unwrap();
    let registry = ModelRegistry::create(auth_with(&serde_json::json!({})), &models_path);
    assert!(registry
        .get_all()
        .iter()
        .any(|model| model.provider == "prime-inference"));
    assert!(registry.get_error().is_some());
}

#[test]
fn explicit_private_ids_are_authorized() {
    let registry = ModelRegistry::in_memory(auth_with(&serde_json::json!({})));
    let private_model = model("internal/custom-private", "prime-inference");
    assert!(!registry.is_authorized_private_model(&private_model));
}

#[test]
fn a_stored_copilot_credential_rewrites_its_models_base_url() {
    // TS `githubCopilotOAuthProvider.modifyModels`: the token's
    // proxy endpoint wins.
    let auth = auth_without_env(&serde_json::json!({
        "github-copilot": {
            "type": "oauth",
            "access": "tid=1;exp=2;proxy-ep=proxy.enterprise.githubcopilot.com",
            "refresh": "gh", "expires": 4_102_444_800_000i64
        }
    }));
    let registry = ModelRegistry::in_memory(auth);
    let grok = registry
        .get_all()
        .iter()
        .find(|model| model.provider == "github-copilot")
        .expect("the copilot models render");
    assert_eq!(grok.base_url, "https://api.enterprise.githubcopilot.com");
    // An enterprise credential without a proxy endpoint routes onto
    // the enterprise API.
    let auth = auth_without_env(&serde_json::json!({
        "github-copilot": {
            "type": "oauth",
            "access": "plain-token", "refresh": "gh", "expires": 4_102_444_800_000i64,
            "enterpriseUrl": "company.ghe.com"
        }
    }));
    let registry = ModelRegistry::in_memory(auth);
    let grok = registry
        .get_all()
        .iter()
        .find(|model| model.provider == "github-copilot")
        .expect("the copilot models render");
    assert_eq!(grok.base_url, "https://copilot-api.company.ghe.com");
}

#[test]
fn a_stored_xai_subscription_switches_its_models_onto_responses() {
    let auth = auth_without_env(&serde_json::json!({
        "xai": {
            "type": "oauth",
            "access": "grok",
            "refresh": "r",
            "expires": 4_102_444_800_000i64,
        }
    }));
    let registry = ModelRegistry::in_memory(auth);
    let grok = registry
        .get_all()
        .iter()
        .find(|model| model.provider == "xai")
        .expect("the xai model renders");
    assert_eq!(grok.api, "openai-responses");
    assert_eq!(grok.base_url, "https://api.x.ai/v1");
    // The catalog's xai model is non-reasoning: "off" is the only
    // supported level whatever the subscription map nulls (the
    // helper answers the non-reasoning branch first).
    let supported = pa_types::ai::thinking_levels::get_supported_thinking_levels(grok);
    assert_eq!(supported, vec![ModelThinkingLevel::Off]);
    // The compat is the shared-key-only object (TS
    // `supportsLongCacheRetention: false`); the responses provider
    // decodes it directly (its own test covers the decode).
    let compat = grok.compat.as_ref().expect("the compat rides the model");
    assert_eq!(
        compat.raw.get("supportsLongCacheRetention"),
        Some(&serde_json::json!(false))
    );
    // The TS flow's explicit maps stay per model id (unit level;
    // the offline catalog no longer carries those models).
    let map_46 = xai_subscription_thinking_map("grok-4.6");
    assert_eq!(map_46.get(&ModelThinkingLevel::Off), Some(&None));
    assert_eq!(
        map_46.get(&ModelThinkingLevel::Xhigh),
        Some(&Some("xhigh".to_string()))
    );
    let map_43 = xai_subscription_thinking_map("grok-4.3");
    assert_eq!(
        map_43.get(&ModelThinkingLevel::Off),
        Some(&Some("none".to_string()))
    );
}

/// A model that already declares addressable levels keeps them under the
/// subscription — a blanket replacement collapsed a thinking-capable route
/// onto the all-null default arm (the `/effort` false refusal).
#[test]
fn a_subscription_model_keeps_its_own_thinking_level_map() {
    let mut grok = model("grok-build-0.1", "xai");
    grok.reasoning = true;
    grok.thinking_level_map =
        Some(std::iter::once((ModelThinkingLevel::Off, Some("none".to_string()))).collect());
    let adapted = xai_subscription_model(&grok);
    assert_eq!(adapted.thinking_level_map, grok.thinking_level_map);

    let bare = model("grok-4.5", "xai");
    assert!(bare.thinking_level_map.is_none());
    let adapted = xai_subscription_model(&bare);
    assert_eq!(
        adapted.thinking_level_map,
        Some(xai_subscription_thinking_map("grok-4.5"))
    );
}

#[test]
fn without_a_stored_xai_subscription_the_models_stay_on_completions() {
    let registry = ModelRegistry::in_memory(auth_without_env(&serde_json::json!({})));
    let grok = registry
        .get_all()
        .iter()
        .find(|model| model.provider == "xai")
        .expect("the xai model renders");
    assert_eq!(grok.api, "openai-completions");
    assert!(grok.thinking_level_map.is_none());
    assert!(grok.compat.is_none());
}

#[tokio::test]
async fn scope_readiness_timeout_keeps_models_available_before_refresh() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let auth = auth_without_env(&serde_json::json!({
        "prime-inference": {
            "type": "api_key", "key": "fixture-key",
            "primeTeam": {"teamId": "team-1", "name": "Fixture"}
        }
    }));
    let mut registry = ModelRegistry::in_memory(auth);
    registry.catalog = Arc::new(pa_models::ModelCatalog::with_urls(None, None, &url, &url));
    let existing = model("internal/existing", "prime-inference");
    let missing = model("internal/missing", "prime-inference");
    registry.models = vec![existing.clone(), missing];
    registry.authorized_private_ids.insert(existing.id.clone());
    registry.authorized_private_models = vec![existing.clone()];
    registry.authorized_team_id = Some("team-1".to_string());
    let patterns = vec![
        "prime-inference/internal/existing".to_string(),
        "prime-inference/internal/missing".to_string(),
    ];
    let resolved = super::super::session_restore::resolve_model_scope_from_models_with_readiness(
        &mut registry,
        &patterns,
        Some(("prime-inference", "internal/missing")),
        false,
        0,
    )
    .await;
    assert!(
        registry.authorized_private_ids.is_empty(),
        "refresh started before timing out"
    );
    assert_eq!(
        resolved
            .into_iter()
            .map(|entry| entry.model)
            .collect::<Vec<_>>(),
        vec![existing]
    );
}
