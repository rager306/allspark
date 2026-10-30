//! Stopping and tearing down: the stop arm, `stop_kernel`, `dispose`, `kill`,
//! and the disposition one settled boot takes (park, kill, or one of the two
//! teardown rejects).
//!
//! Every edge here is ordered against a boot's memo generation: a stop's gate
//! is installed under the same lock `ensure()` selects its startup with, and a
//! settle's disposition is decided under its own lock scope, so no revival can
//! miss a stop and no kernel can park into a provisioner that already reported
//! itself torn down.

use std::sync::Arc;

use crate::kernel::manager::ReplKernelManager;
use crate::kernel::shared::KernelShutdownOptions;

use super::{IpythonKernelProvisioner, ProvisionerState, StartupFailure, StartupResult};

/// How one boot's settle publishes: the normal park, one of the two
/// teardown rejects (a racing dispose; a memo generation invalidated by
/// `kill()`), or the boot's own failure.
pub(super) enum Settle {
    /// The boot's manager parked into the provisioner.
    Published(ReplKernelManager),
    /// A dispose raced the boot: tear the kernel down with the dispose's
    /// snapshot policy and reject with its cause.
    TearDownForDispose {
        manager: ReplKernelManager,
        snapshot: bool,
        duration_ms: u64,
    },
    /// The memo this boot armed was cleared underneath it (`kill()`, or a
    /// defunct clear superseding the generation): never publish a kernel
    /// into a memo generation the boot no longer owns - kill it instead.
    TearDownForKill {
        manager: ReplKernelManager,
        duration_ms: u64,
    },
    /// The boot failed on its own; its failure already carries the cause
    /// and duration.
    Failed(StartupFailure),
}

/// One `stop_kernel` call's starting point.
pub(super) enum StopArm {
    /// Neither a live kernel nor an in-flight boot: nothing to stop.
    Nothing,
    /// This stop's own gate, installed atomically with the manager take
    /// (direct) or the in-flight boot it joins.
    Armed {
        manager: Option<ReplKernelManager>,
        startup: Option<tokio::sync::watch::Receiver<Option<StartupResult>>>,
        stop_tx: tokio::sync::watch::Sender<bool>,
        /// The gate this arm superseded. The stop task waits it before
        /// opening its own, so a revival gated on this stop's gate can
        /// never cross before the superseded stop's final snapshot flush
        /// settles; and because every task waits only a gate OLDER than
        /// its own arm, the waits cannot form a cycle (the
        /// wait-the-installed-gate shape admitted both the missed
        /// supersede window and a three-task deadlock).
        previous_stop: Option<tokio::sync::watch::Receiver<bool>>,
    },
}

impl IpythonKernelProvisioner {
    /// Stop the owned kernel without marking the provisioner disposed (TS
    /// #2483's `stopKernel`): the shutdown flushes the final snapshot, and
    /// the next `ensure()` boots a fresh kernel gated on this stop (the
    /// pending-stop revival gate), so a follow-up turn revives from the
    /// flushed snapshot instead of racing the flush over the same on-disk
    /// file. A kernel still starting up is joined first and shut down the
    /// same way (the TS `managerPromise` arm - the boot is not left
    /// resident); with neither a live kernel nor an in-flight boot there
    /// is nothing to stop.
    ///
    /// Concurrent stops of one in-flight boot chain instead of racing: each
    /// stop arms its own gate superseding the previous, and every stop task
    /// waits the strictly older gate it superseded before opening its own,
    /// so a revival gated on any of them cannot cross before the final
    /// snapshot flush settles.
    ///
    /// Best-effort by construction: a failed shutdown leaves no manager
    /// and the next `ensure()` boots fresh.
    pub async fn stop_kernel(&self, options: Option<KernelShutdownOptions>) {
        let snapshot = options.is_none_or(|o| o.snapshot);
        match self.arm_stop(snapshot) {
            StopArm::Nothing => {}
            StopArm::Armed {
                manager,
                startup,
                stop_tx,
                previous_stop,
                ..
            } => {
                let inner = Arc::clone(&self.inner);
                let stop = tokio::spawn(async move {
                    let manager = if let Some(manager) = manager {
                        Some(manager)
                    } else {
                        if let Some(mut startup) = startup {
                            let _ = startup.wait_for(Option::is_some).await;
                        }
                        inner
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .manager
                            .take()
                    };
                    if let Some(manager) = manager {
                        let _ = manager
                            .shutdown(KernelShutdownOptions {
                                snapshot,
                                drain_host_requests: true,
                            })
                            .await;
                    }
                    // Chained gate (the #3257 shape, kept under the join):
                    // open ours only after the gate THIS arm superseded
                    // settles - never the currently-installed one. A
                    // revival gated on ours then cannot cross before the
                    // superseded stop's final snapshot flush, whether this
                    // task shut its own kernel down or lost the manager
                    // take to the stop that gate belongs to; and because
                    // each task waits only a strictly older gate, the
                    // waits cannot form a cycle.
                    if let Some(mut previous) = previous_stop {
                        let _ = previous.wait_for(|done| *done).await;
                    }
                    let _ = stop_tx.send(true);
                });
                let _ = stop.await;
            }
        }
    }

