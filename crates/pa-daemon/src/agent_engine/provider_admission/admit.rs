//! Taking a slot: the breaker gate, the AIMD admit-or-queue decision, and the
//! two waits — one for a stream, one for a spawn's dispatch.
//!
//! This is the first half of `impl ProviderAdmission`; the second (giving a slot
//! back, dispatching the queue, serving the snapshots) is [`super::settle`].
//! Rust lets one type's inherent impl be split across a crate's modules, and the
//! seam sits at the admission/settlement boundary rather than at an arbitrary
//! line: `settle` is the last method that TAKES capacity and `release` the first
//! that returns it.
//!
//! The breaker gate, the probe bookkeeping and `breaker_settle` stay here rather
//! than in [`super::breaker`], because each takes the registry lock and so
//! belongs beside the registry; `breaker.rs` owns the state machine they drive.
//!
//! Four members are `pub(super)` because `settle` — a sibling module, and so not
//! a descendant that private visibility would reach — calls them: `lock`,
//! `mint`, `telemetry_client`, `track_breaker_transition`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pa_telemetry::{AdmissionBreakerTransition, AdmissionLimitStep, TelemetryClient};
use tokio::sync::oneshot;

use super::breaker::{BreakerGate, BreakerState};
use super::clock::{ClockSource, JitteredStagger, StaggerSource, SystemClock};
use super::{
    Grant, HolderLiveness, LeaseRecord, ProviderAdmission, ProviderKey, ProviderState,
    RegistryState, StreamAdmission, StreamOutcome, Waiter, ACQUIRE_BUDGET, BREAKER_COOLDOWN_MS,
    FLOOR_LIMIT, MAX_QUEUED_WAITERS, RETRY_BUDGET_WINDOW, START_LIMIT,
};

impl ProviderAdmission {
    /// A registry whose AIMD ceiling is `cap` (the settings value). The
    /// start limit clamps to the cap, so a low ceiling is honored
    /// immediately instead of growing into it.
    #[must_use]
    pub fn new(cap: usize) -> Arc<Self> {
        Self::with_stagger(cap, Arc::new(JitteredStagger::new()))
    }

    /// [`Self::new`] with an injected stagger source (tests).
    #[must_use]
    pub fn with_stagger(cap: usize, stagger: Arc<dyn StaggerSource>) -> Arc<Self> {
        Self::with_parts(cap, stagger, Arc::new(SystemClock))
    }

