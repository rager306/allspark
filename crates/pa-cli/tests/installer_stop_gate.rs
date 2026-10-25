//! The installer's daemon stop gate, pinned against the script's own probe:
//! which daemons an update may stop, and which it must leave running.
//!
//! Two rules are load-bearing here, both from the 2026-10-21 field incident
//! (a fork update stopped the co-installed upstream Rust daemon and killed
//! the session that ran it):
//!
//! 1. THE INSTALL-ROOT GATE: a Rust daemon is stopped only when its executable
//!    lives under the prefix this run writes. The pinned rust socket names the
//!    upstream Rust install's endpoint, so a co-installed upstream tree
//!    answers there; identity cannot tell the two installs apart (the fork
//!    builds the same crates, hence the same `pa-daemon-rs-<version>` build
//!    id and schema) — the isolated HOME, hence a different prefix, does.
//! 2. THE ANCESTOR FALLBACK: the self-socket refusal must survive a scrubbed
//!    environment (`env -i`), so the probe also names the daemon whose
//!    `--mode daemon --daemon-socket <socket>` command line owns this
//!    process tree.
//!
//! The probe is EXTRACTED from install-rust.sh and driven as the script runs
//! it (`--kind=ours --prefix=<prefix> <socket>`), against a fake daemon on a
//! temp socket, so the pin holds the shipped text rather than a copy.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The repo root (crates/pa-cli -> crates -> root).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .expect("worktree root")
}

/// The installer script these pins read: the working tree's, or
/// `PRIME_AGENT_INSTALLER_SCRIPT`'s copy — the red-first run points it at the
/// pre-fix script from git, so the tests are proven to fail on the code they
/// guard without editing the working tree.
fn installer_script_text() -> String {
    let script = std::env::var_os("PRIME_AGENT_INSTALLER_SCRIPT")
        .map_or_else(|| repo_root().join("install-rust.sh"), PathBuf::from);
    std::fs::read_to_string(&script)
        .unwrap_or_else(|error| panic!("read the installer script {}: {error}", script.display()))
}

/// The probe's python source, verbatim from the installer's heredoc: the
/// exact bytes the shipped script runs.
fn probe_source() -> String {
    let text = installer_script_text();
    let marker = "cat > \"$probe_py\" <<'PROBE_PY'\n";
    let start = text.find(marker).expect("the probe heredoc") + marker.len();
    let end = start
        + text[start..]
            .find("\nPROBE_PY\n")
            .expect("the probe heredoc's end");
    text[start..end].to_string()
}

/// The probe as a runnable file in `dir`, or `None` when the machine has no
/// python3 (the installer itself bootstraps one through uv; the test drives
/// the probe directly).
fn probe_file(dir: &Path) -> Option<PathBuf> {
    let python = [
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "/opt/homebrew/bin/python3",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())?;
    let path = dir.join("daemon-stop.py");
    std::fs::write(&path, probe_source()).expect("write the probe");
    let status = Command::new(python)
        .arg("-m")
        .arg("py_compile")
        .arg(&path)
        .status();
    assert!(
        status.is_ok_and(|status| status.success()),
        "the extracted probe must compile"
    );
    Some(path)
}

