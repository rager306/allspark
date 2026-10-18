//! Headless e2e for the FORK-ONLY `/admission` command (ADR
//! `ratelimit-admission`, M5): the command asks the daemon for the
//! provider-admission snapshot over the wire (a readonly
//! `provider_admission` action with no worker identity), renders the
//! per-(provider, model) table in the read-only info panel, and ESC
//! closes it. Usage errors stay out of the panel.
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
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pa_tui::interactive::{
    run_interactive, HeadlessPlan, HeadlessStep, InteractiveOptions, ModelSelection,
    SessionSelection, UiMode,
};
use serde_json::{json, Value};

/// Every `provider_admission` command the mock served, in arrival order.
type SeenAdmissionCommands = Arc<Mutex<Vec<Value>>>;

struct MockSupervisor {
    listener: UnixListener,
}

impl MockSupervisor {
    fn bind(socket: &std::path::Path) -> Self {
        MockSupervisor {
            listener: UnixListener::bind(socket).expect("bind mock socket"),
        }
    }

    /// Serve one connection; the `provider_admission` asks are recorded and
    /// answered with a fixed snapshot payload.
    fn serve(self, seen: &SeenAdmissionCommands) {
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
            match command_type.as_str() {
                "create" => {
                    write_json(
                        &mut writer,
                        &json!({
                            "type": "response",
                            "id": id,
                            "command": "create",
                            "success": true,
                            "data": {
                                "activeSessionId": "s1",
                                "id": "s1",
                                "sessionId": "sess-1",
                                "sessionFile": "/tmp/sess-1.jsonl",
                            },
                        }),
                    );
                }
                "get_commands" => {
                    write_json(
                        &mut writer,
                        &json!({
                            "type": "response",
                            "id": id,
                            "command": "get_commands",
                            "success": true,
                            "data": { "commands": [] },
                        }),
                    );
                }
                "attach" => {
                    write_json(&mut writer, &attach_data(id));
                }
                "get_session_stats" => {
                    write_json(
                        &mut writer,
                        &json!({
                            "type": "response",
                            "id": id,
                            "command": "get_session_stats",
                            "success": true,
                            "data": {
                                "contextUsage": { "tokens": 1200, "contextWindow": 200_000 },
                                "cost": 0.01,
                            },
                        }),
                    );
                }
                "provider_admission" => {
                    seen.lock()
                        .expect("admission command log")
                        .push(command.clone());
                    write_json(
                        &mut writer,
                        &json!({
                            "type": "response",
                            "id": id,
                            "command": "provider_admission",
                            "success": true,
                            "data": {
                                "providers": [
                                    {"provider": "zai", "model": "glm-5.3",
                                     "currentLimit": 4, "inFlight": 2, "ceiling": 8,
                                     "streak429": 1, "queueDepth": 3,
                                     "breakerState": "half_open",
                                     "budgetFailures": 2, "budgetSamples": 20},
                                    {"provider": "openai", "model": "gpt-5.6-sol",
                                     "currentLimit": 8, "inFlight": 8, "ceiling": 8,
                                     "streak429": 5, "queueDepth": 2,
                                     "breakerState": "open",
                                     "budgetFailures": 9, "budgetSamples": 10}
                                ],
                                "queuedTotal": 5
                            },
                        }),
                    );
                }
                "detach" => {
                    write_json(
                        &mut writer,
                        &json!({
                            "type": "response",
                            "id": id,
                            "command": "detach",
                            "success": true,
                        }),
                    );
                }
                _ => {
                    write_json(
                        &mut writer,
                        &json!({
                            "type": "response",
                            "id": id,
                            "command": command_type,
                            "success": true,
                            "data": {},
                        }),
                    );
                }
            }
        }
    }
}

fn write_json(writer: &mut UnixStream, value: &Value) {
    let mut line = serde_json::to_string(value).expect("serialize mock frame");
    line.push('\n');
    writer.write_all(line.as_bytes()).expect("write mock frame");
    writer.flush().expect("flush mock frame");
}

