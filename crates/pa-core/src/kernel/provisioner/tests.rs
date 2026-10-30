//! The provisioner's tests, in the order the contracts sit in the code: the
//! settings a real session hands its kernel, then the stop-arm and settle
//! edges, then the retry policy and the boot outcomes.
//!
//! Every test here is the one that stood in the single-file module; the split
//! moved them, it did not change them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::anyhow;

use crate::kernel::bootstrap::KernelPythonSkill;
use crate::kernel::shared::BOOTSTRAP_EXECUTION_TIMEOUT_MS;

use super::settle::{release_spent_stop_arm, settle_decision, SettleDecision};
use super::startup::{
    default_kernel_boot_concurrency, hold_snapshot_flush_gate, resolve_kernel_boot_concurrency,
    startup_failure_is_retryable,
};
use super::*;

/// A shell file the resolver accepts: existing, and executable on unix.
fn executable_shell(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// The scripted session both shell-setting tests build: a model and
/// `stream_fn` the settings validation never reaches, no kernel prewarm.
async fn shell_test_session(
    cwd: PathBuf,
    agent_dir: PathBuf,
) -> anyhow::Result<crate::session_engine::engine::SessionEngine> {
    use crate::session_engine::engine::{create_session, SessionEngineConfig};

    let model = pa_agent::types::Model {
        id: "shell-test".into(),
        name: "Shell test".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 1_000,
        max_tokens: 100,
        max_tokens_explicit: false,
    };
    let provider = Arc::new(pa_agent::scripted::ScriptedProvider::new(model.clone()));
    create_session(SessionEngineConfig {
        cwd,
        agent_dir,
        model: Some(model),
        stream_fn: Some(provider.stream_fn()),
        prewarm_ipython_kernel: Some(false),
        ..Default::default()
    })
    .await
}

#[tokio::test]
async fn session_settings_reach_the_kernel_shell() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
    // Project settings override the global setting before the session captures it.
    let global = executable_shell(dir.path(), "global-bash");
    let project = executable_shell(dir.path(), "project-bash");
    std::fs::write(
        agent_dir.join("settings.json"),
        serde_json::json!({ "shellPath": global.to_string_lossy() }).to_string(),
    )
    .unwrap();
    std::fs::write(
        cwd.join(".prime/agent/settings.json"),
        serde_json::json!({ "shellPath": format!("  {}  ", project.to_string_lossy()) })
            .to_string(),
    )
    .unwrap();
    let engine = shell_test_session(cwd, agent_dir).await.unwrap();
    assert_eq!(engine.provisioner.inner.options.shell_path, Some(project));
}

#[tokio::test]
async fn session_settings_reach_the_kernel_command_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
    std::fs::write(
        agent_dir.join("settings.json"),
        serde_json::json!({ "shellCommandPrefix": "shopt -s expand_aliases" }).to_string(),
    )
    .unwrap();
    let engine = shell_test_session(cwd, agent_dir).await.unwrap();
    assert_eq!(
        engine.provisioner.inner.options.command_prefix.as_deref(),
        Some("shopt -s expand_aliases")
    );
}

#[tokio::test]
async fn a_stored_serper_key_reaches_the_kernel_env() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{ "serper": { "type": "api_key", "key": "!printf key-from-cmd" } }"#,
    )
    .unwrap();
    let engine = shell_test_session(cwd, agent_dir).await.unwrap();
    let injected = engine
        .provisioner
        .inner
        .options
        .env
        .get("SERPER_API_KEY")
        .map(String::as_str);
    // A nonblank ambient SERPER_API_KEY wins (TS parity): the ambient key
    // reaches the kernel through the inherited env, not the wiring.
    if std::env::var("SERPER_API_KEY").is_ok_and(|value| !value.trim().is_empty()) {
        assert_eq!(injected, None);
    } else {
        assert_eq!(injected, Some("key-from-cmd"));
    }
}

#[tokio::test]
async fn an_unsaved_session_blanks_the_kernel_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
    let engine = shell_test_session(cwd, agent_dir).await.unwrap();
    assert_eq!(
        engine
            .provisioner
            .inner
            .options
            .env
            .get("RLM_SESSION_ID")
            .map(String::as_str),
        Some(""),
        "an inherited RLM_SESSION_ID must not reach an unsaved session's kernel"
    );
}

