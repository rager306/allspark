//! PTY multi-client e2e: two real client processes, each on its own PTY,
//! attached to ONE forked daemon — the fan-out, exactly-once, session-isolation,
//! and graceful-disconnect contract the interactive TUI rides.
//!
//! The daemon is the fork's own supervisor (`prime-agent --mode daemon`, or the
//! `pa-daemon supervisor` entry it wraps). Each client is this very test binary
//! re-executed in a child mode (`client_child_mode`) that speaks the SAME JSONL
//! wire the TUI's `pa_tui::daemon_client` speaks, driven line-by-line through its
//! PTY master — a genuine pty per client, no external process beyond the daemon.
//!
//! Lifecycle note (fork contract, upstream #3345): the daemon deliberately KEEPS
//! RUNNING when its last client disconnects — a later client reconnects and finds
//! the session intact. The graceful terminal path is an explicit `shutdown`
//! command, which is what the disconnect tests exercise; asserting an
//! "exits on the last client" behavior would be asserting a behavior the fork
//! does not have.

#![allow(clippy::large_futures)]
// 64-bit-only targets; the narrowing casts sit at bounded OS boundaries.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
// Fn length is a style gate, not correctness.
#![allow(clippy::too_many_lines)]
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nix::fcntl::{fcntl, FcntlArg::F_SETFL, OFlag};
use nix::pty::{openpty, Winsize};
use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};
use serde_json::{json, Value};

/// The socket path the re-executed child client connects to.
const CHILD_SOCKET_ENV: &str = "PA_MULTI_CLIENT_CHILD_SOCKET";

/// The pty window each client renders into (a 0x0 pty carries no bytes).
const PTY_WINSIZE: Winsize = Winsize {
    ws_row: 40,
    ws_col: 120,
    ws_xpixel: 0,
    ws_ypixel: 0,
};

/// The per-wait budget for a client marker (a cold worker boot can take a while).
const MARKER_TIMEOUT: Duration = Duration::from_secs(60);

// ───────────────────────────── child client ─────────────────────────────

/// The child half: runs the JSONL client loop against the parent's daemon. A
/// plain `cargo test` run (no env) passes trivially.
#[test]
fn client_child_mode() {
    let Ok(socket) = std::env::var(CHILD_SOCKET_ENV) else {
        return;
    };
    run_child(&PathBuf::from(socket));
}

/// One inbound message on the child's multiplexed channel.
enum ChildMsg {
    Frame(Value),
    Cmd(String),
    Closed,
}

/// Print one machine-readable marker line the parent greps for.
fn marker(line: &str) {
    println!("<<PA:{line}>>");
    let _ = std::io::stdout().flush();
}

/// Translate one parent command into a daemon `(envelope id, command)` pair.
fn translate_command(cmd: &Value) -> (String, Value) {
    let id = cmd
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("anon")
        .to_string();
    let sid = || cmd.get("sid").and_then(Value::as_str).unwrap_or_default();
    let command = match cmd.get("cmd").and_then(Value::as_str).unwrap_or_default() {
        "create" => {
            json!({ "type": "create", "config": cmd.get("config").cloned().unwrap_or(Value::Null) })
        }
        "attach" => json!({ "type": "attach", "activeSessionId": sid() }),
        "prompt" => json!({
            "type": "prompt",
            "activeSessionId": sid(),
            "message": cmd.get("message").and_then(Value::as_str).unwrap_or_default()
        }),
        "wait_for_idle" => json!({ "type": "wait_for_idle", "activeSessionId": sid() }),
        "get_messages" => json!({ "type": "get_messages", "activeSessionId": sid() }),
        "list" => json!({ "type": "list" }),
        "shutdown" => json!({ "type": "shutdown" }),
        other => json!({ "type": other }),
    };
    (id, command)
}

