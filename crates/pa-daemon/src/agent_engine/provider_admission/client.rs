//! How a worker reaches the shared registry: in-process for tests and
//! single-process hosts, over the supervisor link in production, or not at all.
//!
//! Fail-open is the contract these round trips implement: a transport failure, a
//! refusal that is not the breaker's, or a malformed answer all degrade to
//! "proceed unadmitted" rather than to a hung turn.
//!
//! The request budgets are part of that contract too. An acquire may park in the
//! supervisor's FIFO for [`super::ACQUIRE_BUDGET`], so its link deadline must outlast
//! that queue wait; a settle only mutates counters and must NOT inherit the same
//! deadline, or a wedged link holds a settle for ten minutes while the lease it
//! is settling stays in flight (review #38).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::supervisor_link::SupervisorLink;

use super::{
    ProviderAdmission, ProviderKey, SpawnReservation, StreamAdmission, StreamOutcome,
    ADMISSION_COMMAND,
};

/// How a worker reaches the shared registry.
#[derive(Clone)]
pub enum AdmissionClient {
    /// No admission plane (standalone hosts, scripted engines): every call
    /// admits immediately and no lease is minted.
    Disabled,
    /// The registry in this process (tests, single-process hosts).
    Local(Arc<ProviderAdmission>),
    /// The supervisor's registry over the supervisor link (the worker
    /// path: one process per session, one shared limit).
    Link {
        link: Arc<SupervisorLink>,
        worker_token: String,
        /// Releases whose confirmation the supervisor never gave, retried for as
        /// long as this client lives (review #9). Shared by every clone, so the
        /// worker's one client value owns one ledger.
        ///
        /// The bounded ladder in `release_detached` covers a link that fails for a
        /// moment. An outage LONGER than the ladder used to end the story: the lease
        /// id was dropped with the task and the slot stayed charged for the life of a
        /// LIVE worker, which no reaper reclaims — holder reaping runs on worker
        /// EXIT, and the dead-holder reaper only knows departed workers. The drainer
        /// keeps retrying the idempotent release until the supervisor confirms it, so
        /// the capacity comes back when the link does instead of at the worker's
        /// death.
        pending: Arc<std::sync::Mutex<Vec<u64>>>,
        /// Whether the one drainer is running: a flag rather than a task per lease,
        /// so a burst of aborted streams does not multiply retry loops against a
        /// dead supervisor.
        draining: Arc<AtomicBool>,
    },
}

impl std::fmt::Debug for AdmissionClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionClient::Disabled => formatter.write_str("AdmissionClient::Disabled"),
            AdmissionClient::Local(_) => formatter.write_str("AdmissionClient::Local"),
            AdmissionClient::Link { .. } => formatter.write_str("AdmissionClient::Link"),
        }
    }
}

impl AdmissionClient {
    /// A client over the supervisor link, authenticated by the worker token
    /// (the supervisor attributes every lease to the token's worker, so a
    /// dead worker's slots are reaped).
    #[must_use]
    pub fn link(link: Arc<SupervisorLink>, worker_token: String) -> Self {
        AdmissionClient::Link {
            link,
            worker_token,
            pending: Arc::new(std::sync::Mutex::new(Vec::new())),
            draining: Arc::new(AtomicBool::new(false)),
        }
    }

    #[must_use]
    pub fn local(registry: Arc<ProviderAdmission>) -> Self {
        AdmissionClient::Local(registry)
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !matches!(self, AdmissionClient::Disabled)
    }

    /// Take one stream slot. [`StreamAdmission::Unadmitted`] is fail-open;
    /// [`StreamAdmission::Refused`] is the open breaker (fail over).
    pub async fn acquire_stream(&self, key: &ProviderKey) -> StreamAdmission {
        // An admission round trip is the moment a recovered link is worth
        // noticing: releases the ladder could not confirm get retried here
        // instead of waiting for the worker's death (review #9).
        self.drain_pending();
        match self {
            AdmissionClient::Disabled => StreamAdmission::Unadmitted,
            AdmissionClient::Local(registry) => registry.acquire_stream(key, LOCAL_HOLDER).await,
            AdmissionClient::Link {
                link, worker_token, ..
            } => {
                let Some(data) = self
                    .call(
                        link,
                        worker_token,
                        admission_command(worker_token, "acquire_stream", key, None),
                    )
                    .await
                else {
                    return StreamAdmission::Unadmitted;
                };
                // The open breaker answers a plain refusal; a missing
                // answer stays fail-open (a degraded plane never gates).
                if data.get("refused").and_then(serde_json::Value::as_bool) == Some(true) {
                    return StreamAdmission::Refused;
                }
                let Some(id) = data.get("leaseId").and_then(serde_json::Value::as_u64) else {
                    return StreamAdmission::Unadmitted;
                };
                StreamAdmission::Admitted(id)
            }
        }
    }