#[tokio::test]
async fn without_a_websearch_skill_no_serper_key_env() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
    std::fs::write(
        agent_dir.join("settings.json"),
        serde_json::json!({ "bundledSkills": { "websearch": false } }).to_string(),
    )
    .unwrap();
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{ "serper": { "type": "api_key", "key": "!printf key-from-cmd" } }"#,
    )
    .unwrap();
    let engine = shell_test_session(cwd, agent_dir).await.unwrap();
    assert!(
        !engine
            .provisioner
            .inner
            .options
            .env
            .contains_key("SERPER_API_KEY"),
        "the key rides the env only when a websearch skill is loaded"
    );
}

#[tokio::test]
async fn session_settings_reject_a_missing_shell_path() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
    let missing = cwd.join("no-such-shell");
    std::fs::write(
        cwd.join(".prime/agent/settings.json"),
        serde_json::json!({ "shellPath": missing.to_string_lossy() }).to_string(),
    )
    .unwrap();
    let Err(err) = shell_test_session(cwd, agent_dir).await else {
        panic!("create_session must reject a missing shellPath");
    };
    let message = err.to_string();
    assert!(
        message.contains("shellPath"),
        "error names the setting: {message}"
    );
    assert!(
        message.contains(missing.to_string_lossy().as_ref()),
        "error names the path: {message}"
    );
}

#[test]
fn a_spent_stop_arm_releases_only_its_own_boot() {
    let provisioner = IpythonKernelProvisioner::new(
        PathBuf::from("/tmp"),
        IpythonKernelProvisionerOptions::default(),
    );
    let (armed_tx, armed_rx) = tokio::sync::watch::channel(None::<StartupResult>);
    let (other_tx, other_rx) = tokio::sync::watch::channel(None::<StartupResult>);
    drop((armed_tx, other_tx));
    {
        let mut state = provisioner.lock_state();
        state.pending_stop_for_startup = Some(armed_rx.clone());
        release_spent_stop_arm(&mut state, &other_rx);
        assert!(
            state.pending_stop_for_startup.is_some(),
            "an arm armed for a different boot is not spent by this boot's settle"
        );
        release_spent_stop_arm(&mut state, &armed_rx);
        assert!(
            state.pending_stop_for_startup.is_none(),
            "the arm armed for this boot is spent by its settle"
        );
    }
}

#[test]
fn kill_releases_the_stop_arm_armed_for_the_killed_boot() {
    let provisioner = IpythonKernelProvisioner::new(
        PathBuf::from("/tmp"),
        IpythonKernelProvisionerOptions::default(),
    );
    let (memo_tx, memo_rx) = tokio::sync::watch::channel(None::<StartupResult>);
    drop(memo_tx);
    {
        let mut state = provisioner.lock_state();
        state.startup = Some(memo_rx.clone());
        state.pending_stop_for_startup = Some(memo_rx);
    }
    provisioner.kill();
    let state = provisioner.lock_state();
    assert!(state.startup.is_none(), "kill() invalidates the memo");
    assert!(
        state.pending_stop_for_startup.is_none(),
        "kill() releases the stop arm armed for the boot it invalidates"
    );
    assert_eq!(
        state.doomed_startups.len(),
        1,
        "kill() parks the invalidated memo for dispose() to wait"
    );
}

#[test]
fn a_killed_boot_never_takes_the_dispose_teardown() {
    let provisioner = IpythonKernelProvisioner::new(
        PathBuf::from("/tmp"),
        IpythonKernelProvisionerOptions::default(),
    );
    {
        let mut state = provisioner.lock_state();
        state.disposed = true;
        state.dispose_snapshot = true;
    }
    // A killed generation tears down with kill() semantics even under a
    // racing dispose: the dispose's own snapshot policy cannot resurrect
    // it. The order is the contract — checking disposed first would
    // flush a snapshot kill() forbade.
    {
        let state = provisioner.lock_state();
        assert_eq!(settle_decision(&state, false), SettleDecision::Kill);
        // A live generation honors the dispose; a live undisposed one
        // parks.
        assert_eq!(settle_decision(&state, true), SettleDecision::Dispose);
    }
    {
        let mut state = provisioner.lock_state();
        state.disposed = false;
    }
    let state = provisioner.lock_state();
    assert_eq!(settle_decision(&state, true), SettleDecision::Publish);
}

