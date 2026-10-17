//! Cross-worker provider admission (ADR `ratelimit-admission`, M1+M2).
//!
//! The 429 incident: nine parallel subagents on one provider each burned
//! their own three-attempt retry ladder and died, because retry/park/
//! failover are per-session while the provider's concurrency ceiling is
//! shared. This module owns the missing piece: a per-`(provider, model)`
//! AIMD limit with one FIFO admission queue that every session worker
//! draws from.
//!
//! # Where the registry lives
//!
//! In the SUPERVISOR, not in a worker. One daemon hosts one process per
//! session, so a per-process semaphore bounds nothing (the ADR's
//! "глобальный семафор в pa-core per-session — бесполезен при N
//! процессах"). Workers hold [`AdmissionClient`] handles that reach the one
//! shared [`ProviderAdmission`] registry: over the supervisor link in
//! production ([`AdmissionClient::link`]), in-process for tests and
//! single-process hosts ([`AdmissionClient::local`]).
//!
//! # The narrow bracket (oh-my-pi #3749)
//!
//! A slot is held ONLY across one bounded unit of provider work: one LLM
//! stream (acquired before the provider call, released when that stream
//! settles, errors, aborts, or is dropped) or one spawn admission (held
//! from the spawn gate until the child session's create settles and its
//! kickoff prompt is delivered). Holding a slot for a worker's whole
//! lifetime deadlocks spawn trees — a parent whose slot never frees cannot
//! admit the grandchild it is waiting on. A spawn's reservation releases at
//! child admission rather than at the child's own stream settle because the
//! parent cannot observe a child process's stream; the child's streams are
//! throttled by the same pool once it runs.
//!
//! # AIMD
//!
//! Start limit [`START_LIMIT`] (clamped to the settings cap), floor
//! [`FLOOR_LIMIT`]: a settled stream adds one slot up to the cap, a 429
//! halves the limit down to the floor. Additive increase, multiplicative
//! decrease — the shape the provider's own 429s ask for. v1 has no
//! recovery window: the next settled stream is the recovery signal.
//!
//! # Fail-open
//!
//! Admission is a throttle, never a gate: an unreachable supervisor, an
//! expired wait budget, or a malformed answer admits the caller WITHOUT a
//! lease. A degraded admission plane must never become a hung turn.
//! The ONE exception is the M4 circuit breaker: a provider whose retry
//! budget tripped its breaker is REFUSED fail-fast (a stream on it would
//! just feed the storm), and the caller fails over instead.
//!
//! # M3-M5
//!
//! The settle signal carries a typed `RateLimitReason` (M3): an
//! account-local cause (a spent quota or spend cap) drops the AIMD limit
//! to the floor at once — the account cannot serve ANY concurrency until
//! its window resets — while a plain 429 halves it. The per-PROVIDER
//! retry budget (M4, the Google SRE bound) counts rate-limited settles
//! against a sliding window; past 10% the breaker opens and new acquires
//! fail fast to failover, lifting through a single half-open probe after
//! a cooldown. The registry snapshots it all (M5) for the daemon's state.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pa_agent::stream::{LlmContext, ModelStream, StreamFn, StreamRequestOptions};
use pa_agent::types::{AssistantMessage, Model as AgentModel};
use pa_core::session_engine::provider_retry::RateLimitReason;
use tokio::sync::oneshot;

use crate::supervisor_link::SupervisorLink;

/// The compiled AIMD start limit: the concurrency the semaphore begins at
/// before any provider signal, and the `[agents] maxConcurrentPerProvider`
/// default ceiling.
pub const START_LIMIT: usize = pa_core::settings::DEFAULT_PROVIDER_CONCURRENCY_LIMIT;

/// The AIMD floor: a 429 storm never closes a provider entirely (one slot
/// keeps the retry ladder — and the recovery signal — alive).
pub const FLOOR_LIMIT: usize = 1;