/// The slim attach result: one empty session.
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
                    "sessionName": "admission session",
                    "model": null,
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

fn run_plan(steps: Vec<HeadlessStep>) -> (Vec<String>, SeenAdmissionCommands) {
    std::env::remove_var("TMUX");
    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket = dir.path().join("tui.sock");
    let supervisor = MockSupervisor::bind(&socket);
    let seen: SeenAdmissionCommands = Arc::default();
    let seen_thread = Arc::clone(&seen);
    let handle = std::thread::spawn(move || supervisor.serve(&seen_thread));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let plan = HeadlessPlan {
        steps,
        width: 110,
        height: 30,
    };
    let outcome = runtime
        .block_on(run_interactive(options(socket), UiMode::Headless(plan)))
        .expect("interactive run");
    handle.join().expect("mock supervisor finished");
    (outcome.frames, seen)
}

fn submit(text: &str) -> HeadlessStep {
    HeadlessStep::Submit(text.to_string())
}

fn key(code: KeyCode) -> HeadlessStep {
    HeadlessStep::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn wait_render(needle: &str) -> HeadlessStep {
    HeadlessStep::WaitRender {
        needle: needle.to_string(),
        timeout_ms: 30_000,
    }
}

fn wait_gone(needle: &str) -> HeadlessStep {
    HeadlessStep::WaitGone {
        needle: needle.to_string(),
        timeout_ms: 30_000,
    }
}

/// The panel's key hint (the shared docked-panel grammar): the scroll keys and the close key.
const PANEL_HINT: &str = "Esc close";

/// `/admission` asks the daemon for the readonly snapshot (no worker
/// identity), renders the per-(provider, model) table in the info panel,
/// and ESC closes it without transcript rows.
#[test]
fn admission_command_fetches_snapshot_and_opens_the_panel() {
    let steps = vec![
        submit("/admission"),
        wait_render("Provider Admission"),
        wait_render("zai/glm-5.3  limit=4  in_flight=2  ceiling=8  429_streak=1  queue_depth=3  breaker=half_open  budget=2/20"),
        wait_render("Queued: 5 waiting across providers"),
        key(KeyCode::Esc),
        wait_gone(PANEL_HINT),
    ];
    let (frames, seen) = run_plan(steps);
    assert!(!frames.is_empty(), "frames were captured");
    let rendered = frames.join("\n");
    assert!(
        rendered.contains("openai/gpt-5.6-sol  limit=8  in_flight=8  ceiling=8  429_streak=5  queue_depth=2  breaker=open"),
        "both provider rows rendered:\n{rendered}"
    );

    // The wire ask is the readonly snapshot: the action reads `snapshot`
    // and carries no worker identity.
    let commands = seen.lock().expect("admission command log").clone();
    assert_eq!(commands.len(), 1, "one snapshot ask: {commands:?}");
    assert_eq!(
        commands[0].get("action").and_then(Value::as_str),
        Some("snapshot"),
        "the ask is the readonly snapshot: {commands:?}"
    );
    assert_eq!(
        commands[0].get("workerToken").and_then(Value::as_str),
        Some(""),
        "the client asks without worker identity: {commands:?}"
    );
}

/// A trailing argument is a usage error, not a panel: the draft restores
/// and the error row names the command.
#[test]
fn admission_command_rejects_arguments_with_a_usage_error() {
    let steps = vec![
        submit("/admission now"),
        wait_render("Usage: /admission"),
        key(KeyCode::Esc),
    ];
    let (frames, seen) = run_plan(steps);
    assert!(
        !frames.join("\n").contains("Provider Admission"),
        "no panel opens for a malformed ask"
    );
    assert!(
        seen.lock().expect("admission command log").is_empty(),
        "no daemon ask leaves the client"
    );
}
