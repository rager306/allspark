//! The set-equality oracle end to end (ADR-0001): the real binary's rendered
//! model list carries every model the compiled catalog promises to a
//! credential-free session (the native-free set). A bundled asset that
//! replaces the compiled catalog with a partial snapshot, a discovery filter
//! gone mad, or a registry regression each drop rows — and this test goes
//! red against the binary, not against a re-implementation of it.

use std::process::Command;
use std::sync::Arc;

use pa_core::auth::{AuthStorage, AuthStorageData, NoOAuth};
use pa_core::models::ModelRegistry;

/// The catalog promise: the native-free models a credential-free session can
/// pick (the picker's discovery set under a hermetic registry).
fn promised_native_free_ids() -> Vec<(String, String)> {
    let auth = AuthStorage::in_memory_without_env(&AuthStorageData::default(), Arc::new(NoOAuth));
    let registry = ModelRegistry::in_memory(auth);
    assert!(
        registry.get_error().is_none(),
        "the registry must load without error: {:?}",
        registry.get_error()
    );
    let mut ids: Vec<(String, String)> = registry
        .get_discoverable_models()
        .into_iter()
        .map(|model| (model.provider.clone(), model.id.clone()))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[test]
fn the_binaries_model_list_carries_every_catalog_promise() {
    let promised = promised_native_free_ids();
    assert!(
        !promised.is_empty(),
        "the compiled catalog must promise at least one native-free model"
    );

    let home = tempfile::TempDir::new().expect("sandbox home");
    let cwd = home.path().join("work");
    let agent_dir = home.path().join("agent");
    std::fs::create_dir_all(&cwd).expect("cwd");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");

    // The oracle compares the binary against the COMPILED catalog's promise,
    // so live discovery must not rescue a damaged catalog: the proxies point
    // at a dead local endpoint and every fetch fails fast to the compiled
    // fallback (the exact path a damaged bundled asset rides).
    let output = Command::new(env!("CARGO_BIN_EXE_prime-agent"))
        .args(["model", "list"])
        .env("HOME", home.path())
        .env("PRIME_AGENT_CODING_AGENT_DIR", &agent_dir)
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("https_proxy", "http://127.0.0.1:9")
        .env("http_proxy", "http://127.0.0.1:9")
        .env_remove("PRIME_AGENT_SESSION_DIR")
        .env_remove("PRIME_AGENT_CODING_AGENT_SESSION_DIR")
        .env_remove("PI_OFFLINE")
        .current_dir(&cwd)
        .output()
        .expect("binary present");
    assert!(
        output.status.success(),
        "model list must exit 0, got {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        !stdout.trim().is_empty(),
        "the rendered list must not be empty"
    );

    let missing: Vec<&(String, String)> = promised
        .iter()
        .filter(|(provider, id)| {
            // The table renders the provider and model columns; a row must
            // carry both (the id is unique across the catalog set).
            !(stdout.contains(provider.as_str()) && stdout.contains(id.as_str()))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "the binary's model list dropped catalog-promised models: {missing:?}\n--- rendered ---\n{stdout}"
    );
}