    /// Claim one stop: take a live manager directly, or arm this stop's
    /// own gate for the in-flight boot's parked kernel. The gate is
    /// installed under the same lock `ensure()` uses to select its
    /// startup, so no revival can miss this stop; a second stop of the
    /// same boot simply supersedes the first's gate, and the stops chain
    /// through their superseded gates.
    fn arm_stop(&self, snapshot: bool) -> StopArm {
        let mut state = self.lock_state();
        state.dispose_snapshot = snapshot;
        let manager = state.manager.take();
        let startup = state.startup.clone();
        if manager.is_none() && startup.is_none() {
            return StopArm::Nothing;
        }
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let previous_stop = state.pending_stop.replace(stop_rx);
        // A gate armed with a directly-taken manager guards that manager's
        // shutdown, not a boot; the failed-boot teardown consults this
        // record to leave a stop ACTIVELY armed for the tearing-down boot
        // alone (its own task already covers the flush).
        state.pending_stop_for_startup = if manager.is_some() {
            None
        } else {
            startup.clone()
        };
        StopArm::Armed {
            manager,
            startup,
            stop_tx,
            previous_stop,
        }
    }

    pub async fn dispose(&self, options: Option<KernelShutdownOptions>) {
        let snapshot = options.is_none_or(|o| o.snapshot);
        let (mut startup, mut doomed) = {
            let mut state = self.lock_state();
            state.dispose_snapshot = snapshot;
            state.disposed = true;
            (state.startup.clone(), state.doomed_startups.clone())
        };
        self.inner.dispose_signal.abort();
        // A boot `kill()` invalidated settles like an armed one: the
        // dispose must not return while its kernel is still settling, or a
        // worker exit could orphan it.
        if let Some(startup) = &mut startup {
            let _ = startup.wait_for(Option::is_some).await;
        }
        for memo in &mut doomed {
            let _ = memo.wait_for(Option::is_some).await;
        }
        let manager = self.lock_state().manager.take();
        if let Some(manager) = manager {
            let _ = manager
                .shutdown(KernelShutdownOptions {
                    snapshot,
                    drain_host_requests: true,
                })
                .await;
        }
    }

    /// Kill the owned kernel without a final snapshot (busy-kernel restart).
    /// A kernel still starting up has not been published yet: like TS
    /// `kill()`, the startup memo leaves the generation (invalidation), so
    /// the doomed boot's own settle kills its kernel instead of parking
    /// it, and the next `ensure()` boots fresh - a kill racing a boot
    /// leaves no resident kernel behind. The invalidated memo is PARKED
    /// for `dispose()`: the doomed boot's kernel still exists until its
    /// settle, and a dispose racing the kill must wait it out.
    pub fn kill(&self) {
        let manager = {
            let mut state = self.lock_state();
            // The stop gate armed against the boot this kill invalidates
            // is spent with it.
            if let Some(armed_for) = state.startup.clone() {
                release_spent_stop_arm(&mut state, &armed_for);
            }
            if let Some(doomed) = state.startup.take() {
                state.doomed_startups.push(doomed);
            }
            // The shared progress state belongs to the memo generation
            // kill() just invalidated: its own emits already skip shared
            // writes, and the next ensure() must neither replay the
            // killed boot's stale stage to a fresh handler nor fan the
            // newer boot's stages out to the killed generation's
            // listeners.
            state.startup_listeners.clear();
            state.last_startup_message = None;
            state.manager.take()
        };
        if let Some(manager) = manager {
            manager.kill();
        }
    }
}

/// Release the stop gate armed for `memo`'s boot, if any. The arm's
/// memo receiver is a strong handle to the settled result's manager, and
/// a stale one keeps a parked-or-failed kernel alive past `stop_kernel()`,
/// so a later failed shutdown's process would leak with it as the sole
/// owner. Both spenders call this: the boot's settle and the `kill()` that
/// invalidates the boot.
pub(super) fn release_spent_stop_arm(
    state: &mut ProvisionerState,
    memo: &tokio::sync::watch::Receiver<Option<StartupResult>>,
) {
    if state
        .pending_stop_for_startup
        .as_ref()
        .is_some_and(|armed| armed.same_channel(memo))
    {
        state.pending_stop_for_startup = None;
    }
}

/// The settled boot's disposition: apply the settle decision under the
/// settle's own lock scope.
pub(super) fn park_or_tear_down(
    state: &mut ProvisionerState,
    mine: bool,
    manager: ReplKernelManager,
    duration_ms: u64,
) -> Settle {
    match settle_decision(state, mine) {
        SettleDecision::Kill => Settle::TearDownForKill {
            manager,
            duration_ms,
        },
        SettleDecision::Dispose => Settle::TearDownForDispose {
            manager,
            snapshot: state.dispose_snapshot,
            duration_ms,
        },
        SettleDecision::Publish => {
            state.manager = Some(manager.clone());
            Settle::Published(manager)
        }
    }
}

/// The settle's disposition order, isolated because the ORDER is the
/// contract: the generation check comes FIRST — a boot `kill()` doomed
/// tears down with `kill()` semantics (no flush) even when a dispose raced
/// in behind the kill, because the dispose's own snapshot policy cannot
/// resurrect a generation `kill()` invalidated (a killed+disposed boot
/// reaching Ok must never flush, and kill→ensure→dispose must never
/// overlap two flushes on one `snapshot_dir`). Only a live generation
/// honors the dispose.
pub(super) fn settle_decision(state: &ProvisionerState, mine: bool) -> SettleDecision {
    if !mine {
        SettleDecision::Kill
    } else if state.disposed {
        SettleDecision::Dispose
    } else {
        SettleDecision::Publish
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SettleDecision {
    Kill,
    Dispose,
    Publish,
}