    /// Wait for a free slot to dispatch one spawn on `key`; the answer is
    /// the held lease plus the randomized stagger to wait before starting
    /// the child. `None` = fail-open (the caller starts unadmitted).
    ///
    /// The lease is minted against the CLIENT's own identity, never the spawn id (review
    /// #10): the supervisor only honours a release from the holder recorded at mint, and a
    /// holder of "sub-1a2b3c4d" is a token no worker ever presents, so such a lease leaked a
    /// slot for the life of the worker.
    pub async fn spawn_gate(
        &self,
        key: &ProviderKey,
        spawn_id: &str,
    ) -> Option<(StreamLease, Duration)> {
        self.drain_pending();
        match self {
            AdmissionClient::Disabled => None,
            AdmissionClient::Local(registry) => registry
                .spawn_gate(
                    key,
                    SpawnReservation {
                        holder: LOCAL_HOLDER,
                        spawn_id,
                    },
                )
                .await
                .map(|(id, stagger)| {
                    (
                        StreamLease {
                            id,
                            client: self.clone(),
                            resolved: false,
                        },
                        stagger,
                    )
                }),
            AdmissionClient::Link {
                link, worker_token, ..
            } => {
                let data = self
                    .call(
                        link,
                        worker_token,
                        admission_command(worker_token, "spawn_gate", key, Some(spawn_id)),
                    )
                    .await?;
                let id = data.get("leaseId")?.as_u64()?;
                let stagger_ms = data.get("staggerMs").and_then(serde_json::Value::as_u64)?;
                Some((
                    StreamLease {
                        id,
                        client: self.clone(),
                        resolved: false,
                    },
                    Duration::from_millis(stagger_ms),
                ))
            }
        }
    }

    /// Settle a lease with its AIMD signal (the typed reason rides the
    /// wire when the outcome is rate-limited).
    ///
    /// Answers whether the supervisor CONFIRMED the mutation. A `false` means the
    /// slot is still the supervisor's: holder reaping only runs when the worker
    /// exits, so a live worker that treated a failed round trip as done would
    /// shrink provider capacity by one for the rest of its life (review #7).
    pub async fn settle(&self, lease: u64, outcome: StreamOutcome) -> bool {
        match self {
            AdmissionClient::Disabled => true,
            AdmissionClient::Local(registry) => {
                registry.settle(lease, outcome);
                true
            }
            AdmissionClient::Link {
                link, worker_token, ..
            } => {
                let mut command = lease_command(worker_token, "settle_stream", lease);
                command["outcome"] = serde_json::json!(outcome.as_wire());
                if let StreamOutcome::RateLimited(reason) = outcome {
                    command["reason"] = serde_json::json!(reason.as_wire());
                }
                self.call_within(link, command, request_budget("settle_stream"))
                    .await
                    .is_some()
            }
        }
    }

    /// Give a lease back without an AIMD signal.
    ///
    /// Answers whether the slot is now free: a confirmed release, or the
    /// supervisor answering that this worker does not hold the lease (the answer
    /// to an attempt whose reply was lost — the slot is already back). Anything
    /// else is a link that never answered, and the slot is still charged, for the
    /// same reason as [`Self::settle`].
    pub async fn release(&self, lease: u64) -> bool {
        matches!(
            self.release_attempt(lease).await,
            ReleaseAttempt::Confirmed | ReleaseAttempt::Gone
        )
    }

