//! Headless e2e for the run loop's frame-wake inventory: the pending-work
//! states whose observation needs a loop iteration (first frame after the
//! open, barrier deadlines, expiry arms). The work-conditional tick parks
//! without them, so each member needs its own frame-arm deadline.
#![cfg(unix)]

use pa_tui::interactive::{
    run_interactive, HeadlessPlan, HeadlessStep, InteractiveOptions, ModelSelection,
    SessionSelection, UiMode,
};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

/// The plan height and the mock, in the same shape the other headless
/// batteries use (the mock serves one session and answers the boot handshake).
const PLAN_HEIGHT: u16 = 40;

struct MockBoot {
    listener: UnixListener,
    /// When set, `get_state` requests are never answered — the shape a slow daemon
    /// has from the client's side (review #14: a lookup that must not gate paint).
    hang_get_state: bool,
}

impl MockBoot {
    fn bind(socket: &std::path::Path) -> Self {
        MockBoot {
            listener: UnixListener::bind(socket).expect("bind mock socket"),
            hang_get_state: false,
        }
    }

    /// A mock that never answers `get_state`.
    fn bind_hanging_get_state(socket: &std::path::Path) -> Self {
        MockBoot {
            listener: UnixListener::bind(socket).expect("bind mock socket"),
            hang_get_state: true,
        }
    }

    fn serve(self) {
        self.listener
            .set_nonblocking(true)
            .expect("nonblocking mock listener");
        let idle_until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        let (stream, _) = loop {
            match self.listener.accept() {
                Ok(accepted) => break accepted,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= idle_until {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return,
            }
        };
        let write_stream = stream.try_clone().expect("clone mock socket");
        let mut writer = write_stream;
        let mut reader = BufReader::new(stream);
        let hello = json!({
            "type": "daemon_hello",
            "protocol": {"name": "prime-agent.daemon", "version": 7},
            "serverCapabilities": [],
            "clientId": "mock"
        });
        let _ = writeln!(writer, "{hello}");
        // Serve the boot handshake and then hold the link open without
        // ever sending another frame: the surface is fully quiet after
        // the open — the state the inventory members must wake.
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                        continue;
                    };
                    let command = value
                        .get("command")
                        .and_then(|c| c.get("type"))
                        .and_then(Value::as_str);
                    if self.hang_get_state && command == Some("get_state") {
                        // Deliberately unanswered: the client's own bound is what
                        // must keep painting.
                        continue;
                    }
                    match command {
                        Some("create") => {
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "type": "response", "id": value["id"], "command": "create",
                                    "success": true,
                                    "data": {"activeSessionId": "s1", "id": "s1", "sessionId": "sess-1",
                                             "sessionFile": "/tmp/sess-1.jsonl"}
                                })
                            );
                        }
                        Some("attach") => {
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "type": "response", "id": value["id"], "command": "attach",
                                    "success": true,
                                    "data": {"protocol": {"name": "prime-agent.daemon", "version": 7},
                                             "activeSessionId": "s1",
                                             "snapshot": {"activeSessionId": "s1",
                                                          "summary": {"id": "s1", "cwd": "/tmp"},
                                                          "state": {"activeSessionId": "s1", "cwd": "/tmp",
                                                                    "sessionId": "sess-1"}}}
                                })
                            );
                        }
                        _ => {
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "type": "response", "id": value["id"], "command": value["command"]["type"],
                                    "success": true, "data": {}
                                })
                            );
                        }
                    }
                }
            }
        }
    }
}

fn run_boot_plan(steps: Vec<HeadlessStep>) -> pa_tui::interactive::InteractiveOutcome {
    std::env::remove_var("TMUX");
    run_boot_plan_keeping_env(steps)
}

