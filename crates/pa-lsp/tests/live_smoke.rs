//! Live smoke test: spawn a real language server and drive the client end to
//! end (open a document, wait for `publishDiagnostics`).
//!
//! Ignored by default: it needs an installed server binary and spawns a child
//! process, so the hermetic unit/contract suites stay dependency-free. Run with:
//!
//! ```text
//! PA_LSP_LIVE=1 cargo test -p pa-lsp --test live_smoke -- --ignored --nocapture
//! ```

use std::time::Duration;

use pa_agent::types::AgentTool as _;
use pa_lsp::LspClient;

/// A TypeScript project whose two type errors a language server must publish.
const PACKAGE_JSON: &str = "{\"name\":\"pa_lsp_smoke\",\"version\":\"1.0.0\"}\n";
const TSCONFIG: &str = "{\"compilerOptions\":{\"strict\":true,\"noEmit\":true}}\n";
const BROKEN_TS: &str = "const x: number = \"nope\";\nlet y: string = 42;\n";

#[tokio::test]
#[ignore = "spawns typescript-language-server; run with PA_LSP_LIVE=1 and --ignored"]
async fn typescript_server_reports_type_errors() {
    if std::env::var_os("PA_LSP_LIVE").is_none() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live smoke test");
        return;
    }

    let server = pa_lsp::SERVERS
        .iter()
        .find(|server| server.name == "typescript-language-server")
        .expect("typescript-language-server entry");

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("package.json"), PACKAGE_JSON).unwrap();
    std::fs::write(dir.path().join("tsconfig.json"), TSCONFIG).unwrap();
    // typescript-language-server needs a local `typescript`; link the global one
    // when the caller points `PA_LSP_TYPESCRIPT` at it.
    #[cfg(unix)]
    if let Some(ts) = std::env::var_os("PA_LSP_TYPESCRIPT") {
        let node_modules = dir.path().join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();
        let _ = std::os::unix::fs::symlink(&ts, node_modules.join("typescript"));
    }
    let file = dir.path().join("bad.ts");
    std::fs::write(&file, BROKEN_TS).unwrap();

    let root = pa_lsp::project_root(&file, server);
    let client = LspClient::spawn(server, &root)
        .await
        .expect("spawn typescript-language-server");

    let opened = client.ensure_open(&file).await.expect("didOpen");
    assert!(opened.as_str().starts_with("file://"), "got {opened}");

    let (diagnostics, generation) = client
        .wait_for_diagnostics(&file, Duration::from_secs(30))
        .await
        .expect("a diagnostic within 30s");
    assert!(generation >= 1, "the publish generation must advance");
    assert!(
        !diagnostics.is_empty(),
        "the type errors must produce at least one diagnostic"
    );
    eprintln!("live diagnostics: {diagnostics:#?}");

    // A well-typed sibling must publish an empty set.
    let good = dir.path().join("good.ts");
    std::fs::write(&good, "const ok: number = 1;\n").unwrap();
    client.ensure_open(&good).await.expect("didOpen good");
    if let Some((diagnostics, _)) = client
        .wait_for_diagnostics(&good, Duration::from_secs(30))
        .await
    {
        assert!(
            diagnostics.is_empty(),
            "a well-typed file must be diagnostic-free: {diagnostics:#?}"
        );
    }

    client.shutdown().await;
}

