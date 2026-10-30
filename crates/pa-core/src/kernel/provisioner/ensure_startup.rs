//! The boot itself: the retry loop, the spawn, the namespace restore, and the
//! runtime bootstrap that reports one boot's `kernel_bootstrap_*` facts.
//!
//! The retry loop re-checks the boot's memo generation before every attempt and
//! after every backoff: a generation `kill()` invalidated (or a newer boot's
//! defunct-clear superseded) must not spawn an interpreter, restore a
//! namespace, or surface a restore the live generation never asked for.

use std::sync::Arc;

use anyhow::anyhow;

use crate::kernel::bootstrap::{
    build_rlm_bootstrap_code, parse_unavailable_python_skills, KernelBootstrapProgressHandler,
};
use crate::kernel::manager::{KernelStartOptions, ReplKernelManager};
use crate::kernel::shared::ExecuteStatus;
use crate::kernel::shared::{
    ExecuteOptions, KernelManagerOptions, KernelShutdownOptions, KernelSnapshotConfig,
    BASH_COMPLETED_REQUEST_TYPE, BOOTSTRAP_EXECUTION_TIMEOUT_MS,
};
use crate::kernel::state_snapshot::{manifest_path_in, snapshot_path_in, RestoreResult};

use super::startup::{
    boot_generation_is_live, emit_startup_progress, hold_snapshot_flush_gate,
    resolve_startup_budget_ms, resolve_startup_retries, startup_failure_is_retryable,
    with_kernel_boot_permit, KernelBootstrapOutcome, KernelBootstrapStats, RETRY_BACKOFF_MS,
};
use super::{ProvisionerInner, StartupFailure, StartupResult};

/// Boot the kernel, retrying transient failures with backoff until the
/// retry allowance or the hard startup budget runs out. Returns the booted
/// manager with the boot's duration, or the failure; the publisher task's
/// atomic settle decides whether the manager may park.
pub(super) async fn run_startup(
    inner: Arc<ProvisionerInner>,
    on_progress: Option<KernelBootstrapProgressHandler>,
    pending_stop: Option<tokio::sync::watch::Receiver<bool>>,
    memo: tokio::sync::watch::Receiver<Option<StartupResult>>,
) -> Result<(ReplKernelManager, u64), StartupFailure> {
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_millis(resolve_startup_budget_ms());
    let mut remaining_retries = resolve_startup_retries();
    let mut attempt: u32 = 0;
    let outcome = loop {
        attempt += 1;
        match start_kernel(&inner, on_progress.as_ref(), pending_stop.clone(), &memo).await {
            Ok(manager) => break Ok(manager),
            Err(error) => {
                // A boot whose memo generation kill() (or a newer boot's
                // defunct-clear) invalidated must not retry: each attempt
                // spawns an interpreter and runs restore/bootstrap, and a
                // stale attempt's restore would surface notifications and
                // state a fresher generation never asked for.
                if !boot_generation_is_live(&inner, &memo)
                    || !startup_failure_is_retryable(&error)
                    || remaining_retries == 0
                    || started.elapsed() >= budget
                {
                    break Err(error);
                }
                remaining_retries -= 1;
                let backoff_ms =
                    RETRY_BACKOFF_MS[(attempt as usize - 1).min(RETRY_BACKOFF_MS.len() - 1)];
                emit_startup_progress(
                    &inner,
                    on_progress.as_ref(),
                    &memo,
                    &format!("Kernel start failed; retrying in {backoff_ms}ms…"),
                );
                tokio::select! {
                    () = tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)) => {}
                    () = inner.dispose_signal.cancelled() => break Err(error),
                }
                // Re-check after the backoff: kill() can invalidate the
                // generation while this task sleeps it out.
                if !boot_generation_is_live(&inner, &memo) {
                    break Err(error);
                }
            }
        }
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    // The settle (disposed check, memo-generation check, park-or-reject)
    // runs in the publisher task's atomic scope; this task only reports
    // the boot's own outcome and duration.
    match outcome {
        Ok(manager) => Ok((manager, duration_ms)),
        Err(error) => Err(StartupFailure {
            message: format!("{error:#}"),
            duration_ms,
        }),
    }
}