/// Send one command envelope to the daemon.
fn send_command(writer: &mut UnixStream, id: &str, command: &Value) {
    let envelope = json!({
        "type": "command",
        "id": id,
        "protocol": { "name": "prime-agent.daemon", "version": 7 },
        "command": command,
    });
    let mut line = serde_json::to_string(&envelope).expect("serialize command");
    line.push('\n');
    let _ = writer.write_all(line.as_bytes());
    let _ = writer.flush();
}

/// The child loop: multiplex the daemon socket and stdin over one channel.
fn run_child(socket: &Path) {
    let stream = UnixStream::connect(socket).expect("child connects to the daemon");
    let mut writer = stream.try_clone().expect("clone the child socket");
    let (tx, rx) = mpsc::channel::<ChildMsg>();

    // Socket reader: every inbound frame becomes a channel message.
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(ChildMsg::Closed);
                        break;
                    }
                    Ok(_) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
                            let _ = tx.send(ChildMsg::Frame(value));
                        }
                    }
                }
            }
        });
    }
    // Stdin reader: parent commands arrive as lines through the pty. This
    // thread owns the original sender; the socket reader took a clone.
    {
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                if let Ok(line) = line {
                    if tx.send(ChildMsg::Cmd(line)).is_err() {
                        break;
                    }
                } else {
                    let _ = tx.send(ChildMsg::Closed);
                    break;
                }
            }
        });
    }

    marker(&format!("READY pid={}", std::process::id()));
    loop {
        match rx.recv() {
            Ok(ChildMsg::Frame(value)) => match value.get("type").and_then(Value::as_str) {
                Some("daemon_hello") => marker("HELLO"),
                Some("response") => {
                    let id = value.get("id").and_then(Value::as_str).unwrap_or_default();
                    let success = value
                        .get("success")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let data =
                        serde_json::to_string(&value.get("data").cloned().unwrap_or(Value::Null))
                            .unwrap_or_default();
                    marker(&format!("RESPONSE id={id} success={success} data={data}"));
                }
                Some("session_event") => {
                    let sid = value
                        .get("activeSessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let seq = value
                        .get("meta")
                        .and_then(|meta| meta.get("sequence"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let event_type = value
                        .get("event")
                        .and_then(|event| event.get("type"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    marker(&format!("EVENT sid={sid} seq={seq} type={event_type}"));
                }
                Some(other) => marker(&format!("FRAME type={other}")),
                None => {}
            },
            Ok(ChildMsg::Cmd(line)) => {
                let Ok(cmd) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if cmd.get("cmd").and_then(Value::as_str) == Some("quit") {
                    break;
                }
                let (id, command) = translate_command(&cmd);
                send_command(&mut writer, &id, &command);
            }
            Ok(ChildMsg::Closed) | Err(_) => break,
        }
    }
}

// ───────────────────────────── daemon harness ─────────────────────────────

/// The forked daemon under test; killed at scope exit.
struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// The daemon entry: the fork's `prime-agent --mode daemon` when that binary
/// sits beside this test's `pa-daemon` (the product path), else the
/// `pa-daemon supervisor` entry it wraps (always built for this crate).
fn daemon_launch(dir: &Path, socket: &Path, agent_dir: &Path) -> (Command, PathBuf) {
    let pa_daemon = PathBuf::from(env!("CARGO_BIN_EXE_pa-daemon"));
    let product = pa_daemon
        .parent()
        .map(|profile| profile.join("prime-agent"))
        .filter(|candidate| candidate.exists());
    // The two entries name the socket flag differently: `prime-agent --mode
    // daemon` takes `--daemon-socket`, `pa-daemon supervisor` takes `--socket`.
    let mut command = if let Some(product) = product {
        let mut command = Command::new(product);
        command.arg("--mode").arg("daemon").arg("--daemon-socket");
        command
    } else {
        let mut command = Command::new(&pa_daemon);
        command.arg("supervisor").arg("--socket");
        command
    };
    command
        .arg(socket)
        .current_dir(dir)
        // The isolated HOME/agent dir: a hermetic sandbox for the daemon and
        // every worker/kernel it spawns.
        .env("HOME", dir)
        .env("PRIME_AGENT_CODING_AGENT_DIR", agent_dir)
        .env("PI_OFFLINE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Inherited worker-role env must not leak into the spawned supervisor.
    for var in [
        pa_daemon::worker::WORKER_ROLE_ENV,
        pa_daemon::worker::WORKER_TOKEN_ENV,
        pa_daemon::worker::WORKER_ACTIVE_SESSION_ID_ENV,
        pa_daemon::worker::WORKER_RECOVERY_JOURNAL_ENV,
        pa_daemon::worker::WORKER_SUPERVISOR_SOCKET_ENV,
        pa_daemon::worker::WORKER_SOCKET_ENV,
        pa_daemon::worker::WORKER_INSTANCE_ID_ENV,
        pa_daemon::worker::WORKER_SCRIPT_ENV,
    ] {
        command.env_remove(var);
    }
    // A supervisor killed at teardown must not leak its session workers.
    command.env(
        pa_daemon::worker::WORKER_SUPERVISOR_LOST_EXIT_MS_ENV,
        "15000",
    );
    (command, pa_daemon)
}

#[allow(clippy::zombie_processes)]
fn spawn_daemon(dir: &Path, socket: &Path, agent_dir: &Path) -> Daemon {
    let (mut command, _pa_daemon) = daemon_launch(dir, socket, agent_dir);
    let child = command.spawn().expect("spawn the daemon");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if UnixStream::connect(socket).is_ok() {
            return Daemon {
                child,
                socket: socket.to_path_buf(),
            };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("daemon socket never came up at {}", socket.display());
}

// ───────────────────────────── client harness ─────────────────────────────

/// One pty-backed client process driven line-by-line through its pty master.
struct Client {
    child: Child,
    master: std::fs::File,
    buffer: Vec<u8>,
    name: &'static str,
}

impl Client {
    /// Spawn this test binary in child mode on a fresh pty.
    fn spawn(name: &'static str, socket: &Path) -> Self {
        let pty = openpty(Some(&PTY_WINSIZE), None).expect("open pty");
        // Silence the pty's input echo so the master carries ONLY the child's
        // markers — the command lines the parent writes never bounce back.
        let mut termios = tcgetattr(&pty.slave).expect("read pty termios");
        termios.local_flags.remove(LocalFlags::ECHO);
        termios.local_flags.remove(LocalFlags::ECHONL);
        tcsetattr(&pty.slave, SetArg::TCSANOW, &termios).expect("disable pty echo");

        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
            .arg("--exact")
            .arg("client_child_mode")
            // `--nocapture` lets the child's `println!` markers reach the pty
            // instead of libtest's per-test capture sink.
            .arg("--nocapture")
            .env(CHILD_SOCKET_ENV, socket)
            .env("TERM", "xterm-256color")
            .stdin(slave_stdio(&pty.slave))
            .stdout(slave_stdio(&pty.slave))
            .stderr(slave_stdio(&pty.slave));
        for var in [
            pa_daemon::worker::WORKER_ROLE_ENV,
            pa_daemon::worker::WORKER_TOKEN_ENV,
            pa_daemon::worker::WORKER_ACTIVE_SESSION_ID_ENV,
            pa_daemon::worker::WORKER_RECOVERY_JOURNAL_ENV,
            pa_daemon::worker::WORKER_SUPERVISOR_SOCKET_ENV,
            pa_daemon::worker::WORKER_SOCKET_ENV,
            pa_daemon::worker::WORKER_INSTANCE_ID_ENV,
            pa_daemon::worker::WORKER_SCRIPT_ENV,
        ] {
            command.env_remove(var);
        }
        let child = command.spawn().expect("spawn client child");
        let master = pty.master.into();
        fcntl(
            std::fs::File::as_raw_fd(&master),
            F_SETFL(OFlag::O_NONBLOCK),
        )
        .expect("pty master non-blocking");
        let mut client = Client {
            child,
            master,
            buffer: Vec::new(),
            name,
        };
        client.wait_for("READY", MARKER_TIMEOUT);
        client
    }

    /// Drain every byte currently readable from the master into the buffer.
    fn pump(&mut self) {
        let mut chunk = [0u8; 8192];
        loop {
            match self.master.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    /// Write one command line to the child's stdin (through the master).
    fn send(&mut self, command: &Value) {
        let mut line = serde_json::to_string(command).expect("serialize client command");
        line.push('\n');
        self.master
            .write_all(line.as_bytes())
            .expect("write client command");
        self.master.flush().expect("flush client command");
    }

    /// Every `<<PA:...>>` marker line received so far.
    fn markers(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.buffer)
            .lines()
            .filter(|line| line.contains("<<PA:"))
            .map(|line| {
                line.split("<<PA:")
                    .nth(1)
                    .and_then(|rest| rest.split(">>").next())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// Block until a marker containing `needle` arrives; return that marker.
    fn wait_for(&mut self, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(found) = self
                .markers()
                .into_iter()
                .find(|marker| marker.contains(needle))
            {
                return found;
            }
            self.pump();
            if Instant::now() >= deadline {
                let tail = String::from_utf8_lossy(&self.buffer);
                panic!(
                    "client {}: timed out waiting for marker {needle:?}; pty tail:\n{tail}",
                    self.name
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Send a command and block until its RESPONSE marker; return the marker.
    fn request(&mut self, command: &Value) -> String {
        let id = command
            .get("id")
            .and_then(Value::as_str)
            .expect("command id")
            .to_string();
        self.send(command);
        self.wait_for(&format!("RESPONSE id={id} "), MARKER_TIMEOUT)
    }

    /// All EVENT markers for one session, as `(sequence, event_type)` pairs.
    fn events_for(&self, sid: &str) -> Vec<(u64, String)> {
        self.markers()
            .iter()
            .filter_map(|marker| {
                let mut fields = marker.split_whitespace();
                if fields.next() != Some("EVENT") {
                    return None;
                }
                let mut sid_seen = None;
                let mut seq = None;
                let mut event_type = None;
                for field in fields {
                    if let Some(value) = field.strip_prefix("sid=") {
                        sid_seen = Some(value.to_string());
                    } else if let Some(value) = field.strip_prefix("seq=") {
                        seq = value.parse::<u64>().ok();
                    } else if let Some(value) = field.strip_prefix("type=") {
                        event_type = Some(value.to_string());
                    }
                }
                (sid_seen.as_deref() == Some(sid))
                    .then(|| (seq.unwrap_or(0), event_type.unwrap_or_default()))
            })
            .collect()
    }

    /// Close the connection and reap the child (a graceful client disconnect).
    fn disconnect(&mut self) {
        self.send(&json!({ "cmd": "quit" }));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn slave_stdio(slave: &OwnedFd) -> Stdio {
    slave.try_clone().expect("clone pty slave").into()
}

// ───────────────────────────── shared fixtures ─────────────────────────────

/// The isolated sandbox: temp dir, agent dir, sessions dir, socket, and the
/// scripted-engine config the faux provider replays.
struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    agent_dir: PathBuf,
    session_dir: PathBuf,
    socket: PathBuf,
    script: PathBuf,
}

impl Sandbox {
    fn new(responses: &[&str]) -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let root = dir.path().to_path_buf();
        let agent_dir = root.join("agent");
        let session_dir = agent_dir.join("sessions");
        std::fs::create_dir_all(&session_dir).expect("sessions dir");
        let socket = root.join("daemon.sock");
        let script = root.join("script.json");
        let responses: Vec<Value> = responses
            .iter()
            .map(|text| json!({ "text": text, "delayMs": 0 }))
            .collect();
        std::fs::write(&script, json!({ "responses": responses }).to_string())
            .expect("write script");
        Self {
            _dir: dir,
            root,
            agent_dir,
            session_dir,
            socket,
            script,
        }
    }

    fn spawn(&self) -> Daemon {
        spawn_daemon(&self.root, &self.socket, &self.agent_dir)
    }

    fn create_config(&self) -> Value {
        json!({
            "cwd": self.root.to_string_lossy(),
            "sessionDir": self.session_dir.to_string_lossy(),
            "script": self.script.to_string_lossy(),
        })
    }

    fn client(&self, name: &'static str) -> Client {
        Client::spawn(name, &self.socket)
    }
}

/// Extract the created session id from a create RESPONSE marker's `data` JSON.
fn created_session(marker: &str) -> String {
    let data = marker.split("data=").nth(1).expect("response data field");
    let value: Value = serde_json::from_str(data).expect("response data json");
    value
        .get("activeSessionId")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .expect("created session id")
        .to_string()
}

/// The assistant texts in a `get_messages` RESPONSE marker's `data`.
fn message_texts(marker: &str) -> Vec<String> {
    let data = marker.split("data=").nth(1).expect("response data field");
    let value: Value = serde_json::from_str(data).expect("response data json");
    value
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
                .filter_map(|message| message.get("content").and_then(Value::as_str))
                .map(std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn attach(client: &mut Client, id: &str, sid: &str) {
    let response = client.request(&json!({ "cmd": "attach", "id": id, "sid": sid }));
    assert!(
        response.contains("success=true"),
        "attach {id} failed: {response}"
    );
}

fn prompt(client: &mut Client, id: &str, sid: &str, message: &str) {
    let response = client.request(&json!({
        "cmd": "prompt",
        "id": id,
        "sid": sid,
        "message": message
    }));
    assert!(
        response.contains("success=true"),
        "prompt {id} failed: {response}"
    );
    let idle =
        client.request(&json!({ "cmd": "wait_for_idle", "id": format!("{id}-idle"), "sid": sid }));
    assert!(
        idle.contains("success=true"),
        "wait_for_idle {id} failed: {idle}"
    );
}

/// No session-event sequence may appear twice in one client's stream — the
/// exactly-once fan-out contract.
fn assert_no_duplicate_sequences(client: &Client, sid: &str) {
    let mut sequences: Vec<u64> = client
        .events_for(sid)
        .into_iter()
        .map(|(sequence, _)| sequence)
        .collect();
    let total = sequences.len();
    sequences.sort_unstable();
    sequences.dedup();
    assert_eq!(
        sequences.len(),
        total,
        "client {} delivered a duplicated session-event sequence (fan-out must be exactly-once)",
        client.name
    );
}

/// The shutdown response's daemon-exit path: wait for the daemon to exit and its
/// socket to disappear.
fn assert_daemon_exits(daemon: &mut Daemon) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if daemon.child.try_wait().ok().flatten().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the daemon did not exit after the shutdown command"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !daemon.socket.exists(),
        "the daemon removed its socket on exit"
    );
}

// ───────────────────────────── tests ─────────────────────────────

/// Two clients on one daemon share a session: B attaches and sees the same
/// transcript A built, both receive the next turn's fan-out, and neither sees a
/// duplicated event.
#[test]
fn two_clients_share_one_session_with_exactly_once_fanout() {
    let sandbox = Sandbox::new(&["alpha answer", "beta answer"]);
    let mut daemon = sandbox.spawn();
    let mut a = sandbox.client("A");
    let mut b = sandbox.client("B");

    // A creates the session and runs the first turn.
    let created =
        a.request(&json!({ "cmd": "create", "id": "c1", "config": sandbox.create_config() }));
    assert!(created.contains("success=true"), "create failed: {created}");
    let sid = created_session(&created);
    attach(&mut a, "a1", &sid);
    prompt(&mut a, "p1", &sid, "first");
    assert!(
        a.events_for(&sid)
            .iter()
            .any(|(_, event)| event == "agent_end"),
        "A saw the first turn settle"
    );

    // B attaches to the SAME session and its snapshot carries A's transcript.
    attach(&mut b, "a2", &sid);
    let snapshot = b.request(&json!({ "cmd": "get_messages", "id": "m0", "sid": &sid }));
    assert!(
        message_texts(&snapshot).contains(&"alpha answer".to_string()),
        "B's snapshot carries A's first turn: {snapshot}"
    );

    // A runs a second turn: BOTH clients receive it (fan-out), each exactly once.
    prompt(&mut a, "p2", &sid, "second");
    b.wait_for("type=agent_end", MARKER_TIMEOUT);

    let a_events = a.events_for(&sid);
    let b_events = b.events_for(&sid);
    assert!(
        a_events.iter().any(|(_, event)| event == "agent_end"),
        "A received the second turn's settle"
    );
    assert!(
        b_events.iter().any(|(_, event)| event == "agent_end"),
        "B received the second turn's settle (fan-out)"
    );
    assert_no_duplicate_sequences(&a, &sid);
    assert_no_duplicate_sequences(&b, &sid);

    // Both read the same two-turn transcript.
    let a_messages = a.request(&json!({ "cmd": "get_messages", "id": "ma", "sid": &sid }));
    let b_messages = b.request(&json!({ "cmd": "get_messages", "id": "mb", "sid": &sid }));
    assert_eq!(
        message_texts(&a_messages),
        vec!["alpha answer".to_string(), "beta answer".to_string()],
        "A's transcript: {a_messages}"
    );
    assert_eq!(
        message_texts(&b_messages),
        message_texts(&a_messages),
        "both clients observe the identical transcript"
    );

    // The terminal path: an explicit shutdown stops the daemon cleanly.
    let shutdown = a.request(&json!({ "cmd": "shutdown", "id": "sd" }));
    assert!(
        shutdown.contains("success=true"),
        "shutdown failed: {shutdown}"
    );
    assert_daemon_exits(&mut daemon);
}

/// Session isolation: a client only receives the events of the session it is
/// attached to. A and B each own a session; neither sees the other's fan-out.
#[test]
fn sessions_are_isolated_between_clients() {
    let sandbox = Sandbox::new(&["answer for A", "answer for B"]);
    let mut daemon = sandbox.spawn();
    let mut a = sandbox.client("A");
    let mut b = sandbox.client("B");

    let created_a =
        a.request(&json!({ "cmd": "create", "id": "ca", "config": sandbox.create_config() }));
    let sid_a = created_session(&created_a);
    let created_b =
        b.request(&json!({ "cmd": "create", "id": "cb", "config": sandbox.create_config() }));
    let sid_b = created_session(&created_b);
    assert_ne!(sid_a, sid_b, "each client owns a distinct session");

    attach(&mut a, "aa", &sid_a);
    attach(&mut b, "ab", &sid_b);

    // Drive a turn on A, then one on B.
    prompt(&mut a, "pa", &sid_a, "turn a");
    prompt(&mut b, "pb", &sid_b, "turn b");

    // Give any cross-session leakage a window to land before asserting absence.
    std::thread::sleep(Duration::from_millis(500));
    a.pump();
    b.pump();

    assert!(
        a.events_for(&sid_b).is_empty(),
        "A received NO event for B's session: {:?}",
        a.events_for(&sid_b)
    );
    assert!(
        b.events_for(&sid_a).is_empty(),
        "B received NO event for A's session: {:?}",
        b.events_for(&sid_a)
    );
    // Each client did receive its OWN session's turn.
    assert!(
        a.events_for(&sid_a)
            .iter()
            .any(|(_, event)| event == "agent_end"),
        "A received its own turn"
    );
    assert!(
        b.events_for(&sid_b)
            .iter()
            .any(|(_, event)| event == "agent_end"),
        "B received its own turn"
    );

    let shutdown = a.request(&json!({ "cmd": "shutdown", "id": "sd" }));
    assert!(shutdown.contains("success=true"));
    assert_daemon_exits(&mut daemon);
}

/// Graceful disconnect: A leaves, B keeps working on the shared session; the
/// daemon stays up for B (the fork keeps it running when a client closes,
/// upstream #3345), and an explicit shutdown from B ends it cleanly.
#[test]
fn a_client_disconnecting_leaves_the_other_working() {
    let sandbox = Sandbox::new(&["one", "two"]);
    let mut daemon = sandbox.spawn();
    let mut a = sandbox.client("A");
    let mut b = sandbox.client("B");

    let created =
        a.request(&json!({ "cmd": "create", "id": "c1", "config": sandbox.create_config() }));
    let sid = created_session(&created);
    attach(&mut a, "a1", &sid);
    attach(&mut b, "a2", &sid);
    prompt(&mut a, "p1", &sid, "first");

    // A disconnects; B must keep serving the shared session.
    a.disconnect();

    // B still reads the transcript A built...
    let messages = b.request(&json!({ "cmd": "get_messages", "id": "m1", "sid": &sid }));
    assert!(
        message_texts(&messages).contains(&"one".to_string()),
        "B still reads the shared transcript after A left: {messages}"
    );
    // ...and can run its own turn to completion.
    prompt(&mut b, "p2", &sid, "second");
    let after = b.request(&json!({ "cmd": "get_messages", "id": "m2", "sid": &sid }));
    assert_eq!(
        message_texts(&after),
        vec!["one".to_string(), "two".to_string()],
        "B's turn landed after A left: {after}"
    );

    // B's explicit shutdown ends the daemon.
    let shutdown = b.request(&json!({ "cmd": "shutdown", "id": "sd" }));
    assert!(
        shutdown.contains("success=true"),
        "shutdown failed: {shutdown}"
    );
    assert_daemon_exits(&mut daemon);
}

/// The fork contract, pinned: when the last client disconnects WITHOUT a
/// shutdown, the daemon stays up and a fresh client reconnects to find the
/// session intact. This is the deliberate divergence from "exit on last
/// client" (upstream #3345) that the disconnect tests must not mis-assert.
#[test]
fn daemon_survives_the_last_client_and_a_fresh_client_reconnects() {
    let sandbox = Sandbox::new(&["kept alive"]);
    let mut daemon = sandbox.spawn();

    let sid = {
        let mut a = sandbox.client("A");
        let created =
            a.request(&json!({ "cmd": "create", "id": "c1", "config": sandbox.create_config() }));
        let sid = created_session(&created);
        attach(&mut a, "a1", &sid);
        prompt(&mut a, "p1", &sid, "keep");
        a.disconnect();
        sid
    };

    // The daemon is still alive after its only client left.
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        daemon.child.try_wait().ok().flatten().is_none(),
        "the daemon stays up when its last client disconnects (fork contract)"
    );
    assert!(
        UnixStream::connect(&sandbox.socket).is_ok(),
        "the daemon socket still accepts connections"
    );

    // A fresh client reconnects and finds the session intact.
    let mut c = sandbox.client("C");
    let messages = c.request(&json!({ "cmd": "get_messages", "id": "m1", "sid": &sid }));
    assert!(
        message_texts(&messages).contains(&"kept alive".to_string()),
        "a fresh client reconnects to the surviving session: {messages}"
    );

    let shutdown = c.request(&json!({ "cmd": "shutdown", "id": "sd" }));
    assert!(shutdown.contains("success=true"));
    assert_daemon_exits(&mut daemon);
}
