//! Boot: `prewarm` and `ensure`, the memoized startup concurrent callers join,
//! and the publisher task that settles it.
//!
//! [`IpythonKernelProvisioner::ensure`] memoizes the in-flight boot so joined
//! callers share one startup; the publisher task it spawns settles that boot
//! through [`super::settle`]'s atomics, so the boot itself only reports its own
//! outcome and duration.

use std::path::Path;
use std::sync::Arc;

use anyhow::anyhow;

use crate::kernel::bootstrap::KernelBootstrapProgressHandler;
use crate::kernel::cancellation::AbortSignal;
use crate::kernel::manager::ReplKernelManager;
use crate::kernel::shared::KernelShutdownOptions;

use super::ensure_startup::run_startup;
use super::settle::{park_or_tear_down, release_spent_stop_arm, Settle};
use super::{IpythonKernelProvisioner, IpythonKernelProvisionerOptions, StartupFailure};

impl IpythonKernelProvisioner {
    /// Start the kernel in the background. Failures are swallowed here and
    /// surface on the next `ensure()`.
    pub fn prewarm(&self) {
        let provisioner = self.clone();
        tokio::spawn(async move {
            let _ = provisioner.ensure(None, None).await;
        });
    }

    /// The kernel manager, starting it first when necessary. Concurrent callers join one startup.
    ///
    /// # Errors
    ///
    /// Returns an error when the abort signal is already cancelled or fires
    /// during the wait, when the provisioner was disposed, or when the kernel
    /// startup fails (all joined callers see the same failure).
    ///
    /// # Panics
    ///
    /// Panics if the shared startup watch settles without publishing a
    /// result; the publisher always sends the result before clearing the
    /// memo, so this is a construction invariant rather than a runtime
    /// path.
    pub async fn ensure(
        &self,
        on_progress: Option<KernelBootstrapProgressHandler>,
        signal: Option<AbortSignal>,
    ) -> anyhow::Result<ReplKernelManager> {
        if let Some(signal) = &signal {
            if signal.is_aborted() {
                return Err(anyhow!("Python execution aborted"));
            }
        }
        // The guard is strictly scoped to this decision block: a
        // conditionally-dropped non-Send MutexGuard would make ensure()
        // non-Send.
        let mut startup = {
            let mut state = self.lock_state();
            if state.disposed {
                return Err(anyhow!("Kernel provisioner disposed"));
            }
            if let Some(manager) = &state.manager {
                if manager.is_defunct() {
                    state.manager = None;
                    state.startup = None;
                }
            }
            if let Some(manager) = state.manager.clone() {
                return Ok(manager);
            }
            if let Some(progress) = &on_progress {
                if let Some(message) = state.last_startup_message.as_deref() {
                    progress(message);
                }
                state.startup_listeners.push(progress.clone());
            }
            if let Some(startup) = &state.startup {
                startup.clone()
            } else {
                let (done_tx, done_rx) = tokio::sync::watch::channel(None);
                let inner = Arc::clone(&self.inner);
                let pending_stop = state.pending_stop.clone();
                let startup_progress = on_progress.clone();
                let startup_memo = done_rx.clone();
                tokio::spawn(async move {
                    let boot = tokio::spawn(run_startup(
                        inner.clone(),
                        startup_progress,
                        pending_stop,
                        startup_memo.clone(),
                    ));
                    let result = match boot.await {
                        Ok(result) => result,
                        Err(error) => {
                            // A panicked boot never reached its own settle;
                            // the settle below still runs so the listener
                            // state clears and the next ensure() boots fresh.
                            Err(StartupFailure {
                                message: format!("kernel startup task failed: {error}"),
                                duration_ms: 0,
                            })
                        }
                    };
                    // Atomic settle/publish: the disposed check, the
                    // memo-generation check, the listener teardown, and the
                    // manager publication share ONE lock scope. Separate
                    // scopes let a dispose or a kill land between the check
                    // and the publish - the boot would park a live kernel
                    // into a provisioner that already reported itself torn
                    // down.
                    let settle = {
                        let mut state = inner
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let mine = state
                            .startup
                            .as_ref()
                            .is_some_and(|memo| memo.same_channel(&startup_memo));
                        // The listener state belongs to the ACTIVE memo
                        // generation: a doomed boot settling against a
                        // newer memo must not wipe the newer boot's
                        // progress listeners or its replayed stage. With
                        // no memo armed at all the entries are stale
                        // (their waiters already hold results) - clear.
                        if mine || state.startup.is_none() {
                            state.startup_listeners.clear();
                            state.last_startup_message = None;
                        }
                        match result {
                            Ok((manager, duration_ms)) => {
                                park_or_tear_down(&mut state, mine, manager, duration_ms)
                            }
                            Err(failure) => Settle::Failed(failure),
                        }
                    };
                    let settled = match settle {
                        Settle::Published(manager) => Ok(manager),
                        Settle::TearDownForDispose {
                            manager,
                            snapshot,
                            duration_ms,
                        } => {
                            // A dispose raced the boot: tear the kernel down
                            // with the dispose's snapshot policy and reject
                            // with its abort error instead of handing joined
                            // callers a manager that is already dead.
                            let _ = manager
                                .shutdown(KernelShutdownOptions {
                                    snapshot,
                                    drain_host_requests: true,
                                })
                                .await;
                            Err(StartupFailure {
                                message: "Kernel provisioner disposed during startup".to_string(),
                                duration_ms,
                            })
                        }
                        Settle::TearDownForKill {
                            manager,
                            duration_ms,
                        } => {
                            // TS kill() settles the pending startup and kills
                            // the manager it produced.
                            if !manager.is_defunct() {
                                manager.kill();
                            }
                            Err(StartupFailure {
                                message: "Kernel provisioner killed during startup".to_string(),
                                duration_ms,
                            })
                        }
                        Settle::Failed(failure) => Err(failure),
                    };
                    // Publish the completed result before clearing the memo:
                    // joined waiters keep the manager even if stop takes it.
                    let _ = done_tx.send(Some(settled));
                    // Clear only the memo this task installed (TS ensure()'s
                    // `managerPromise === startup` guard): a boot's manager
                    // can turn defunct inside the park->publish->clear window
                    // and another ensure() defunct-clears it and arms a NEWER
                    // memo already; an unconditional clear would wipe that
                    // memo mid-boot and arm a transient duplicate kernel.
                    let mut state = inner
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if state
                        .startup
                        .as_ref()
                        .is_some_and(|memo| memo.same_channel(&startup_memo))
                    {
                        state.startup = None;
                    }
                    release_spent_stop_arm(&mut state, &startup_memo);
                    // A memo kill() parked for this boot is spent with the
                    // settle the dispose() parker waits on.
                    state
                        .doomed_startups
                        .retain(|parked| !parked.same_channel(&startup_memo));
                });
                state.startup = Some(done_rx.clone());
                done_rx
            }
        };
        // Aborting one waiter leaves the shared startup alive for other
        // callers, like TS raceWithAbort(managerPromise, signal).
        let wait = async { startup.wait_for(Option::is_some).await };
        let completion = match &signal {
            Some(signal) => tokio::select! {
                result = wait => result,
                () = signal.cancelled() => return Err(anyhow!("Python execution aborted")),
            },
            None => wait.await,
        };
        if let Some(signal) = &signal {
            if signal.is_aborted() {
                return Err(anyhow!("Python execution aborted"));
            }
        }
        completion.map_err(|error| anyhow!("kernel startup task closed: {error}"))?;
        let settled = startup
            .borrow()
            .clone()
            .expect("settled startup has a result");
        match settled {
            Ok(manager) => Ok(manager),
            Err(failure) => Err(anyhow!(
                "kernel startup failed after {}ms: {}",
                failure.duration_ms,
                failure.message
            )),
        }
    }
}

/// Same as [`IpythonKernelProvisioner::new`] for a `Path`-shaped cwd.
#[must_use]
pub fn provisioner_for_path(
    cwd: &Path,
    options: IpythonKernelProvisionerOptions,
) -> IpythonKernelProvisioner {
    IpythonKernelProvisioner::new(cwd.to_path_buf(), options)
}
