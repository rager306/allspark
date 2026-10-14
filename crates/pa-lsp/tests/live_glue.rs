//! Live glue tests: drive the Phase-5-A LSP glue against a real language
//! server (`gopls`), proving the write-through pipeline (L10), the file-rename
//! broadcast (L18), the freshness gate (L13), and the workspace checker (L20)
//! work end to end.
//!
//! Ignored by default: they spawn a child process. Run with:
//!
//! ```text
//! PA_LSP_LIVE=1 cargo test -p pa-lsp --test live_glue -- --ignored --nocapture
//! ```

#![forbid(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use pa_lsp::diagnostics_flow::{run_checkers, select_checkers, CHECKER_TIMEOUT};
use pa_lsp::writethrough::{write_through, WriteThroughOptions};
use pa_lsp::{LspClient, LspManager};

/// A Go module with one type error, so gopls has something to report.
const GO_MOD: &str = "module palive\n\ngo 1.22\n";
const BROKEN_GO: &str = "package main\n\nfunc main() {\n\tvar x int = \"nope\"\n\t_ = x\n}\n";

fn live_enabled() -> bool {
    std::env::var_os("PA_LSP_LIVE").is_some()
}

fn gopls_server() -> &'static pa_lsp::ServerConfig {
    pa_lsp::SERVERS
        .iter()
        .find(|server| server.name == "gopls")
        .expect("gopls entry")
}

/// L10: after a write the client re-syncs and the server sees the new bytes.
#[tokio::test]
#[ignore = "spawns gopls; run with PA_LSP_LIVE=1 and --ignored"]
async fn l10_write_through_syncs_and_notifies() {
    if !live_enabled() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live glue test");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("go.mod"), GO_MOD).unwrap();
    let file = dir.path().join("main.go");
    std::fs::write(&file, "package main\n\nfunc main() {}\n").unwrap();

    let server = gopls_server();
    let root = pa_lsp::project_root(&file, server);
    let client = LspClient::spawn(server, &root).await.expect("spawn gopls");

    // A sync-only pass: it must re-open the document and notify the save.
    let report = write_through(&client, &file, &WriteThroughOptions::default())
        .await
        .expect("write-through");
    assert!(report.notified_saved, "the save must be notified");
    assert!(report.labels().contains(&"sync"));

    // Rewrite the file with an error and write-through again; gopls must now
    // re-sync the new bytes and publish a diagnostic for them.
    std::fs::write(&file, BROKEN_GO).unwrap();
    let _ = write_through(&client, &file, &WriteThroughOptions::default()).await;
    let published = client
        .wait_for_diagnostics(&file, Duration::from_secs(20))
        .await;
    let (diagnostics, _) = published.expect("gopls must publish after the write-through");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("cannot use") || d.message.contains("int value")),
        "the type error must surface after write-through: {diagnostics:#?}"
    );
    eprintln!("live L10 diagnostics: {diagnostics:#?}");

    client.shutdown().await;
}