/// Boot one kernel, restore the prior namespace, then run the runtime bootstrap. Reports the result
/// through `on_bootstrap_result` once per actual boot.
async fn start_kernel(
    inner: &Arc<ProvisionerInner>,
    on_progress: Option<&KernelBootstrapProgressHandler>,
    pending_stop: Option<tokio::sync::watch::Receiver<bool>>,
    memo: &tokio::sync::watch::Receiver<Option<StartupResult>>,
) -> anyhow::Result<ReplKernelManager> {
    let started = std::time::Instant::now();
    let cold = !inner
        .options
        .snapshot_dir
        .as_ref()
        .is_some_and(|dir| snapshot_path_in(dir).exists());
    let result = start_kernel_impl(inner, on_progress, pending_stop, memo).await;
    if let Some(report) = &inner.options.on_bootstrap_result {
        report(KernelBootstrapStats {
            cold,
            duration_ms: started.elapsed().as_millis() as u64,
            outcome: match &result {
                Ok(_) => KernelBootstrapOutcome::Ready,
                Err(_) => KernelBootstrapOutcome::Error,
            },
        });
    }
    result
}

/// The bootstrap itself; see [`start_kernel`].
async fn start_kernel_impl(
    inner: &Arc<ProvisionerInner>,
    on_progress: Option<&KernelBootstrapProgressHandler>,
    mut pending_stop: Option<tokio::sync::watch::Receiver<bool>>,
    memo: &tokio::sync::watch::Receiver<Option<StartupResult>>,
) -> anyhow::Result<ReplKernelManager> {
    let options = &inner.options;
    let cwd = inner.cwd.clone();
    let dispose_signal = inner.dispose_signal.clone();
    // The boot-permit closure moves its own clone; the bootstrap below runs
    // on the same shared signal.
    let permit_dispose_signal = dispose_signal.clone();
    // Wait for this provisioner's own in-flight stop_kernel() — and its
    // final snapshot flush — before reading that snapshot back (TS #2483's
    // `pendingStop` gate; a completed stop awaits instantly and each stop
    // supersedes the previous). `ready_gate` stays the cross-provisioner
    // /reload arm.
    if let Some(stop_gate) = &mut pending_stop {
        if !*stop_gate.borrow() {
            emit_startup_progress(
                inner,
                on_progress,
                memo,
                "Waiting for the previous kernel to stop...",
            );
        }
        let _ = stop_gate.wait_for(|done| *done).await;
    }
    // Wait for a previous provisioner (e.g. on /reload) to finish disposing —
    // and flushing its final snapshot — before reading that snapshot back.
    if let Some(gate) = options.ready_gate.clone() {
        gate().await;
    }
    let snapshot_dir = options.snapshot_dir.clone();
    // The bounded-await default keys on the bash.completed host handler:
    // one source of truth (the registry the daemon registers the notice in).
    let bootstrap_code = build_rlm_bootstrap_code(
        &options.python_skills,
        options
            .host_handlers
            .get(BASH_COMPLETED_REQUEST_TYPE)
            .is_some(),
    );
    let mut env = options.env.clone();
    if let Some(shell_path) = &options.shell_path {
        env.insert(
            "PRIME_AGENT_BASH_SHELL".into(),
            shell_path.to_string_lossy().to_string(),
        );
    }
    if let Some(command_prefix) = &options.command_prefix {
        env.insert(
            "PRIME_AGENT_BASH_COMMAND_PREFIX".into(),
            command_prefix.clone(),
        );
    }
    let snapshot = snapshot_dir.as_ref().map(|dir| KernelSnapshotConfig {
        path: snapshot_path_in(dir),
        manifest_path: manifest_path_in(dir),
        max_bytes: None,
        max_variable_bytes: None,
        debounce_ms: None,
    });
    let stderr_log_path = snapshot_dir
        .as_ref()
        .map(|dir| dir.join("kernel-stderr.log"));
    let manager = ReplKernelManager::new(KernelManagerOptions {
        python: options.python.clone(),
        cwd: Some(cwd),
        env,
        session_id: options.session_id.clone(),
        host_handlers: options.host_handlers.clone(),
        python_skills: options.python_skills.clone(),
        on_background_work_settled: options.on_background_work_settled.clone(),
        snapshot,
        bootstrap_code: Some(bootstrap_code.clone()),
        stderr_log_path,
    });

    emit_startup_progress(inner, on_progress, memo, "Starting Python kernel...");
    // Only the process spawn + ready handshake contends for OS resources under
    // a fan-out, and it is bounded by start()'s own timeout — so the permit
    // covers only start(). Restore/bootstrap run per-kernel afterwards.
    let start = manager.start(KernelStartOptions {
        signal: None,
        on_bootstrap_progress: on_progress.cloned(),
    });
    let permit_inner = Arc::clone(inner);
    let permit_memo = memo.clone();
    let boot = async {
        with_kernel_boot_permit(move || async move {
            // Disposed, or its memo generation kill() invalidated, while
            // queued for the permit — don't spawn a kernel nobody wants.
            // The permit frees after the stop/ready gates below, so a boot
            // that waited those gates rechecks liveness here, the last
            // point before the interpreter spawn.
            if permit_dispose_signal.is_aborted() {
                return Err(anyhow!("Kernel provisioner disposed before start"));
            }
            if !boot_generation_is_live(&permit_inner, &permit_memo) {
                return Err(anyhow!("Kernel provisioner killed before start"));
            }
            start.await
        })
        .await
    };
    if let Err(error) = boot.await {
        // Never leak the kernel process if startup fails after spawn — and
        // never surface the failure before the teardown (final snapshot flush
        // included) finished.
        let (snapshot_policy, _teardown_gate) = hold_snapshot_flush_gate(inner, memo);
        let _ = manager
            .shutdown(KernelShutdownOptions {
                snapshot: snapshot_policy,
                drain_host_requests: true,
            })
            .await;
        // The drained stderr tail is the only extra evidence a failed boot
        // leaves; attach it to the cause.
        let stderr_tail = {
            let tail = manager.kernel_stderr();
            let chars: Vec<char> = tail.chars().collect();
            let start = chars.len().saturating_sub(2048);
            chars[start..].iter().collect::<String>()
        };
        let error = if stderr_tail.trim().is_empty() {
            error.context("kernel start")
        } else {
            error.context(format!(
                "kernel start; kernel stderr tail:
{stderr_tail}"
            ))
        };
        return Err(error);
    }

    // Revive a prior session's namespace before the bootstrap, so the
    // bootstrap then overwrites live handles (rlm, skills) on top of anything restored.
    let mut pending_restore: Option<RestoreResult> = None;
    let mut snapshot_existed = false;
    if let Some(dir) = &snapshot_dir {
        snapshot_existed = snapshot_path_in(dir).exists();
        emit_startup_progress(inner, on_progress, memo, "Restoring Python state...");
        let restore = manager.restore_state().await;
        if snapshot_existed {
            pending_restore = Some(restore.unwrap_or_default());
        }
    }
    emit_startup_progress(inner, on_progress, memo, "Preparing Python runtime...");
    // The bootstrap runs on the dispose signal (TS startKernel races every
    // boot stage against the dispose-linked abort): a dispose mid-bootstrap
    // settles the cell aborted, and the aborted-status arm below tears the
    // kernel down instead of leaking it into a disposed provisioner.
    let bootstrap = manager
        .execute_bounded(
            &bootstrap_code,
            ExecuteOptions {
                signal: Some(dispose_signal.clone()),
                ..Default::default()
            },
            Some(BOOTSTRAP_EXECUTION_TIMEOUT_MS),
        )
        .await;
    match bootstrap {
        Ok(bootstrap) if bootstrap.status == ExecuteStatus::Ok && dispose_signal.is_aborted() => {
            // The cell completed, but the provisioner was disposed under it:
            // the same teardown as the aborted-status arm, with the honest
            // disposed-startup cause (TS startKernel's abort error).
            let (snapshot_policy, _teardown_gate) = hold_snapshot_flush_gate(inner, memo);
            let _ = manager
                .shutdown(KernelShutdownOptions {
                    snapshot: snapshot_policy,
                    drain_host_requests: true,
                })
                .await;
            return Err(anyhow!("Kernel provisioner disposed during startup"));
        }
        Ok(bootstrap) if bootstrap.status == ExecuteStatus::Ok => {
            if snapshot_existed {
                // The just-restored namespace is fresh: the debounced
                // auto-snapshot would rewrite identical content.
                manager.mark_restored_namespace_fresh();
            }
            // Broken skill imports stay importable-looking placeholders;
            // report them so the model learns before its first call, not
            // from the placeholder's error (TS startKernel) - but only a
            // LIVE generation may report: the callback shares the restore
            // notice's mailbox, and a boot kill() invalidated must not
            // append stale skills-unavailable rows the next turn shows the
            // model. Same contract as the restore gate below: the
            // generation check and the callback share ONE lock scope, and
            // the callbacks must not re-enter the provisioner.
            let unavailable = parse_unavailable_python_skills(&bootstrap.stdout);
            if let Some(errors) = unavailable {
                let state = inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.disposed
                    && state
                        .startup
                        .as_ref()
                        .is_some_and(|armed| armed.same_channel(memo))
                {
                    if let Some(on_unavailable_skills) = &inner.options.on_unavailable_skills {
                        on_unavailable_skills(&errors);
                    }
                }
            }
        }
        Ok(bootstrap) => {
            // The kernel booted but its runtime did not initialize: the venv is the prime suspect,
            // so drop the memoized runtime-ready result.
            crate::kernel::bootstrap::invalidate_runtime_probe_cache();
            let details = [bootstrap.stderr.clone()]
                .into_iter()
                .chain(bootstrap.error.iter().map(|e| e.traceback.join("\n")))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            // TS startKernel's catch shuts the failed boot down with the
            // dispose snapshot policy (default true), not `false`.
            let (snapshot_policy, _teardown_gate) = hold_snapshot_flush_gate(inner, memo);
            let _ = manager
                .shutdown(KernelShutdownOptions {
                    snapshot: snapshot_policy,
                    drain_host_requests: true,
                })
                .await;
            // An aborted bootstrap with a live dispose signal is the bound
            // firing on a kernel that stopped answering: name the lost
            // bootstrap instead of a bare runtime failure.
            let error = if bootstrap.status == ExecuteStatus::Aborted
                && !dispose_signal.is_aborted()
            {
                anyhow!(
                    "Failed to initialize rlm runtime in the Python kernel: \
                     the runtime bootstrap did not finish within {BOOTSTRAP_EXECUTION_TIMEOUT_MS}ms:\n{details}"
                )
            } else {
                anyhow!("Failed to initialize rlm runtime in the Python kernel:\n{details}")
            };
            return Err(error);
        }
        Err(error) => {
            let (snapshot_policy, _teardown_gate) = hold_snapshot_flush_gate(inner, memo);
            let _ = manager
                .shutdown(KernelShutdownOptions {
                    snapshot: snapshot_policy,
                    drain_host_requests: true,
                })
                .await;
            return Err(error);
        }
    }

    // Only tell the model what was revived once the kernel is actually usable —
    // a notice claiming restored state must never outlive a failed bootstrap,
    // and only a LIVE generation may surface it: a boot kill() invalidated
    // restores its snapshot only for the settle to kill its kernel, so its
    // restore must not fire the notification nor pollute last_restore for
    // the fresher generation. The liveness check, the write, AND the
    // callback share ONE lock scope (this provisioner invokes its
    // callbacks under the state lock - the startup-progress listeners
    // already do - and they must not re-enter the provisioner), so a
    // kill() can no longer slip between the check and the notice.
    if let Some(restore) = pending_restore {
        let mut state = inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.disposed
            && state
                .startup
                .as_ref()
                .is_some_and(|armed| armed.same_channel(memo))
        {
            state.last_restore = Some(restore.clone());
            if let Some(on_restore) = &inner.options.on_restore {
                on_restore(&restore);
            }
        }
    }
    Ok(manager)
}