    /// [`Self::new`] with injected stagger and clock sources (tests):
    /// the scripted clock drives breaker transitions without sleeping.
    #[must_use]
    pub fn with_parts(
        cap: usize,
        stagger: Arc<dyn StaggerSource>,
        clock: Arc<dyn ClockSource>,
    ) -> Arc<Self> {
        let cap = cap.max(FLOOR_LIMIT);
        Arc::new(Self {
            cap,
            start: START_LIMIT.clamp(FLOOR_LIMIT, cap),
            floor: FLOOR_LIMIT.min(cap),
            stagger,
            clock,
            liveness: Mutex::new(None),
            state: Mutex::new(RegistryState {
                providers: HashMap::new(),
                breakers: HashMap::new(),
                leases: HashMap::new(),
                // Finding #6: sequential lease ids from 1 are predictable;
                // start from a random offset so a lease id carries no
                // information about how many leases came before.
                next_lease: uuid::Uuid::new_v4().as_u128() as u64 | 1,
            }),
            telemetry: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// The limit a provider starts at (the AIMD start, clamped to the cap).
    #[must_use]
    pub fn start_limit(&self) -> usize {
        self.start
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wire the adoption-telemetry seam: the daemon's telemetry client,
    /// attached once the supervisor has built the registry (the supervisor
    /// owns the client's lifecycle). Every breaker transition, AIMD limit
    /// step, and served snapshot rollup then rides it — counters only,
    /// never holder or model text.
    pub fn attach_telemetry(&self, client: TelemetryClient) {
        *self
            .telemetry
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(client);
    }

    /// The attached client, when the seam is wired (`track` never blocks,
    /// so the emission sites may hold the registry lock).
    pub(super) fn telemetry_client(&self) -> Option<TelemetryClient> {
        self.telemetry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Emit one breaker flip's adoption event: `failures` is the
    /// retry-budget window's rate-limited count at the decision moment.
    pub(super) fn track_breaker_transition(
        &self,
        provider: &str,
        from: BreakerState,
        to: BreakerState,
        failures: usize,
    ) {
        let Some(client) = self.telemetry_client() else {
            return;
        };
        AdmissionBreakerTransition {
            provider: provider.to_string(),
            from_state: from.as_wire(),
            to_state: to.as_wire(),
            failure_count: failures as u64,
        }
        .track(&client);
    }

    /// Emit one AIMD limit step (`admission_promoted` up,
    /// `admission_demoted` down): the direction is the limits themselves.
    fn track_limit_step(
        &self,
        provider: &str,
        old_limit: usize,
        new_limit: usize,
        reason: &'static str,
    ) {
        let Some(client) = self.telemetry_client() else {
            return;
        };
        let step = AdmissionLimitStep {
            provider: provider.to_string(),
            new_limit: new_limit as u64,
            old_limit: old_limit as u64,
            reason,
        };
        if new_limit > old_limit {
            step.track_promoted(&client);
        } else {
            step.track_demoted(&client);
        }
    }

    /// Take a slot for `key` right now if the limit allows it; `None` means
    /// the caller must queue.
    fn admit_now(
        &self,
        state: &mut RegistryState,
        key: &ProviderKey,
        holder: &str,
        dead: &[u64],
    ) -> Option<u64> {
        let start = self.start;
        // Dead slots are reclaimed only when the caller found them AND the key is
        // full: a free slot needs no explanation, and the liveness lookup is an await
        // that cannot run here.
        if !dead.is_empty()
            && state
                .providers
                .get(key)
                .is_some_and(|entry| entry.in_flight >= entry.limit)
        {
            Self::reclaim_holders(state, key, dead);
        }
        let admitted = {
            let entry = state
                .providers
                .entry(key.clone())
                .or_insert_with(|| ProviderState {
                    limit: start,
                    in_flight: 0,
                    consecutive_429: 0,
                    waiters: VecDeque::new(),
                });
            // A queued waiter outranks a fresh arrival (FIFO, never
            // bypassed): the slot goes to the queue's head first.
            if entry.waiters.is_empty() && entry.in_flight < entry.limit {
                entry.in_flight += 1;
                Some((entry.limit, entry.in_flight))
            } else {
                None
            }
        };
        let (limit, in_flight) = admitted?;
        let lease = Self::mint(state, key, holder);
        tracing::debug!(
            provider = %key.provider,
            model = %key.model,
            holder,
            limit,
            in_flight,
            lease,
            "provider admission: admit"
        );
        Some(lease)
    }

    /// Mint a lease for `key`; the caller holds the registry lock and has
    /// already counted the slot.
    pub(super) fn mint(state: &mut RegistryState, key: &ProviderKey, holder: &str) -> u64 {
        let id = state.next_lease;
        state.next_lease = state.next_lease.wrapping_add(1);
        state.leases.insert(
            id,
            LeaseRecord {
                key: key.clone(),
                holder: holder.to_string(),
            },
        );
        id
    }

    /// Park a waiter in `key`'s FIFO queue.
    fn enqueue(
        &self,
        state: &mut RegistryState,
        key: &ProviderKey,
        holder: &str,
    ) -> Option<(oneshot::Receiver<Grant>, Arc<Waiter>)> {
        let start = self.start;
        let waiter = Arc::new(Waiter {
            holder: holder.to_string(),
            grant: Mutex::new(None),
        });
        let (tx, rx) = oneshot::channel();
        *waiter.grant.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
        let entry = state
            .providers
            .entry(key.clone())
            .or_insert_with(|| ProviderState {
                limit: start,
                in_flight: 0,
                consecutive_429: 0,
                waiters: VecDeque::new(),
            });
        // A queue past the ceiling refuses: the caller fails over now instead of
        // holding a slot of memory for up to `ACQUIRE_BUDGET` and joining a herd when
        // the breaker re-opens (review #39).
        if entry.waiters.len() >= MAX_QUEUED_WAITERS {
            tracing::info!(
                provider = %key.provider,
                model = %key.model,
                holder,
                queued = entry.waiters.len(),
                "provider admission: queue full, refusing"
            );
            return None;
        }
        entry.waiters.push_back(Arc::clone(&waiter));
        let queued = entry.waiters.len();
        let limit = entry.limit;
        let in_flight = entry.in_flight;
        tracing::info!(
            provider = %key.provider,
            model = %key.model,
            holder,
            queued,
            limit,
            in_flight,
            "provider admission: queue"
        );
        Some((rx, waiter))
    }

    /// The leases on `key` whose holder is provably gone.
    ///
    /// Returns empty when the key is not full (nothing to reclaim) or when no
    /// liveness source is installed. The lookup itself is the cost this pays to
    /// avoid: without it, a slot held by a killed worker can only be recovered by
    /// waiting out `ACQUIRE_BUDGET`.
    async fn dead_leases_for(&self, key: &ProviderKey) -> Vec<u64> {
        let (full, candidates) = {
            let state = self.lock();
            let full = state
                .providers
                .get(key)
                .is_some_and(|entry| entry.in_flight >= entry.limit);
            let candidates: Vec<(u64, String)> = state
                .leases
                .iter()
                .filter(|(_, record)| record.key == *key)
                .map(|(id, record)| (*id, record.holder.clone()))
                .collect();
            (full, candidates)
        };
        if !full || candidates.is_empty() {
            return Vec::new();
        }
        let alive = {
            let guard = self
                .liveness
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.as_ref().map(Arc::clone)
        };
        let Some(alive) = alive else {
            return Vec::new();
        };
        let mut dead = Vec::new();
        for (lease, holder) in candidates {
            let probe = alive(holder);
            match probe.await {
                Ok(true) => {}
                // A live answer of `false` means the worker is gone; an errored probe
                // (a panicked join) is treated the same: the slot is not worth a
                // 600-second wait on a source that cannot answer.
                Ok(false) | Err(_) => dead.push(lease),
            }
        }
        dead
    }

    /// Teach the registry how to tell a live lease holder from a dead one.
    ///
    /// The supervisor owns worker lifetime, so the supervisor supplies this; a
    /// registry used without a supervisor keeps `None` and the old behaviour.
    pub fn set_holder_liveness(&self, alive: HolderLiveness) {
        *self
            .liveness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(alive);
    }

    /// Reclaim slots whose holder is provably dead, returning how many were freed.
    ///
    /// `dead` is decided by the caller BEFORE this takes the lock: asking the worker
    /// registry is an await, and holding the admission mutex across it would put a
    /// file-system-ish lookup on every other acquirer's critical path.
    fn reclaim_holders(state: &mut RegistryState, key: &ProviderKey, dead: &[u64]) -> usize {
        let _ = key;
        let dead: Vec<u64> = dead.to_vec();
        if dead.is_empty() {
            return 0;
        }
        for lease in &dead {
            if let Some(record) = state.leases.remove(lease) {
                if let Some(entry) = state.providers.get_mut(&record.key) {
                    entry.in_flight = entry.in_flight.saturating_sub(1);
                }
            }
        }
        tracing::info!(
            provider = %key.provider,
            model = %key.model,
            reclaimed = dead.len(),
            "provider admission: reclaimed slot(s) of dead lease holders"
        );
        dead.len()
    }

    /// Take a stream slot for `key`, parking in the key's FIFO queue when
    /// the limit is reached. [`StreamAdmission::Unadmitted`] means the
    /// wait budget expired: the caller proceeds unadmitted (fail-open);
    /// [`StreamAdmission::Refused`] means the provider's breaker is
    /// open: the caller fails over instead.
    pub async fn acquire_stream(&self, key: &ProviderKey, holder: &str) -> StreamAdmission {
        self.acquire_stream_within(key, holder, ACQUIRE_BUDGET)
            .await
    }

    /// [`Self::acquire_stream`] with an explicit wait budget.
    pub async fn acquire_stream_within(
        &self,
        key: &ProviderKey,
        holder: &str,
        budget: Duration,
    ) -> StreamAdmission {
        // Resolve liveness BEFORE taking the registry lock: asking the worker registry
        // is an await, and holding the admission mutex across it would put a lookup on
        // every other acquirer's critical path. Only a full key needs the answer.
        let dead = self.dead_leases_for(key).await;
        let (pending, is_probe) = {
            // The instrumented form wraps the whole locked decision (wait
            // for the registry lock plus the admit/queue verdict) under one
            // per-(provider, model) label; the plain form is byte-identical
            // to the pre-profiling code.
            #[cfg(feature = "profiling")]
            let mut state = hotpath::measure_block!({ "admission:acquire" }, self.lock());
            #[cfg(not(feature = "profiling"))]
            let mut state = self.lock();
            let gate = self.breaker_gate(&mut state, key);
            if matches!(gate, BreakerGate::Refuse) {
                return StreamAdmission::Refused;
            }
            // This acquirer is the half-open probe when the gate said so:
            // if it never reaches a lease, the probe must un-wedge (below).
            let is_probe = matches!(gate, BreakerGate::Probe);
            match self.admit_now(&mut state, key, holder, &dead) {
                Some(lease) => {
                    if is_probe {
                        Self::note_probe(&mut state, &key.provider, lease);
                    }
                    return StreamAdmission::Admitted(lease);
                }
                None => match self.enqueue(&mut state, key, holder) {
                    Some(queued) => (queued, is_probe),
                    // The queue is full: fail over now, like a spent wait budget.
                    None => return StreamAdmission::Refused,
                },
            }
        };
        let (rx, waiter) = pending;
        match tokio::time::timeout(budget, rx).await {
            Ok(Ok(grant)) => {
                if is_probe {
                    let mut state = self.lock();
                    Self::note_probe(&mut state, &key.provider, grant.lease);
                }
                StreamAdmission::Admitted(grant.lease)
            }
            // The sender dropped (registry shutdown): fail open.
            Ok(Err(_)) => StreamAdmission::Unadmitted,
            Err(_) => {
                self.abandon(&waiter, key);
                if is_probe {
                    self.unwedge_probe(&key.provider);
                }
                tracing::info!(
                    provider = %key.provider,
                    model = %key.model,
                    holder,
                    "provider admission: stream wait budget expired, admitting unadmitted"
                );
                StreamAdmission::Unadmitted
            }
        }
    }

    /// The breaker's verdict for one STREAM acquire. `Probe` arms this
    /// acquirer as the single half-open probe (the state is already
    /// `HalfOpen` when it returns). Spawns never gate: a spawn
    /// reservation is the create round trip, not provider traffic — the
    /// child's own first stream gates itself (review #11 adjudicated: the
    /// M4 contract stands, the finding is a policy change, not a defect).
    fn breaker_gate(&self, state: &mut RegistryState, key: &ProviderKey) -> BreakerGate {
        let now = self.clock.now_ms();
        let breaker = state.breakers.entry(key.provider.clone()).or_default();
        match breaker.state {
            BreakerState::Closed => BreakerGate::Pass,
            BreakerState::Open => {
                if now.saturating_sub(breaker.opened_at_ms) >= BREAKER_COOLDOWN_MS {
                    breaker.state = BreakerState::HalfOpen;
                    let failures = breaker.failures();
                    tracing::info!(
                        provider = %key.provider,
                        "provider admission: breaker half-open, dispatching one probe"
                    );
                    self.track_breaker_transition(
                        &key.provider,
                        BreakerState::Open,
                        BreakerState::HalfOpen,
                        failures,
                    );
                    BreakerGate::Probe
                } else {
                    tracing::debug!(
                        provider = %key.provider,
                        model = %key.model,
                        "provider admission: circuit open, acquire refused"
                    );
                    BreakerGate::Refuse
                }
            }
            // A probe is already in flight: everything else fail-fasts.
            BreakerState::HalfOpen => BreakerGate::Refuse,
        }
    }

    /// Bind a minted lease as the provider's half-open probe.
    fn note_probe(state: &mut RegistryState, provider: &str, lease: u64) {
        if let Some(breaker) = state.breakers.get_mut(provider) {
            breaker.probe_lease = Some(lease);
        }
    }

    /// A probe that never reached a lease (its wait expired first) goes
    /// back to OPEN with a fresh cooldown, so the breaker cannot wedge
    /// in half-open with no probe in flight.
    fn unwedge_probe(&self, provider: &str) {
        let now = self.clock.now_ms();
        let mut state = self.lock();
        if let Some(breaker) = state.breakers.get_mut(provider) {
            if breaker.state == BreakerState::HalfOpen && breaker.probe_lease.is_none() {
                breaker.state = BreakerState::Open;
                breaker.opened_at_ms = now;
                self.track_breaker_transition(
                    provider,
                    BreakerState::HalfOpen,
                    BreakerState::Open,
                    breaker.failures(),
                );
            }
        }
    }

    /// Wait for a free slot to dispatch one spawn on `key`; the answer is
    /// the lease plus the randomized stagger to wait before starting the
    /// child. `None` = wait budget expired (fail-open: start anyway).
    pub async fn spawn_gate(&self, key: &ProviderKey, spawn_id: &str) -> Option<(u64, Duration)> {
        self.spawn_gate_within(key, spawn_id, ACQUIRE_BUDGET).await
    }

    /// [`Self::spawn_gate`] with an explicit wait budget.
    pub async fn spawn_gate_within(
        &self,
        key: &ProviderKey,
        spawn_id: &str,
        budget: Duration,
    ) -> Option<(u64, Duration)> {
        // A spawn reservation can also be held by a dead worker; the same evidence
        // applies (the spawn gate draws from the same pool).
        let dead = self.dead_leases_for(key).await;
        let pending = {
            let mut state = self.lock();
            // Deliberately NOT gated on the breaker (review #11 reopened and closed WONTFIX):
            // M4's decision, pinned by `an_open_breaker_does_not_gate_spawn_admissions`, is
            // that a spawn reservation is the create round trip, not provider traffic — the
            // child's own first stream gates itself and fail-fasts to failover. Gating the
            // create would break session startup on a cooling-down provider. The pool cost is
            // real but bounded by the spawn's own lifetime. The two tests that pin this
            // contract were written before the review and are not wrong.
            match self.admit_now(&mut state, key, spawn_id, &dead) {
                Some(lease) => {
                    let stagger_ms = self.stagger.next_delay_ms();
                    tracing::info!(
                        provider = %key.provider,
                        model = %key.model,
                        spawn_id,
                        lease,
                        stagger_ms,
                        "provider admission: spawn dispatch"
                    );
                    return Some((lease, Duration::from_millis(stagger_ms)));
                }
                None => self.enqueue(&mut state, key, spawn_id),
            }
        };
        // A full queue refuses the spawn the same way a spent budget does: the child
        // starts unadmitted rather than waiting 600 s for a slot that cannot arrive.
        let (rx, waiter) = pending?;
        match tokio::time::timeout(budget, rx).await {
            Ok(Ok(grant)) => Some((grant.lease, Duration::from_millis(grant.stagger_ms))),
            Ok(Err(_)) => None,
            Err(_) => {
                self.abandon(&waiter, key);
                tracing::info!(
                    provider = %key.provider,
                    model = %key.model,
                    spawn_id,
                    "provider admission: spawn wait budget expired, dispatching unadmitted"
                );
                None
            }
        }
    }

    /// Drop a waiter whose wait budget expired (its grant, if one already
    /// landed, is handed straight back to the pool).
    fn abandon(&self, waiter: &Arc<Waiter>, key: &ProviderKey) {
        let mut state = self.lock();
        let removed = state.providers.get_mut(key).is_some_and(|entry| {
            let before = entry.waiters.len();
            entry.waiters.retain(|queued| !Arc::ptr_eq(queued, waiter));
            entry.waiters.len() != before
        });
        // `removed` means the waiter never got a grant, so the pool is
        // untouched. A waiter already popped for a grant the receiver no
        // longer wants has had its slot returned by the dispatch loop's
        // failed send — nothing to undo here either way.
        let _ = removed;
    }

    /// Settle a lease with its AIMD signal and free the slot.
    pub fn settle(&self, lease: u64, outcome: StreamOutcome) {
        // The instrumented form measures the whole settle span (lock wait +
        // AIMD update + queue dispatch decision) under one label; the plain
        // form is the pre-profiling code.
        #[cfg(feature = "profiling")]
        let mut state = hotpath::measure_block!({ "admission:settle" }, self.lock());
        #[cfg(not(feature = "profiling"))]
        let mut state = self.lock();
        let Some(record) = state.leases.remove(&lease) else {
            return;
        };
        let cap = self.cap;
        let floor = self.floor;
        let mut report = None;
        if let Some(entry) = state.providers.get_mut(&record.key) {
            let old_limit = entry.limit;
            match outcome {
                StreamOutcome::Settled => {
                    entry.consecutive_429 = 0;
                    if entry.limit < cap {
                        entry.limit += 1;
                    }
                }
                StreamOutcome::Failed => {
                    // No AIMD signal either way: the provider is broken, and the right
                    // concurrency for a broken provider is not something this loop knows.
                    // The breaker below is what stops traffic.
                }
                StreamOutcome::RateLimited(reason) => {
                    entry.consecutive_429 = entry.consecutive_429.saturating_add(1);
                    // M3: an account-local cause (a spent quota or spend
                    // cap) drops the limit to the floor at once — the
                    // account cannot serve ANY concurrency until its
                    // window resets, so only the recovery probe remains.
                    entry.limit = if reason.is_account_local() {
                        floor
                    } else {
                        (entry.limit / 2).max(floor)
                    };
                }
            }
            if entry.limit != old_limit {
                let reason = match outcome {
                    StreamOutcome::Settled => "settled",
                    StreamOutcome::RateLimited(reason) if reason.is_account_local() => {
                        "account_local"
                    }
                    StreamOutcome::RateLimited(_) => "rate_limited",
                    StreamOutcome::Failed => "failed",
                };
                self.track_limit_step(&record.key.provider, old_limit, entry.limit, reason);
            }
            entry.in_flight = entry.in_flight.saturating_sub(1);
            report = Some((entry.limit, entry.in_flight));
        }
        let blocked = |state: &RegistryState| {
            matches!(
                super::breaker::breaker_snapshot(&state.breakers, &record.key.provider).state,
                BreakerState::Open | BreakerState::HalfOpen
            )
        };
        let was_blocked = blocked(&state);
        self.breaker_settle(&mut state, &record.key.provider, lease, outcome);
        // A CLOSED breaker releases the whole provider, not just the model whose
        // probe settled: the breaker is provider-wide (`state.breakers[provider]`)
        // while the queues are per `(provider, model)`, so a backlog parked on
        // another model of the same provider stayed stranded after recovery
        // (review #6). Closing is the only transition that unblocks them — open
        // and half-open refuse in `dispatch` — so the wake-up is gated on it.
        let blocked_now = blocked(&state);
        if was_blocked && !blocked_now {
            let keys: Vec<ProviderKey> = state
                .providers
                .keys()
                .filter(|key| key.provider == record.key.provider)
                .cloned()
                .collect();
            for key in keys {
                self.dispatch(&mut state, &key);
            }
        } else {
            self.dispatch(&mut state, &record.key);
        }
        if let Some((limit, in_flight)) = report {
            tracing::debug!(
                provider = %record.key.provider,
                model = %record.key.model,
                ?outcome,
                limit,
                in_flight,
                "provider admission: settle"
            );
        }
    }

    /// Fold one settle into the provider's retry budget and breaker
    /// (M4): every settle is a sample; a rate-limited OR failed settle is a
    /// failure. Past 10% of the window the breaker opens; the half-open probe's
    /// settle is the only verdict that transitions it.
    ///
    /// `Failed` counts as a failure because a 5xx storm is exactly the condition the
    /// breaker exists for. Counting only `RateLimited` meant a provider returning 500
    /// to everything never tripped it (review #10).
    fn breaker_settle(
        &self,
        state: &mut RegistryState,
        provider: &str,
        lease: u64,
        outcome: StreamOutcome,
    ) {
        let now = self.clock.now_ms();
        let failed = matches!(
            outcome,
            StreamOutcome::RateLimited(_) | StreamOutcome::Failed
        );
        let Some(breaker) = state.breakers.get_mut(provider) else {
            return;
        };
        breaker.window.push_back(failed);
        while breaker.window.len() > RETRY_BUDGET_WINDOW {
            breaker.window.pop_front();
        }
        let is_probe = breaker.probe_lease == Some(lease);
        match breaker.state {
            BreakerState::HalfOpen if is_probe => {
                breaker.probe_lease = None;
                if failed {
                    breaker.state = BreakerState::Open;
                    breaker.opened_at_ms = now;
                    let failures = breaker.failures();
                    tracing::info!(
                        provider,
                        failures,
                        samples = breaker.window.len(),
                        "provider admission: breaker re-opened, the probe was rate-limited"
                    );
                    self.track_breaker_transition(
                        provider,
                        BreakerState::HalfOpen,
                        BreakerState::Open,
                        failures,
                    );
                } else {
                    breaker.state = BreakerState::Closed;
                    // A fresh budget: the storm's evidence must not
                    // re-trip the breaker on the next sample.
                    let failures = breaker.failures();
                    breaker.window.clear();
                    tracing::info!(
                        provider,
                        "provider admission: breaker closed, the probe settled"
                    );
                    self.track_breaker_transition(
                        provider,
                        BreakerState::HalfOpen,
                        BreakerState::Closed,
                        failures,
                    );
                }
            }
            BreakerState::Closed => {
                if breaker.budget_exceeded() {
                    breaker.state = BreakerState::Open;
                    breaker.opened_at_ms = now;
                    let failures = breaker.failures();
                    tracing::info!(
                        provider,
                        failures,
                        samples = breaker.window.len(),
                        "provider admission: breaker opened, the retry budget is spent"
                    );
                    self.track_breaker_transition(
                        provider,
                        BreakerState::Closed,
                        BreakerState::Open,
                        failures,
                    );
                }
            }
            // Open with in-flight streams settling: their samples land in
            // the window; only the probe (or the cooldown) lifts the state.
            BreakerState::Open | BreakerState::HalfOpen => {}
        }
    }
}
