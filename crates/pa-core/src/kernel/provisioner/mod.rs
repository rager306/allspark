//! The per-session kernel provisioner: owns one kernel manager, guards its
//! startup, revives the saved namespace before the runtime bootstrap, and
//! disposes/kills on demand. Reader tasks hold only weak references, so no
//! kernel outlives its provisioner; `dispose()` flushes the snapshot first.
//!
//! The provisioner was one 2 000-line file; it is now one module per
//! responsibility:
//!
//! | module | holds |
//! |---|---|
//! | [`ensure`] | `prewarm`, `ensure`, and the publisher task that settles a boot |
//! | [`ensure_startup`] | the boot itself: retries, spawn, restore, runtime bootstrap |
//! | [`settle`] | the stop arm, `stop_kernel`/`dispose`/`kill`, and the settle decision |
//! | [`startup`] | the retry policy, the boot liveness checks, the flush gate, the boot permits |
//! | [`options`] | [`IpythonKernelProvisionerOptions`] |
//! | [`namespace`] | the two namespace readers over a running kernel |
//! | [`tests`] | the tests, grouped by the contract each one pins |
//!
//! The state the lifecycle modules share ([`ProvisionerInner`],
//! [`ProvisionerState`], and the startup memo every boot is armed against)
//! lives here, so the two `impl` blocks on [`IpythonKernelProvisioner`] in
//! [`ensure`] and [`settle`] reach the same owned state. The public surface is
//! re-exported here, so `kernel::provisioner::NAME` keeps every former path.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::kernel::bootstrap::KernelBootstrapProgressHandler;
#[cfg(test)]
use crate::kernel::bootstrap::KernelPythonSkill;
use crate::kernel::cancellation::AbortSignal;
use crate::kernel::manager::ReplKernelManager;
use crate::kernel::state_snapshot::RestoreResult;

mod ensure;
mod ensure_startup;
mod namespace;
mod options;
mod settle;
mod startup;

#[cfg(test)]
mod tests;

pub use ensure::provisioner_for_path;
pub use options::IpythonKernelProvisionerOptions;
pub use startup::{
    KernelBootstrapOutcome, KernelBootstrapResultHandler, KernelBootstrapStats, RestoreCallback,
    UnavailableSkillsCallback,
};

/// Why and how long one startup failed, published through the shared startup
/// watch so every joined `ensure()` caller sees the same cause and duration
/// instead of a bare "kernel startup failed".
#[derive(Clone)]
struct StartupFailure {
    /// Full error chain (`{:#}` formatting).
    message: String,
    duration_ms: u64,
}

type StartupResult = Result<ReplKernelManager, StartupFailure>;

struct ProvisionerState {
    manager: Option<ReplKernelManager>,
    /// Shared result of the in-flight boot. Joined callers retain its manager
    /// even when a concurrent stop takes the provisioner's live owner.
    startup: Option<tokio::sync::watch::Receiver<Option<StartupResult>>>,
    startup_listeners: Vec<KernelBootstrapProgressHandler>,
    last_startup_message: Option<String>,
    last_restore: Option<RestoreResult>,
    disposed: bool,
    /// Snapshot policy of the dispose that aborted a startup, honored by
    /// the failed startup's own teardown.
    dispose_snapshot: bool,
    /// The in-flight `stop_kernel` shutdown (TS #2483's `pendingStop`): a
    /// revival boot waits for it to finish flushing its final snapshot
    /// before reading that snapshot back, so the two kernels never race
    /// over the same on-disk file. The receiver yields `true` once the
    /// recorded stop settles (a settled stop awaits instantly; a dead
    /// sender errs and unblocks the same way); each stop supersedes the
    /// previous.
    pending_stop: Option<tokio::sync::watch::Receiver<bool>>,
    /// The startup memo the installed pending-stop gate is armed against;
    /// `None` when the gate was armed with a directly-taken manager. The
    /// failed-boot teardown consults it to keep the gate of a stop
    /// ACTIVELY armed for its own boot (that stop's task waits the boot's
    /// memo, which settles only after the teardown): the teardown's flush
    /// is already covered, so it installs no replacement gate.
    pending_stop_for_startup: Option<tokio::sync::watch::Receiver<Option<StartupResult>>>,
    /// Startup memos `kill()` invalidated but whose boots have not settled
    /// yet. The memo's generation is dead (the boot tears down with `kill()`
    /// semantics) yet the boot's kernel still exists, so a later
    /// `dispose()` must wait its settle the same way it waits an armed
    /// memo — without the park, a kill followed by a dispose skips the
    /// doomed boot entirely and can orphan its kernel at worker exit.
    /// Each parked memo is spent by its own boot's settle.
    doomed_startups: Vec<tokio::sync::watch::Receiver<Option<StartupResult>>>,
}

/// Owns one kernel for one session: starts it, memoizes the startup so concurrent callers join the
/// same boot.
///
/// Cloning shares the same kernel and startup state.
#[derive(Clone)]
pub struct IpythonKernelProvisioner {
    inner: Arc<ProvisionerInner>,
}

struct ProvisionerInner {
    cwd: PathBuf,
    options: IpythonKernelProvisionerOptions,
    state: Mutex<ProvisionerState>,
    dispose_signal: AbortSignal,
}

impl IpythonKernelProvisioner {
    pub fn new(cwd: impl Into<PathBuf>, options: IpythonKernelProvisionerOptions) -> Self {
        Self {
            inner: Arc::new(ProvisionerInner {
                cwd: cwd.into(),
                options,
                state: Mutex::new(ProvisionerState {
                    manager: None,
                    startup: None,
                    startup_listeners: Vec::new(),
                    last_startup_message: None,
                    last_restore: None,
                    disposed: false,
                    dispose_snapshot: true,
                    pending_stop: None,
                    pending_stop_for_startup: None,
                    doomed_startups: Vec::new(),
                }),
                dispose_signal: AbortSignal::new(),
            }),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ProvisionerState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The kernel manager, once a startup has completed successfully.
    #[must_use]
    pub fn manager(&self) -> Option<ReplKernelManager> {
        self.lock_state().manager.clone()
    }

    /// The default time limit the kernel executor arms for user cells
    /// (`None`: never armed).
    #[must_use]
    pub fn cell_timeout_ms(&self) -> Option<u64> {
        self.inner.options.cell_timeout_ms
    }

    /// Where a clipped stdout/stderr keeps its original stream.
    #[must_use]
    pub fn evidence_dir(&self) -> Option<PathBuf> {
        self.inner.options.evidence_dir.clone()
    }

    /// The Python skills pre-imported at kernel bootstrap.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn python_skills(&self) -> &[KernelPythonSkill] {
        &self.inner.options.python_skills
    }

    /// Result of reviving a prior session's namespace on the last kernel start.
    #[must_use]
    pub fn last_restore(&self) -> Option<RestoreResult> {
        self.lock_state().last_restore.clone()
    }

    /// Whether a kernel has finished starting and is currently running.
    #[must_use]
    pub fn has_running_kernel(&self) -> bool {
        self.manager().is_some_and(|m| m.is_running())
    }
}
