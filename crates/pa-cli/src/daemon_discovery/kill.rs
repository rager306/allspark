//! Process-kill primitives: every signal is identity-gated by the process start
//! id so a recycled pid can never be mistaken for the process discovery saw.

use std::path::Path;

use pa_core::platform::process::{kill_pid, Signal};
use pa_types::platform::process::{is_process_alive, process_start_id};

use super::{evaluate_shutdown_quiet_period, DaemonStateRoot, DiscoveredDaemonProcess};

/// How long the residual sweep may run before it declares the listener
/// set stuck.
const SHUTDOWN_CONVERGENCE_TIMEOUT_MS: u128 = 10_000;

/// Verified force-kill, hardened: SIGTERM, a 1s grace, then SIGKILL, then a
/// poll loop (25ms slices, 1s deadline) reporting the kill only once the
/// process is confirmed gone (zombies count as dead — [`is_process_alive`]).
/// Deliberate TS divergence (TS fires SIGKILL and returns without verifying):
/// a stop path must not claim a stop a D-state process never performed.
/// Whether the pid still names the process the caller resolved.
///
/// The same decision at BOTH ends of the ladder: before any signal, and again before the
/// SIGKILL escalation a second later (review #5). Extracted so the decision has one
/// definition and a test that can state what it means — the RACE itself (a pid recycled
/// inside that second) cannot be reproduced in-process, so what is pinned here is the rule,
/// plus a source check that the ladder consults it at both ends.
fn signal_identity_matches(expected_start_id: Option<&str>, pid: u32) -> bool {
    let Some(expected) = expected_start_id else {
        // No resolved identity: the caller's own gate is authoritative.
        return true;
    };
    process_start_id(pid).as_deref() == Some(expected)
}