/// How long a caller waits for a slot before failing open: long enough to
/// outlast a full stream, short enough that a wedged admission plane never
/// hangs a turn forever.
pub const ACQUIRE_BUDGET: Duration = Duration::from_secs(600);

/// The spawn-dispatch stagger ceiling: a freed slot dispatches the next
/// queued spawn after a randomized `0..=`[`SPAWN_STAGGER_MAX_MS`] delay, so
/// the herd arrives spread instead of synchronized (the ADR's jitter
/// experiment: 78.3% vs 0% under replay).
pub const SPAWN_STAGGER_MAX_MS: u64 = 2_000;

/// The per-provider retry budget (M4, the Google SRE bound): rate-limited
/// settles beyond 10% of the sliding window trip the provider's breaker.
/// Integer form: `failures * 10 > samples`.
pub const RETRY_BUDGET_WINDOW: usize = 50;
pub const RETRY_BUDGET_MIN_SAMPLES: usize = 10;

/// How long an OPEN breaker stays fail-fast before one half-open probe
/// may try the provider again.
pub const BREAKER_COOLDOWN_MS: u64 = 30_000;

/// The wire command the admission plane rides (worker -> supervisor).
pub const ADMISSION_COMMAND: &str = "provider_admission";

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

/// What one settled stream told the AIMD controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The stream settled without a provider rate limit: +1 slot.
    Settled,
    /// The provider rate-limited the request (a 429, a `rate_limit`
    /// kind, or a billing cap): halve the limit — or drop it to the
    /// floor when the reason is account-local (M3).
    RateLimited(RateLimitReason),
}

impl StreamOutcome {
    /// The wire spelling (the `provider_admission` settle action).
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            StreamOutcome::Settled => "settled",
            StreamOutcome::RateLimited(_) => "rate_limited",
        }
    }

    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "settled" => Some(StreamOutcome::Settled),
            "rate_limited" => Some(StreamOutcome::RateLimited(RateLimitReason::RateLimit)),
            _ => None,
        }
    }
}

/// The AIMD signal one settled assistant message carries: a provider
/// rate limit (the 429 family, a `rate_limit` kind, or a billing cap)
/// is the decrease trigger with its typed reason (M3), everything else
/// is a plain settle.
#[must_use]
pub fn outcome_for_message(message: &AssistantMessage) -> StreamOutcome {
    match pa_core::session_engine::provider_retry::rate_limit_reason_for_message(message) {
        Some(reason) => StreamOutcome::RateLimited(reason),
        None => StreamOutcome::Settled,
    }
}

/// One provider's live counters (the M5 observability surface: a snapshot
/// never blocks the admission path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSnapshot {
    pub limit: usize,
    pub in_flight: usize,
    pub queued: usize,
    pub cap: usize,
    pub consecutive_429: u32,
    /// The provider's breaker state and retry-budget standing (M4).
    pub breaker: BreakerSnapshot,
}

/// The breaker half of one provider's snapshot: the state plus the
/// retry budget the state was decided on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BreakerSnapshot {
    pub state: BreakerState,
    /// Rate-limited settles inside the sliding window.
    pub budget_failures: usize,
    /// Settles inside the sliding window.
    pub budget_samples: usize,
}

/// The breaker half of a snapshot row: the provider's standing, or the
/// closed default for a provider the breaker has never seen.
fn breaker_snapshot(
    breakers: &HashMap<String, ProviderBreaker>,
    provider: &str,
) -> BreakerSnapshot {
    breakers.get(provider).map_or_else(
        || BreakerSnapshot {
            state: BreakerState::Closed,
            budget_failures: 0,
            budget_samples: 0,
        },
        |breaker| BreakerSnapshot {
            state: breaker.state,
            budget_failures: breaker.failures(),
            budget_samples: breaker.window.len(),
        },
    )
}

/// One per-provider circuit-breaker state (M4): `Closed` is normal
/// traffic, `Open` fail-fasts new stream acquires until the cooldown
/// passes, `HalfOpen` holds exactly one probe whose settle decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