#[test]
fn a_doomed_boot_teardown_never_flushes_a_snapshot() {
    let provisioner = IpythonKernelProvisioner::new(
        PathBuf::from("/tmp"),
        IpythonKernelProvisionerOptions::default(),
    );
    let (memo_tx, memo_rx) = tokio::sync::watch::channel(None::<StartupResult>);
    drop(memo_tx);
    // A live boot's failed teardown keeps the dispose policy (true by
    // default) and installs the revival gate for its own flush: a
    // replacement boot that arms after this decision restores only
    // after the flush settles.
    {
        let mut state = provisioner.lock_state();
        state.startup = Some(memo_rx.clone());
    }
    let (flushes, gate) = hold_snapshot_flush_gate(&provisioner.inner, &memo_rx);
    assert!(flushes);
    assert!(
        provisioner.lock_state().pending_stop.is_some(),
        "a flushing teardown gates the revival on its own flush"
    );
    assert!(!provisioner
        .lock_state()
        .pending_stop
        .as_ref()
        .is_some_and(|g| *g.borrow()));
    drop(gate);
    // A boot kill() invalidated must never flush: the replacement
    // generation is already restoring the same on-disk payload.
    provisioner.kill();
    {
        let mut state = provisioner.lock_state();
        state.pending_stop = None;
    }
    let (flushes, gate) = hold_snapshot_flush_gate(&provisioner.inner, &memo_rx);
    assert!(!flushes);
    assert!(gate.is_none());
    assert!(
        provisioner.lock_state().pending_stop.is_none(),
        "a doomed teardown installs no gate"
    );
    // A disposed boot keeps the dispose's own policy (dispose does not
    // clear the memo) and gates the same way.
    let (disposed_tx, disposed_rx) = tokio::sync::watch::channel(None::<StartupResult>);
    drop(disposed_tx);
    {
        let mut state = provisioner.lock_state();
        state.startup = Some(disposed_rx.clone());
        state.disposed = true;
    }
    let (flushes, gate) = hold_snapshot_flush_gate(&provisioner.inner, &disposed_rx);
    assert!(flushes);
    assert!(gate.is_some());
    drop(gate);
    {
        let mut state = provisioner.lock_state();
        state.dispose_snapshot = false;
    }
    let (flushes, gate) = hold_snapshot_flush_gate(&provisioner.inner, &disposed_rx);
    assert!(!flushes);
    assert!(gate.is_none());
    // A stop actively armed for THIS boot covers the flush through its
    // own memo wait (it settles only after this teardown): the
    // teardown leaves that stop's gate untouched.
    let (armed_tx, armed_rx) = tokio::sync::watch::channel(false);
    {
        let mut state = provisioner.lock_state();
        state.dispose_snapshot = true;
        state.pending_stop = Some(armed_rx.clone());
        state.pending_stop_for_startup = Some(disposed_rx.clone());
    }
    let (flushes, gate) = hold_snapshot_flush_gate(&provisioner.inner, &disposed_rx);
    assert!(flushes);
    assert!(
        gate.is_none(),
        "an active stop armed for this boot covers the teardown flush"
    );
    assert!(
        provisioner
            .lock_state()
            .pending_stop
            .as_ref()
            .is_some_and(|g| g.same_channel(&armed_rx)),
        "the active stop's gate survives the teardown decision"
    );
    drop(armed_tx);
    // A merely installed, long-settled older gate covers nothing:
    // the teardown gate replaces it, or a kill() racing the teardown
    // would leave the replacement boot restoring against this
    // teardown's flush.
    {
        let mut state = provisioner.lock_state();
        state.pending_stop_for_startup = None;
    }
    let (flushes, gate) = hold_snapshot_flush_gate(&provisioner.inner, &disposed_rx);
    assert!(flushes);
    assert!(
        gate.is_some(),
        "a stale settled gate must not suppress the teardown gate"
    );
    assert!(
        !provisioner
            .lock_state()
            .pending_stop
            .as_ref()
            .is_some_and(|g| g.same_channel(&armed_rx)),
        "the stale gate is replaced"
    );
    drop(gate);
}

#[test]
fn boot_concurrency_defaults_and_override() {
    let default = default_kernel_boot_concurrency();
    assert!(default >= 4);
    std::env::set_var("PRIME_AGENT_MAX_CONCURRENT_KERNEL_BOOTS", "2");
    assert_eq!(resolve_kernel_boot_concurrency(), 2);
    std::env::set_var("PRIME_AGENT_MAX_CONCURRENT_KERNEL_BOOTS", "0");
    assert_eq!(resolve_kernel_boot_concurrency(), default);
    std::env::set_var("PRIME_AGENT_MAX_CONCURRENT_KERNEL_BOOTS", "junk");
    assert_eq!(resolve_kernel_boot_concurrency(), default);
    std::env::set_var("PRIME_AGENT_MAX_CONCURRENT_KERNEL_BOOTS", "1000");
    assert_eq!(resolve_kernel_boot_concurrency(), 64);
    std::env::remove_var("PRIME_AGENT_MAX_CONCURRENT_KERNEL_BOOTS");
}

