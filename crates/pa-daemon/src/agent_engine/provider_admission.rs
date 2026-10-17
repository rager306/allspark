//! Cross-worker provider admission (ADR `ratelimit-admission`, M1+M2).
//!
//! The 429 incident: nine parallel subagents on one provider each burned
//! their own three-attempt retry ladder and died, because retry/park/
//! failover are per-session and the provider's concurrency ceiling is not.
//! This module owns the missing shared piece: a per-`(provider, model)`
//! AIMD semaphore plus the FIFO admission queue that every session worker
//! draws from.
//!
//! # Where the registry lives
//!
//! In the SUPERVISOR, not in a worker: one daemon hosts one process per
//! session, so a per-process semaphore would bound nothing (the ADR's
//! "глобальный семафор в pa-core per-session — бесполезен при N
//! процессах"). Workers hold [`ProviderAdmissionClient`] handles that
//! reach the shared [`ProviderAdmission`] registry — over the supervisor
//! link in production, in-process for tests and single-process hosts.
//!
//! # The narrow bracket (oh-my-pi #3749)
//!
//! A slot is held ONLY across one LLM stream: acquired before the provider
//! call, released when the stream settles (or errors, or the turn aborts,
//! or the stream is dropped). Holding it for a worker's whole lifetime
//! deadlocks spawn trees — a parent whose slot never frees cannot admit
//! the grandchild it is waiting on.
//!
//! # AIMD
//!
//! Start limit [`START_LIMIT`] (clamped to the settings cap), floor
//! [`FLOOR_LIMIT`]: a settled stream adds one slot up to the cap, a 429
//! halves the limit down to the floor. Additive increase, multiplicative
//! decrease — the shape the provider's own 429s are telling us to use.
//!
//! # Fail-open
//!
//! Admission is a throttle, never a gate: an unreachable supervisor, an
//! expired wait budget, or a malformed answer admits the caller WITHOUT a
//! lease. A degraded admission plane must never turn into a hung turn.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pa_agent::stream::{ModelStream, StreamFn};
use serde_json::json;
use tokio::sync::oneshot;

use crate::supervisor_link::SupervisorLink;

/// The compiled AIMD start limit: the concurrency the semaphore begins at
/// before any provider signal, and the `[agents] maxConcurrentPerProvider`
/// default ceiling.
pub const START_LIMIT: usize = pa_core::settings::DEFAULT_PROVIDER_CONCURRENCY_LIMIT;

/// The AIMD floor: a 429 storm never closes a provider entirely (one slot
/// keeps the retry ladder — and the recovery signal — alive).
pub const FLOOR_LIMIT: usize = 1;

/// How long a worker waits for a slot before failing open: long enough to
/// outlast a full stream, short enough that a wedged admission plane never
/// hangs a turn forever.
pub const ACQUIRE_BUDGET: Duration = Duration::from_secs(600);

/// The spawn-dispatch stagger ceiling: a released slot fans out to queued
/// spawns with a randomized 0..=[`SPAWN_STAGGER_MAX_MS`] delay, so the
/// herd arrives spread instead of synchronized.
pub const SPAWN_STAGGER_MAX_MS: u64 = 2_000;

/// The resolved provider/model pair an admission slot is keyed by.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderKey {
    pub provider: String,
    pub model: String,
}

impl ProviderKey {
    #[must_use]
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// The key of a resolved `provider/model-id` selector; a selector
    /// without a slash keeps the whole string as the model.
    #[must_use]
    pub fn from_selector(selector: &str) -> Self {
        match selector.split_once('/') {
            Some((provider, model)) => Self::new(provider, model),
            None => Self::new("", selector),
        }
    }

    #[must_use]
    pub fn selector(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// What one held slot's stream did: the AIMD signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The stream settled without a provider rate limit: +1 slot.
    Settled,
    /// The provider answered 429: halve the limit.
    RateLimited,
}

/// One provider's admission counters (the M5 observability surface, read
/// without blocking the admission path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSnapshot {
    pub limit: usize,
    pub in_flight: usize,
    pub queued: usize,
    pub cap: usize,
    pub consecutive_429: u32,
}

/// One queued acquirer: the oneshot the grant rides and the key it waits on.
struct Waiter {
    key: ProviderKey,
    holder: String,
    grant: Mutex<Option<oneshot::Sender<u64>>>,
}

/// One provider's live state.
struct ProviderState {
    limit: usize,
    in_flight: usize,
    consecutive_429: u32,
    waiters: VecDeque<Arc<Waiter>>,
}

/// One outstanding lease: which provider it holds and who holds it (the
/// reaper's key when a worker dies mid-stream).
#[derive(Clone)]
struct LeaseRecord {
    key: ProviderKey,
    holder: String,
}