/// The process identity the caller already resolved is re-checked IMMEDIATELY BEFORE the
/// SIGKILL escalation (review #5, residual), and once more before the escalation is allowed
/// at all.
///
/// The escalation waits a second after SIGTERM. That second is the window: a SIGTERM that
/// lands kills the daemon discovery named, the kernel may recycle its pid, and a blind
/// `SIGKILL` then reaches a process nobody asked to stop. `None` keeps the caller's own
/// gate authoritative (the residual sweep has already matched a socket's listener).
pub(super) fn force_kill_daemon_identified(pid: u32, expected_start_id: Option<&str>) -> bool {
    // The gate comes FIRST, not just before the escalation: if the pid already names a
    // different process, nothing here was asked to kill it, so no signal is sent at all.
    if !signal_identity_matches(expected_start_id, pid) {
        return true;
    }
    kill_daemon(pid);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // The pid now names a different process (or none): the daemon this call was asked to
    // kill is already gone, so the kill is complete and the escalation is not sent.
    if !signal_identity_matches(expected_start_id, pid) {
        return true;
    }
    let _ = kill_pid(pid as i32, Signal::Kill);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if !is_alive(pid) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

pub(super) fn kill_daemon(pid: u32) {
    if pid > 0 {
        let _ = kill_pid(pid as i32, Signal::Term);
    }
}

pub(super) fn is_alive(pid: u32) -> bool {
    is_process_alive(pid).unwrap_or(false)
}

/// Remove a socket file if present; false when the unlink fails. Never-touch
/// paths are refused here as the last line of containment.
pub(super) fn remove_socket_file(socket_path: &Path) -> bool {
    if super::is_never_touch(socket_path) {
        return false;
    }
    match std::fs::remove_file(socket_path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

/// Stop every worker tracked for one supervisor socket and clean up its records.
/// Returns the failure reasons. Workers are read from the given agent dir.
pub(super) fn force_stop_tracked_workers(
    supervisor_socket_path: &Path,
    agent_dir: &Path,
) -> Vec<String> {
    let mut failures = Vec::new();
    for worker in super::find_all_tracked_workers(agent_dir) {
        if worker.supervisor_socket_path != supervisor_socket_path {
            continue;
        }
        if !stop_tracked_process(worker.pid, worker.process_start_id.as_deref()) {
            failures.push(format!("could not safely stop worker (pid {})", worker.pid));
            continue;
        }
        let _ = remove_socket_file(&worker.worker_socket_path);
        let _ = std::fs::remove_file(&worker.descriptor_path);
        let _ = std::fs::remove_file(&worker.recovery_journal_path);
    }
    failures
}

/// Identity-gated stop: SIGTERM the worker (it exits keeping its resume entry),
/// then SIGKILL if it hangs. The start-id gate defeats pid reuse.
pub(super) fn stop_tracked_process(pid: u32, expected_start_id: Option<&str>) -> bool {
    if !is_alive(pid) {
        return true;
    }
    if let Some(expected) = expected_start_id {
        if process_start_id(pid).as_deref() != Some(expected) {
            return false;
        }
    }
    let _ = kill_pid(pid as i32, Signal::Term);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while is_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if !is_alive(pid) {
        return true;
    }
    if let Some(expected) = expected_start_id {
        if process_start_id(pid).as_deref() != Some(expected) {
            return false;
        }
    }
    let _ = kill_pid(pid as i32, Signal::Kill);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while is_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    !is_alive(pid)
}

/// The `--force` residual sweep: kill whatever product listeners remain in
/// the invocation's state root until the set quiets down or proves stuck;
/// report the survivors as failures. The sweep re-scans with the same root,
/// so daemons in any other root — or on the never-touch list — are never
/// candidates, and worker sockets never appear in the census (a supervisor's
/// workers stop through their tracked descriptors; deliberate TS divergence:
/// TS's sweep killed worker listeners too).
pub(super) fn terminate_verified_residuals(
    root: &DaemonStateRoot,
    stopped: &mut Vec<(String, String)>,
    failed: &mut Vec<(String, String)>,
    handled_pids: &std::collections::HashSet<u32>,
) {
    let started = std::time::Instant::now();
    let mut previous_signature: Option<String> = None;
    let mut quiet_since: Option<u128> = None;
    loop {
        let listeners = super::scan_listening_daemons(root);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        if listeners.is_empty() {
            previous_signature = None;
            quiet_since = quiet_since.or(Some(now));
            if evaluate_shutdown_quiet_period(now, quiet_since) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        quiet_since = None;
        let signature = listener_signature(&listeners);
        if started.elapsed().as_millis() >= SHUTDOWN_CONVERGENCE_TIMEOUT_MS {
            record_residuals(&listeners, failed, "kept respawning during shutdown");
            return;
        }
        if previous_signature.as_deref() == Some(signature.as_str()) {
            record_residuals(&listeners, failed, "remained after shutdown");
            return;
        }
        previous_signature = Some(signature);
        let mut seen_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for listener in &listeners {
            if !seen_pids.insert(listener.pid) {
                continue;
            }
            let already_reported = handled_pids.contains(&listener.pid);
            if terminate_verified_listener(listener) && !already_reported {
                stopped.push((
                    listener.socket_path.display().to_string(),
                    format!("stopped residual daemon process (pid {})", listener.pid),
                ));
            }
        }
    }
}

pub(super) fn record_residuals(
    listeners: &[DiscoveredDaemonProcess],
    failed: &mut Vec<(String, String)>,
    reason: &str,
) {
    for listener in listeners {
        let identity = process_start_id(listener.pid).map_or_else(
            || format!("pid {}, process identity unavailable", listener.pid),
            |start_id| format!("pid {}, start {start_id}", listener.pid),
        );
        failed.push((
            listener.socket_path.display().to_string(),
            format!("daemon {reason} ({identity})"),
        ));
    }
}

/// Kill one verified listener: the start-id gate must still name the
/// same process before and after the signal.
pub(super) fn terminate_verified_listener(listener: &DiscoveredDaemonProcess) -> bool {
    let Some(start_id) = process_start_id(listener.pid) else {
        return false;
    };
    if process_start_id(listener.pid).as_deref() != Some(start_id.as_str()) {
        return false;
    }
    let _ = kill_pid(listener.pid as i32, Signal::Term);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while process_start_id(listener.pid).as_deref() == Some(start_id.as_str())
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if process_start_id(listener.pid).as_deref() == Some(start_id.as_str()) {
        let _ = kill_pid(listener.pid as i32, Signal::Kill);
        // Verified death (the same hardening as `force_kill_daemon_identified`): a single
        // post-SIGKILL check races a slow teardown, so poll the identity.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while process_start_id(listener.pid).as_deref() == Some(start_id.as_str())
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    process_start_id(listener.pid).as_deref() != Some(start_id.as_str())
}

pub(super) fn listener_signature(listeners: &[DiscoveredDaemonProcess]) -> String {
    let mut parts: Vec<String> = listeners
        .iter()
        .map(|listener| {
            format!(
                "{}:{}:{}",
                listener.pid,
                process_start_id(listener.pid).unwrap_or_else(|| "unknown".to_string()),
                listener.socket_path.display()
            )
        })
        .collect();
    parts.sort();
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// A state root inside a test-created fixture dir; the ambient
    /// environment's real paths are never part of any sweep.
    fn fixture_root(dir: &Path) -> DaemonStateRoot {
        DaemonStateRoot {
            agent_dir: dir.join("agent"),
            socket_dir: Some(dir.join("agent").join("sockets")),
            default_socket_path: dir.join("agent").join("sockets").join("daemon.sock"),
        }
    }

    #[test]
    fn residual_sweep_over_an_empty_fixture_root_reports_nothing() {
        // No product daemon is spawned here: the sweep over an empty listener
        // set completes after the quiet period and reports nothing.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = fixture_root(tmp.path());
        let mut stopped: Vec<(String, String)> = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        terminate_verified_residuals(&root, &mut stopped, &mut failed, &HashSet::new());
        assert_eq!(stopped.len(), 0);
        assert_eq!(failed.len(), 0);
    }

    #[test]
    fn residual_sweep_never_touches_a_never_touch_dir() {
        // The ambient mission daemon's workers listen under these dirs; a
        // sweep rooted on them must see no listeners and harm nothing.
        for dir in ["/tmp/mission-tmp/prime-agent-1000", "/tmp/prime-agent-1000"] {
            let root = DaemonStateRoot {
                agent_dir: PathBuf::from(dir),
                socket_dir: Some(PathBuf::from(dir)),
                default_socket_path: PathBuf::from(dir).join("daemon.sock"),
            };
            let mut stopped: Vec<(String, String)> = Vec::new();
            let mut failed: Vec<(String, String)> = Vec::new();
            terminate_verified_residuals(&root, &mut stopped, &mut failed, &HashSet::new());
            assert!(
                stopped.is_empty(),
                "sweep rooted at {dir} must stop nothing"
            );
            assert!(failed.is_empty(), "sweep rooted at {dir} must fail nothing");
        }
    }

    fn spawn_term_ignoring_shell() -> std::process::Child {
        // A shell that swallows SIGTERM: the force-kill must escalate to SIGKILL
        // (the grandchild `sleep 1` exits on its own shortly after, no strays).
        std::process::Command::new("sh")
            .arg("-c")
            .arg("trap : TERM; while :; do sleep 1; done")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a TERM-ignoring shell")
    }

    #[test]
    fn a_killable_process_is_confirmed_dead_before_the_escalation() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a sleep child");
        assert!(
            force_kill_daemon_identified(child.id(), None),
            "a TERM-killable process must die before SIGKILL"
        );
        let _ = child.wait();
    }

    /// The identity rule, stated as a decision (review #5).
    ///
    /// The failure this finding names is a RACE — the pid is recycled inside the one-second
    /// grace between SIGTERM and SIGKILL — and no in-process test can produce that: the
    /// kernel assigns pid numbers, and a test cannot make the same number belong to another
    /// process while the ladder is sleeping. So the rule has its own definition and its own
    /// test, and a second test below pins that the ladder CONSULTS it at both ends; the two
    /// together discriminate the mutation the earlier version of this test missed (removing
    /// only the pre-SIGKILL check left a `Some("0")` fixture green, because the FIRST gate
    /// returned before the ladder ran).
    #[test]
    fn the_identity_rule_refuses_a_pid_that_no_longer_matches() {
        let mut child = spawn_term_ignoring_shell();
        let pid = child.id();
        let real = process_start_id(pid).expect("the fixture has a start id");
        assert!(
            signal_identity_matches(Some(&real), pid),
            "the same process passes the gate"
        );
        assert!(
            !signal_identity_matches(Some("0"), pid),
            "a different start id is a different process"
        );
        assert!(
            signal_identity_matches(None, pid),
            "no resolved identity leaves the caller's own gate authoritative"
        );
        assert!(
            force_kill_daemon_identified(pid, Some("0")),
            "the daemon the caller asked about is gone, so the kill is complete"
        );
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(
            is_alive(pid),
            "a pid whose start id no longer matches must receive NO signal at all"
        );
        assert!(
            force_kill_daemon_identified(pid, Some(&real)),
            "a matching identity escalates and confirms the death"
        );
        let _ = child.wait();
    }

    /// Both ends of the ladder consult the identity rule — and this pins the SOURCE, because
    /// the race the rule exists for cannot be staged from a test.
    ///
    /// `force_kill_daemon_identified` sends SIGTERM, sleeps a second, then escalates. The
    /// check before the SIGKILL is the one the wave-2 review removed in a mutation and the
    /// test did not notice; a pin on the call sites is what makes that removal fail.
    #[test]
    fn the_escalation_re_checks_the_identity_it_started_with() {
        let source = include_str!("kill.rs");
        let start = source
            .find("pub(super) fn force_kill_daemon_identified")
            .expect("the identified form exists");
        let end = source[start..]
            .find("\n}\n")
            .map(|at| start + at)
            .expect("the function ends");
        let body = &source[start..end];
        // The CONDITION, not a call count (review #5's own residual, measured): counting
        // `signal_identity_matches(` stays green under `... && false`, because the call is
        // still there — only its verdict stopped mattering. Each gate's condition must be the
        // call alone.
        let conditions: Vec<&str> = body
            .split("if !signal_identity_matches(expected_start_id, pid)")
            .skip(1)
            .filter_map(|rest| rest.split('{').next())
            .collect();
        assert_eq!(
            conditions.len(),
            2,
            "the ladder must consult the identity rule at BOTH ends — before any signal and \
             before the SIGKILL escalation; one check leaves the pid-reuse window open: {body}"
        );
        for condition in conditions {
            assert!(
                condition.trim().is_empty(),
                "the gate must hang on the identity verdict ALONE; anything appended can \
                 deactivate it (`&& false` keeps the call and admits the pid): {condition:?}"
            );
        }
    }

    #[test]
    fn a_term_ignoring_process_is_escalated_and_its_death_confirmed() {
        let mut child = spawn_term_ignoring_shell();
        // This child ignores the 1s SIGTERM grace, so the confirmed-death
        // verdict can only come from the post-SIGKILL verify loop.
        assert!(
            force_kill_daemon_identified(child.id(), None),
            "the post-SIGKILL verify loop must confirm the death"
        );
        let _ = child.wait();
    }

    #[test]
    fn an_already_dead_pid_is_reported_dead() {
        let mut child = std::process::Command::new("true")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a true child");
        let pid = child.id();
        let _ = child.wait();
        assert!(force_kill_daemon_identified(pid, None));
    }

    #[test]
    fn terminate_verified_listener_confirms_the_death_of_a_live_listener() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a sleep child");
        let pid = child.id();
        // A dead direct child lingers as a zombie until its parent reaps it, and
        // a zombie's start id never changes — the test reaps concurrently.
        let reaper = std::thread::spawn(move || child.wait());
        let listener = DiscoveredDaemonProcess {
            pid,
            socket_path: PathBuf::from("/tmp/never-a-listener.sock"),
            uptime_seconds: None,
        };
        assert!(terminate_verified_listener(&listener));
        let _ = reaper.join();
    }

    #[test]
    fn never_touch_socket_files_are_refused() {
        assert!(!remove_socket_file(Path::new(
            "/tmp/mission-daemon/daemon.sock"
        )));
        assert!(remove_socket_file(Path::new(
            "/tmp/discovery-no-such-socket.sock"
        )));
    }
}
