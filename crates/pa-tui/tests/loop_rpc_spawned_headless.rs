//! Headless e2e for the loop's inline-RPC starvation family: the post-compaction
//! transcript rebuild and tray-stats fetch run as spawned tasks (a slow daemon
//! answer must never hold the render/input loop — AGENTS.md "Performance and the
//! critical path"): the keystroke after a slow compaction rebuild must land
//! immediately, and the slow rebuild itself must still replace the chat.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use pa_tui::interactive::{
    run_interactive, HeadlessPlan, HeadlessStep, InteractiveOptions, ModelSelection,
    SessionSelection, UiMode,
};
use serde_json::{json, Value};

/// The scripted slow `get_messages` answer: long enough that an inline rebuild
/// blows any reasonable input-latency bound, short enough to stay under the
/// UI's own 10s request bound so the reply still applies.
const SLOW_GET_MESSAGES_MS: u64 = 4000;
/// The input-latency bound the marker must beat: far above a green run's
/// milliseconds, half the scripted stall a red (inline) run cannot avoid.
const INPUT_BOUND: Duration = Duration::from_secs(2);
/// The headless run's overall bound (green runs settle in well under the slow
/// reply; the bound only names a wedge).
const RUN_BOUND: Duration = Duration::from_secs(30);

fn write_json(writer: &mut UnixStream, value: &Value) {
    let mut line = serde_json::to_string(value).expect("serialize mock frame");
    line.push('\n');
    writer.write_all(line.as_bytes()).expect("write mock frame");
    writer.flush().expect("flush mock frame");
}

fn event(payload: &Value, sequence: u64) -> Value {
    json!({
        "type": "session_event",
        "activeSessionId": "s1",
        "event": payload.clone(),
        "meta": { "sequence": sequence },
    })
}

fn generic_data() -> Value {
    json!({
        "activeSessionId": "s1",
        "id": "s1",
        "sessionId": "sess-1",
        "sessionFile": "/tmp/sess-1.jsonl",
    })
}

/// The settled turn the mock streams after `prompt_and_wait`: a short streamed
/// reply, then a compaction that succeeds (the durable transcript was rebuilt).
fn stream_turn_with_compaction(writer: &mut UnixStream) {
    write_json(writer, &event(&json!({ "type": "turn_start" }), 2));
    write_json(
        writer,
        &event(
            &json!({
                "type": "message_update",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "turn reply" }],
                },
            }),
            3,
        ),
    );
    write_json(writer, &event(&json!({ "type": "turn_end" }), 4));
    write_json(
        writer,
        &event(&json!({ "type": "compaction_start", "reason": "auto" }), 5),
    );
    write_json(
        writer,
        &event(
            &json!({
                "type": "compaction_end", "reason": "auto",
                "result": { "summary": "compacted summary", "tokensBefore": 4200 },
            }),
            6,
        ),
    );
}

