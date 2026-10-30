//! The startup policy: the boot permits, the stage a boot publishes, whether its
//! memo generation is still live, and how many attempts a failure may retry.
//!
//! Everything here is about ONE boot's generation and ONE teardown's flush: a
//! boot whose memo generation is dead publishes nothing, retries nothing, and a
//! failed boot's teardown gates the next revival on its own final snapshot
//! flush.

use std::sync::{Arc, Mutex};

use crate::kernel::bootstrap::{KernelBootstrapProgressHandler, UnavailablePythonSkills};
use crate::kernel::state_snapshot::RestoreResult;

use super::{ProvisionerInner, StartupResult};

/// Publishes the restore outcome once the kernel is usable.
pub type RestoreCallback = Arc<dyn Fn(&RestoreResult) + Send + Sync>;

/// Publishes the skills that failed to import into a freshly started
/// kernel (import name -> import error), once the kernel is usable.
pub type UnavailableSkillsCallback = Arc<dyn Fn(&UnavailablePythonSkills) + Send + Sync>;

/// Outcome of one full kernel bootstrap (spawn + handshake + namespace
/// restore + runtime bootstrap), reported once per actual boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelBootstrapOutcome {
    /// The kernel process is running, revived, and runtime-bootstrapped.
    Ready,
    /// Any stage failed; the kernel was torn down before the error surfaced.
    Error,
}

/// One kernel boot's facts for the `kernel_bootstrap_*` session counters.
#[derive(Debug, Clone, Copy)]
pub struct KernelBootstrapStats {
    /// No prior namespace snapshot existed to restore (fresh session vs a
    /// revived one).
    pub cold: bool,
    pub outcome: KernelBootstrapOutcome,
    /// Wall time of the whole bootstrap, milliseconds.
    pub duration_ms: u64,
}

/// Reports kernel bootstrap results (the `kernel_bootstrap_*` session counters).
pub type KernelBootstrapResultHandler = Arc<dyn Fn(KernelBootstrapStats) + Send + Sync>;

/// Above core count because boots are IO-bound, capped so a fan-out can't
/// thrash the FS past the ready-handshake window.
pub(super) fn default_kernel_boot_concurrency() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    16.min((cores * 2).max(4))
}

pub(super) fn resolve_kernel_boot_concurrency() -> usize {
    let Ok(raw) = std::env::var("PRIME_AGENT_MAX_CONCURRENT_KERNEL_BOOTS") else {
        return default_kernel_boot_concurrency();
    };
    if raw.is_empty() || !raw.chars().all(|c| c.is_ascii_digit()) {
        return default_kernel_boot_concurrency();
    }
    let parsed: usize = raw.parse().unwrap_or(0);
    if parsed < 1 {
        return default_kernel_boot_concurrency();
    }
    parsed.min(64)
}

/// Semaphore bounding concurrent kernel boots. Resolved lazily on first boot so
/// an env override set before the first kernel starts is honored.
static BOOT_PERMITS: Mutex<Option<Arc<tokio::sync::Semaphore>>> = Mutex::new(None);

pub(super) async fn with_kernel_boot_permit<F, Fut>(boot: F) -> Fut::Output
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future,
{
    let permits = {
        let mut guard = BOOT_PERMITS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get_or_insert_with(|| {
                Arc::new(tokio::sync::Semaphore::new(
                    resolve_kernel_boot_concurrency(),
                ))
            })
            .clone()
    };
    let _permit = permits.acquire().await;
    boot().await
}