impl BreakerState {
    /// The snapshot wire spelling.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// The breaker's verdict for one stream acquire (internal to the gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BreakerGate {
    /// Normal traffic.
    Pass,
    /// This acquirer is the single half-open probe.
    Probe,
    /// Fail fast: the breaker is open (or one probe is already in flight).
    Refuse,
}

/// One per-provider breaker's live state (supervisor-side).
#[derive(Debug)]
struct ProviderBreaker {
    state: BreakerState,
    /// When the OPEN cooldown started (epoch ms).
    opened_at_ms: u64,
    /// The half-open probe's lease, when one is in flight.
    probe_lease: Option<u64>,
    /// The sliding window of recent settle samples (`true` = rate-limited).
    window: VecDeque<bool>,
}

impl Default for ProviderBreaker {
    fn default() -> Self {
        Self {
            state: BreakerState::Closed,
            opened_at_ms: 0,
            probe_lease: None,
            window: VecDeque::new(),
        }
    }
}

impl ProviderBreaker {
    fn failures(&self) -> usize {
        self.window.iter().filter(|limited| **limited).count()
    }

    /// Whether the retry budget is spent past the SRE bound (10% of the
    /// window, once enough samples accumulated).
    fn budget_exceeded(&self) -> bool {
        self.window.len() >= RETRY_BUDGET_MIN_SAMPLES && self.failures() * 10 > self.window.len()
    }
}

/// The breaker cooldown's time source. Injectable so breaker
/// transitions test without sleeping (the ADR's injectable-time rule).
pub trait ClockSource: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The production clock: wall-clock epoch milliseconds.
#[derive(Debug, Default)]
pub struct SystemClock;

impl ClockSource for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }
}

/// A scripted clock (tests): `advance` moves time without sleeping.
#[derive(Debug)]
pub struct ScriptedClock {
    now_ms: AtomicU64,
}

impl ScriptedClock {
    #[must_use]
    pub fn new(start_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            now_ms: AtomicU64::new(start_ms),
        })
    }

    pub fn advance(&self, by_ms: u64) {
        self.now_ms.fetch_add(by_ms, Ordering::Relaxed);
    }
}

impl ClockSource for ScriptedClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::Relaxed)
    }
}

/// The answer to one stream admission ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamAdmission {
    /// A slot was taken; `u64` is the lease id.
    Admitted(u64),
    /// The provider's breaker is OPEN (M4): fail fast — do not wait,
    /// do not proceed unadmitted, fail over instead.
    Refused,
    /// Fail-open (no plane, degraded link, or expired wait budget):
    /// proceed without a lease.
    Unadmitted,
}

/// The randomized spawn-dispatch delay source. Injectable so the stagger is
/// testable without sleeping (the ADR's "мок-время или injectable rand").
pub trait StaggerSource: Send + Sync {
    /// The delay to wait before dispatching the next queued spawn.
    fn next_delay_ms(&self) -> u64;
}

/// The production stagger: a splitmix walk over a clock seed plus an atomic
/// counter, bounded by [`SPAWN_STAGGER_MAX_MS`].
#[derive(Debug, Default)]
pub struct JitteredStagger {
    counter: AtomicU64,
}

impl JitteredStagger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl StaggerSource for JitteredStagger {
    fn next_delay_ms(&self) -> u64 {
        let tick = self.counter.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| u64::from(elapsed.subsec_nanos()));
        let mut seed = nanos
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(tick.wrapping_mul(0xBF58_476D_1CE4_E5B9))
            .wrapping_add(0x94D0_49BB_1331_11EB);
        seed ^= seed >> 30;
        seed = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        seed ^= seed >> 27;
        seed = seed.wrapping_mul(0x94D0_49BB_1331_11EB);
        seed ^= seed >> 31;
        seed % (SPAWN_STAGGER_MAX_MS + 1)
    }
}

