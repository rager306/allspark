//! Headless e2e for the FORK-ONLY status-line facts: the tray's `cache N%` segment
//! and the `N since compact` counter, both over the real UI loop and a scripted
//! daemon. The cache rate comes from `get_cache_stats` — the payload `/cache`
//! renders — and the counter from the attach state, so these frames prove the whole
//! wire path, not just the pure renderer.
#![cfg(unix)]
// Casts: structurally bounded terminal-layout arithmetic; guarded conversions add panic paths.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
// Render routes are flat tables (one arm per route); splitting adds indirection.
#![allow(clippy::too_many_lines)]
// Widget state structs carry independent flag bits.
#![allow(clippy::struct_excessive_bools, clippy::fn_params_excessive_bools)]
// Futures are bounded by the surface's lifetime; boxing adds a steady-state allocation.
#![allow(clippy::large_futures)]
// The wrappers preserve a uniform Result-returning API surface.
#![allow(clippy::unnecessary_wraps)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use pa_tui::interactive::{
    run_interactive, HeadlessPlan, HeadlessStep, InteractiveOptions, ModelSelection,
    SessionSelection, UiMode,
};
use serde_json::{json, Value};

/// What the scripted daemon reports for this session's cache traffic. The tray's
/// segment must read exactly this fold.
const CACHE_MODELS: &str = r#"[{"provider": "anthropic", "id": "claude-opus-4-6",
    "input": 1000, "cacheRead": 9000, "cacheWrite": 0, "totalTokens": 10000,
    "cost": {"input": 0.005, "cacheRead": 0.0045, "cacheWrite": 0.0, "total": 0.0095}}]"#;

struct MockSupervisor {
    listener: UnixListener,
}

impl MockSupervisor {
    fn bind(socket: &std::path::Path) -> Self {
        MockSupervisor {
            listener: UnixListener::bind(socket).expect("bind mock socket"),
        }
    }

    /// Serve one connection: answer the tray's `get_cache_stats` ask and one prompt.
    fn serve(self) {
        let (stream, _) = self.listener.accept().expect("accept");
        let write_stream = stream.try_clone().expect("clone mock socket");
        let mut writer = write_stream;
        let mut reader = BufReader::new(stream);

        let hello = json!({
            "type": "daemon_hello",
            "protocol": { "name": "prime-agent.daemon", "version": 7 },
            "serverCapabilities": [],
            "clientId": "mock",
        });
        write_json(&mut writer, &hello);

        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(envelope) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let id = envelope.get("id").and_then(Value::as_str).unwrap_or("");
            let command = envelope.get("command").cloned().unwrap_or(Value::Null);
            let command_type = command
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let data = match command_type.as_str() {
                "create" => json!({
                    "activeSessionId": "s1",
                    "id": "s1",
                    "sessionId": "sess-1",
                    "sessionFile": "/tmp/sess-1.jsonl",
                }),
                "attach" => {
                    write_json(&mut writer, &attach_data(id));
                    continue;
                }
                // The SAME payload `/cache` renders: the tray folds it with the panel's
                // own `cache_totals`.
                "get_cache_stats" => json!({
                    "models": serde_json::from_str::<Value>(CACHE_MODELS).expect("fixture"),
                }),
                _ => json!({}),
            };
            write_json(
                &mut writer,
                &json!({
                    "type": "response",
                    "id": id,
                    "command": command_type,
                    "success": true,
                    "data": data,
                }),
            );
        }
    }
}

fn write_json(writer: &mut UnixStream, value: &Value) {
    let mut line = serde_json::to_string(value).expect("serialize mock frame");
    line.push('\n');
    writer.write_all(line.as_bytes()).expect("write mock frame");
    writer.flush().expect("flush mock frame");
}

/// The slim attach result: one empty session whose state carries the model, the
/// context usage, and the post-compaction turn count.
fn attach_data(id: &str) -> Value {
    json!({
        "type": "response",
        "id": id,
        "command": "attach",
        "success": true,
        "data": {
            "protocol": { "name": "prime-agent.daemon", "version": 7 },
            "activeSessionId": "s1",
            "snapshot": {
                "activeSessionId": "s1",
                "summary": { "id": "s1", "cwd": "/tmp" },
                "state": {
                    "activeSessionId": "s1",
                    "cwd": "/tmp",
                    "sessionId": "sess-1",
                    "sessionName": "statusline",
                    "model": { "id": "claude-opus-4-6", "provider": "anthropic", "reasoning": false },
                    "thinkingLevel": "default",
                    "contextUsage": { "tokens": 92_400, "contextWindow": 200_000, "percent": 46.2 },
                    "turnsSinceCompaction": 7,
                    "isStreaming": false,
                    "isCompacting": false,
                    "sessionActions": { "queuedCount": 0, "steering": [], "followUps": [] },
                },
                "messages": [],
                "lastEventSequence": 0,
                "lastEventCursor": null,
            },
            "client": { "id": "mock", "capabilities": [] },
            "lastEventSequence": 0,
            "lastEventCursor": null,
        },
    })
}