/// The snapshot policy for a failed boot's teardown: the provisioner's
/// dispose policy while the boot's memo generation is still armed, but
/// NEVER a flush for a boot `kill()` invalidated — the replacement
/// generation is already restoring from the same on-disk payload, and a
/// failed doomed boot's teardown flushing over it would rewrite a healthy
/// snapshot (and the kernel stderr log) out from under the new kernel.
/// `dispose()` does not clear the memo, so a disposed boot keeps the
/// dispose's own policy.
///
/// When the policy still flushes, this ALSO installs a pending-stop gate
/// for the teardown's own flush — the decision and the gate share ONE
/// lock scope, so a `kill()` landing between the decision and the flush
/// still leaves the replacement boot gated on this teardown: it restores
/// only after the flush settles (the same revival gate `stop_kernel`
/// arms against its own flush). A stop actively armed for this boot
/// already covers the flush (its task waits this boot's memo, which
/// settles only after the teardown), so the gate stays untouched in
/// that one case; any older, merely installed gate - including one long
/// settled and left in place - covers nothing and is replaced. Hold the
/// returned sender across the shutdown — the gate opens when the
/// teardown scope drops it (a dead sender unblocks the waiters the same
/// way a settled one does).
#[must_use]
pub(super) fn hold_snapshot_flush_gate(
    inner: &Arc<ProvisionerInner>,
    memo: &tokio::sync::watch::Receiver<Option<StartupResult>>,
) -> (bool, Option<tokio::sync::watch::Sender<bool>>) {
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let flush = state
        .startup
        .as_ref()
        .is_some_and(|armed| armed.same_channel(memo))
        && state.dispose_snapshot;
    // A stop actively armed for THIS boot covers the teardown's flush
    // through its own memo wait (the stop's task waits this boot's
    // settle, which happens only after the teardown); a merely
    // installed older gate - including one long settled and left in
    // place - covers nothing, so this teardown's gate replaces it.
    let covered_by_active_stop = state
        .pending_stop_for_startup
        .as_ref()
        .is_some_and(|armed_for| armed_for.same_channel(memo));
    if !flush || covered_by_active_stop {
        return (flush, None);
    }
    let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
    state.pending_stop = Some(gate_rx);
    (flush, Some(gate_tx))
}

/// The boot's memo is still the armed startup generation AND the
/// provisioner is not disposed: a failed attempt may retry, and a
/// restore may surface. `kill()` (or a newer boot's defunct-clear)
/// invalidating the memo makes a further attempt stale work whose
/// kernel the settle would only have to kill.
pub(super) fn boot_generation_is_live(
    inner: &Arc<ProvisionerInner>,
    memo: &tokio::sync::watch::Receiver<Option<StartupResult>>,
) -> bool {
    let state = inner
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    !state.disposed
        && state
            .startup
            .as_ref()
            .is_some_and(|armed| armed.same_channel(memo))
}

pub(super) fn emit_startup_progress(
    inner: &Arc<ProvisionerInner>,
    on_progress: Option<&KernelBootstrapProgressHandler>,
    memo: &tokio::sync::watch::Receiver<Option<StartupResult>>,
    message: &str,
) {
    let mut state = inner
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The shared stage belongs to the ACTIVE memo generation: a boot
    // kill() invalidated must not overwrite the newer boot's replayed
    // stage nor fire the newer generation's listeners with its own
    // (the boot's own handler below is its caller's, not shared
    // state, so it keeps firing).
    if state
        .startup
        .as_ref()
        .is_some_and(|armed| armed.same_channel(memo))
    {
        state.last_startup_message = Some(message.to_string());
        for listener in &state.startup_listeners {
            listener(message);
        }
    }
    if let Some(on_progress) = on_progress {
        on_progress(message);
    }
}

/// Extra startup attempts beyond the first. The promise here is resilience against a wedged boot,
/// not masking a broken setup.
pub(super) const DEFAULT_STARTUP_RETRIES: u32 = 1;
pub(super) const DEFAULT_STARTUP_BUDGET_MS: u64 = 90_000;
pub(super) const RETRY_BACKOFF_MS: [u64; 4] = [250, 1_000, 2_500, 5_000];

pub(super) fn resolve_startup_retries() -> u32 {
    match std::env::var("PRIME_AGENT_KERNEL_STARTUP_RETRIES") {
        Ok(raw) => raw
            .trim()
            .parse::<u32>()
            .map_or(DEFAULT_STARTUP_RETRIES, |n| n.min(5)),
        Err(_) => DEFAULT_STARTUP_RETRIES,
    }
}

pub(super) fn resolve_startup_budget_ms() -> u64 {
    match std::env::var("PRIME_AGENT_KERNEL_STARTUP_BUDGET_MS") {
        Ok(raw) => raw
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_STARTUP_BUDGET_MS),
        Err(_) => DEFAULT_STARTUP_BUDGET_MS,
    }
}

/// A failed boot the provisioner may retry on its own: transient spawn or
/// ready-handshake problems. Structural failures never auto-retry.
pub(super) fn startup_failure_is_retryable(error: &anyhow::Error) -> bool {
    const FATAL_MARKERS: [&str; 9] = [
        "provisioner disposed",
        "provisioner killed",
        "aborted",
        "Failed to set up the Python kernel runtime",
        "PRIME_AGENT_KERNEL_PYTHON points to a Python",
        "Failed to initialize rlm runtime",
        "Update prime-agent-runtime in the kernel Python",
        "Kernel start superseded",
        "Kernel was disposed during startup",
    ];
    let chain = format!("{error:#}");
    !FATAL_MARKERS.iter().any(|marker| chain.contains(marker))
}