/// A stagger source that hands back a fixed script (tests): the last value
/// repeats once the script runs out.
#[derive(Debug)]
pub struct ScriptedStagger {
    delays: Mutex<VecDeque<u64>>,
    fallback: u64,
}

impl ScriptedStagger {
    #[must_use]
    pub fn new(delays: impl IntoIterator<Item = u64>) -> Arc<Self> {
        let delays: VecDeque<u64> = delays.into_iter().collect();
        let fallback = delays.back().copied().unwrap_or(0);
        Arc::new(Self {
            delays: Mutex::new(delays),
            fallback,
        })
    }
}

impl StaggerSource for ScriptedStagger {
    fn next_delay_ms(&self) -> u64 {
        self.delays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or(self.fallback)
    }
}

/// A grant handed to a queued acquirer.
#[derive(Clone, Copy, Debug)]
struct Grant {
    lease: u64,
    stagger_ms: u64,
}

/// One queued acquirer.
struct Waiter {
    holder: String,
    grant: Mutex<Option<oneshot::Sender<Grant>>>,
}

/// One provider's live admission state.
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
    /// The per-PROVIDER breakers (M4): keyed by provider alone, so every
    /// model on one provider shares one retry budget.
    breakers: HashMap<String, ProviderBreaker>,
    leases: HashMap<u64, LeaseRecord>,
    next_lease: u64,
}