    /// One release round trip, keeping the distinction the retry logic needs: an
    /// ANSWER that the lease is not held means the slot is free (the answer to a
    /// lost earlier attempt), while a LINK failure means it is still charged.
    async fn release_attempt(&self, lease: u64) -> ReleaseAttempt {
        match self {
            AdmissionClient::Disabled => ReleaseAttempt::Confirmed,
            AdmissionClient::Local(registry) => {
                registry.release(lease);
                ReleaseAttempt::Confirmed
            }
            AdmissionClient::Link {
                link, worker_token, ..
            } => {
                let request = lease_command(worker_token, "release_stream", lease);
                let Ok(response) = link
                    .request(request, request_budget("release_stream"))
                    .await
                else {
                    return ReleaseAttempt::Unreachable;
                };
                if response.success {
                    return ReleaseAttempt::Confirmed;
                }
                // The supervisor's ownership refusal is the answer to a release whose
                // earlier attempt DID land: the lease is gone, which is the goal.
                // Retrying it would keep the drainer awake for the worker's lifetime.
                if response
                    .error
                    .is_some_and(|error| error.contains(NOT_HELD_MARKER))
                {
                    return ReleaseAttempt::Gone;
                }
                ReleaseAttempt::Unreachable
            }
        }
    }

    /// The fire-and-forget release a `Drop` can afford (the abort path and
    /// a dropped stream): never blocks the caller.
    ///
    /// A short ladder first, because a link that failed for a moment (a supervisor
    /// restart, a dropped connection) usually answers the next attempt. The lease
    /// is recorded BEFORE that ladder starts, and a failed ladder leaves it in
    /// [`PendingReleases`]: an outage longer than the ladder used to drop the id
    /// with the task, and the slot stayed charged for the life of a LIVE worker —
    /// holder reaping runs on worker EXIT, and the dead-holder reaper only knows
    /// workers that are gone, so enough leaks pushed later requests through the
    /// whole 600-second admission budget (review #9). The drainer keeps retrying
    /// the idempotent release until the supervisor confirms it.
    pub fn release_detached(&self, lease: u64) {
        let pending = match self {
            // A local registry releases in-process and synchronously.
            AdmissionClient::Local(registry) => {
                registry.release(lease);
                return;
            }
            // A disabled plane mints no lease, so there is nothing to release.
            AdmissionClient::Disabled => return,
            AdmissionClient::Link { pending, .. } => pending,
        };
        // Recorded first: the record is what survives the ladder, and doing it
        // after a failed attempt would lose the lease if the task is cancelled.
        pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(lease);
        let client = self.clone();
        let Some(handle) = runtime_handle() else {
            // No runtime (a teardown): nothing can be spawned HERE, but the lease
            // is recorded, so the next admission round trip through this client
            // starts the drainer. Only a worker that never talks to the provider
            // again is left to the supervisor's holder reaping.
            return;
        };
        handle.spawn(async move {
            client.retry_pending_release(lease).await;
            client.drain_pending();
        });
    }

    /// The first rungs of one release's ladder, inline in the spawning task.
    async fn retry_pending_release(&self, lease: u64) {
        for attempt in 0..RELEASE_ATTEMPTS {
            match self.release_attempt(lease).await {
                ReleaseAttempt::Confirmed | ReleaseAttempt::Gone => {
                    self.forget_pending(lease);
                    return;
                }
                ReleaseAttempt::Unreachable => {}
            }
            if attempt + 1 < RELEASE_ATTEMPTS {
                tokio::time::sleep(RELEASE_BACKOFF[attempt.min(RELEASE_BACKOFF.len() - 1)]).await;
            }
        }
        tracing::warn!(
            lease,
            "provider admission: release unconfirmed after the retry ladder; keeping it for \
             retry while this worker runs"
        );
    }

    /// Spawn the one drainer if a lease is pending and none is running.
    ///
    /// Called at every admission round trip, so a link that comes back is used by
    /// the next call the worker makes rather than only by a new lease's drop.
    fn drain_pending(&self) {
        let AdmissionClient::Link {
            pending, draining, ..
        } = self
        else {
            return;
        };
        let Some(handle) = runtime_handle() else {
            return;
        };
        let has_pending = !pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if !has_pending || draining.swap(true, Ordering::SeqCst) {
            return;
        }
        let client = self.clone();
        handle.spawn(async move { client.drain_pending_loop().await });
    }