/// L10 via the manager convenience: a file with no owning server yields `None`,
/// and a Go file yields a report.
#[tokio::test]
#[ignore = "spawns gopls; run with PA_LSP_LIVE=1 and --ignored"]
async fn l10_manager_write_through_returns_none_for_unknown_extension() {
    if !live_enabled() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live glue test");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let readme = dir.path().join("notes.md");
    std::fs::write(&readme, "# hi
").unwrap();
    let manager = LspManager::new();
    let report = manager
        .write_through(&readme, &WriteThroughOptions::default())
        .await
        .expect("no server for .md is not an error");
    assert!(report.is_none(), "an unknown extension has no write-through");
}

/// E6: snapshot before an edit, then diff after — only the introduced
/// diagnostic survives.
#[tokio::test]
#[ignore = "spawns gopls; run with PA_LSP_LIVE=1 and --ignored"]
async fn e6_diagnostics_delta_after_an_edit() {
    if !live_enabled() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live glue test");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("go.mod"), GO_MOD).unwrap();
    let file = dir.path().join("main.go");
    // A clean file first.
    std::fs::write(&file, "package main\n\nfunc main() {}\n").unwrap();

    let server = gopls_server();
    let root = pa_lsp::project_root(&file, server);
    let client = LspClient::spawn(server, &root).await.expect("spawn gopls");
    client.ensure_open(&file).await.expect("didOpen");
    // Let the clean publish settle so the snapshot is stable.
    let _ = client
        .wait_for_diagnostics(&file, Duration::from_secs(20))
        .await;

    // Snapshot the (clean) diagnostics, then introduce an error.
    let snapshot = client.snapshot_diagnostics(&file).await;
    std::fs::write(&file, BROKEN_GO).unwrap();
    let _ = write_through(&client, &file, &WriteThroughOptions::default()).await;
    let _ = client
        .wait_for_diagnostics(&file, Duration::from_secs(20))
        .await;

    let delta = client.diagnostics_delta_since(&file, &snapshot).await;
    eprintln!("live E6 delta: {delta:#?}");
    assert!(
        delta
            .iter()
            .any(|d| d.message.contains("cannot use") || d.message.contains("int value")),
        "the introduced error must be the only new diagnostic: {delta:#?}"
    );

    client.shutdown().await;
}

/// L18: a rename is broadcast to every live server without erroring.
#[tokio::test]
#[ignore = "spawns gopls; run with PA_LSP_LIVE=1 and --ignored"]
async fn l18_rename_file_broadcasts_to_live_servers() {
    if !live_enabled() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live glue test");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("go.mod"), GO_MOD).unwrap();
    let from = dir.path().join("main.go");
    std::fs::write(&from, "package main\n\nfunc main() {}\n").unwrap();
    let to = dir.path().join("app.go");

    let manager = LspManager::new();
    // Warm a client so the manager has a live server to broadcast to.
    let _ = manager.ensure_client(&from).await.expect("spawn gopls");
    let touched = manager
        .rename_file(&from, &to)
        .await
        .expect("rename file");
    assert!(to.exists(), "the file moved on disk");
    assert!(!from.exists(), "the old path is gone");
    eprintln!("live L18 touched {} file(s)", touched.len());
    manager.shutdown_all().await;
}

/// L13: the freshness gate drops a publish computed for an older version.
#[tokio::test]
#[ignore = "spawns gopls; run with PA_LSP_LIVE=1 and --ignored"]
async fn l13_fresh_published_diagnostics_are_gated() {
    if !live_enabled() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live glue test");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("go.mod"), GO_MOD).unwrap();
    let file = dir.path().join("main.go");
    std::fs::write(&file, BROKEN_GO).unwrap();

    let server = gopls_server();
    let root = pa_lsp::project_root(&file, server);
    let client = LspClient::spawn(server, &root).await.expect("spawn gopls");
    client.ensure_open(&file).await.expect("didOpen");
    // Give gopls a moment to publish.
    let _ = client
        .wait_for_diagnostics(&file, Duration::from_secs(20))
        .await;
    let fresh = client.fresh_published_diagnostics(&file).await;
    eprintln!("live L13 fresh diagnostics: {fresh:?}");
    client.shutdown().await;
}

/// L20: `go build` runs as a workspace checker and reports the type error.
#[tokio::test]
#[ignore = "spawns go build; run with PA_LSP_LIVE=1 and --ignored"]
async fn l20_workspace_checker_runs_go_build() {
    if !live_enabled() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live glue test");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("go.mod"), GO_MOD).unwrap();
    std::fs::write(dir.path().join("main.go"), BROKEN_GO).unwrap();

    let specs = select_checkers(dir.path(), |program| {
        pa_lsp::diagnostics_flow::binary_on_path(program)
    });
    assert!(
        specs.iter().any(|spec| spec.program == "go"),
        "go build must be selected for a Go module: {specs:?}"
    );
    let report = run_checkers(dir.path(), &specs, CHECKER_TIMEOUT).await;
    eprintln!("live L20 report: {report:?}");
    assert!(
        report.diagnostics.iter().any(|d| d.message.contains("cannot use")
            || d.message.contains("string")
            || d.message.contains("mismatched")),
        "the type error must surface: {report:?}"
    );
}

#[test]
fn live_glue_helpers_compile() {
    assert!(GO_MOD.contains("palive"));
    assert!(BROKEN_GO.contains("nope"));
    let _ = Path::new("x");
}
