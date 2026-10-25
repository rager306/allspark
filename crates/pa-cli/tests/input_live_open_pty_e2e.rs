// The Tier-C/D ruling (fleet-uniform, 2026-09-28): stack-resident futures
// by design on hot paths; the fn-length threshold is a style gate, not
// correctness; the harness fns are intentionally linear; 64-bit targets -
// the narrowing sits at OS/protocol boundaries where the values are
// bounded (pid syscalls, epoch/elapsed milliseconds).
#![allow(
    clippy::large_futures,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

//! Real-pty e2e for the input-latency contract (the operator's
//! "first page fast, time-to-type slow" report + AGENTS.md
//! "Performance and the critical path"): a NEW session's interactive
//! surface must accept keystrokes the moment its startup chrome is
//! mounted — the session open (create, attach, the dock folds, every
//! daemon readiness behind them) runs in the background, so the echo
//! never waits on it (TS `init()`: `ui.start()` is live before
//! `rebindCurrentSession()` awaits). The mock supervisor stalls the
//! `attach` response far past the echo deadline, so an open-gated input
//! path cannot fake a pass.
//!
//! The typed-ahead contract rides the same run: text typed during the
//! stall plus an Enter queued behind the open must submit once the
//! session lands — never lost, never errored.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::pty::{openpty, Winsize};
use serde_json::{json, Value};

mod support;

use support::PtyReader;

use pa_tui::interactive::{
    run_interactive, InteractiveOptions, ModelSelection, SessionSelection, UiMode,
};

/// The child-mode socket: set (with the socket path) only when this very
/// binary is re-executed as the product-under-test.
const CHILD_SOCKET_ENV: &str = "PA_INPUT_LIVE_CHILD_SOCKET";

/// How long the mock supervisor holds the `attach` response: far past the
/// echo deadline below, so the open is provably still in flight while the
/// keystrokes are expected to echo.
const OPEN_STALL_MS: u64 = 2_500;

/// The pty window the child renders into (a 0x0 pty renders nothing; the
/// grid the echo oracle reads matches the window the child lays out).
/// The child half of the e2e: runs the real interactive loop in terminal
/// mode against the parent's mock supervisor. A plain `cargo test` run
/// (no `CHILD_SOCKET_ENV`) passes trivially — only the parent test drives
/// the real path.
#[test]
fn input_live_open_child_mode() {
    let Ok(socket) = std::env::var(CHILD_SOCKET_ENV) else {
        return;
    };
    let options = child_options(PathBuf::from(socket));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let _ = runtime.block_on(run_interactive(options, UiMode::Terminal));
}