/// A fake supervisor: one hello per connection naming `executable`, then the
/// envelope's reply. Records every command type it is sent (a stop gate test
/// asserts that `shutdown` never arrives) and stops listening on a shutdown
/// request, the way the real daemon does.
struct FakeDaemon {
    socket: PathBuf,
    commands: Arc<std::sync::Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeDaemon {
    fn start(dir: &Path, executable: &Path) -> Self {
        std::fs::create_dir_all(dir).expect("fake daemon dir");
        let socket = dir.join("daemon.sock");
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("bind the fake daemon");
        listener
            .set_nonblocking(true)
            .expect("non-blocking accept loop");
        let commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let hello_exe = executable.to_string_lossy().to_string();
        let seen = Arc::clone(&commands);
        let flag = Arc::clone(&stop);
        let socket_for_thread = socket.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => return,
                };
                if serve(stream, &hello_exe, &seen) {
                    // The shutdown request: the real daemon stops listening
                    // and its socket stops answering.
                    let _ = std::fs::remove_file(&socket_for_thread);
                    return;
                }
            }
        });
        FakeDaemon {
            socket,
            commands,
            stop,
            thread: Some(thread),
        }
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("commands lock").clone()
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Serve one connection; `true` when it carried a shutdown command.
fn serve(stream: UnixStream, hello_exe: &str, commands: &std::sync::Mutex<Vec<String>>) -> bool {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut writer = stream.try_clone().expect("clone the stream");
    let hello = format!(
        "{{\"type\":\"daemon_hello\",\"protocol\":{{\"name\":\"prime-agent.daemon\",\"version\":7}},\
         \"schemaId\":\"protocol-7-schema-30-8e4b17c2a9f5\",\
         \"runtime\":{{\"buildId\":\"pa-daemon-rs-0.0.0\",\"executablePath\":\"{hello_exe}\"}},\
         \"supervisorPid\":{pid}}}\n",
        pid = std::process::id()
    );
    if writer.write_all(hello.as_bytes()).is_err() || writer.flush().is_err() {
        return false;
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if matches!(reader.read_line(&mut line), Ok(0) | Err(_)) {
        return false;
    }
    let envelope: serde_json::Value = match serde_json::from_str(&line) {
        Ok(value) => value,
        Err(_) => return false,
    };
    let command = envelope
        .get("command")
        .and_then(|command| command.get("type"))
        .and_then(|kind| kind.as_str())
        .unwrap_or("")
        .to_string();
    commands
        .lock()
        .expect("commands lock")
        .push(command.clone());
    let data = if command == "list" {
        "{\"sessions\":[{},{}]}".to_string()
    } else {
        "{}".to_string()
    };
    let reply = format!(
        "{{\"type\":\"response\",\"id\":{id},\"success\":true,\"data\":{data}}}\n",
        id = envelope.get("id").cloned().unwrap_or_default()
    );
    let _ = writer.write_all(reply.as_bytes());
    let _ = writer.flush();
    command == "shutdown"
}

/// Run the probe exactly as the installer does for an install-owned socket:
/// `--prefix` is the run's prefix and `--payload-dir` the invoking payload's
/// tree (both are install roots; either may be empty, as in a standalone
/// `curl | sh` run).
fn run_probe(probe: &Path, socket: &Path, prefix: &Path, payload_dir: &str) -> Output {
    Command::new(
        [
            "/usr/bin/python3",
            "/usr/local/bin/python3",
            "/opt/homebrew/bin/python3",
        ]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .expect("python3"),
    )
    .arg(probe)
    .arg("--kind=ours")
    .arg("--force=0")
    .arg(format!("--prefix={}", prefix.display()))
    .arg(format!("--payload-dir={payload_dir}"))
    .arg(socket)
    .env_clear()
    .env("PATH", "/usr/bin:/bin")
    .output()
    .expect("run the probe")
}

fn run_probe_fork(probe: &Path, socket: &Path, prefix: &Path, payload_dir: &str) -> Output {
    Command::new(
        [
            "/usr/bin/python3",
            "/usr/local/bin/python3",
            "/opt/homebrew/bin/python3",
        ]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .expect("python3"),
    )
    .arg(probe)
    .arg("--kind=ours")
    .arg("--force=0")
    .arg("--fork-run")
    .arg(format!("--prefix={}", prefix.display()))
    .arg(format!("--payload-dir={payload_dir}"))
    .arg(socket)
    .env_clear()
    .env("PATH", "/usr/bin:/bin")
    .output()
    .expect("run the probe")
}

fn verdict(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Wait for the daemon's first recorded command (the probe's own request may
/// land a moment after its verdict prints).
fn wait_for_commands(daemon: &FakeDaemon) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let commands = daemon.commands();
        if !commands.is_empty() {
            return commands;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Vec::new()
}

/// THE INSTALL-ROOT GATE, both directions: a daemon whose executable lives
/// under this install's prefix is stopped (the update flow's whole point), and
/// a daemon from another install — the co-installed upstream tree of the
/// 2026-10-21 incident — is reported as foreign and NEVER commanded, not even
/// with the session-count `list`.
#[test]
fn the_probe_stops_only_a_daemon_running_this_installs_payload() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let Some(probe) = probe_file(dir.path()) else {
        eprintln!("skipping: no python3 for the installer's probe");
        return;
    };
    // The install this run writes: the script's PREFIX.
    let prefix = dir.path().join("prefix");
    let payload_dir = prefix.join("share/prime-agent");
    std::fs::create_dir_all(&payload_dir).expect("payload dir");
    let payload = payload_dir.join("prime-agent");
    std::fs::write(&payload, b"payload").expect("payload file");

    // This install's own payload: stopped, with the session count read.
    let own = FakeDaemon::start(&dir.path().join("own"), &payload);
    let output = run_probe(&probe, &own.socket, &prefix, "");
    assert_eq!(
        verdict(&output),
        "rust:stopped:2",
        "the install's own daemon is stopped: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        wait_for_commands(&own),
        vec!["list".to_string(), "shutdown".to_string()],
        "the owned daemon receives the count and the graceful shutdown"
    );

    // Another install's daemon on this product's socket: left running.
    let foreign_dir = dir.path().join("foreign");
    std::fs::create_dir_all(&foreign_dir).expect("foreign dir");
    let upstream_payload = dir.path().join("upstream/share/prime-agent/prime-agent");
    std::fs::create_dir_all(upstream_payload.parent().expect("upstream dir"))
        .expect("upstream payload dir");
    std::fs::write(&upstream_payload, b"payload").expect("upstream payload file");
    let foreign = FakeDaemon::start(&foreign_dir, &upstream_payload);
    let output = run_probe(&probe, &foreign.socket, &prefix, "");
    // The gate refuses the candidate at the hello: the count request never
    // follows, so the daemon sees no command at all.
    assert_eq!(
        verdict(&output),
        format!("foreign:{}", upstream_payload.display()),
        "another install's daemon is named, not stopped: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        foreign.commands().is_empty(),
        "the other install's daemon must never be commanded: {:?}",
        foreign.commands()
    );

    // The fork's launcher layout: the payload lives in libexec/ BESIDE the
    // prefix, not under it, so the prefix alone would call this install's own
    // daemon foreign. The invoking payload's tree is the second install root
    // and must recognise it.
    let fork_root = dir.path().join("fork");
    let fork_payload = fork_root.join("libexec/prime-agent");
    std::fs::create_dir_all(fork_payload.parent().expect("fork payload dir"))
        .expect("fork libexec dir");
    std::fs::write(&fork_payload, b"payload").expect("fork payload file");
    let own_payload_dir = fork_payload.parent().expect("payload dir");
    let fork_socket_prefix = fork_root.join("share");
    let fork_state = FakeDaemon::start(&dir.path().join("fork-state"), &fork_payload);
    let output = run_probe(
        &probe,
        &fork_state.socket,
        &fork_socket_prefix,
        &own_payload_dir.to_string_lossy(),
    );
    assert_eq!(
        verdict(&output),
        "rust:stopped:2",
        "the invoking payload's tree is this install's own: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // THE UPSTREAM TAKEOVER still works: a trunk `prime-agent update` run
    // writes into the rust install root (~/.local, or PRIME_AGENT_RUST_PREFIX),
    // so that install's own daemon IS this run's to stop — the rule keys on the
    // install root, never on which product built the binary.
    let upstream_root = dir.path().join("upstream-root");
    let takeover_payload = upstream_root.join("share/prime-agent/prime-agent");
    std::fs::create_dir_all(takeover_payload.parent().expect("takeover payload dir"))
        .expect("takeover payload dir");
    std::fs::write(&takeover_payload, b"payload").expect("takeover payload file");
    let takeover = FakeDaemon::start(&dir.path().join("takeover-state"), &takeover_payload);
    let output = run_probe(&probe, &takeover.socket, &upstream_root, "");
    assert_eq!(
        verdict(&output),
        "rust:stopped:2",
        "a run writing into the rust install root stops that root's own daemon: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// THE ANCESTOR FALLBACK (the `env -i` hole): the self-socket refusal reads
/// env vars a daemon exports, which a scrubbed invocation drops. The probe
/// must still name the daemon whose `--mode daemon --daemon-socket <socket>`
/// command line owns this process tree — and name nothing when no ancestor is
/// a daemon.
#[test]
fn the_probe_names_the_daemon_this_process_tree_runs_under() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let Some(probe) = probe_file(dir.path()) else {
        eprintln!("skipping: no python3 for the installer's probe");
        return;
    };
    let python = [
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "/opt/homebrew/bin/python3",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
    .expect("python3");

    // No daemon-shaped ancestor: the walk answers nothing. (This test binary
    // itself may run under a daemon when the whole suite is daemon-hosted —
    // then only the daemon case below is observable; assert it only when the
    // tree is plain.)
    let bare = Command::new(python)
        .arg(&probe)
        .arg("--ancestor-daemon-socket")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run the walk");
    let host_socket = verdict(&bare);
    assert!(
        host_socket.is_empty() || host_socket.starts_with('/'),
        "the walk answers a socket path or nothing: {host_socket}"
    );
    // A ts daemon has no executable path; a plain tree answers nothing.
    assert!(
        !host_socket.contains("stale") && !host_socket.contains("error"),
        "the walk never prints a probe verdict: {host_socket}"
    );

    // An ancestor whose argv[0] is a product binary and whose command line
    // carries the daemon flags: the walk names its socket. The product name
    // is what the walk gates on, so a copy of `sh` under that name is the
    // cheapest honest stand-in for the supervisor process (a real supervisor
    // must not be spawned here).
    let socket = dir.path().join("owned.sock");
    let daemon_named = dir.path().join("prime-agent");
    // The stand-in's OWN argv: argv[0] is the product name and the first
    // argument is the daemon role — exactly the shipped shape
    // (`<payload> --mode daemon --daemon-socket <socket>`).
    // The stand-in must stay the probe's parent, or its own argv would be
    // replaced by the probe's and the walk would see only python: no `exec`,
    // and a trailing no-op so `sh` cannot execute the last command away.
    std::fs::write(
        &daemon_named,
        format!(
            "#!/bin/sh\n{} {} --ancestor-daemon-socket > {}\n:\n",
            python,
            probe.display(),
            dir.path().join("walk.out").display()
        ),
    )
    .expect("write the daemon stand-in");
    make_executable(&daemon_named);
    let status = Command::new(&daemon_named)
        .args(["--mode", "daemon", "--daemon-socket"])
        .arg(&socket)
        .stdin(Stdio::null())
        .status()
        .expect("run the walk under a daemon-shaped ancestor");
    assert!(status.success(), "the daemon-shaped ancestor runs");
    let walked = std::fs::read_to_string(dir.path().join("walk.out")).expect("walk output");
    assert_eq!(
        walked.trim(),
        socket.to_string_lossy(),
        "the walk names the ancestor daemon's socket"
    );
}

/// THE FIELD RELAPSE of the 2026-10-21 incident (regression): a fork run and the
/// upstream `Rust` install share the same PREFIX, and the upstream lives at
/// PREFIX/share/prime-agent — INSIDE the prefix. A fork update run from a bare
/// shell (HOME=/root -> PREFIX=/root/.local) therefore claimed the whole prefix
/// as its own and stopped the co-installed upstream daemon on
/// /tmp/prime-agent-rust-<uid>/daemon.sock. With --fork-run the fork owns ONLY
/// its payload tree (libexec/prime-agent); the prefix root is dropped.
#[test]
fn a_fork_run_does_not_claim_the_prefix_shared_with_the_upstream_install() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let Some(probe) = probe_file(dir.path()) else {
        eprintln!("skipping: no python3 for the installer's probe");
        return;
    };
    // The fork layout: payload in libexec/, prefix elsewhere (the shared one).
    let shared_prefix = dir.path().join("shared-local");
    let fork_payload = shared_prefix.join("libexec/prime-agent");
    std::fs::create_dir_all(fork_payload.parent().expect("libexec dir")).expect("fork payload dir");
    std::fs::write(&fork_payload, b"payload").expect("fork payload file");

    // The upstream `Rust` install: PREFIX/share/prime-agent — inside the shared
    // prefix, OUTSIDE the fork's payload tree.
    let upstream_payload = shared_prefix.join("share/prime-agent/prime-agent");
    std::fs::create_dir_all(upstream_payload.parent().expect("upstream dir"))
        .expect("upstream payload dir");
    std::fs::write(&upstream_payload, b"payload").expect("upstream payload file");
    let upstream = FakeDaemon::start(&dir.path().join("upstream-state"), &upstream_payload);

    let output = run_probe_fork(
        &probe,
        &upstream.socket,
        &shared_prefix,
        fork_payload.parent().unwrap().to_str().unwrap(),
    );
    assert_eq!(
        verdict(&output),
        format!("foreign:{}", upstream_payload.display()),
        "a fork run must not claim the prefix the upstream install lives under: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        upstream.commands().is_empty(),
        "the upstream daemon must never be commanded by a fork run: {:?}",
        upstream.commands()
    );

    // The fork's OWN daemon (under its payload tree) is still stopped.
    let own = FakeDaemon::start(&dir.path().join("own-state"), &fork_payload);
    let output = run_probe_fork(
        &probe,
        &own.socket,
        &shared_prefix,
        fork_payload.parent().unwrap().to_str().unwrap(),
    );
    assert_eq!(
        verdict(&output),
        "rust:stopped:2",
        "the fork's own daemon is stopped: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("make executable");
}

/// The stop handler's source block, verbatim: `stop_daemon_candidate` from
/// its definition through the outer `esac`, so the pin holds the shipped text.
fn stop_handler_block() -> String {
    let text = installer_script_text();
    let start = text
        .find("stop_daemon_candidate() {")
        .expect("the stop candidate handler");
    let end = start
        + text[start..]
            .find("\n  esac\n}\n")
            .expect("the handler's outer esac");
    text[start..end + "\n  esac\n}\n".len()].to_string()
}

/// THE FOREIGN VERDICT'S REPORT CONTRACT (the shell half): a `foreign:`
/// answer must land in the summary as a skip, must NOT record a stop (the
/// post-stop verify and the "previous Rust daemon was stopped for this
/// update" next-step line both key off that record), and must never be
/// counted as a stopped Rust daemon. The probe's own gate is the test above;
/// this pins what the operator reads.
#[test]
fn a_foreign_daemon_is_reported_as_a_skip_and_never_as_a_stop() {
    let block = stop_handler_block();
    let harness = format!(
        "#!/bin/sh\n\
         set -u\n\
         {block}\n\
         say() {{ printf 'say: %s\\n' \"$*\"; }}\n\
         note() {{ printf 'note: %s\\n' \"$*\"; }}\n\
         todo() {{ printf 'todo: %s\\n' \"$*\"; }}\n\
         step_start() {{ :; }}\n\
         step_ok() {{ :; }}\n\
         step_fail() {{ :; }}\n\
         step_clear() {{ :; }}\n\
         PREFIX=\"/fixture/prefix\"\n\
         FORCE=0\n\
         ui_pid=\"\"\n\
         probe_py=/dev/null\n\
         live_daemon_socket=\"\"\n\
         install_payload_dir=\"\"\n\
         ts_stop_summary=\"\"\n\
         ts_stop_found_any=\"\"\n\
         ts_stop_refused=\"\"\n\
         ts_stop_rust_stopped=\"\"\n\
         printf 'SOCKET=[%s] EXISTS=%s\\n' \"$SOCKET\" \"$(test -e \"$SOCKET\" && echo yes || echo no)\"\n\
         stop_daemon_candidate \"$SOCKET\" ours\n\
         printf 'SUMMARY=%s' \"$ts_stop_summary\"\n\
         printf 'RECORDED=%s RUST_STOPPED=%s\\n' \"$last_stop_recorded\" \"$ts_stop_rust_stopped\"\n"
    );
    let dir = tempfile::tempdir().expect("scratch dir");
    let path = dir.path().join("handler.sh");
    std::fs::write(&path, harness).expect("write the harness");
    // A stub probe (the UVPY slot): the handler's report contract is the
    // subject, so the verdict is exactly the case's.
    let stub = dir.path().join("stub-probe");
    std::fs::write(&stub, "#!/bin/sh\nprintf '%s\\n' \"$STUB_VERDICT\"\n")
        .expect("write the stub probe");
    make_executable(&stub);
    // A REAL unix socket: the handler probes `-e` and the shipped script's
    // sockets are socket files, so the harness must not need a special case.
    let socket = dir.path().join("daemon.sock");
    let _listener = UnixListener::bind(&socket).expect("bind the fixture socket");

    let run = |verdict: &str| {
        Command::new("sh")
            .arg(&path)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("SOCKET", &socket)
            .env("UVPY", &stub)
            .env("STUB_VERDICT", verdict)
            .output()
            .expect("run the handler harness")
    };

    let foreign = run("foreign:/home/someone/.local/share/prime-agent/prime-agent");
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&foreign.stdout),
        String::from_utf8_lossy(&foreign.stderr)
    );
    assert!(
        output.contains("SUMMARY=daemon: left running on") && output.contains("another install"),
        "the foreign daemon lands in the summary as a skip: {output}"
    );
    assert!(
        output.contains("RECORDED=no") && output.contains("RUST_STOPPED="),
        "a skipped foreign daemon records no stop and no rust stop: {output}"
    );

    let own = run("rust:stopped:0");
    let output = String::from_utf8_lossy(&own.stdout).to_string();
    assert!(
        output.contains("RECORDED=yes") && output.contains("RUST_STOPPED=yes"),
        "the install's own daemon records its stop: {output}"
    );
}

// ── THE FORK INSTALL LAYOUT (regressions) ────────────────────────────────────
//
// Three field failures happened because the installer was upstream-shaped while
// the fork's release archive is not:
//
//   1. `allspark update` ran the UPSTREAM installer (no fork channel in the
//      installed launcher) and it stopped BOTH daemons (measured 2026-10-24:
//      allspark 14:18:01, upstream 14:18:06).
//   2. The fork archive packs `bin/allspark` + `libexec/prime-agent`, while the
//      installer demanded `<stage>/prime-agent` — its own release could not be
//      installed by its own installer.
//   3. The archive was named `linux-x86_64` while the installer's channel
//      contract is `linux-x64`.
//
// These pin the three so a future edit cannot silently restore the mismatch.

/// The installer must derive a fork layout (payload tree + launcher name) from
/// the same signals the stop gate uses.
#[test]
fn the_installer_carries_a_fork_layout_branch() {
    let text = installer_script_text();
    assert!(
        text.contains("fork_layout=\"no\""),
        "the installer must compute its fork layout separately from the stop gate"
    );
    assert!(
        text.contains("share_dir=\"${PREFIX}/libexec\""),
        "a fork install publishes its payload under libexec/, never over upstream's share/prime-agent"
    );
    assert!(
        text.contains("launcher=\"${bin_dir}/allspark\""),
        "a fork install writes bin/allspark, not bin/prime-agent"
    );
    // And the fork branch must ALSO carry the isolated-home/socket contract,
    // because this launcher is what a release install runs.
    for needle in [
        "ALLSPARK_HOME",
        "allspark-daemon.sock",
        "PRIME_AGENT_DOWNLOAD_BASE_URL",
    ] {
        assert!(
            text.contains(needle),
            "the fork launcher must carry {needle}: a release install has no other place to set it"
        );
    }
}

/// Both shipped archive layouts must be accepted, and the published tree must be
/// the payload's own directory (never the whole stage, which would nest another
/// bin/ + libexec/ under libexec/).
#[test]
fn the_installer_accepts_both_archive_layouts() {
    let text = installer_script_text();
    assert!(
        text.contains("stage}/libexec/${BINARY_NAME}"),
        "the payload lookup must accept the fork layout (libexec/prime-agent)"
    );
    assert!(
        text.contains("payload_root=\"$(dirname \"$payload_src\")\""),
        "the publish must rename the payload's own directory, not the stage root"
    );
    assert!(
        text.contains("mv \"$payload_root\" \"$share_dir\""),
        "the publish must use payload_root (a fork stage root would nest bin/ under libexec/)"
    );
}

/// The channel platform name is part of the wire contract between the release
/// workflow's manifest and the installer's expectation.
#[test]
fn the_channel_platform_name_matches_the_installer() {
    let text = installer_script_text();
    assert!(
        text.contains("CHANNEL_PLATFORM=linux-x64"),
        "the installer expects the linux-x64 platform name; the release must publish that spelling"
    );
    let workflow = std::fs::read_to_string(repo_root().join(".github/workflows/release-fork.yml"))
        .expect("read the fork release workflow");
    assert!(
        !workflow.contains("linux-x86_64"),
        "the release workflow must not publish linux-x86_64: the installer's contract is linux-x64"
    );
    assert!(
        workflow.contains("\"platform\": \"linux-x64\""),
        "the channel manifest's platform row must match CHANNEL_PLATFORM"
    );
    for needle in [
        "dist/stable",
        "dist/latest.json",
        "        stable\n",
        "        latest.json\n",
    ] {
        assert!(
            workflow.contains(needle),
            "the archive upload must carry the channel assets ({needle} missing)"
        );
    }
}

/// The completion line names the product the user actually types.
#[test]
fn the_completion_line_names_the_installed_launcher() {
    let text = installer_script_text();
    assert!(
        text.contains("Run %sallspark%s to start"),
        "a fork install must tell the user to run allspark"
    );
    assert!(
        text.contains("fork_layout") && text.contains("if [ \"$fork_layout\" = \"yes\" ]"),
        "the naming must be branched on the fork layout, not applied unconditionally"
    );
}

/// A fork build's `--version` answers `allspark <product> (fork build fork.NNN;
/// upstream prime-agent <product>)`; the version pre-flight must compare the
/// PRODUCT version, not the literal archive-name string.
#[test]
fn the_version_preflight_understands_the_fork_identity_line() {
    let text = installer_script_text();
    assert!(
        text.contains("allspark\\ *)"),
        "the pre-flight must recognise the fork identity line"
    );
    assert!(
        text.contains("expected_version=\"${expected_version%-fork.*}\""),
        "the pre-flight must strip the fork build id before comparing versions"
    );
}