#[tokio::test]
async fn ensure_rejects_aborted_signal() {
    let provisioner =
        IpythonKernelProvisioner::new("/tmp", IpythonKernelProvisionerOptions::default());
    let error = provisioner
        .ensure(None, Some(AbortSignal::aborted()))
        .await
        .expect_err("aborted startup must reject");
    assert!(error.to_string().contains("aborted"));
}

#[tokio::test]
async fn dispose_then_ensure_fails() {
    let provisioner =
        IpythonKernelProvisioner::new("/tmp", IpythonKernelProvisionerOptions::default());
    provisioner.dispose(None).await;
    let error = provisioner
        .ensure(None, None)
        .await
        .expect_err("disposed provisioner");
    assert!(error.to_string().contains("disposed"));
}

#[test]
fn retryable_failure_classification() {
    assert!(startup_failure_is_retryable(&anyhow!(
        "failed to spawn kernel python /x"
    )));
    assert!(startup_failure_is_retryable(&anyhow!(
        "Kernel did not become ready within 30000ms. stderr tail: ..."
    )));
    assert!(!startup_failure_is_retryable(&anyhow!(
        "Kernel provisioner disposed before start"
    )));
    assert!(!startup_failure_is_retryable(&anyhow!(
        "Kernel provisioner killed before start"
    )));
    assert!(!startup_failure_is_retryable(&anyhow!(
        "Python execution aborted"
    )));
    assert!(!startup_failure_is_retryable(&anyhow!(
        "PRIME_AGENT_KERNEL_PYTHON points to a Python missing a current prime-agent-runtime: /bad"
    )));
    assert!(!startup_failure_is_retryable(&anyhow!(
            "Kernel runtime speaks protocol 2, expected 3. \
             Update prime-agent-runtime in the kernel Python (PRIME_AGENT_KERNEL_PYTHON) to match this prime-agent."
        )));
}

#[tokio::test]
async fn startup_failure_surfaces_cause_and_duration() {
    let options = IpythonKernelProvisionerOptions {
        python: Some(PathBuf::from("/nonexistent/kernel-python-for-test")),
        ..Default::default()
    };
    let provisioner = IpythonKernelProvisioner::new("/tmp", options);
    let error = provisioner
        .ensure(None, None)
        .await
        .expect_err("bogus python must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("kernel startup failed after "),
        "error must carry the duration: {message}"
    );
    assert!(
        message.contains("ms: "),
        "error must carry the duration unit: {message}"
    );
    assert!(
        message.contains("failed to spawn"),
        "error must carry the spawn cause: {message}"
    );
    // The next ensure() retries fresh rather than rethrowing the memo.
    assert!(provisioner.ensure(None, None).await.is_err());
}

#[tokio::test]
async fn startup_retries_transient_failure() {
    // One retry (the default), so the bogus-python boot fails twice and
    // the backoff (>= 250ms) shows up in the elapsed time.
    let started = std::time::Instant::now();
    let options = IpythonKernelProvisionerOptions {
        python: Some(PathBuf::from("/nonexistent/kernel-python-for-test")),
        ..Default::default()
    };
    let provisioner = IpythonKernelProvisioner::new("/tmp", options);
    assert!(provisioner.ensure(None, None).await.is_err());
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(250),
        "the retry backoff must elapse before the failure surfaces"
    );
}

#[tokio::test]
async fn startup_retry_cancelled_by_dispose() {
    let options = IpythonKernelProvisionerOptions {
        python: Some(PathBuf::from("/nonexistent/kernel-python-for-test")),
        ..Default::default()
    };
    let provisioner = IpythonKernelProvisioner::new("/tmp", options);
    let p = provisioner.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        p.dispose(None).await;
    });
    // The boot fails; the dispose cancels any pending retry, so ensure()
    // settles without hanging on the backoff chain.
    let started = std::time::Instant::now();
    let _ = provisioner.ensure(None, None).await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "dispose during retry must cancel the backoff promptly"
    );
}

#[tokio::test]
async fn clone_shares_kernel_state() {
    let provisioner =
        IpythonKernelProvisioner::new("/tmp", IpythonKernelProvisionerOptions::default());
    let clone = provisioner.clone();
    provisioner.dispose(None).await;
    assert!(clone.ensure(None, None).await.is_err());
}