/// The pty harnesses serialize: each drives a raw pty; concurrent
/// byte-level waits flake on the shared sandbox CPUs.
static HARNESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A keystroke echoes into the rendered surface while the session open is
/// still in flight (the attach is stalled for `OPEN_STALL_MS`), and the
/// typed-ahead text plus the queued Enter submit once the open lands.
#[test]
fn typing_echoes_while_the_session_open_is_stalled() {
    if !session_runner() {
        return;
    }
    match nix::unistd::setsid() {
        Ok(_) => {}
        Err(error) => panic!("the harness could not start a fresh session: {error}"),
    }
    let _lock = match HARNESS_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let commands = Arc::new(Mutex::new(Vec::new()));
    let mut harness = InputLiveHarness::start(Arc::clone(&commands));

    // The startup chrome (the new chat's brand splash) paints first: the
    // surface is mounted and in raw mode by then.
    harness.wait_from_start("prime agent", "the startup chrome painted");

    // Type while the open is still stalled. The echo must land inside the
    // echo deadline — far inside the attach's stall window.
    harness.write_typed("zzq");
    let echo_at = harness.wait_from_start("zzq", "the keystroke echoed while the open was stalled");

    // The queued submit: the typed-ahead text plus Enter ride the opening
    // phase's queue and dispatch once the session lands.
    harness.write_typed("\r");

    // Let the stalled attach land and the fold run.
    let content_at = harness.wait_from_start(
        "openprobe settled answer",
        "the attach's content frame painted",
    );

    // The red-first oracle, ordering-based (load-robust where a wall
    // clock is not): the keystroke echo must have painted while the
    // attach was still stalled, so it became visible strictly BEFORE the
    // attach's content frame in the byte stream. An open-gated input path
    // cannot fake this — its first echo frame can only follow the
    // stalled open, the same event that paints the content frame.
    assert!(
        echo_at < content_at,
        "the keystroke echo (byte {echo_at}) painted only after the          attach's content frame (byte {content_at}): the input path was          open-gated"
    );

    // The typed-ahead submit was delivered: the mock supervisor saw the
    // prompt carrying the typed text.
    let deadline = Instant::now() + Duration::from_secs(10);
    let submitted = loop {
        if let Some(found) = find_prompt_with(&commands, "zzq") {
            break found;
        }
        if Instant::now() > deadline {
            let seen = commands.lock().expect("commands").clone();
            panic!("the typed-ahead submit never reached the daemon; commands seen: {seen:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = submitted;

    // The harness drops here: the child dies with the test on every path.
}

/// Whether this runner is attached to a controlling-terminal session (the
/// child re-exec needs a session it can leave and re-enter safely).
fn session_runner() -> bool {
    // SAFETY: tcgetpgrp only queries the fd's foreground process group.
    let foreground = unsafe { libc::tcgetpgrp(0) };
    if foreground < 0 {
        eprintln!(
            "no controlling-terminal session on the runner (tcgetpgrp(fd 0) \
             failed); skipping the input-live open e2e — it needs a \
             controlling-terminal session to drive the pty child"
        );
        return false;
    }
    true
}

/// One pty-backed product child plus the mock supervisor it opens against.
struct InputLiveHarness {
    child: Child,
    server: Option<std::thread::JoinHandle<()>>,
    /// Whether the mock's `accept` landed: `Drop` joins the serve thread
    /// only then, so a child that dies before connecting cannot hang the
    /// teardown on the still-accepting listener.
    connected: Arc<std::sync::atomic::AtomicBool>,
    master: PtyReader,
}

impl InputLiveHarness {
    fn write_typed(&mut self, text: &str) {
        self.master.write_typed(text);
    }

    fn wait_from_start(&mut self, needle: &str, what: &str) -> usize {
        self.master.wait_for_screen(needle, what, 30_000)
    }

    fn start(commands: Arc<Mutex<Vec<String>>>) -> InputLiveHarness {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let socket = dir.path().join("tui.sock");
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let supervisor = MockSupervisor::bind(&socket);
        let server = {
            let connected = Arc::clone(&connected);
            Some(std::thread::spawn(move || {
                supervisor.serve(&commands, &connected);
            }))
        };
        let pty = openpty(
            Some(&Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .expect("open pty");
        let child = spawn_child(&socket, &pty.slave);
        // Leak the temp dir's socket path on purpose: the child needs the
        // socket for the lifetime of the test, and the whole tree dies
        // with the child at teardown.
        std::mem::forget(dir);
        InputLiveHarness {
            child,
            server,
            connected,
            master: PtyReader::new(pty.master),
        }
    }
}

impl Drop for InputLiveHarness {
    fn drop(&mut self) {
        // The teardown runs on every exit path — including the panic
        // unwinds the oracles below produce — so no product child ever
        // outlives the test that spawned it. The child's death ends the
        // mock's read loop (the serve thread joins only when its accept
        // landed; a child that died pre-connect leaves the listener
        // thread to the process exit instead of hanging the drop).
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.connected.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some(server) = self.server.take() {
                let _ = server.join();
            }
        }
    }
}

/// The mock's recorded commands: one line per received command envelope.
fn find_prompt_with(commands: &Arc<Mutex<Vec<String>>>, text: &str) -> Option<String> {
    commands
        .lock()
        .expect("commands")
        .iter()
        .find(|line| line.contains("\"prompt\"") && line.contains(text))
        .cloned()
}

/// A child process group of this very binary, re-executed in child mode
/// with the pty slave as its terminal and no tmux.
fn spawn_child(socket: &std::path::Path, slave: &OwnedFd) -> Child {
    fn make_process_group() -> std::io::Result<()> {
        nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0))?;
        Ok(())
    }
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .arg("--exact")
        .arg("input_live_open_child_mode")
        .env(CHILD_SOCKET_ENV, socket)
        .env_remove("TMUX")
        .stdin(slave_as_stdio(slave))
        .stdout(slave_as_stdio(slave))
        .stderr(slave_as_stdio(slave));
    // SAFETY: the pre_exec hook is the supported std seam for
    // process-group setup; it runs post-fork pre-exec in the child only
    // and cannot disturb this process.
    unsafe { command.pre_exec(make_process_group) };
    command.spawn().expect("spawn child")
}

fn slave_as_stdio(slave: &OwnedFd) -> Stdio {
    slave.try_clone().expect("clone pty slave").into()
}

fn child_options(socket: PathBuf) -> InteractiveOptions {
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
        // The fresh-launch flow: a NEW session, so the startup chrome
        // mounts before the open (the exact surface the report measured).
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

/// The supervisor stand-in: the `create` answers immediately, the
/// `attach` stalls past the echo deadline, everything else answers
/// generically. Every received command is recorded for the typed-ahead
/// submit assertion.
struct MockSupervisor {
    listener: std::os::unix::net::UnixListener,
    socket_path: std::path::PathBuf,
}

impl MockSupervisor {
    fn bind(socket: &std::path::Path) -> MockSupervisor {
        MockSupervisor {
            listener: std::os::unix::net::UnixListener::bind(socket).expect("bind mock socket"),
            socket_path: socket.to_path_buf(),
        }
    }

    fn serve(self, commands: &Arc<Mutex<Vec<String>>>, connected: &std::sync::atomic::AtomicBool) {
        let (stream, _) = self.listener.accept().expect("accept");
        connected.store(true, std::sync::atomic::Ordering::SeqCst);
        let write_stream = stream.try_clone().expect("clone mock socket");
        let mut writer = write_stream;
        let mut reader = BufReader::new(stream);

        let hello = json!({
            "type": "daemon_hello",
            "socketPath": self.socket_path.display().to_string(),
            "protocol": { "name": "prime-agent.daemon", "version": 7 },
            "serverCapabilities": ["kernel_bash_activity"],
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
            commands
                .lock()
                .expect("commands")
                .push(format!("{command_type}: {command}"));
            match command_type.as_str() {
                "create" => {
                    write_json(
                        &mut writer,
                        &json!({
                            "type": "response",
                            "id": id,
                            "command": "create",
                            "success": true,
                            "data": { "activeSessionId": "s1", "sessionId": "sess-1" },
                        }),
                    );
                }
                "attach" => {
                    // The stall: the open is provably in flight while the
                    // keystrokes are expected to echo.
                    std::thread::sleep(Duration::from_millis(OPEN_STALL_MS));
                    write_json(&mut writer, &attach_data(id));
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

fn write_json(writer: &mut std::os::unix::net::UnixStream, value: &Value) {
    let mut line = serde_json::to_string(value).expect("serialize mock frame");
    line.push('\n');
    writer.write_all(line.as_bytes()).expect("write mock frame");
    writer.flush().expect("flush mock frame");
}

/// The attach result: one live session whose transcript carries a marker
/// row — the content frame the fold paints once the stall ends.
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
                    "sessionName": "input live probe",
                    "model": "faux-1",
                    "isStreaming": false,
                    "isCompacting": false,
                    "sessionActions": { "queuedCount": 0, "steering": [], "followUps": [] },
                },
                "messages": [
                    { "role": "user", "content": "openprobe marker question", "timestamp": 1 },
                    { "role": "assistant", "content": "openprobe settled answer", "provider": "scripted", "model": "faux-1", "timestamp": 2 },
                ],
                "lastEventSequence": 0,
                "lastEventCursor": null,
            },
            "client": { "id": "mock", "capabilities": [] },
            "lastEventSequence": 0,
            "lastEventCursor": null,
        },
    })
}
