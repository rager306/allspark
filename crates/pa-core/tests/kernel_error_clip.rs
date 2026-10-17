//! Error-payload clipping at settle: `ename`, `evalue`, and the traceback's
//! joined length cap like result text, so a megabyte `AssertionError` repr
//! cannot ride into the transcript and rebill every turn.
#![cfg(unix)]
// Kernel manager futures follow the crate's existing integration-test disposition.
#![allow(clippy::large_futures)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use pa_core::kernel::manager::{KernelStartOptions, ReplKernelManager};
use pa_core::kernel::shared::{
    ExecuteOptions, ExecuteResult, ExecuteStatus, HostRequestHandlers, KernelError,
    KernelManagerOptions, KernelShutdownOptions,
};
use serde_json::json;

const FAKE_RUNTIME: &str = r#"#!/usr/bin/env python3
import json
import sys

print(json.dumps({"event": "ready", "protocol": 3, "python": "3.13.0"}), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    if req.get("type") == "execute":
        try:
            error = json.loads(req["code"])
        except ValueError:
            error = None
        if isinstance(error, dict):
            frame = {"event": "error", "id": req["id"]}
            frame.update(error)
            print(json.dumps(frame), flush=True)
            print(json.dumps({"event": "done", "id": req.get("id"), "status": "error"}), flush=True)
            continue
    print(json.dumps({"event": "done", "id": req.get("id"), "status": "ok"}), flush=True)
"#;

fn marker(cap: usize) -> String {
    format!("\n[... output truncated at {cap} chars ...]")
}

/// A manager over the fake runtime, plus the temp dir keeping it alive.
fn fake_runtime_manager(session: &str) -> (tempfile::TempDir, ReplKernelManager) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let python = dir.path().join("fake-kernel");
    std::fs::write(&python, FAKE_RUNTIME).expect("write fake runtime");
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let manager = ReplKernelManager::new(KernelManagerOptions {
        python: Some(python),
        cwd: Some(dir.path().to_path_buf()),
        env: HashMap::new(),
        session_id: Some(format!("{session}-error-clip-test")),
        host_handlers: HostRequestHandlers::new(),
        python_skills: Vec::new(),
        snapshot: None,
        bootstrap_code: None,
        stderr_log_path: None,
        on_background_work_settled: None,
    });
    (dir, manager)
}

async fn start(manager: &ReplKernelManager) {
    tokio::time::timeout(
        Duration::from_secs(10),
        manager.start(KernelStartOptions::default()),
    )
    .await
    .expect("fake runtime must become ready")
    .expect("fake runtime must start");
}

/// Execute one cell carrying an error spec (or any other JSON value for a
/// plain ok cell) and await its settled result.
async fn run(
    manager: &ReplKernelManager,
    spec: serde_json::Value,
    max_output_chars: Option<usize>,
) -> ExecuteResult {
    tokio::time::timeout(
        Duration::from_secs(10),
        manager.execute(
            &serde_json::to_string(&spec).expect("encode cell"),
            ExecuteOptions {
                max_output_chars,
                ..ExecuteOptions::default()
            },
        ),
    )
    .await
    .expect("reader must continue serving cells")
    .expect("execute must succeed")
}

async fn shutdown(manager: &ReplKernelManager) {
    tokio::time::timeout(
        Duration::from_secs(10),
        manager.shutdown(KernelShutdownOptions::default()),
    )
    .await
    .expect("shutdown must finish")
    .expect("shutdown must succeed");
}

/// The rebill scenario: a megabyte `AssertionError` repr in `evalue` and the
/// traceback's frame settles clipped to the default cap, and the clipped
/// settle leaves the reader serving later cells.
#[tokio::test]
async fn megabyte_error_repr_clips_at_the_default_cap() {
    let (_dir, manager) = fake_runtime_manager("megabyte");
    start(&manager).await;

    let huge = "é".repeat(70_000);
    let result = run(
        &manager,
        json!({
            "ename": "AssertionError",
            "evalue": huge,
            "traceback": [format!("AssertionError: {huge}")],
        }),
        None,
    )
    .await;
    assert_eq!(result.status, ExecuteStatus::Error);
    assert_eq!(
        result.error,
        Some(KernelError {
            ename: "AssertionError".to_string(),
            evalue: format!("{}{}", "é".repeat(65_536), marker(65_536)),
            traceback: vec![format!(
                "AssertionError: {}{}",
                "é".repeat(65_520),
                marker(65_536)
            )],
        })
    );

    let follow = run(&manager, json!("ok"), None).await;
    assert_eq!(
        (follow.status, follow.result, follow.error),
        (ExecuteStatus::Ok, None, None)
    );
    shutdown(&manager).await;
}

/// An error under the cap settles byte-identical.
#[tokio::test]
async fn small_error_passes_through_untouched() {
    let (_dir, manager) = fake_runtime_manager("small");
    start(&manager).await;

    let result = run(
        &manager,
        json!({"ename": "ValueError", "evalue": "boom", "traceback": ["ab", "cd"]}),
        Some(64),
    )
    .await;
    assert_eq!(result.status, ExecuteStatus::Error);
    assert_eq!(
        result.error,
        Some(KernelError {
            ename: "ValueError".to_string(),
            evalue: "boom".to_string(),
            traceback: vec!["ab".to_string(), "cd".to_string()],
        })
    );
    shutdown(&manager).await;
}

/// `ename` and `evalue` clip like result text, and the traceback clips on
/// its joined length: whole frames while they fit, the crossing frame cut
/// on char boundaries, the rest dropped.
#[tokio::test]
async fn error_fields_and_traceback_clip_on_char_boundaries() {
    let (_dir, manager) = fake_runtime_manager("fields");
    start(&manager).await;

    let result = run(
        &manager,
        json!({"ename": "EEEEEEEEEE", "evalue": "🦀🦀🦀", "traceback": []}),
        Some(4),
    )
    .await;
    assert_eq!(
        result.error,
        Some(KernelError {
            ename: format!("EEEE{}", marker(4)),
            evalue: "🦀🦀🦀".to_string(),
            traceback: Vec::new(),
        })
    );

    let result = run(
        &manager,
        json!({"ename": "E", "evalue": "🦀🦀🦀", "traceback": []}),
        Some(2),
    )
    .await;
    assert_eq!(
        result.error,
        Some(KernelError {
            ename: "E".to_string(),
            evalue: format!("🦀🦀{}", marker(2)),
            traceback: Vec::new(),
        })
    );

    // 4 + 1 + 4 chars fit; the third frame crosses at 10 of 20 scalars.
    let result = run(
        &manager,
        json!({"ename": "E", "evalue": "", "traceback": ["aaaa", "bbbb", "cccccccccccc", "dropped"]}),
        Some(20),
    )
    .await;
    assert_eq!(
        result.error,
        Some(KernelError {
            ename: "E".to_string(),
            evalue: String::new(),
            traceback: vec![
                "aaaa".to_string(),
                "bbbb".to_string(),
                format!("cccccccccc{}", marker(20)),
            ],
        })
    );

    // The cut lands inside a multibyte frame and stays on char boundaries.
    let result = run(
        &manager,
        json!({"ename": "E", "evalue": "", "traceback": ["ééé", "界🦀x"]}),
        Some(5),
    )
    .await;
    assert_eq!(
        result.error,
        Some(KernelError {
            ename: "E".to_string(),
            evalue: String::new(),
            traceback: vec!["ééé".to_string(), format!("界{}", marker(5))],
        })
    );
    shutdown(&manager).await;
}
