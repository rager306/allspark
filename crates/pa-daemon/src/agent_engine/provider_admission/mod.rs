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
//!
//! Every transition also rides the adoption-telemetry seam
//! ([`ProviderAdmission::attach_telemetry`]): breaker flips, AIMD limit
//! steps, and served snapshot rollups — fire-and-forget counters only.
//!
//! # Layout
//!
//! This module was one 1 959-line file. It is now this facade plus one file per
//! responsibility:
//!
//! | module | holds |
//! |---|---|
//! | [`breaker`] | the per-provider breaker: state, retry budget, half-open probe, the fail-fast refusal |
//! | [`clock`] | the injectable time sources behind the cooldown and the spawn stagger |
//! | [`admit`] | taking a slot: the breaker gate, AIMD admit-or-queue, the stream and spawn waits |
//! | [`settle`] | giving a slot back: settle/release, queue dispatch, the snapshot rows |
//! | [`client`] | the worker-side handle: local and supervisor-link round trips, the wire bodies |
//! | [`stream`] | the `StreamFn` wrapper that owns one slot for exactly one stream's lifetime |
//! | [`tests`] | the red-first admission battery |
//!
//! The facade keeps the TYPE definitions and the submodules keep the BEHAVIOUR.
//! That cut is load-bearing, not cosmetic: `impl ProviderAdmission` was one
//! 942-line block that had to become two (`admit` through `settle`, `settle` from
//! `release`), and both halves read the registry's record fields directly. A
//! private item is visible to the module that defines it and every descendant,
//! so a record declared HERE costs no visibility widening at all — whereas
//! declaring it inside `admit` would put `settle` across a sibling boundary and
//! need `pub(super)` on each field of all five records.
//!
//! The public surface is re-exported below, so every path that existed before
//! the split keeps resolving: `provider_admission::AdmissionClient`,
//! `provider_admission::ProviderKey`, `provider_admission::ADMISSION_COMMAND`
//! and the rest.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pa_agent::types::AssistantMessage;
use pa_core::session_engine::provider_retry::RateLimitReason;
use pa_telemetry::TelemetryClient;
use tokio::sync::oneshot;

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
/// Most waiters one `(provider, model)` queue may hold.
///
/// Unbounded, a provider outage turned every arriving request into a queue entry that
/// lived for the full `ACQUIRE_BUDGET` (600 s): memory grew with the outage's length,
/// and when the breaker re-opened the whole queue was dispatched at once as a herd
/// (review #39).
///
/// 256 is deliberately generous — the measured normal case is single digits (nine
/// parallel subagents against a limit of six leaves three queued) — so it never binds
/// a healthy burst. Past it a waiter is REFUSED rather than queued: the caller fails
/// over immediately, which is the same answer the wait budget would have given six
/// hundred seconds later, minus the memory and the herd.
pub const MAX_QUEUED_WAITERS: usize = 256;

pub const RETRY_BUDGET_WINDOW: usize = 50;
pub const RETRY_BUDGET_MIN_SAMPLES: usize = 10;

/// How long an OPEN breaker stays fail-fast before one half-open probe
/// may try the provider again.
pub const BREAKER_COOLDOWN_MS: u64 = 30_000;

/// The wire command the admission plane rides (worker -> supervisor).
pub const ADMISSION_COMMAND: &str = "provider_admission";

/// Who asks for a spawn slot, and which spawn it is for (review #10).
///
/// The two are NOT interchangeable. `holder` is the identity that may later settle or release
/// the minted lease — a worker token on the link path, the in-process holder name otherwise —
/// while `spawn_id` only labels the reservation in the log. The spawn path used to pass the
/// spawn id AS the holder, which no worker token could ever match: the supervisor refuses a
/// release from a token that does not own the lease, so every spawn lease was unreleasable and
/// each spawn leaked a slot for the life of the worker. The same mismatch made a live worker's
/// reservation look DEAD to the reaper, which could then hand its slot to another acquirer.
#[derive(Clone, Copy, Debug)]
pub struct SpawnReservation<'a> {
    pub holder: &'a str,
    pub spawn_id: &'a str,
}

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
    /// The request failed for a reason that is NOT a rate limit — a 5xx, a
    /// timeout, a transport error.
    ///
    /// Review #10: these used to settle as [`Self::Settled`], which is `+1` to the
    /// limit and a clean sample for the breaker. A 5xx storm therefore INFLATED the
    /// concurrency limit against a provider that was failing every request, while
    /// diluting the breaker's failure ratio at exactly the moment shedding was
    /// needed. A failure is not a success, and it is not a rate limit either: it
    /// releases the slot and counts as a breaker failure, but sends no AIMD signal,
    /// because the correct concurrency for a broken provider is unknown — the
    /// breaker's job is to stop traffic, not to guess a number.
    Failed,
}