fn options(socket: PathBuf) -> InteractiveOptions {
    InteractiveOptions {
        models: None,
        socket_path: socket,
        cwd: PathBuf::from("/tmp"),
        session_dir: None,
        script_path: None,
        model_selection: ModelSelection::default(),
        model_catalog: Vec::new(),
        model_configured_providers: std::collections::HashSet::default(),
        model_recent_models: Vec::new(),
        default_thinking_level: None,
        no_session: false,
        session: SessionSelection::New,
        initial_message: None,
        show_images: true,
        fullscreen_mouse: true,
        theme: "prime".to_string(),
        code_block_indent: "  ".to_string(),
        tree_filter_mode: String::new(),
        branch_summary_skip_prompt: false,
        version: "0.0.0".to_string(),
        onboarding: None,
        telemetry_disabled: None,
        client_auth: None,
        traces: None,
        provider_auth: None,
        update_commands: None,
        telemetry: None,
        keybindings: pa_tui::keybindings::KeybindingsManager::new(),
        session_rlm_depth: None,
        prompt_stash: std::sync::Arc::default(),
        session_has_children: false,
        restore_dock_focus: false,
        client_settings: None,
    }
}

fn run_plan(steps: Vec<HeadlessStep>) -> Vec<String> {
    std::env::remove_var("TMUX");
    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket = dir.path().join("tui.sock");
    let supervisor = MockSupervisor::bind(&socket);
    let handle = std::thread::spawn(move || supervisor.serve());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let plan = HeadlessPlan {
        steps,
        width: 100,
        height: 30,
    };
    let outcome = runtime
        .block_on(run_interactive(options(socket), UiMode::Headless(plan)))
        .expect("interactive run");
    handle.join().expect("mock supervisor finished");
    outcome.frames
}

/// The tray's whole readout over the wire: the model, the context segment, the
/// cache rate from `get_cache_stats`, and the post-compaction turn count from the
/// attach state.
#[test]
fn the_tray_reads_the_cache_rate_and_the_compaction_count() {
    let steps = vec![HeadlessStep::WaitMs(400)];
    let frames = run_plan(steps);
    assert!(!frames.is_empty(), "frames were captured");
    let all = frames.join("\n");
    assert!(
        all.contains(
            "claude-opus-4-6 \u{00b7} 92k (46%) \u{00b7} cache 90% \u{00b7} 7 since compact"
        ),
        "the tray carries the cache rate and the compaction count: {all}"
    );
}

/// A session with no cache traffic and no compaction renders neither fork segment:
/// `cache 0%` would dress a non-measurement as a measurement, and the turn count
/// would just repeat the transcript.
#[test]
fn a_quiet_session_renders_no_cache_or_compaction_segment() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket = dir.path().join("tui.sock");
    let listener = UnixListener::bind(&socket).expect("bind mock socket");
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let write_stream = stream.try_clone().expect("clone mock socket");
        let mut writer = write_stream;
        let mut reader = BufReader::new(stream);
        write_json(
            &mut writer,
            &json!({
                "type": "daemon_hello",
                "protocol": { "name": "prime-agent.daemon", "version": 7 },
                "serverCapabilities": [],
                "clientId": "mock",
            }),
        );
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(envelope) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let id = envelope.get("id").and_then(Value::as_str).unwrap_or("");
            let command = envelope.get("command").cloned().unwrap_or(Value::Null);
            let command_type = command
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if command_type == "attach" {
                let mut attach = attach_data(id);
                attach["data"]["snapshot"]["state"]
                    .as_object_mut()
                    .expect("state")
                    .remove("turnsSinceCompaction");
                write_json(&mut writer, &attach);
                continue;
            }
            let data = match command_type.as_str() {
                "create" => json!({
                    "activeSessionId": "s1",
                    "id": "s1",
                    "sessionId": "sess-1",
                    "sessionFile": "/tmp/sess-1.jsonl",
                }),
                // Cache writes but no reads: a real zero rate, which the tray must
                // HIDE, not render as `cache 0%`.
                "get_cache_stats" => json!({
                    "models": [{
                        "provider": "anthropic", "id": "claude-opus-4-6",
                        "input": 0, "cacheRead": 0, "cacheWrite": 4000,
                        "totalTokens": 4000,
                        "cost": {"input": 0, "cacheRead": 0, "cacheWrite": 0.02, "total": 0.02}
                    }],
                }),
                _ => json!({}),
            };
            write_json(
                &mut writer,
                &json!({
                    "type": "response",
                    "id": id,
                    "command": command_type,
                    "success": true,
                    "data": data,
                }),
            );
        }
    });
    std::env::remove_var("TMUX");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let plan = HeadlessPlan {
        steps: vec![HeadlessStep::WaitMs(400)],
        width: 100,
        height: 30,
    };
    let outcome = runtime
        .block_on(run_interactive(options(socket), UiMode::Headless(plan)))
        .expect("interactive run");
    handle.join().expect("mock supervisor finished");
    let all = outcome.frames.join("\n");
    assert!(
        all.contains("92k (46%)"),
        "the context segment renders: {all}"
    );
    assert!(
        !all.contains("cache "),
        "no cache segment without traffic: {all}"
    );
    assert!(
        !all.contains("since compact"),
        "no compaction count before the first compaction: {all}"
    );
}