/// Serve the compaction scenario: attach answers the empty-session snapshot;
/// `prompt_and_wait` answers and then streams a settled turn whose tail
/// compaction succeeds; `get_messages` (the rebuild fetch) answers on a
/// four-second delay from its own thread so the request line keeps flowing.
fn serve_compaction(listener: &UnixListener) {
    let (stream, _) = listener.accept().expect("accept");
    let mut writer = stream.try_clone().expect("clone mock socket");
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
        let command_type = envelope
            .get("command")
            .and_then(|command| command.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match command_type.as_str() {
            "attach" => {
                write_json(
                    &mut writer,
                    &json!({
                        "type": "response", "id": id, "command": "attach", "success": true,
                        "data": {
                            "activeSessionId": "s1",
                            "snapshot": {
                                "state": {
                                    "sessionId": "sess-1",
                                    "sessionName": "compaction session",
                                    "isStreaming": false,
                                },
                                "messages": [],
                            },
                        },
                    }),
                );
            }
            "prompt_and_wait" => {
                write_json(
                    &mut writer,
                    &json!({
                        "type": "response", "id": id, "command": "prompt_and_wait",
                        "success": true, "data": {},
                    }),
                );
                stream_turn_with_compaction(&mut writer);
            }
            "get_rlm_children" => {
                write_json(
                    &mut writer,
                    &json!({
                        "type": "response", "id": id, "command": "get_rlm_children",
                        "success": true,
                        "data": { "eventSequence": 6 },
                    }),
                );
            }
            "get_messages" => {
                // The scripted slow RPC: the reply leaves on its own thread so
                // the request line keeps serving the rest of the run.
                let mut slow = writer.try_clone().expect("clone slow-reply socket");
                let id = id.to_string();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(SLOW_GET_MESSAGES_MS));
                    let mut line = serde_json::to_string(&json!({
                        "type": "response", "id": id, "command": "get_messages",
                        "success": true,
                        "data": { "messages": [
                            { "role": "user", "content": "rebuilt transcript", "timestamp": 1 },
                        ]},
                    }))
                    .expect("serialize slow reply");
                    line.push('\n');
                    // The client may already be gone at the far end of the delay.
                    let _ = slow.write_all(line.as_bytes());
                    let _ = slow.flush();
                });
            }
            _ => {
                write_json(
                    &mut writer,
                    &json!({
                        "type": "response", "id": id, "command": command_type,
                        "success": true, "data": generic_data(),
                    }),
                );
            }
        }
    }
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
        fullscreen_mouse: false,
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

/// Run one plan against the compaction mock and return the rendered frames.
fn run_compaction_plan(steps: Vec<HeadlessStep>) -> Vec<String> {
    std::env::remove_var("TMUX");
    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket = dir.path().join("tui.sock");
    let listener = UnixListener::bind(&socket).expect("bind mock socket");
    let handle = std::thread::spawn(move || serve_compaction(&listener));
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
        .block_on(async {
            tokio::time::timeout(
                RUN_BOUND,
                Box::pin(run_interactive(options(socket), UiMode::Headless(plan))),
            )
            .await
        })
        .expect("run inside the bound")
        .expect("interactive run");
    handle.join().expect("mock supervisor finished");
    outcome.frames
}

/// A compaction whose rebuild fetch is slow must not starve the input path: the
/// keystroke after the compaction settles lands within the input bound, not
/// after the scripted four-second `get_messages` reply an inline rebuild waits
/// out. The rebuild itself still applies when the slow reply lands.
#[test]
fn a_slow_compaction_rebuild_does_not_starve_input() {
    let (t0_tx, t0_rx) = std::sync::mpsc::channel();
    let (t1_tx, t1_rx) = std::sync::mpsc::channel();
    let frames = run_compaction_plan(vec![
        HeadlessStep::Timestamp(t0_tx),
        // The barrier releases only once every streamed event of the turn was
        // applied — compaction end included — so the keystroke below provably
        // queues behind the compaction's transcript-stale moment.
        HeadlessStep::SubmitAndSettle {
            text: "run the turn".to_string(),
            timeout_ms: 5000,
        },
        HeadlessStep::Type("x".to_string()),
        HeadlessStep::Timestamp(t1_tx),
        // The functionality check: the slow rebuild still replaces the chat.
        HeadlessStep::WaitRender {
            needle: "rebuilt transcript".to_string(),
            timeout_ms: 9000,
        },
    ]);
    let t0 = t0_rx.recv().expect("t0 marker consumed").0;
    let t1 = t1_rx.recv().expect("t1 marker consumed").0;
    assert!(
        t1.duration_since(t0) < INPUT_BOUND,
        "the keystroke after the compaction waited out the slow rebuild fetch: {:?} >= {:?}",
        t1.duration_since(t0),
        INPUT_BOUND,
    );
    let all = frames.join("\n");
    assert!(
        all.contains("rebuilt transcript"),
        "the slow rebuild never applied to the chat:\n{all}",
    );
}