/// The same run WITHOUT clearing `TMUX`: the probe test needs the env the product
/// reads, not the sanitized default.
fn run_boot_plan_keeping_env(steps: Vec<HeadlessStep>) -> pa_tui::interactive::InteractiveOutcome {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket: PathBuf = dir.path().join("tui.sock");
    let supervisor = MockBoot::bind(&socket);
    let handle = std::thread::spawn(move || supervisor.serve());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let outcome = runtime
        .block_on(run_interactive(
            options(socket),
            UiMode::Headless(HeadlessPlan {
                steps,
                width: 100,
                height: PLAN_HEIGHT,
            }),
        ))
        .expect("interactive run");
    let _ = handle.join();
    outcome
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
        show_images: false,
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

/// The first frame paints without any input after the open: the open's rebuild leaves the surface
/// dirty, so the only wake is the dirty frame's own deadline on the frame arm.
#[test]
fn the_first_frame_paints_on_a_fully_quiet_link() {
    let outcome = run_boot_plan(vec![]);
    assert!(
        !outcome.frames.is_empty(),
        "the open's first frame rendered with no other wake:\n{}",
        outcome.frames.join("\n")
    );
}

/// A `WaitRender` barrier whose needle never arrives pops on its deadline: the barrier's
/// re-check runs at the loop top, so the deadline's wake is the only thing that observes it.
#[test]
fn a_wait_render_barrier_times_out_on_a_quiet_surface() {
    let outcome = run_boot_plan(vec![HeadlessStep::WaitRender {
        needle: "this text never arrives".to_string(),
        timeout_ms: 400,
    }]);
    let all = outcome.frames.join("\n");
    assert!(
        all.contains("timed out waiting"),
        "the barrier's deadline fired the timeout note:\n{all}"
    );
}

/// A blocked tmux probe must not delay the first frame (review #39).
///
/// The probe spawns `tmux` and waits up to 2 s for it. Awaiting it before the
/// first draw put a process spawn — and its timeout — on the paint path, so an
/// open under a slow tmux painted nothing until the probe gave up.
///
/// The test owns the runtime so it can measure the RUN, not the teardown: dropping
/// a multi-thread runtime waits for `spawn_blocking` tasks, and the probe runs in
/// one, so including the drop would time the probe regardless of the fix.
#[test]
fn a_blocked_tmux_probe_does_not_delay_the_first_frame() {
    use std::os::unix::fs::PermissionsExt as _;

    let stub_dir = tempfile::TempDir::new().expect("stub dir");
    let tmux = stub_dir.path().join("tmux");
    // Answers too late for any paint path to have waited on it.
    std::fs::write(&tmux, "#!/bin/sh\nsleep 5\n").expect("write the stub");
    std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let previous_path = std::env::var_os("PATH");
    let mut path = std::ffi::OsString::from(stub_dir.path());
    if let Some(existing) = &previous_path {
        path.push(":");
        path.push(existing);
    }
    std::env::set_var("PATH", &path);
    std::env::set_var("TMUX", "/tmp/fake-tmux-session,1,0");

    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket: PathBuf = dir.path().join("tui.sock");
    let supervisor = MockBoot::bind(&socket);
    let handle = std::thread::spawn(move || supervisor.serve());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let started = std::time::Instant::now();
    let outcome = runtime
        .block_on(run_interactive(
            options(socket),
            UiMode::Headless(HeadlessPlan {
                steps: Vec::new(),
                width: 100,
                height: PLAN_HEIGHT,
            }),
        ))
        .expect("interactive run");
    let elapsed = started.elapsed();
    // Teardown is NOT the paint path: do not let the blocked probe's host task
    // count against the run. `shutdown_background` returns immediately.
    runtime.shutdown_background();
    let _ = handle.join();

    std::env::remove_var("TMUX");
    match previous_path {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }

    assert!(
        !outcome.frames.is_empty(),
        "the first frame must paint while the tmux probe is still blocked"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the open waited on the tmux probe ({elapsed:?}); the probe must not be on the paint path"
    );
}

/// A slow model lookup must not hold the first frame (review #14).
///
/// `current_model_provider` is a bounded `GetState` request, and the ban-risk
/// warning behind it reads session state and the credential — each with the
/// 10-second request bound. Awaiting that chain before the first draw meant a
/// slow or wedged daemon could keep the surface blank for tens of seconds.
///
/// The mock never answers `get_state`, so only a paint path that does NOT wait on
/// the lookup can produce frames at all.
#[test]
fn a_hanging_model_lookup_does_not_hold_the_first_frame() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let socket: PathBuf = dir.path().join("tui.sock");
    let supervisor = MockBoot::bind_hanging_get_state(&socket);
    let handle = std::thread::spawn(move || supervisor.serve());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let started = std::time::Instant::now();
    let outcome = runtime
        .block_on(run_interactive(
            options(socket),
            UiMode::Headless(HeadlessPlan {
                steps: Vec::new(),
                width: 100,
                height: PLAN_HEIGHT,
            }),
        ))
        .expect("interactive run");
    let elapsed = started.elapsed();
    // Teardown is not the paint path (see the tmux test).
    runtime.shutdown_background();
    let _ = handle.join();

    assert!(
        !outcome.frames.is_empty(),
        "the first frame must paint while the model lookup is still unanswered"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the open waited on the model lookup ({elapsed:?}); it must not be on the paint path"
    );
}