impl StreamOutcome {
    /// The wire spelling (the `provider_admission` settle action).
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            StreamOutcome::Settled => "settled",
            StreamOutcome::RateLimited(_) => "rate_limited",
            StreamOutcome::Failed => "failed",
        }
    }

    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "settled" => Some(StreamOutcome::Settled),
            "rate_limited" => Some(StreamOutcome::RateLimited(RateLimitReason::RateLimit)),
            // Without this arm `as_wire()` WRITES "failed" and `from_wire()` reads it back as
            // `None`, which the supervisor turns into `Settled` — `+1` to the concurrency
            // limit for a failed request. The local classifier was fixed and the wire path
            // still downgraded it, so a 5xx storm kept inflating the limit in production
            // (review #10; the write/read asymmetry is what hid it).
            "failed" => Some(StreamOutcome::Failed),
            _ => None,
        }
    }
}

/// The AIMD signal one settled assistant message carries.
///
/// Three outcomes, not two (review #10). A provider rate limit (the 429 family, a
/// `rate_limit` kind, a billing cap) is the decrease trigger with its typed reason (M3); a
/// FAILURE that is not a rate limit — a 5xx, a timeout, a transport error — is
/// [`StreamOutcome::Failed`]; only a message with no provider failure at all is a plain
/// settle.
///
/// The second arm used to be lumped into `Settled`, which is `+1` to the concurrency limit
/// and a clean breaker sample. A provider returning 500 to every request therefore RAISED
/// the limit against itself while diluting the breaker's failure ratio.
#[must_use]
pub fn outcome_for_message(message: &AssistantMessage) -> StreamOutcome {
    use pa_core::session_engine::provider_retry::{
        provider_stream_failure_kind, provider_stream_failure_status, rate_limit_reason_for_message,
    };
    if let Some(reason) = rate_limit_reason_for_message(message) {
        return StreamOutcome::RateLimited(reason);
    }
    // "A provider failure that is not a rate limit." The disclosure gate is the same one the
    // 402 diagnosis uses: lifecycle and abort conversions stay silent, so they are not
    // failures here either.
    let status = provider_stream_failure_status(message);
    let kind = provider_stream_failure_kind(message);
    let failure_shaped = status.is_some_and(|status| status >= 500)
        || kind.as_deref().is_some_and(|kind| {
            matches!(
                kind,
                "server_error" | "timeout" | "transport" | "connection" | "overloaded"
            )
        })
        || message.error_message.is_some();
    if failure_shaped {
        return StreamOutcome::Failed;
    }
    StreamOutcome::Settled
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

// The registry's own record types live here rather than in a submodule: `admit`
// and `settle` both read their fields directly, and a private item is visible to
// the module that defines it and every descendant, so keeping them in the facade
// costs no visibility widening.

/// A grant handed to a queued acquirer.
#[derive(Clone, Copy, Debug)]
struct Grant {
    lease: u64,
    stagger_ms: u64,
}

/// What one queued acquirer is (review #8).
///
/// The provider's circuit breaker allows exactly one acquirer to run while it is HALF-OPEN:
/// the probe. That probe may arrive at a full pool, in which case it PARKS in the FIFO like
/// any other acquirer — so the queue has to be able to tell it apart from the backlog, which
/// must stay parked until the probe's verdict closes the breaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaiterRole {
    /// Normal traffic: grantable only while the breaker is closed.
    Ordinary,
    /// The single half-open probe: grantable while the breaker is half-open.
    Probe,
}