/// Drive the full tool path (`LspManager` + `LspTool::execute`) against the
/// same broken project, proving the `diagnostics` action returns the compiler
/// error through the `AgentTool` contract.
#[tokio::test]
#[ignore = "spawns typescript-language-server; run with PA_LSP_LIVE=1 and --ignored"]
async fn tool_reports_diagnostics_for_a_file() {
    if std::env::var_os("PA_LSP_LIVE").is_none() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live smoke test");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("package.json"), PACKAGE_JSON).unwrap();
    std::fs::write(dir.path().join("tsconfig.json"), TSCONFIG).unwrap();
    let file = dir.path().join("bad.ts");
    std::fs::write(&file, BROKEN_TS).unwrap();
    #[cfg(unix)]
    if let Some(ts) = std::env::var_os("PA_LSP_TYPESCRIPT") {
        let node_modules = dir.path().join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();
        let _ = std::os::unix::fs::symlink(&ts, node_modules.join("typescript"));
    }

    let manager = std::sync::Arc::new(pa_lsp::LspManager::new());
    let tool = std::sync::Arc::new(pa_lsp::LspTool::new(
        manager.clone(),
        dir.path().to_path_buf(),
    ));

    // Warm the client so diagnostics have a server to come from.
    let warm = tool
        .clone()
        .execute(
            "call-1".to_string(),
            serde_json::json!({ "action": "symbols", "file": "bad.ts" }),
            pa_agent::abort::AbortSignal::default(),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .expect("symbols action");
    assert!(!warm.is_error, "symbols must not be an error: {warm:?}");

    let result = tool
        .clone()
        .execute(
            "call-2".to_string(),
            serde_json::json!({ "action": "diagnostics", "file": "bad.ts", "timeout": 30 }),
            pa_agent::abort::AbortSignal::default(),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .expect("diagnostics action");
    assert!(!result.is_error, "diagnostics must not be an error");
    let text = match &result.content[0] {
        pa_agent::types::ToolResultContent::Text(text) => text.text.clone(),
        other @ pa_agent::types::ToolResultContent::Image(_) => {
            panic!("expected text content, got {other:?}")
        }
    };
    eprintln!("tool diagnostics output:\n{text}");
    assert!(
        text.contains("not assignable"),
        "the compiler error must appear in the tool output: {text}"
    );

    manager.shutdown_all().await;
}

#[test]
fn live_test_helpers_compile() {
    // Keeps the non-ignored run honest: the fixtures are exercised for
    // compilation even when the live test is skipped.
    assert!(BROKEN_TS.contains("nope"));
    assert!(PACKAGE_JSON.contains("pa_lsp_smoke"));
}

/// Drive the LSP-backed symbol-layer actions (`find_referencing_symbols`,
/// `find_implementations`, `capabilities`) against the broken TS project.
#[tokio::test]
#[ignore = "spawns typescript-language-server; run with PA_LSP_LIVE=1 and --ignored"]
async fn symbol_layer_lsp_actions_work() {
    if std::env::var_os("PA_LSP_LIVE").is_none() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live smoke test");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("package.json"), PACKAGE_JSON).unwrap();
    std::fs::write(dir.path().join("tsconfig.json"), TSCONFIG).unwrap();
    #[cfg(unix)]
    if let Some(ts) = std::env::var_os("PA_LSP_TYPESCRIPT") {
        let node_modules = dir.path().join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();
        let _ = std::os::unix::fs::symlink(&ts, node_modules.join("typescript"));
    }
    let source = "export interface Shape { area(): number }\n\
                  export function useShape(s: Shape): number { return s.area(); }\n";
    std::fs::write(dir.path().join("shape.ts"), source).unwrap();

    let manager = std::sync::Arc::new(pa_lsp::LspManager::new());
    let tool = std::sync::Arc::new(pa_lsp::LspTool::new(
        manager.clone(),
        dir.path().to_path_buf(),
    ));

    // capabilities: spawns the client and returns its advertised capabilities.
    let caps = tool
        .clone()
        .execute(
            "caps".to_string(),
            serde_json::json!({ "action": "capabilities", "file": "shape.ts" }),
            pa_agent::abort::AbortSignal::default(),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .expect("capabilities action");
    assert!(!caps.is_error, "capabilities must not error: {caps:?}");

    // find_referencing_symbols on `area` should find the use in `useShape`.
    let refs = tool
        .clone()
        .execute(
            "refs".to_string(),
            serde_json::json!({
                "action": "find_referencing_symbols",
                "file": "shape.ts",
                "symbol": "Shape/area",
                "timeout": 30
            }),
            pa_agent::abort::AbortSignal::default(),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .expect("find_referencing_symbols action");
    assert!(!refs.is_error, "references must not error: {refs:?}");

    manager.shutdown_all().await;
}