    /// Retry every recorded lease until it is confirmed, then stop.
    ///
    /// The pause between rounds is [`PENDING_RELEASE_PAUSE`]: the drainer is the
    /// slow lane, so it must not hammer a supervisor that is down, and it is on
    /// nobody's path — a confirmed release clears the id and the loop ends.
    async fn drain_pending_loop(&self) {
        let AdmissionClient::Link {
            pending, draining, ..
        } = self
        else {
            return;
        };
        loop {
            let first = pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .first()
                .copied();
            let Some(lease) = first else {
                draining.store(false, Ordering::SeqCst);
                // A lease recorded between the empty read and the flag clear must
                // not be stranded: re-check under the cleared flag.
                let raced = !pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_empty();
                if raced && !draining.swap(true, Ordering::SeqCst) {
                    continue;
                }
                return;
            };
            match self.release_attempt(lease).await {
                ReleaseAttempt::Confirmed | ReleaseAttempt::Gone => {
                    self.forget_pending(lease);
                    continue;
                }
                ReleaseAttempt::Unreachable => {}
            }
            tokio::time::sleep(PENDING_RELEASE_PAUSE).await;
        }
    }

    fn forget_pending(&self, lease: u64) {
        if let AdmissionClient::Link { pending, .. } = self {
            pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|queued| *queued != lease);
        }
    }

    /// How many unconfirmed releases the ledger still holds (tests observe the
    /// retention directly; the counter is the ledger's own length).
    #[cfg(test)]
    pub(super) fn pending_release_count(&self) -> usize {
        match self {
            AdmissionClient::Link { pending, .. } => pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            AdmissionClient::Local(_) | AdmissionClient::Disabled => 0,
        }
    }

    /// One admission round trip: `None` on any transport or refusal
    /// failure (fail-open at the call sites).
    async fn call(
        &self,
        link: &Arc<SupervisorLink>,
        _worker_token: &str,
        command: serde_json::Value,
    ) -> Option<serde_json::Value> {
        let action = command
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.call_within(link, command, request_budget(&action))
            .await
    }

    /// [`Self::call`] with an explicit deadline, for actions that never wait in a queue.
    async fn call_within(
        &self,
        link: &Arc<SupervisorLink>,
        command: serde_json::Value,
        budget: Duration,
    ) -> Option<serde_json::Value> {
        let response = link.request(command, budget).await.ok()?;
        if !response.success {
            return None;
        }
        Some(response.data.unwrap_or(serde_json::Value::Null))
    }
}

/// What one release round trip established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseAttempt {
    /// The supervisor took the release: the slot is free.
    Confirmed,
    /// The supervisor ANSWERED that this worker does not hold the lease. That is
    /// the answer to a release whose earlier attempt landed and whose answer was
    /// lost — the slot is already free, so a retry would be permanent noise.
    Gone,
    /// No answer (a dead or restarting link): the slot is still charged.
    Unreachable,
}

/// The supervisor's ownership refusal for a lease this worker no longer holds
/// (`supervisor/provider_admission.rs` answers it for every `settle_stream` /
/// `release_stream` against a lease the registry does not hold). Matched by text
/// because the two ends only meet on the JSON wire.
const NOT_HELD_MARKER: &str = "not held by this worker";

/// The current runtime's handle, when there is one: a `Drop` can run in a
/// teardown with no runtime, where nothing can be spawned.
fn runtime_handle() -> Option<tokio::runtime::Handle> {
    tokio::runtime::Handle::try_current().ok()
}

/// The holder name a local (in-process) client reports: one process, one
/// pool, so the reaper never needs to tell holders apart.
const LOCAL_HOLDER: &str = "local";

/// A settle/release round trip is short: the supervisor only mutates
/// counters. A queued acquire rides [`super::ACQUIRE_BUDGET`] instead.
/// The worker-side link deadline must cover the supervisor-side queue wait
/// ([`super::ACQUIRE_BUDGET`]) plus slack, or a worker admitted late finds its
/// link dead and the lease orphans (finding #4: 30s vs 600s minted
/// orphans on every deep-queue spawn).
pub(super) const ADMISSION_REQUEST_BUDGET: Duration = Duration::from_secs(605);

/// How many times a detached release is attempted before the worker gives up.
///
/// Bounded, because the launch path must not accumulate tasks on a supervisor that is gone:
/// the ladder spans roughly half a minute of retries over a 10-second request budget each,
/// which outlasts a supervisor restart and still ends.
const RELEASE_ATTEMPTS: usize = 3;

/// The pause between detached-release attempts. The first retry is immediate enough to hide
/// from a caller, and the last is long enough to outlast a supervisor's own restart window.
const RELEASE_BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(2)];