impl WaiterRole {
    /// Whether a waiter of this role may be granted while the provider's breaker stands in
    /// `breaker` (review #8).
    ///
    /// A CLOSED breaker grants the whole queue in FIFO order, including a probe that queued
    /// while half-open and outlived the verdict. A HALF-OPEN one grants ONLY the probe, at
    /// whatever depth it sits: the backlog queued before the trip must not drain beside it.
    /// An OPEN one grants nobody.
    fn admitted_by(self, breaker: BreakerState) -> bool {
        match breaker {
            BreakerState::Closed => true,
            BreakerState::HalfOpen => matches!(self, Self::Probe),
            BreakerState::Open => false,
        }
    }
}

/// One queued acquirer.
struct Waiter {
    holder: String,
    role: WaiterRole,
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

/// One per-provider breaker's live state (supervisor-side). Declared here with
/// the other registry records: `RegistryState` owns it, and both impl halves
/// read its fields, so a private record in the facade keeps those reads legal
/// without widening anything.
#[derive(Debug)]
struct ProviderBreaker {
    state: BreakerState,
    /// When the OPEN cooldown started (epoch ms).
    opened_at_ms: u64,
    /// The half-open probe's lease, when one is in flight.
    probe_lease: Option<u64>,
    /// The sliding window of recent settle samples (`true` = a failure: rate-limited or
    /// otherwise failed).
    window: VecDeque<bool>,
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
///
/// Answers "is this lease holder still alive", asynchronously.
///
/// A named type because the registry stores one behind a mutex and the inline
/// `Arc<dyn Fn(..) -> JoinHandle<_>>` spelling is unreadable at a call site.
pub type HolderLiveness = Arc<dyn Fn(String) -> tokio::task::JoinHandle<bool> + Send + Sync>;

pub struct ProviderAdmission {
    cap: usize,
    start: usize,
    floor: usize,
    stagger: Arc<dyn StaggerSource>,
    clock: Arc<dyn ClockSource>,
    /// Whether a lease's holder is still alive, when the owner can answer.
    ///
    /// A slot is normally freed by its own `Drop` (`release_detached`), but a drop
    /// that lands outside a tokio runtime cannot spawn the release, and a worker
    /// killed mid-stream never drops at all. The old recovery was the worker-exit
    /// reap — correct, and useless while the worker is alive holding a slot nobody
    /// is using: the next acquirer then waits the full `ACQUIRE_BUDGET` (600 s) for a
    /// slot that will never come back.
    ///
    /// With this predicate the decision is made on EVIDENCE at admission time
    /// instead of on a timer: when no slot is free, leases of dead holders are
    /// reclaimed first. `None` keeps the old behaviour for owners that cannot answer
    /// (tests, the daemonless path).
    liveness: Mutex<Option<HolderLiveness>>,
    state: Mutex<RegistryState>,
    /// The adoption-telemetry seam (None until the owner attaches the
    /// daemon's client): counters-only events, fire-and-forget.
    telemetry: Mutex<Option<TelemetryClient>>,
}

// ---- the behaviour, one file per responsibility ---------------------------

mod admit;
mod breaker;
mod client;
mod clock;
mod settle;
mod stream;

#[cfg(test)]
mod tests;

// ---- re-exports: the surface the split must not have moved ----------------
//
// `ProviderSnapshot`, `BreakerSnapshot`, `StreamAdmission` and the nine consts
// above are declared in THIS file. Everything below moved out and is named here
// again under the path it had before the split.
pub use self::breaker::BreakerState;
pub use self::client::{AdmissionClient, KeyResolver, StreamLease};
pub use self::clock::{
    ClockSource, JitteredStagger, ScriptedClock, ScriptedStagger, StaggerSource, SystemClock,
};
pub use self::settle::ProviderRow;
pub use self::stream::admit_stream_fn;

// ---- the names only the battery reaches -----------------------------------
//
// The battery opens with `use super::*` and drives these four helpers, plus the
// `PoisonError` and stream types it used to get from the flat module's own
// imports. All of them are internal here — a library build references none — so
// the bindings exist only in a test build. That keeps the battery's header
// byte-identical to what it was before the split.
#[cfg(test)]
use pa_agent::stream::{LlmContext, ModelStream, StreamFn};
#[cfg(test)]
use std::sync::PoisonError;

#[cfg(test)]
use self::breaker::breaker_refusal_message;
#[cfg(test)]
use self::client::{request_budget, ADMISSION_REQUEST_BUDGET, MUTATION_REQUEST_BUDGET};