#[tokio::test]
async fn prewarm_swallows_failure_and_keeps_lazy_fallback() {
    let options = IpythonKernelProvisionerOptions {
        python: Some(PathBuf::from("/nonexistent/kernel-python-for-test")),
        ..Default::default()
    };
    let provisioner = IpythonKernelProvisioner::new("/tmp", options);
    provisioner.prewarm();
    // Let the background startup settle into its failure.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !provisioner.has_running_kernel(),
        "the failed prewarm must not leave a running kernel"
    );
    // The next ensure() surfaces the swallowed cause, not a memo hang.
    let error = provisioner
        .ensure(None, None)
        .await
        .expect_err("the bogus python must fail ensure too");
    assert!(
        format!("{error:#}").contains("failed to spawn"),
        "ensure must surface the spawn cause: {error:#}"
    );
}

/// A ready kernel that stops reading stdin must not wedge its bootstrap
/// request or the failed boot's shutdown behind a full pipe.
#[cfg(unix)]
#[tokio::test]
async fn nonreading_kernel_fails_bootstrap_without_parking_shutdown() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let python = dir.path().join("ready-but-not-reading");
    std::fs::write(
            &python,
            "#!/usr/bin/env python3\nimport json, time\nprint(json.dumps({'event': 'ready', 'protocol': 3, 'python': '3.13.0'}), flush=True)\ntime.sleep(30)\n",
        )
        .expect("write fake kernel");
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake kernel");
    let provisioner = IpythonKernelProvisioner::new(
        dir.path(),
        IpythonKernelProvisionerOptions {
            python: Some(python),
            python_skills: vec![KernelPythonSkill {
                name: "oversized".into(),
                import_name: "x".repeat(1024 * 1024),
                package_path: dir.path().into(),
                pyproject_path: dir.path().join("pyproject.toml"),
            }],
            ..Default::default()
        },
    );
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(9),
        provisioner.ensure(None, None),
    )
    .await
    .expect("timed out bootstrap must not park during failed-boot cleanup")
    .expect_err("kernel never reads bootstrap");
    assert!(format!("{error:#}").contains("runtime bootstrap did not finish"));
    assert!(!provisioner.has_running_kernel());
}

/// A kernel that answers the ready handshake but never answers the
/// bootstrap execute (the wedged-kernel shape: the frame is lost inside
/// the kernel) fails `ensure` with the bound's message instead of parking
/// forever, tears its kernel down, and does not auto-retry the fatal
/// failure.
#[cfg(unix)]
#[tokio::test]
async fn silent_bootstrap_fails_bounded_and_leaves_no_kernel() {
    use std::os::unix::fs::PermissionsExt;

    // Speaks protocol v3: answers the ready handshake, stays silent on
    // every execute (the runtime bootstrap included), and answers the
    // shutdown frame so a teardown does not wait out its kill deadline.
    const SILENT_BOOTSTRAP_RUNTIME: &str = r#"#!/usr/bin/env python3
import json
import os
import sys

base = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(base, "starts"), "a") as f:
    f.write("x")
print(json.dumps({"event": "ready", "protocol": 3, "python": "3.13.0"}), flush=True)
for line in sys.stdin:
    try:
        req = json.loads(line)
    except Exception:
        continue
    if req.get("type") == "shutdown":
        print(json.dumps({"event": "done", "id": req.get("id"), "status": "ok"}), flush=True)
        break
"#;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let python = dir.path().join("fake-kernel");
    std::fs::write(&python, SILENT_BOOTSTRAP_RUNTIME).expect("write fake runtime");
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake runtime");
    let provisioner = IpythonKernelProvisioner::new(
        dir.path(),
        IpythonKernelProvisionerOptions {
            python: Some(python),
            ..Default::default()
        },
    );

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        provisioner.ensure(None, None),
    )
    .await
    .expect("the bounded bootstrap must settle the boot")
    .expect_err("a kernel that never answers the bootstrap must not report success");
    let chain = format!("{outcome:#}");
    assert!(
        chain.contains("Failed to initialize rlm runtime"),
        "{chain}"
    );
    assert!(
        chain.contains(&format!(
            "the runtime bootstrap did not finish within {BOOTSTRAP_EXECUTION_TIMEOUT_MS}ms"
        )),
        "{chain}"
    );
    assert!(
        !provisioner.has_running_kernel(),
        "the failed boot must tear its kernel down"
    );
    assert_eq!(
        std::fs::metadata(dir.path().join("starts")).map_or(0, |m| m.len()),
        1,
        "the fatal classification must not auto-retry the boot"
    );
}
