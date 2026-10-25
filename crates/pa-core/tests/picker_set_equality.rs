//! The set-equality oracle at the source level (ADR-0001): the picker's model set loads
//! from a hermetic registry, every matrix row cites a path the tree honours, and the
//! `MOCK-path-routed-responses` row names exactly the path-routed oracle tests that exist
//! in `pa-ai/src/testing/mock_server.rs`. This is the governor contract, not the deferred
//! T1a/T1b picker pty e2e.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pa_core::auth::{AuthStorage, AuthStorageData, NoOAuth};
use pa_core::models::ModelRegistry;
use serde_yaml::Value;

/// The repo root sits two levels above `crates/pa-core`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .expect("crates/pa-core sits two levels under the repo root")
}

/// The cited source paths of one row: `axes.code`, like the governor's `row-file-missing`.
fn cited_code_paths(row: &Value) -> Vec<&str> {
    row.get("axes")
        .and_then(|axes| axes.get("code"))
        .and_then(Value::as_sequence)
        .map(|seq| seq.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn picker_set_equality_holds_at_the_source_level() {
    let root = repo_root();

    // 1. The picker's source set loads through the registry (hermetic: no ambient
    //    credential source, no `models.json`), and is a set — no duplicate ids.
    let auth = AuthStorage::in_memory_without_env(&AuthStorageData::default(), Arc::new(NoOAuth));
    let registry = ModelRegistry::in_memory(auth);
    assert!(
        registry.get_error().is_none(),
        "the registry must load without error: {:?}",
        registry.get_error()
    );
    assert!(
        !registry.get_all().is_empty(),
        "the compiled catalog must promise at least one model"
    );
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for model in registry.get_discoverable_models() {
        assert!(
            seen.insert((model.provider.as_str(), model.id.as_str())),
            "duplicate discoverable model {}/{}",
            model.provider,
            model.id
        );
    }

    // 2. The matrix parses.
    let matrix_path = root.join("doc/testing/test-matrix.yaml");
    let matrix_text = std::fs::read_to_string(&matrix_path).expect("read test-matrix.yaml");
    let document: Value = serde_yaml::from_str(&matrix_text).expect("test-matrix.yaml parses");
    let rows = document
        .get("rows")
        .and_then(Value::as_sequence)
        .expect("test-matrix.yaml has a rows list");

    // 3. Every row citing a code axis honours at least one path in the tree (`doc/`
    //    contract paths are exempt, same rule as the governor's `row-file-missing`).
    let mut absent: Vec<String> = Vec::new();
    for row in rows {
        let cited = cited_code_paths(row);
        if cited.is_empty() {
            continue;
        }
        let id = row.get("id").and_then(Value::as_str).unwrap_or("<missing id>");
        let honoured = cited
            .iter()
            .any(|path| path.starts_with("doc/") || root.join(path).exists());
        if !honoured {
            absent.push(format!("{id} cites none of {cited:?}"));
        }
    }
    assert!(
        absent.is_empty(),
        "matrix rows whose code axis exists nowhere:\n{}",
        absent.join("\n")
    );

    // 4. The path-routed oracle row cites exactly the tests the mock server defines.
    const MOCK_ROW_ID: &str = "MOCK-path-routed-responses";
    const MOCK_SERVER: &str = "crates/pa-ai/src/testing/mock_server.rs";
    const PATH_ROUTED_ORACLES: [&str; 4] = [
        "a_path_keyed_response_is_served_only_to_that_path",
        "a_path_queue_that_runs_dry_falls_back_to_the_global_queue",
        "the_key_is_the_path_so_a_query_string_does_not_claim_a_keyed_response",
        "an_unkeyed_path_falls_through_to_the_global_queue",
    ];
    let mock_row = rows
        .iter()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(MOCK_ROW_ID))
        .unwrap_or_else(|| panic!("{MOCK_ROW_ID} row is missing from the matrix"));
    let cited_oracles: Vec<&str> = mock_row
        .get("oracle")
        .and_then(Value::as_sequence)
        .map(|seq| seq.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for name in PATH_ROUTED_ORACLES {
        assert!(
            cited_oracles.contains(&name),
            "{MOCK_ROW_ID} must cite oracle {name}"
        );
    }
    let server = std::fs::read_to_string(root.join(MOCK_SERVER)).expect("read mock_server.rs");
    for name in PATH_ROUTED_ORACLES {
        assert!(
            server.contains(&format!("fn {name}")),
            "{MOCK_SERVER} must define the path-routed oracle test fn {name}"
        );
    }
}