/// How long the pending-release drainer waits between rounds.
///
/// Longer than the ladder's last rung on purpose: the drainer is the slow lane for
/// a supervisor that is down or restarting, it is on nobody's path, and a slot
/// that comes back a few seconds late is far cheaper than a retry storm against a
/// dead link (review #9).
const PENDING_RELEASE_PAUSE: Duration = Duration::from_secs(15);

/// The deadline for a settle/release round trip.
///
/// Those actions only mutate counters — there is no queue to wait in — so inheriting the
/// 605-second acquire budget meant a wedged supervisor link held a worker's settle for ten
/// minutes while the lease it was settling stayed in flight (review #38). 10 s is ~1000x
/// the measured round trip and far under any retry deadline. Failure is handled the way
/// these callers already expect: `call_within` returns `None`, the settle is dropped, and
/// the supervisor's holder reaping covers the lease.
pub(super) const MUTATION_REQUEST_BUDGET: Duration = Duration::from_secs(10);

/// The deadline one admission action gets.
///
/// A single owner for the rule: `acquire_stream`/`spawn_gate` may wait in the supervisor's
/// queue, everything else only mutates counters. Written as a match so a new action must
/// be classified, and testable without a fake supervisor link (review #38).
pub(super) fn request_budget(action: &str) -> Duration {
    match action {
        // Only these two may park in the supervisor's FIFO for up to ACQUIRE_BUDGET.
        "acquire_stream" | "spawn_gate" => ADMISSION_REQUEST_BUDGET,
        // Settle and release only mutate counters; `snapshot` reads them. An UNKNOWN
        // action is refused by the supervisor, so waiting ten minutes for that refusal is
        // pure latency — the short budget is right for it too.
        _ => MUTATION_REQUEST_BUDGET,
    }
}

/// The wire body of one admission request.
fn admission_command(
    worker_token: &str,
    action: &str,
    key: &ProviderKey,
    spawn_id: Option<&str>,
) -> serde_json::Value {
    let mut command = serde_json::json!({
        "type": ADMISSION_COMMAND,
        "workerToken": worker_token,
        "action": action,
        "provider": key.provider,
        "model": key.model,
    });
    if let Some(spawn_id) = spawn_id {
        command["spawnId"] = serde_json::json!(spawn_id);
    }
    command
}

/// [`admission_command`] for a lease-only action: the supervisor knows the
/// provider from the lease.
fn lease_command(worker_token: &str, action: &str, lease: u64) -> serde_json::Value {
    serde_json::json!({
        "type": ADMISSION_COMMAND,
        "workerToken": worker_token,
        "action": action,
        "leaseId": lease,
    })
}

/// One held admission slot. Settling or releasing consumes it; dropping it
/// releases the slot fire-and-forget (the abort path).
pub struct StreamLease {
    pub(super) id: u64,
    pub(super) client: AdmissionClient,
    /// Set once the settle/release round trip RESOLVED. `id` stays valid until
    /// then so a cancelled future, a dropped stream, or a failed call still
    /// reaches `Drop`'s detached release — clearing `id` first orphaned the slot
    /// on a live worker (review #7). The flag, not a zeroed id, is what keeps a
    /// resolved lease from releasing twice.
    pub(super) resolved: bool,
}

impl StreamLease {
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Settle the slot with its AIMD signal.
    ///
    /// The id is cleared only after the call RESOLVES: clearing it first
    /// disabled `Drop`'s detached release, so a cancelled future or a failed
    /// round trip left the slot held forever on a live worker (review #7).
    pub async fn settle(mut self, outcome: StreamOutcome) {
        // `resolved` only on a CONFIRMED settle. A timeout or a refusal leaves it
        // false, so `Drop` still runs its detached release and the slot comes
        // back instead of staying held by a live worker (review #7).
        self.resolved = self.client.settle(self.id, outcome).await;
    }

    /// Give the slot back without a signal.
    pub async fn release(mut self) {
        self.resolved = self.client.release(self.id).await;
    }
}

impl Drop for StreamLease {
    fn drop(&mut self) {
        // A resolved lease already gave its slot back; an unresolved one still
        // holds it, whether the caller dropped it, the future was cancelled, or
        // the round trip failed.
        if !self.resolved {
            self.client.release_detached(self.id);
        }
    }
}

/// The provider key a stream is charged to: the live provider target's
/// resolved pair (a `set_model`/failover swap must be charged to the
/// provider that actually serves the request).
pub type KeyResolver = Arc<dyn Fn() -> ProviderKey + Send + Sync>;