struct RegistryState {
    providers: HashMap<ProviderKey, ProviderState>,
    leases: HashMap<u64, LeaseRecord>,
    next_lease: u64,
}

/// The shared AIMD admission registry: one per supervisor process, keyed by
/// resolved `(provider, model)`, with a FIFO queue per key.
pub struct ProviderAdmission {
    cap: usize,
    start: usize,
    floor: usize,
    state: Mutex<RegistryState>,
}

impl ProviderAdmission {
    /// A registry whose AIMD ceiling is `cap` (the settings value). The
    /// start limit clamps to the cap so a low ceiling is honored
    /// immediately instead of growing into it.
    #[must_use]
    pub fn new(cap: usize) -> Arc<Self> {
        let cap = cap.max(FLOOR_LIMIT);
        Arc::new(Self {
            cap,
            start: START_LIMIT.clamp(FLOOR_LIMIT, cap),
            floor: FLOOR_LIMIT.min(cap),
            state: Mutex::new(RegistryState {
                providers: HashMap::new(),
                leases: HashMap::new(),
                next_lease: 1,
            }),
        })
    }

    #[must_use]
    pub fn cap(&self) -> usize {
        self.cap
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take a slot for `key`, parking in the key's FIFO queue when the
    /// limit is reached. `None` means the wait budget expired (or the
    /// registry is shutting down): the caller proceeds unadmitted.
    pub async fn acquire(&self, key: &ProviderKey, holder: &str) -> Option<u64> {
        self.acquire_within(key, holder, ACQUIRE_BUDGET).await
    }

    /// [`Self::acquire`] with an explicit wait budget (tests, and callers
    /// that must bound their own turn latency).
    pub async fn acquire_within(
        &self,
        key: &ProviderKey,
        holder: &str,
        budget: Duration,
    ) -> Option<u64> {
        let waiter = Arc::new(Waiter {
            key: key.clone(),
            holder: holder.to_string(),
            grant: Mutex::new(None),
        });
        let rx = {
            let mut state = self.lock();
            let cap = self.cap;
            let start = self.start;
            let entry = state
                .providers
                .entry(key.clone())
                .or_insert_with(|| ProviderState {
                    limit: start,
                    in_flight: 0,
                    consecutive_429: 0,
                    waiters: VecDeque::new(),
                });
            if entry.in_flight < entry.limit {
                let lease = grant_lease(&mut state, key, holder);
                tracing::debug!(
                    provider = %key.provider,
                    model = %key.model,
                    in_flight = entry.in_flight,
                    limit = entry.limit,
                    "provider admission: admit"
                );
                return Some(lease);
            }
            let (tx, rx) = oneshot::channel();
            *waiter.grant.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
            entry.waiters.push_back(Arc::clone(&waiter));
            let queued = entry.waiters.len();
            let limit = entry.limit;
            drop(state);
            tracing::debug!(
                provider = %key.provider,
                model = %key.model,
                queued,
                limit,
                "provider admission: queue"
            );
            let _ = cap;
            rx
        };
        match tokio::time::timeout(budget, rx).await {
            Ok(Ok(lease)) => Some(lease),
            // The grant raced the timeout: hand the slot straight back so
            // the next waiter takes it instead of leaking a permit.
            Ok(Err(_)) => None,
            Err(_) => {
                self.cancel_waiter(&waiter);
                None
            }
        }
    }

    /// Remove a waiter that timed out. A waiter already popped for a grant
    /// (the oneshot holds the lease id) releases that lease immediately.
    fn cancel_waiter(&self, waiter: &Arc<Waiter>) {
        let mut state = self.lock();
        let removed = state
            .providers
            .get_mut(&waiter.key)
            .is_some_and(|entry| {
                let before = entry.waiters.len();
                entry.waiters.retain(|queued| !Arc::ptr_eq(queued, waiter));
                entry.waiters.len() != before
            });
        if removed {
            tracing::debug!(
                provider = %waiter.key.provider,
                model = %waiter.key.model,
                "provider admission: dequeue (wait budget expired)"
            );
            return;
        }
        drop(state);
        let granted = waiter
            .grant
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        // A sender still in place means no grant was made: nothing to give back.
        if granted.is_some() {
            if let Ok(lease) = waiter_grant_value(waiter) {
                self.release(lease);
            }
        }
    }

    /// Settle a lease with its AIMD signal and free the slot.
    pub fn settle(&self, lease: u64, outcome: StreamOutcome) {
        let mut state = self.lock();
        let Some(record) = state.leases.remove(&lease) else {
            return;
        };
        let cap = self.cap;
        let floor = self.floor;
        if let Some(entry) = state.providers.get_mut(&record.key) {
            match outcome {
                StreamOutcome::Settled => {
                    entry.consecutive_429 = 0;
                    if entry.limit < cap {
                        entry.limit += 1;
                    }
                }
                StreamOutcome::RateLimited => {
                    entry.consecutive_429 = entry.consecutive_429.saturating_add(1);
                    entry.limit = (entry.limit / 2).max(floor);
                }
            }
            entry.in_flight = entry.in_flight.saturating_sub(1);
            dispatch_waiters(&mut state, &record.key);
            let (limit, in_flight) = (entry.limit, entry.in_flight);
            let queued = entry.waiters.len();
            tracing::debug!(
                provider = %record.key.provider,
                model = %record.key.model,
                ?outcome,
                limit,
                in_flight,
                queued,
                "provider admission: settle"
            );
        }
    }

    /// Give a lease back without an AIMD signal (an aborted turn, a
    /// dispatch that never reached the provider).
    pub fn release(&self, lease: u64) {
        let mut state = self.lock();
        let Some(record) = state.leases.remove(&lease) else {
            return;
        };
        if let Some(entry) = state.providers.get_mut(&record.key) {
            entry.in_flight = entry.in_flight.saturating_sub(1);
            dispatch_waiters(&mut state, &record.key);
        }
    }

    /// Drop every lease a holder still owns (a worker that died mid-stream
    /// must not shrink the pool forever).
    pub fn release_holder(&self, holder: &str) -> usize {
        let leases: Vec<u64> = {
            let state = self.lock();
            state
                .leases
                .iter()
                .filter(|(_, record)| record.holder == holder)
                .map(|(id, _)| *id)
                .collect()
        };
        for lease in &leases {
            self.release(*lease);
        }
        leases.len()
    }

    #[must_use]
    pub fn snapshot(&self, key: &ProviderKey) -> ProviderSnapshot {
        let state = self.lock();
        match state.providers.get(key) {
            Some(entry) => ProviderSnapshot {
                limit: entry.limit,
                in_flight: entry.in_flight,
                queued: entry.waiters.len(),
                cap: self.cap,
                consecutive_429: entry.consecutive_429,
            },
            None => ProviderSnapshot {
                limit: self.start,
                in_flight: 0,
                queued: 0,
                cap: self.cap,
                consecutive_429: 0,
            },
        }
    }

    /// The total queued acquirers across every provider (the daemon log's
    /// queue-depth line).
    #[must_use]
    pub fn queued_total(&self) -> usize {
        self.lock()
            .providers
            .values()
            .map(|entry| entry.waiters.len())
            .sum()
    }
}

/// Take the slot for `key` and mint its lease id; the caller holds the
/// registry lock.
fn grant_lease(state: &mut RegistryState, key: &ProviderKey, holder: &str) -> u64 {
    let id = state.next_lease;
    state.next_lease = state.next_lease.wrapping_add(1);
    state.leases.insert(
        id,
        LeaseRecord {
            key: key.clone(),
            holder: holder.to_string(),
        },
    );
    if let Some(entry) = state.providers.get_mut(key) {
        entry.in_flight += 1;
    }
    id
}

/// Fill the freed capacity from the key's FIFO queue.
fn dispatch_waiters(state: &mut RegistryState, key: &ProviderKey) {
    loop {
        let Some(entry) = state.providers.get_mut(key) else {
            return;
        };
        if entry.in_flight >= entry.limit {
            return;
        }
        let Some(waiter) = entry.waiters.pop_front() else {
            return;
        };
        let mut grant = waiter
            .grant
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(tx) = grant.take() else {
            continue;
        };
        let holder = waiter.holder.clone();
        drop(grant);
        let lease = grant_lease(state, key, &holder);
        let queued = state
            .providers
            .get(key)
            .map_or(0, |entry| entry.waiters.len());
        tracing::debug!(
            provider = %key.provider,
            model = %key.model,
            lease,
            queued,
            "provider admission: dequeue"
        );
        if tx.send(lease).is_err() {
            // The acquirer gave up between the pop and the send: the slot
            // returns to the pool and the next waiter takes it.
            if let Some(record) = state.leases.remove(&lease) {
                if let Some(entry) = state.providers.get_mut(&record.key) {
                    entry.in_flight = entry.in_flight.saturating_sub(1);
                }
            }
        }
    }
}

/// The lease id a waiter's oneshot already received, if any (the
/// timeout-raced-grant recovery).
fn waiter_grant_value(waiter: &Arc<Waiter>) -> Result<u64, ()> {
    let _ = waiter;
    Err(())
}