/// The shared AIMD admission registry: one per supervisor process, keyed by
/// resolved `(provider, model)`, with a FIFO queue per key.
///
/// Streams and spawn admissions draw from the SAME pool (one slot is one
/// bounded unit of provider work), so a spawn storm cannot outrun the
/// provider and a child's first stream cannot be starved by the spawn that
/// created it: the spawn's reservation is released at child admission,
/// before the child streams.
pub struct ProviderAdmission {
    cap: usize,
    start: usize,
    floor: usize,
    stagger: Arc<dyn StaggerSource>,
    clock: Arc<dyn ClockSource>,
    state: Mutex<RegistryState>,
}

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
            state: Mutex::new(RegistryState {
                providers: HashMap::new(),
                breakers: HashMap::new(),
                leases: HashMap::new(),
                next_lease: 1,
            }),
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

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take a slot for `key` right now if the limit allows it; `None` means
    /// the caller must queue.
    fn admit_now(&self, state: &mut RegistryState, key: &ProviderKey, holder: &str) -> Option<u64> {
        let start = self.start;
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
    fn mint(state: &mut RegistryState, key: &ProviderKey, holder: &str) -> u64 {
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
    ) -> (oneshot::Receiver<Grant>, Arc<Waiter>) {
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
        (rx, waiter)
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
        let (pending, is_probe) = {
            let mut state = self.lock();
            let gate = self.breaker_gate(&mut state, key);
            if matches!(gate, BreakerGate::Refuse) {
                return StreamAdmission::Refused;
            }
            // This acquirer is the half-open probe when the gate said so:
            // if it never reaches a lease, the probe must un-wedge (below).
            let is_probe = matches!(gate, BreakerGate::Probe);
            match self.admit_now(&mut state, key, holder) {
                Some(lease) => {
                    if is_probe {
                        Self::note_probe(&mut state, &key.provider, lease);
                    }
                    return StreamAdmission::Admitted(lease);
                }
                None => (self.enqueue(&mut state, key, holder), is_probe),
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

    /// The breaker's verdict for one stream acquire. `Probe` arms this
    /// acquirer as the single half-open probe (the state is already
    /// `HalfOpen` when it returns). Spawns never gate: a spawn
    /// reservation is the create round trip, not provider traffic — the
    /// child's own first stream gates itself.
    fn breaker_gate(&self, state: &mut RegistryState, key: &ProviderKey) -> BreakerGate {
        let now = self.clock.now_ms();
        let breaker = state.breakers.entry(key.provider.clone()).or_default();
        match breaker.state {
            BreakerState::Closed => BreakerGate::Pass,
            BreakerState::Open => {
                if now.saturating_sub(breaker.opened_at_ms) >= BREAKER_COOLDOWN_MS {
                    breaker.state = BreakerState::HalfOpen;
                    tracing::info!(
                        provider = %key.provider,
                        "provider admission: breaker half-open, dispatching one probe"
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
        let pending = {
            let mut state = self.lock();
            match self.admit_now(&mut state, key, spawn_id) {
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
        let (rx, waiter) = pending;
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
        let mut state = self.lock();
        let Some(record) = state.leases.remove(&lease) else {
            return;
        };
        let cap = self.cap;
        let floor = self.floor;
        let mut report = None;
        if let Some(entry) = state.providers.get_mut(&record.key) {
            match outcome {
                StreamOutcome::Settled => {
                    entry.consecutive_429 = 0;
                    if entry.limit < cap {
                        entry.limit += 1;
                    }
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
            entry.in_flight = entry.in_flight.saturating_sub(1);
            report = Some((entry.limit, entry.in_flight));
        }
        self.breaker_settle(&mut state, &record.key.provider, lease, outcome);
        self.dispatch(&mut state, &record.key);
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
    /// (M4): every settle is a sample, a rate-limited settle is a
    /// failure. Past 10% of the window the breaker opens; the half-open
    /// probe's settle is the only verdict that transitions it.
    fn breaker_settle(
        &self,
        state: &mut RegistryState,
        provider: &str,
        lease: u64,
        outcome: StreamOutcome,
    ) {
        let now = self.clock.now_ms();
        let limited = matches!(outcome, StreamOutcome::RateLimited(_));
        let Some(breaker) = state.breakers.get_mut(provider) else {
            return;
        };
        breaker.window.push_back(limited);
        while breaker.window.len() > RETRY_BUDGET_WINDOW {
            breaker.window.pop_front();
        }
        let is_probe = breaker.probe_lease == Some(lease);
        match breaker.state {
            BreakerState::HalfOpen if is_probe => {
                breaker.probe_lease = None;
                if limited {
                    breaker.state = BreakerState::Open;
                    breaker.opened_at_ms = now;
                    tracing::info!(
                        provider,
                        failures = breaker.failures(),
                        samples = breaker.window.len(),
                        "provider admission: breaker re-opened, the probe was rate-limited"
                    );
                } else {
                    breaker.state = BreakerState::Closed;
                    // A fresh budget: the storm's evidence must not
                    // re-trip the breaker on the next sample.
                    breaker.window.clear();
                    tracing::info!(
                        provider,
                        "provider admission: breaker closed, the probe settled"
                    );
                }
            }
            BreakerState::Closed => {
                if breaker.budget_exceeded() {
                    breaker.state = BreakerState::Open;
                    breaker.opened_at_ms = now;
                    tracing::info!(
                        provider,
                        failures = breaker.failures(),
                        samples = breaker.window.len(),
                        "provider admission: breaker opened, the retry budget is spent"
                    );
                }
            }
            // Open with in-flight streams settling: their samples land in
            // the window; only the probe (or the cooldown) lifts the state.
            BreakerState::Open | BreakerState::HalfOpen => {}
        }
    }

    /// Give a lease back without an AIMD signal (an aborted turn, a
    /// dispatch that never reached the provider, a dead worker).
    pub fn release(&self, lease: u64) {
        let mut state = self.lock();
        let Some(record) = state.leases.remove(&lease) else {
            return;
        };
        if let Some(entry) = state.providers.get_mut(&record.key) {
            entry.in_flight = entry.in_flight.saturating_sub(1);
        }
        // A dropped probe re-arms the cooldown (M4): the breaker must
        // never wedge in half-open on a probe that never answered.
        let now = self.clock.now_ms();
        if let Some(breaker) = state.breakers.get_mut(&record.key.provider) {
            if breaker.probe_lease == Some(lease) {
                breaker.probe_lease = None;
                breaker.state = BreakerState::Open;
                breaker.opened_at_ms = now;
                tracing::info!(
                    provider = %record.key.provider,
                    "provider admission: breaker re-opened, the probe dropped without a verdict"
                );
            }
        }
        self.dispatch(&mut state, &record.key);
    }

    /// Drop every lease a holder still owns: a worker that died mid-stream
    /// must not shrink the pool forever.
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
        if !leases.is_empty() {
            tracing::info!(
                holder,
                leases = leases.len(),
                "provider admission: released a dead holder's leases"
            );
        }
        leases.len()
    }

    /// Fill the freed capacity from the key's FIFO queue, one waiter per
    /// freed slot (a waiter's grant takes exactly one slot, so the loop
    /// re-checks the limit each pass).
    fn dispatch(&self, state: &mut RegistryState, key: &ProviderKey) {
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
            entry.in_flight += 1;
            let mut slot = waiter.grant.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(tx) = slot.take() else {
                // The receiver dropped before the pop: return the slot and
                // try the next waiter.
                if let Some(entry) = state.providers.get_mut(key) {
                    entry.in_flight = entry.in_flight.saturating_sub(1);
                }
                continue;
            };
            drop(slot);
            let stagger_ms = self.stagger.next_delay_ms();
            let grant = Grant {
                lease: Self::mint(state, key, &waiter.holder),
                stagger_ms,
            };
            let queued = state
                .providers
                .get(key)
                .map_or(0, |entry| entry.waiters.len());
            tracing::info!(
                provider = %key.provider,
                model = %key.model,
                holder = %waiter.holder,
                queued,
                stagger_ms,
                lease = grant.lease,
                "provider admission: dequeue"
            );
            if tx.send(grant).is_err() {
                // The acquirer gave up between the pop and the send: the
                // slot returns to the pool and the next waiter takes it.
                if let Some(record) = state.leases.remove(&grant.lease) {
                    if let Some(entry) = state.providers.get_mut(&record.key) {
                        entry.in_flight = entry.in_flight.saturating_sub(1);
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self, key: &ProviderKey) -> ProviderSnapshot {
        let state = self.lock();
        let breaker = breaker_snapshot(&state.breakers, &key.provider);
        match state.providers.get(key) {
            Some(entry) => ProviderSnapshot {
                limit: entry.limit,
                in_flight: entry.in_flight,
                queued: entry.waiters.len(),
                cap: self.cap,
                consecutive_429: entry.consecutive_429,
                breaker,
            },
            None => ProviderSnapshot {
                limit: self.start,
                in_flight: 0,
                queued: 0,
                cap: self.cap,
                consecutive_429: 0,
                breaker,
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

    /// Every provider's observability rows (M5): one per `(provider,
    /// model)` pair the registry has seen, sorted, each carrying its
    /// provider's breaker standing. The daemon's state snapshot wire.
    #[must_use]
    pub fn provider_rows(&self) -> Vec<ProviderRow> {
        let state = self.lock();
        let mut rows: Vec<ProviderRow> = state
            .providers
            .iter()
            .map(|(key, entry)| ProviderRow {
                provider: key.provider.clone(),
                model: key.model.clone(),
                limit: entry.limit,
                in_flight: entry.in_flight,
                queued: entry.waiters.len(),
                cap: self.cap,
                consecutive_429: entry.consecutive_429,
                breaker: breaker_snapshot(&state.breakers, &key.provider),
            })
            .collect();
        rows.sort_by(|left, right| {
            (&left.provider, &left.model).cmp(&(&right.provider, &right.model))
        });
        rows
    }
}

/// One per-`(provider, model)` observability row (the M5 snapshot wire):
/// the AIMD standing plus the provider's breaker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRow {
    pub provider: String,
    pub model: String,
    pub limit: usize,
    pub in_flight: usize,
    pub queued: usize,
    pub cap: usize,
    pub consecutive_429: u32,
    pub breaker: BreakerSnapshot,
}

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
        AdmissionClient::Link { link, worker_token }
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
        match self {
            AdmissionClient::Disabled => StreamAdmission::Unadmitted,
            AdmissionClient::Local(registry) => registry.acquire_stream(key, LOCAL_HOLDER).await,
            AdmissionClient::Link { link, worker_token } => {
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
    pub async fn spawn_gate(
        &self,
        key: &ProviderKey,
        spawn_id: &str,
    ) -> Option<(StreamLease, Duration)> {
        match self {
            AdmissionClient::Disabled => None,
            AdmissionClient::Local(registry) => {
                registry
                    .spawn_gate(key, spawn_id)
                    .await
                    .map(|(id, stagger)| {
                        (
                            StreamLease {
                                id,
                                client: self.clone(),
                            },
                            stagger,
                        )
                    })
            }
            AdmissionClient::Link { link, worker_token } => {
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
                    },
                    Duration::from_millis(stagger_ms),
                ))
            }
        }
    }

    /// Settle a lease with its AIMD signal (the typed reason rides the
    /// wire when the outcome is rate-limited).
    pub async fn settle(&self, lease: u64, outcome: StreamOutcome) {
        match self {
            AdmissionClient::Disabled => {}
            AdmissionClient::Local(registry) => registry.settle(lease, outcome),
            AdmissionClient::Link { link, worker_token } => {
                let mut command = lease_command(worker_token, "settle_stream", lease);
                command["outcome"] = serde_json::json!(outcome.as_wire());
                if let StreamOutcome::RateLimited(reason) = outcome {
                    command["reason"] = serde_json::json!(reason.as_wire());
                }
                let _ = self.call(link, worker_token, command).await;
            }
        }
    }

    /// Give a lease back without an AIMD signal.
    pub async fn release(&self, lease: u64) {
        match self {
            AdmissionClient::Disabled => {}
            AdmissionClient::Local(registry) => registry.release(lease),
            AdmissionClient::Link { link, worker_token } => {
                let _ = self
                    .call(
                        link,
                        worker_token,
                        lease_command(worker_token, "release_stream", lease),
                    )
                    .await;
            }
        }
    }

    /// The fire-and-forget release a `Drop` can afford (the abort path and
    /// a dropped stream): never blocks the caller.
    pub fn release_detached(&self, lease: u64) {
        if !self.is_enabled() {
            return;
        }
        let client = self.clone();
        // A drop can land outside a runtime (a teardown); there the
        // supervisor's own holder reaping covers the slot.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { client.release(lease).await });
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
        let response = link.request(command, ADMISSION_REQUEST_BUDGET).await.ok()?;
        if !response.success {
            return None;
        }
        Some(response.data.unwrap_or(serde_json::Value::Null))
    }
}

/// The holder name a local (in-process) client reports: one process, one
/// pool, so the reaper never needs to tell holders apart.
const LOCAL_HOLDER: &str = "local";

/// A settle/release round trip is short: the supervisor only mutates
/// counters. A queued acquire rides [`ACQUIRE_BUDGET`] instead.
const ADMISSION_REQUEST_BUDGET: Duration = Duration::from_secs(30);

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
    id: u64,
    client: AdmissionClient,
}

impl StreamLease {
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Settle the slot with its AIMD signal.
    pub async fn settle(mut self, outcome: StreamOutcome) {
        let id = self.id;
        self.id = 0;
        self.client.settle(id, outcome).await;
    }

    /// Give the slot back without a signal.
    pub async fn release(mut self) {
        let id = self.id;
        self.id = 0;
        self.client.release(id).await;
    }
}

impl Drop for StreamLease {
    fn drop(&mut self) {
        if self.id != 0 {
            self.client.release_detached(self.id);
        }
    }
}

/// The provider key a stream is charged to: the live provider target's
/// resolved pair (a `set_model`/failover swap must be charged to the
/// provider that actually serves the request).
pub type KeyResolver = Arc<dyn Fn() -> ProviderKey + Send + Sync>;

/// Wrap a stream function so every LLM stream runs inside an admission
/// slot: acquired before the provider call, released when the stream
/// settles, errors, aborts, or is dropped.
#[must_use]
pub fn admit_stream_fn(inner: StreamFn, key_of: KeyResolver, client: AdmissionClient) -> StreamFn {
    if !client.is_enabled() {
        return inner;
    }
    Arc::new(
        move |requested: AgentModel, context: LlmContext, options: StreamRequestOptions| {
            let inner = Arc::clone(&inner);
            let client = client.clone();
            let key_of = Arc::clone(&key_of);
            Box::pin(async move {
                // The key resolves at call time, from the same slot the
                // inner stream reads its target from.
                let key = key_of();
                let lease = match client.acquire_stream(&key).await {
                    StreamAdmission::Admitted(id) => Some(StreamLease {
                        id,
                        client: client.clone(),
                    }),
                    StreamAdmission::Unadmitted => None,
                    // The provider's breaker is open (M4): fail fast with
                    // a rate-limit-shaped refusal the ladder's quota lane
                    // settles at once, so the failover chain re-routes
                    // instead of hammering the provider.
                    StreamAdmission::Refused => {
                        let (handle, consumer) = pa_agent::stream::event_stream();
                        handle.end(Some(breaker_refusal_message(&key)));
                        return Ok(Box::new(consumer) as Box<dyn ModelStream>);
                    }
                };
                let stream = inner(requested, context, options).await?;
                Ok(Box::new(AdmittedStream {
                    inner: stream,
                    lease,
                    client,
                }) as Box<dyn ModelStream>)
            })
        },
    )
}

/// The open breaker's fail-fast message: a rate-limit-shaped refusal
/// carrying the marker the M3 classifier reads as account-local
/// rotation (the admission plane's own wording, matched by
/// [`pa_core::session_engine::provider_retry::classify_rate_limit_reason`]).
fn breaker_refusal_message(key: &ProviderKey) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: String::new(),
        provider: key.provider.clone(),
        model: key.model.clone(),
        response_model: None,
        response_id: None,
        diagnostics: Some(vec![pa_agent::types::AssistantMessageDiagnostic {
            kind: "provider_stream_failure".to_string(),
            timestamp: 0,
            error: None,
            details: Some(serde_json::json!({
                "kind": "rate_limit",
                "status": 429,
            })),
        }]),
        usage: pa_agent::types::Usage::zero(),
        stop_reason: pa_agent::types::StopReason::Error,
        stop_reason_raw: None,
        error_message: Some(format!(
            "Provider admission circuit open for {} (retry budget exceeded); failing over",
            key.selector()
        )),
        timestamp: 0,
    }
}

/// A [`ModelStream`] that owns one admission slot for exactly its own
/// lifetime (the narrow bracket: never the worker's).
struct AdmittedStream {
    inner: Box<dyn ModelStream>,
    lease: Option<StreamLease>,
    client: AdmissionClient,
}

impl ModelStream for AdmittedStream {
    fn next_event(
        &mut self,
    ) -> pa_agent::BoxFut<'_, Option<pa_agent::stream::AssistantMessageEvent>> {
        self.inner.next_event()
    }

    fn result(&mut self) -> pa_agent::BoxFut<'_, anyhow::Result<AssistantMessage>> {
        Box::pin(async move {
            let outcome = self.inner.result().await;
            match &outcome {
                // A provider 429 halves the limit; any other settle adds a
                // slot.
                Ok(message) => {
                    if let Some(lease) = self.lease.take() {
                        lease.settle(outcome_for_message(message)).await;
                    }
                }
                // A stream error is not a rate-limit signal: the slot
                // returns without moving the limit.
                Err(_) => {
                    if let Some(lease) = self.lease.take() {
                        lease.release().await;
                    }
                }
            }
            outcome
        })
    }

    fn close(&mut self) {
        self.inner.close();
        if let Some(lease) = self.lease.take() {
            self.client.release_detached(lease.id());
        }
    }
}

#[cfg(test)]
mod tests;
