//! Provider retry policy: the single shared policy for provider-failure retries
//! (permanent kinds, Retry-After-aware capped delays), shared by the session
//! auto-retry loop and one-shot completion helpers; failures ride the assistant
//! message as a `provider_stream_failure` diagnostic (kind, status, `retryAfterMs`).

use pa_agent::abort::AbortSignal;
use pa_agent::types::{AssistantMessage, StopReason};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRetryPolicy {
    pub enabled: bool,
    pub max_retries: u32,
    pub base_delay_ms: u64,
    /// Max server-requested retry delay before giving up; 0 disables the cap.
    pub max_retry_delay_ms: u64,
    /// Ceiling on the exponential backoff; the default is unbounded
    /// (the TS quick-retry loop grows without bound).
    pub max_delay_ms: u64,
}

/// No backoff ceiling (the TS quick-retry schedule).
pub const UNBOUNDED_BACKOFF_MS: u64 = u64::MAX;

/// Default policy (also the settings defaults).
pub const DEFAULT_PROVIDER_RETRY_POLICY: ProviderRetryPolicy = ProviderRetryPolicy {
    enabled: true,
    max_retries: 3,
    base_delay_ms: 2000,
    max_retry_delay_ms: 60000,
    max_delay_ms: UNBOUNDED_BACKOFF_MS,
};

/// Resolution of one retry-delay decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRetryDelay {
    /// Wait `delay_ms`, then retry.
    Wait { delay_ms: u64 },
    /// The server-requested wait exceeds the policy cap; give up.
    ExceedsCap { retry_after_ms: u64 },
    /// The failure is account-local (a spent quota or spend cap): no
    /// quick retry rides this ladder — settle it now, the failover
    /// chain re-routes and the quota park seam owns any reported reset
    /// (that hand-off is the [`ProviderRetryDelay::ExceedsCap`] arm).
    Settle,
}

/// Node caps timers at 2^31-1 ms; longer delays overflow setTimeout and fire
/// after ~1ms. Cap the computed wait at the same bound.
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

/// Local listener/lifecycle crashes are not provider failures; never retry them.
#[must_use]
pub fn is_agent_lifecycle_failure(message: &AssistantMessage) -> bool {
    message.diagnostics.as_ref().is_some_and(|diagnostics| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == "agent_lifecycle_failure")
    })
}

/// The faux test provider's queue running dry is deterministic; retrying it
/// only stalls tests.
#[must_use]
pub fn is_faux_provider_queue_exhausted(message: &AssistantMessage) -> bool {
    message.provider == "faux"
        && message.error_message.as_deref() == Some("No more faux responses queued")
}

/// A context-overflow failure: re-issuing it unchanged can never
/// succeed; the compact-and-retry recovery owns it.
#[must_use]
pub fn is_context_overflow_failure(message: &AssistantMessage, context_window: u64) -> bool {
    // The shared overflow classifier works over the wire message shape;
    // a round-trip failure means no usage/error fields to inspect.
    let Some(wire) = serde_json::to_value(message)
        .ok()
        .and_then(|value| serde_json::from_value::<pa_types::ai::AssistantMessage>(value).ok())
    else {
        return false;
    };
    pa_ai::is_context_overflow(&wire, (context_window > 0).then_some(context_window))
}

/// The model router's tool-use rejection marker, matched case-insensitively but
/// only together with the classified 404 status: the text alone is
/// provider-controllable and must never steer the retry policy on its own.
const UNSUPPORTED_TOOL_FAILURE_MARKER: &str = "no endpoints found that support tool use";

/// A router rejection for a model that cannot serve tool use: a
/// permanent capability mismatch (every provider rejects identically),
/// never retried and never failed over.
#[must_use]
pub fn is_unsupported_tool_failure(message: &AssistantMessage) -> bool {
    provider_stream_failure_status(message) == Some(404)
        && message.error_message.as_deref().is_some_and(|error| {
            error
                .to_ascii_lowercase()
                .contains(UNSUPPORTED_TOOL_FAILURE_MARKER)
        })
}

/// The `details` payload of the `provider_stream_failure` diagnostic.
#[must_use]
pub fn provider_stream_failure_details(message: &AssistantMessage) -> Option<&Value> {
    message
        .diagnostics
        .as_ref()?
        .iter()
        .find(|diagnostic| diagnostic.kind == "provider_stream_failure")
        .and_then(|diagnostic| diagnostic.details.as_ref())
        .filter(|details| details.is_object())
}

pub fn provider_stream_failure_kind(message: &AssistantMessage) -> Option<String> {
    provider_stream_failure_details(message)?
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn provider_stream_failure_retry_after_ms(message: &AssistantMessage) -> Option<u64> {
    provider_stream_failure_details(message)?
        .get("retryAfterMs")
        .and_then(Value::as_u64)
}

pub fn provider_stream_failure_status(message: &AssistantMessage) -> Option<u16> {
    provider_stream_failure_details(message)?
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
}

/// The failure-scoped disclosure's gate. Lifecycle and faux queue
/// failures and abort conversions stay silent (the 402 diagnosis: only
/// a real provider failure must never settle silently).
#[must_use]
pub fn has_provider_stream_failure(message: &AssistantMessage) -> bool {
    !is_faux_provider_queue_exhausted(message) && provider_stream_failure_details(message).is_some()
}

/// Deterministic rejections never retry; auth gets one retry before it
/// can be marked stale. A 404 is transient (routing blips). Safety
/// filters never retry (TS #2472).
#[must_use]
pub fn is_permanent_provider_failure_kind(
    kind: Option<&str>,
    retries_performed: u32,
    status: Option<u16>,
) -> bool {
    match kind {
        Some("invalid_request") if status == Some(404) => false,
        // A payment failure never refills mid-ladder, so the plain
        // retry loops settle it on the first attempt (the disclosure
        // row still fires); the failover chain intercepts it first to
        // re-route (#4713).
        Some("invalid_request" | "refusal" | "permission" | "safety" | "payment_required") => true,
        Some("auth") => retries_performed > 0,
        _ => false,
    }
}

/// Jitter band on the computed backoff (SANCTIONED DIVERGENCE from TS
/// `providerRetryDelay`, which has none): sessions hammering one rate-limited
/// provider do not re-converge on the same exponential-ladder ticks (the
/// 429-storm operator incident). The jittered value is what the caller waits AND reports.
const RETRY_JITTER_FRACTION: f64 = 0.2;

/// The retry wait for `delay_ms`, jittered by `rand01` (a uniform sample in
/// `[0, 1]`; `0.5` is the identity); pure so tests stay deterministic.
#[must_use]
pub fn jittered_delay_ms(delay_ms: u64, rand01: f64) -> u64 {
    let rand01 = rand01.clamp(0.0, 1.0);
    let factor = 1.0 + RETRY_JITTER_FRACTION * (2.0 * rand01 - 1.0);
    ((delay_ms as f64) * factor).round() as u64
}

/// One uniform sample in `[0, 1]` for [`jittered_delay_ms`]: a time-seeded
/// xorshift step (uniformity is not security here, only spread).
pub fn retry_jitter_rand01() -> f64 {
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::from(duration.subsec_nanos()) ^ (duration.as_secs() << 32)
        });
    let mut x = nanos ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    (x % 10_000) as f64 / 10_000.0
}

/// Delay before retry `attempt` (1-based), honoring a server-requested wait.
#[must_use]
pub fn provider_retry_delay(
    attempt: u32,
    retry_after_ms: Option<u64>,
    policy: &ProviderRetryPolicy,
) -> ProviderRetryDelay {
    if let Some(retry_after_ms) = retry_after_ms {
        if policy.max_retry_delay_ms > 0 && retry_after_ms > policy.max_retry_delay_ms {
            return ProviderRetryDelay::ExceedsCap { retry_after_ms };
        }
    }
    let exponential = policy
        .base_delay_ms
        .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)))
        .min(policy.max_delay_ms);
    let delay_ms = exponential
        .max(retry_after_ms.unwrap_or(0))
        .min(MAX_TIMER_DELAY_MS);
    ProviderRetryDelay::Wait { delay_ms }
}

// ===== M3: typed rate-limit reasons (ADR `ratelimit-admission`) =====

/// The ceiling on honoring a server-requested wait (RFC 9110 §10.2.3):
/// Retry-After is the MINIMUM wait on a retryable class, never more than
/// 60s. A long hint waits the cap and retries instead of giving up —
/// the give-up is what killed subagents in the 429 incident.
pub const RETRY_AFTER_CAP_MS: u64 = 60_000;

/// A rate-classified server hint at or beyond this bound reads as an
/// account quota window (oh-my-pi `LONG_RATE_LIMIT_DELAY_MS`): the
/// park/failover lane, never a 60s-capped retry.
pub const LONG_RATE_HINT_MS: u64 = 5 * 60_000;

/// The 429 attempt ceiling (the ADR band 5-7): a rate-limited failure
/// outlives the provider's window on class backoffs, so its ladder gets
/// six tries instead of the policy default's three.
pub const RATE_LIMIT_MAX_ATTEMPTS: u32 = 6;

/// The class backoffs (oh-my-pi's ladder, the ADR's table).
pub const QUOTA_EXHAUSTED_BACKOFF_MS: u64 = 30 * 60_000;
pub const RATE_LIMIT_BACKOFF_MS: u64 = 30_000;
pub const CONCURRENT_BACKOFF_MS: u64 = 5_000;
pub const CAPACITY_BACKOFF_BASE_MS: u64 = 45_000;
/// The capacity class's spread: 45s ± 15s (thundering-herd guard).
pub const CAPACITY_BACKOFF_JITTER_MS: u64 = 15_000;
pub const SERVER_ERROR_BACKOFF_MS: u64 = 20_000;

/// The typed cause of one rate-limited failure (ADR M3): drives the
/// retry discipline, the admission AIMD signal, and the circuit breaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateLimitReason {
    /// Account-local usage quota is spent; it resets at the provider's
    /// window. Never quick-retried — failover and the park own it.
    QuotaExhausted,
    /// A rolling request-rate cap; retry after the class backoff.
    RateLimit,
    /// A cap on concurrent requests; sheds fast (5s class).
    Concurrent,
    /// The model or provider is out of capacity (overloaded, 503/529).
    Capacity,
    /// A server-side failure inside a rate-limit envelope (20s class).
    Server,
    /// A billing ceiling (spend cap, prepaid balance, 402):
    /// account-local, never retried, no retry-after honored.
    SpendCap,
}

impl RateLimitReason {
    /// The wire spelling (the `provider_admission` settle reason).
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::QuotaExhausted => "quota_exhausted",
            Self::RateLimit => "rate_limit",
            Self::Concurrent => "concurrent",
            Self::Capacity => "capacity",
            Self::Server => "server",
            Self::SpendCap => "spend_cap",
        }
    }

    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "quota_exhausted" => Some(Self::QuotaExhausted),
            "rate_limit" => Some(Self::RateLimit),
            "concurrent" => Some(Self::Concurrent),
            "capacity" => Some(Self::Capacity),
            "server" => Some(Self::Server),
            "spend_cap" => Some(Self::SpendCap),
            _ => None,
        }
    }

    /// Whether the cause is account-local: the provider cannot serve
    /// THIS account at any concurrency until its window resets, so the
    /// failure never quick-retries and the AIMD limit drops to the
    /// floor at once.
    #[must_use]
    pub fn is_account_local(self) -> bool {
        matches!(self, Self::QuotaExhausted | Self::SpendCap)
    }

    /// The class backoff. `rand01` is a uniform `[0, 1]` sample (only
    /// the capacity class spreads: base ± [`CAPACITY_BACKOFF_JITTER_MS`]).
    #[must_use]
    pub fn backoff_ms(self, rand01: f64) -> u64 {
        let rand01 = rand01.clamp(0.0, 1.0);
        match self {
            Self::QuotaExhausted => QUOTA_EXHAUSTED_BACKOFF_MS,
            Self::RateLimit => RATE_LIMIT_BACKOFF_MS,
            Self::Concurrent => CONCURRENT_BACKOFF_MS,
            Self::Capacity => {
                let spread = CAPACITY_BACKOFF_JITTER_MS as f64 * (2.0 * rand01 - 1.0);
                (CAPACITY_BACKOFF_BASE_MS as f64 + spread).round() as u64
            }
            Self::Server => SERVER_ERROR_BACKOFF_MS,
            // A spend cap has no retry-after discipline to honor: it
            // never waits, the failover lane takes it at once.
            Self::SpendCap => 0,
        }
    }
}

/// Whether any occurrence of `first` sits within `span` bytes of one of
/// `seconds` (the oh-my-pi inter-word patterns, length-based, on
/// already-lowercased text).
fn any_phrase_within(lower: &str, first: &str, seconds: &[&str], span: usize) -> bool {
    let floor = |index: usize| {
        let mut index = index;
        while index > 0 && !lower.is_char_boundary(index) {
            index -= 1;
        }
        index
    };
    let ceil = |index: usize| {
        let mut index = index;
        while index < lower.len() && !lower.is_char_boundary(index) {
            index += 1;
        }
        index
    };
    lower.match_indices(first).any(|(start, _)| {
        let after = start + first.len();
        let from = floor(start.saturating_sub(span));
        let to = ceil((after + span).min(lower.len()));
        lower
            .get(from..to)
            .is_some_and(|window| seconds.iter().any(|second| window.contains(second)))
    })
}

/// Classify one rate-limited failure's status and text into a
/// [`RateLimitReason`] (the oh-my-pi ladder over EN/RU/CN wording;
/// fork-local: pa-ai's `classify_stream_failure` already named the
/// family, this refines the cause inside it). Priority: the admission
/// breaker's marker > spend caps > long quota resets > concurrency >
/// capacity > CN quota (guarded by CN transient caps) > account-scoped
/// rate limits > rate caps > generic quota > server > status fallback.
#[must_use]
pub fn classify_rate_limit_reason(status: Option<u16>, text: &str) -> RateLimitReason {
    let lower = text.to_lowercase();
    let has = |phrase: &str| lower.contains(phrase);
    let has_any = |phrases: &[&str]| phrases.iter().any(|phrase| lower.contains(phrase));

    // The admission breaker's own fail-fast marker: rotate like an
    // account-local quota (oh-my-pi maps opaque 429s to rotation too).
    if has("admission circuit open") {
        return RateLimitReason::QuotaExhausted;
    }
    // Billing ceilings never refill inside a ladder; 402 is the
    // billing status (pa-ai pins its kind to payment_required).
    if status == Some(402)
        || has_any(&[
            "spend limit",
            "spending limit",
            "spending-limit",
            "spend cap",
            "spending cap",
            "prepaid balance",
            "usage_pricing_required",
            "usage credits are required",
            "credits_required",
            "лимит расходов",
        ])
    {
        return RateLimitReason::SpendCap;
    }
    // Long-window quota resets (Antigravity's wording): rotate, do not
    // back off on this credential.
    if has_any(&["quota will reset", "exhausted your capacity"]) {
        return RateLimitReason::QuotaExhausted;
    }
    // Concurrency caps shed and back off quickly.
    if any_phrase_within(
        &lower,
        "concurrent",
        &["limit", "quota", "exceed", "reach", "too many"],
        60,
    ) || has_any(&["too many concurrent", "too many parallel"])
        || any_phrase_within(&lower, "одновременн", &["запрос", "превыш", "лимит"], 48)
        || any_phrase_within(&lower, "параллельн", &["запрос", "превыш", "лимит"], 48)
    {
        return RateLimitReason::Concurrent;
    }
    // Capacity/overload wording (503/529 land here through the status
    // fallback below when the text is silent).
    if has_any(&[
        "overloaded",
        "overload",
        "at capacity",
        "model capacity",
        "capacity exhausted",
        "перегружен",
        "перегрузка",
        "нет свободных мощностей",
    ]) {
        return RateLimitReason::Capacity;
    }
    // CN quota exhaustion (Zhipu/Zhenhru windows) guarded by CN
    // transient caps: a per-minute or concurrency cap phrased with
    // 上限 must stay transient, not burn the credential.
    let cn_quota = has_any(&[
        "额度已用完",
        "额度不足",
        "配额已耗尽",
        "配额不足",
        "余额不足",
    ]) || any_phrase_within(&lower, "使用", &["上限"], 30)
        || any_phrase_within(&lower, "用量", &["上限"], 30)
        || any_phrase_within(&lower, "限额", &["重置"], 30);
    let cn_transient = has_any(&["速率限制", "请求过于频繁", "过于频繁", "稍后重试"])
        || any_phrase_within(&lower, "速率", &["上限"], 12)
        || any_phrase_within(&lower, "频率", &["上限"], 12)
        || any_phrase_within(&lower, "每分钟", &["上限"], 12)
        || any_phrase_within(&lower, "并发", &["上限"], 12);
    if cn_quota && !cn_transient {
        return RateLimitReason::QuotaExhausted;
    }
    // Account-scoped caps rotate the credential (an org/workspace rate
    // limit is quota, not a rolling request cap).
    if any_phrase_within(
        &lower,
        "rate limit",
        &["account", "organization", "workspace", "team", "overall"],
        80,
    ) || any_phrase_within(
        &lower,
        "лимит",
        &["аккаунт", "организац", "рабочего пространства"],
        80,
    ) {
        return RateLimitReason::QuotaExhausted;
    }
    // RU exhaustion phrasing ("лимит ... исчерпан"), guarded by RU
    // transient markers: "достигнут лимит запросов в минуту" is a
    // rolling cap, not a spent daily quota.
    let ru_transient = has_any(&[
        "в минуту",
        "в секунду",
        "слишком часто",
        "слишком много запросов",
        "частот",
    ]);
    if !ru_transient
        && (any_phrase_within(&lower, "исчерп", &["лимит", "квота", "бюджет"], 48)
            || any_phrase_within(&lower, "достигнут", &["лимит", "квота"], 48))
    {
        return RateLimitReason::QuotaExhausted;
    }
    // Rolling rate caps (including TPM/RPM wording that reads quota-ish).
    if has_any(&[
        "rate limit",
        "rate_limit",
        "rate-limit",
        "ratelimit",
        "too many requests",
        "too frequent",
        "per minute",
        "per second",
        "throttl",
    ]) || has_any(&[
        "слишком много запросов",
        "превышен лимит запросов",
        "лимит запросов",
        "превышена частота",
        "слишком часто",
        "частоту запросов",
        "速率限制",
        "请求频率",
        "过于频繁",
        "稍后重试",
    ]) {
        return RateLimitReason::RateLimit;
    }
    // Generic quota exhaustion.
    if has_any(&[
        "quota",
        "usage limit",
        "usage_limit",
        "usage_limit_reached",
        "run out of credits",
        "out of credits",
        "insufficient balance",
        "insufficient credits",
        "insufficient_quota",
        "credits exhausted",
        "credits depleted",
        "exceeded your current quota",
    ]) || has_any(&[
        "квота",
        "недостаточно средств",
        "недостаточно баланса",
        "недостаточно кредитов",
    ]) {
        return RateLimitReason::QuotaExhausted;
    }
    if has_any(&[
        "internal server error",
        "internal error",
        "server error",
        "внутренняя ошибка",
        "ошибка сервера",
    ]) {
        return RateLimitReason::Server;
    }
    match status {
        Some(503 | 529) => RateLimitReason::Capacity,
        Some(500..=599) => RateLimitReason::Server,
        // A rate-shaped failure with no finer signal: the conservative
        // retryable class (the 30s window most providers run).
        _ => RateLimitReason::RateLimit,
    }
}

/// The rate-limit reason one failed message carries, when it is
/// rate-limit-shaped: a 429/402 status, the `rate_limit` or
/// `payment_required` classified kinds, or a 429 in the error text.
/// Every other failure answers `None` (the plain exponential ladder).
#[must_use]
pub fn rate_limit_reason_for_message(message: &AssistantMessage) -> Option<RateLimitReason> {
    let kind = provider_stream_failure_kind(message);
    if kind.as_deref() == Some("payment_required") {
        return Some(RateLimitReason::SpendCap);
    }
    let status = provider_stream_failure_status(message);
    let rate_shaped = kind.as_deref() == Some("rate_limit")
        || matches!(status, Some(429 | 402))
        || message
            .error_message
            .as_deref()
            .is_some_and(|error| error.contains("429"));
    if !rate_shaped {
        return None;
    }
    Some(classify_rate_limit_reason(
        status,
        message.error_message.as_deref().unwrap_or(""),
    ))
}

/// The M3 reason-aware retry delay: Retry-After is the MINIMUM wait,
/// honored up to [`RETRY_AFTER_CAP_MS`] within the policy's
/// `maxRetryDelayMs` budget (a hint above the policy cap stays the
/// above-cap give-up: the budget contract wins). Without a
/// hint the class backoff replaces the exponential ladder (RATE 30s,
/// CONCURRENT 5s, CAPACITY 45±15s, SERVER 20s). An account-local cause
/// (quota/spend cap, or a hint at or beyond [`LONG_RATE_HINT_MS`])
/// never quick-retries: it settles for the failover chain, keeping the
/// above-cap `ExceedsCap` hand-off the quota park seam hangs on.
/// `rand01` is a uniform `[0, 1]` sample for the capacity spread.
#[must_use]
pub fn provider_retry_delay_for_reason(
    reason: Option<RateLimitReason>,
    attempt: u32,
    retry_after_ms: Option<u64>,
    policy: &ProviderRetryPolicy,
    rand01: f64,
) -> ProviderRetryDelay {
    let Some(reason) = reason else {
        // Outside the rate-limit family the shared ladder stands.
        return provider_retry_delay(attempt, retry_after_ms, policy);
    };
    let escalated = reason == RateLimitReason::RateLimit
        && retry_after_ms.is_some_and(|hint| hint >= LONG_RATE_HINT_MS);
    if reason.is_account_local() || escalated {
        return match retry_after_ms {
            Some(hint) if policy.max_retry_delay_ms > 0 && hint > policy.max_retry_delay_ms => {
                ProviderRetryDelay::ExceedsCap {
                    retry_after_ms: hint,
                }
            }
            _ => ProviderRetryDelay::Settle,
        };
    }
    // The user's `retry.provider.maxRetryDelayMs` is a budget contract and
    // wins over the hint: a wait the policy refuses stays the above-cap
    // hand-off (`ExceedsCap` -> park/failover seam), never a silent
    // budget overrun. Inside the budget the hint is a minimum honored up
    // to the M3 ceiling.
    let delay_ms = match retry_after_ms {
        Some(hint) => {
            if policy.max_retry_delay_ms > 0 && hint > policy.max_retry_delay_ms {
                return ProviderRetryDelay::ExceedsCap {
                    retry_after_ms: hint,
                };
            }
            hint.min(RETRY_AFTER_CAP_MS)
        }
        None => reason.backoff_ms(rand01),
    };
    ProviderRetryDelay::Wait {
        delay_ms: delay_ms.min(MAX_TIMER_DELAY_MS),
    }
}

/// The effective attempt ceiling for one failed message (M3): a
/// retryable rate-limit failure gets at least [`RATE_LIMIT_MAX_ATTEMPTS`]
/// tries (the ADR band 5-7 — the policy default of three is what killed
/// subagents inside a provider's 30s window); everything else keeps the
/// policy ceiling.
#[must_use]
pub fn effective_max_retries(policy_max_retries: u32, reason: Option<RateLimitReason>) -> u32 {
    if reason.is_some_and(|reason| !reason.is_account_local()) {
        policy_max_retries.max(RATE_LIMIT_MAX_ATTEMPTS)
    } else {
        policy_max_retries
    }
}

/// One-shot completion with the shared retry policy, for consumers outside the session
/// auto-retry loop. A message whose stop reason is `error` is classified against the
/// policy and retried when transient; `wait` returning `false` aborts with `Aborted`.
///
/// # Errors
///
/// Returns the `attempt` future's error when the completion attempt fails.
pub async fn complete_with_provider_retry<A, AF, W, WF>(
    policy: &ProviderRetryPolicy,
    signal: Option<&AbortSignal>,
    mut wait: W,
    mut attempt: A,
) -> anyhow::Result<AssistantMessage>
where
    A: FnMut() -> AF,
    AF: std::future::Future<Output = anyhow::Result<AssistantMessage>>,
    W: FnMut(std::time::Duration) -> WF,
    WF: std::future::Future<Output = bool>,
{
    let max_retries = if policy.enabled {
        policy.max_retries
    } else {
        0
    };
    let mut retries_performed = 0u32;
    loop {
        let message = attempt().await?;
        if message.stop_reason != StopReason::Error {
            return Ok(message);
        }
        if signal.is_some_and(AbortSignal::is_aborted) {
            return Ok(with_stop_reason_aborted(message));
        }
        let reason = rate_limit_reason_for_message(&message);
        if retries_performed >= effective_max_retries(max_retries, reason)
            || is_agent_lifecycle_failure(&message)
            || is_faux_provider_queue_exhausted(&message)
            || is_unsupported_tool_failure(&message)
        {
            return Ok(message);
        }
        let kind = provider_stream_failure_kind(&message);
        let status = provider_stream_failure_status(&message);
        if is_permanent_provider_failure_kind(kind.as_deref(), retries_performed, status) {
            return Ok(message);
        }
        let delay = provider_retry_delay_for_reason(
            reason,
            retries_performed + 1,
            provider_stream_failure_retry_after_ms(&message),
            policy,
            retry_jitter_rand01(),
        );
        let ProviderRetryDelay::Wait { delay_ms } = delay else {
            // ExceedsCap and Settle both settle here: the one-shot
            // helper has no failover chain or park seam to hand to.
            return Ok(message);
        };
        // The rate-limit classes carry their own jitter discipline (the
        // capacity spread); a server hint is a minimum the generic ±20%
        // band must not undercut.
        let delay_ms = if reason.is_some() {
            delay_ms
        } else {
            jittered_delay_ms(delay_ms, retry_jitter_rand01())
        };
        if !wait(std::time::Duration::from_millis(delay_ms)).await {
            return Ok(with_stop_reason_aborted(message));
        }
        retries_performed += 1;
    }
}

fn with_stop_reason_aborted(mut message: AssistantMessage) -> AssistantMessage {
    message.stop_reason = StopReason::Aborted;
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_agent::types::{AssistantContent, AssistantMessageDiagnostic, TextContent, Usage};

    fn error_message(
        kind: Option<&str>,
        status: Option<u16>,
        retry_after_ms: Option<u64>,
    ) -> AssistantMessage {
        let details = serde_json::json!({
            "kind": kind,
            "status": status,
            "retryAfterMs": retry_after_ms,
        });
        AssistantMessage {
            content: vec![AssistantContent::Text(TextContent {
                text: String::new(),
                text_signature: None,
            })],
            api: String::new(),
            provider: "test".to_string(),
            model: "m".to_string(),
            response_model: None,
            response_id: None,
            diagnostics: Some(vec![AssistantMessageDiagnostic {
                kind: "provider_stream_failure".to_string(),
                timestamp: 0,
                error: None,
                details: Some(details),
            }]),
            usage: Usage::zero(),
            stop_reason: StopReason::Error,
            stop_reason_raw: None,
            error_message: Some("provider failed".to_string()),
            timestamp: 0,
        }
    }

    fn ok_message() -> AssistantMessage {
        AssistantMessage {
            content: vec![AssistantContent::Text(TextContent {
                text: "done".to_string(),
                text_signature: None,
            })],
            api: String::new(),
            provider: "test".to_string(),
            model: "m".to_string(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::zero(),
            stop_reason: StopReason::Stop,
            stop_reason_raw: None,
            error_message: None,
            timestamp: 0,
        }
    }

    /// SANCTIONED DIVERGENCE, operator ruling 2026-09-23: ±20% around
    /// the computed backoff.
    #[test]
    fn jitter_stays_inside_the_band_and_mid_is_identity() {
        assert_eq!(jittered_delay_ms(1000, 0.5), 1000);
        assert_eq!(jittered_delay_ms(1000, 0.0), 800);
        assert_eq!(jittered_delay_ms(1000, 1.0), 1200);
        assert_eq!(jittered_delay_ms(1000, 7.5), 1200); // clamped high
        assert_eq!(jittered_delay_ms(1000, -0.5), 800); // clamped low
        assert_eq!(jittered_delay_ms(0, 0.1), 0);
        assert_eq!(jittered_delay_ms(1, 0.5), 1);
        assert_eq!(jittered_delay_ms(1, 0.1), 1); // 0.8 rounds to 1
        assert_eq!(jittered_delay_ms(2, 0.0), 2); // 1.6 rounds to 2
        for _ in 0..64 {
            let sample = retry_jitter_rand01();
            assert!((0.0..=1.0).contains(&sample), "sample {sample}");
        }
    }

    // ===== M3: typed rate-limit reasons =====

    #[test]
    fn rate_limit_reasons_classify_from_status_and_text() {
        use super::RateLimitReason::*;
        // (status, kind, text, expected) — EN, RU, and CN provider
        // wording, in the classifier's priority order.
        let cases: &[(Option<u16>, Option<&str>, &str, RateLimitReason)] = &[
            // The admission breaker's own fail-fast marker rotates.
            (
                Some(429),
                Some("rate_limit"),
                "Provider admission circuit open for zai/glm-5.3 (retry budget exceeded)",
                QuotaExhausted,
            ),
            // Spend caps and billing statuses.
            (
                Some(429),
                Some("rate_limit"),
                "You have exceeded your monthly spending cap",
                SpendCap,
            ),
            (
                None,
                Some("rate_limit"),
                "usage_pricing_required: prepaid balance used up",
                SpendCap,
            ),
            (Some(402), None, "insufficient credits", SpendCap),
            // Long-window quota resets.
            (
                Some(429),
                Some("rate_limit"),
                "You have exhausted your capacity on this model. Your quota will reset after 05:00",
                QuotaExhausted,
            ),
            // Concurrency caps.
            (
                Some(429),
                Some("rate_limit"),
                "API Error: 429 Too many concurrent requests",
                Concurrent,
            ),
            (
                Some(429),
                Some("rate_limit"),
                "Превышено количество одновременных запросов к API",
                Concurrent,
            ),
            // Capacity / overload.
            (
                Some(429),
                Some("rate_limit"),
                "The model is overloaded, please retry",
                Capacity,
            ),
            (
                None,
                Some("rate_limit"),
                "провайдер перегружен, попробуйте позже",
                Capacity,
            ),
            // CN quota windows, guarded against CN transient caps.
            (
                Some(429),
                Some("rate_limit"),
                "429 已达到 5 小时的使用上限。您的限额将在 2026-08-06 20:06:00 重置。",
                QuotaExhausted,
            ),
            (
                Some(429),
                Some("rate_limit"),
                "当前已达到 Token Plan 用量上限，请明天再试",
                QuotaExhausted,
            ),
            (
                None,
                Some("rate_limit"),
                "余额不足，请充值后重试",
                QuotaExhausted,
            ),
            // CN transient caps stay retryable.
            (
                None,
                Some("rate_limit"),
                "请求过于频繁，请稍后重试",
                RateLimit,
            ),
            (
                None,
                Some("rate_limit"),
                "速率达到上限，请稍后再试",
                RateLimit,
            ),
            // Account-scoped caps rotate the credential.
            (
                Some(429),
                Some("rate_limit"),
                "You have reached your account rate limit",
                QuotaExhausted,
            ),
            (
                Some(429),
                Some("rate_limit"),
                "Превышен лимит запросов для вашего аккаунта",
                QuotaExhausted,
            ),
            // Rolling rate caps.
            (
                Some(429),
                Some("rate_limit"),
                "Too many requests, slow down",
                RateLimit,
            ),
            (
                None,
                Some("rate_limit"),
                "rate limit exceeded, retry in 30s",
                RateLimit,
            ),
            (
                Some(429),
                Some("rate_limit"),
                "Превышен лимит запросов в минуту",
                RateLimit,
            ),
            // Generic quota exhaustion.
            (
                None,
                Some("rate_limit"),
                "You have hit your usage limit",
                QuotaExhausted,
            ),
            (
                None,
                Some("rate_limit"),
                "You exceeded your current quota, please check your plan and billing details",
                QuotaExhausted,
            ),
            (
                None,
                Some("rate_limit"),
                "insufficient_quota",
                QuotaExhausted,
            ),
            (
                None,
                Some("rate_limit"),
                "Квота запросов исчерпана",
                QuotaExhausted,
            ),
            (
                None,
                Some("rate_limit"),
                "Лимит запросов исчерпан на сегодня",
                QuotaExhausted,
            ),
            (
                None,
                Some("rate_limit"),
                "недостаточно средств на балансе",
                QuotaExhausted,
            ),
            // Server failures inside a rate envelope.
            (None, Some("rate_limit"), "internal server error", Server),
            // Status fallbacks for silent text.
            (Some(529), Some("rate_limit"), "provider down", Capacity),
            (Some(503), Some("rate_limit"), "provider down", Capacity),
            (Some(500), Some("rate_limit"), "provider down", Server),
            (Some(429), Some("rate_limit"), "", RateLimit),
            (None, Some("rate_limit"), "provider down", RateLimit),
        ];
        for (status, _kind, text, expected) in cases {
            assert_eq!(
                classify_rate_limit_reason(*status, text),
                *expected,
                "text {text:?} with status {status:?}"
            );
        }
    }

    #[test]
    fn message_reasons_read_kind_status_and_text() {
        // payment_required is the spend cap, by kind, regardless of text.
        assert_eq!(
            rate_limit_reason_for_message(&error_message(
                Some("payment_required"),
                Some(402),
                None
            )),
            Some(RateLimitReason::SpendCap)
        );
        // A quota-worded rate limit classifies account-local.
        let quota = AssistantMessage {
            error_message: Some("You have hit your usage limit".to_string()),
            ..error_message(Some("rate_limit"), Some(429), None)
        };
        assert_eq!(
            rate_limit_reason_for_message(&quota),
            Some(RateLimitReason::QuotaExhausted)
        );
        // A plain 429 is the retryable class.
        assert_eq!(
            rate_limit_reason_for_message(&error_message(Some("rate_limit"), Some(429), None)),
            Some(RateLimitReason::RateLimit)
        );
        // Every other failure is outside the family.
        assert_eq!(
            rate_limit_reason_for_message(&error_message(Some("server_error"), Some(500), None)),
            None
        );
        assert_eq!(rate_limit_reason_for_message(&ok_message()), None);
    }

    #[test]
    fn reason_wire_spellings_round_trip() {
        let reasons = [
            RateLimitReason::QuotaExhausted,
            RateLimitReason::RateLimit,
            RateLimitReason::Concurrent,
            RateLimitReason::Capacity,
            RateLimitReason::Server,
            RateLimitReason::SpendCap,
        ];
        for reason in reasons {
            assert_eq!(RateLimitReason::from_wire(reason.as_wire()), Some(reason));
        }
        assert_eq!(RateLimitReason::from_wire("nonsense"), None);
    }

    #[test]
    fn retry_after_is_the_minimum_wait_capped_at_60s() {
        let policy = DEFAULT_PROVIDER_RETRY_POLICY;
        // A small hint is honored exactly (the ADR's "5s, not 30s"): it
        // replaces both the exponential ladder and the class backoff.
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::RateLimit),
                1,
                Some(5_000),
                &policy,
                0.5,
            ),
            ProviderRetryDelay::Wait { delay_ms: 5_000 }
        );
        // A hint above the POLICY cap is the budget's give-up: it settles
        // into the failover/park seam instead of silently overrunning the
        // user's maxRetryDelayMs (the failover reroute is the better
        // remedy for a long-hinted 429 than a 60s sit).
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::RateLimit),
                1,
                Some(120_000),
                &policy,
                0.5,
            ),
            ProviderRetryDelay::ExceedsCap {
                retry_after_ms: 120_000
            }
        );
        // Inside an admitting budget the hint honors the M3 ceiling: a
        // 120s hint under a 180s budget waits the 60s ceiling, never
        // gives up and never overruns the ceiling.
        let admitting = ProviderRetryPolicy {
            max_retry_delay_ms: 180_000,
            ..policy
        };
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::RateLimit),
                1,
                Some(120_000),
                &admitting,
                0.5,
            ),
            ProviderRetryDelay::Wait { delay_ms: 60_000 }
        );
        // Without a hint the class backoff replaces the ladder.
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::RateLimit),
                1,
                None,
                &policy,
                0.5,
            ),
            ProviderRetryDelay::Wait { delay_ms: 30_000 }
        );
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::Concurrent),
                1,
                None,
                &policy,
                0.5,
            ),
            ProviderRetryDelay::Wait { delay_ms: 5_000 }
        );
        assert_eq!(
            provider_retry_delay_for_reason(Some(RateLimitReason::Server), 1, None, &policy, 0.5,),
            ProviderRetryDelay::Wait { delay_ms: 20_000 }
        );
        // The capacity class spreads ±15s around 45s.
        assert_eq!(
            provider_retry_delay_for_reason(Some(RateLimitReason::Capacity), 1, None, &policy, 0.5,),
            ProviderRetryDelay::Wait { delay_ms: 45_000 }
        );
        assert_eq!(
            provider_retry_delay_for_reason(Some(RateLimitReason::Capacity), 1, None, &policy, 0.0,),
            ProviderRetryDelay::Wait { delay_ms: 30_000 }
        );
        assert_eq!(
            provider_retry_delay_for_reason(Some(RateLimitReason::Capacity), 1, None, &policy, 1.0,),
            ProviderRetryDelay::Wait { delay_ms: 60_000 }
        );
        // Outside the family the shared exponential ladder stands.
        assert_eq!(
            provider_retry_delay_for_reason(None, 1, None, &policy, 0.5),
            provider_retry_delay(1, None, &policy)
        );
    }

    #[test]
    fn account_local_reasons_settle_and_long_hints_keep_the_park_hand_off() {
        let policy = DEFAULT_PROVIDER_RETRY_POLICY;
        // A spent quota without a reported reset settles at once: no
        // quick retry, the failover chain owns the re-route.
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::QuotaExhausted),
                1,
                None,
                &policy,
                0.5,
            ),
            ProviderRetryDelay::Settle
        );
        // A spend cap never honors a retry-after.
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::SpendCap),
                1,
                Some(10_000),
                &policy,
                0.5,
            ),
            ProviderRetryDelay::Settle
        );
        // A quota reset beyond the cap keeps the ExceedsCap hand-off
        // the park seam hangs on (do not break the park).
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::QuotaExhausted),
                1,
                Some(3_600_000),
                &policy,
                0.5,
            ),
            ProviderRetryDelay::ExceedsCap {
                retry_after_ms: 3_600_000
            }
        );
        // A RATE hint at or beyond the long-hint bound escalates to the
        // quota lane (oh-my-pi LONG_RATE_LIMIT_DELAY_MS): park/reroute,
        // never a 60s-capped retry.
        assert_eq!(
            provider_retry_delay_for_reason(
                Some(RateLimitReason::RateLimit),
                1,
                Some(4_363_000),
                &policy,
                0.5,
            ),
            ProviderRetryDelay::ExceedsCap {
                retry_after_ms: 4_363_000
            }
        );
        // A non-rate failure keeps the old above-cap give-up.
        assert_eq!(
            provider_retry_delay_for_reason(None, 1, Some(4_363_000), &policy, 0.5),
            ProviderRetryDelay::ExceedsCap {
                retry_after_ms: 4_363_000
            }
        );
    }

    #[test]
    fn rate_limited_failures_get_the_m3_attempt_ceiling() {
        assert_eq!(
            effective_max_retries(3, Some(RateLimitReason::RateLimit)),
            RATE_LIMIT_MAX_ATTEMPTS
        );
        assert_eq!(
            effective_max_retries(7, Some(RateLimitReason::Capacity)),
            7,
            "a policy already above the band keeps its ceiling"
        );
        assert_eq!(
            effective_max_retries(3, Some(RateLimitReason::QuotaExhausted)),
            3,
            "account-local causes settle before the ceiling matters"
        );
        assert_eq!(effective_max_retries(3, None), 3);
    }

    #[test]
    fn retry_delay_exponentially_backs_off() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 2000,
            max_retry_delay_ms: 60000,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        assert_eq!(
            provider_retry_delay(1, None, &policy),
            ProviderRetryDelay::Wait { delay_ms: 2000 }
        );
        assert_eq!(
            provider_retry_delay(2, None, &policy),
            ProviderRetryDelay::Wait { delay_ms: 4000 }
        );
        assert_eq!(
            provider_retry_delay(3, None, &policy),
            ProviderRetryDelay::Wait { delay_ms: 8000 }
        );
        assert_eq!(
            provider_retry_delay(1, Some(9000), &policy),
            ProviderRetryDelay::Wait { delay_ms: 9000 }
        );
    }

    #[test]
    fn retry_delay_caps_server_requests() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 2000,
            max_retry_delay_ms: 60000,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        assert_eq!(
            provider_retry_delay(1, Some(60001), &policy),
            ProviderRetryDelay::ExceedsCap {
                retry_after_ms: 60001
            }
        );
        // Cap disabled (0): the server wait is honored.
        let uncapped = ProviderRetryPolicy {
            max_retry_delay_ms: 0,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
            ..policy
        };
        assert_eq!(
            provider_retry_delay(1, Some(120_000), &uncapped),
            ProviderRetryDelay::Wait { delay_ms: 120_000 }
        );
    }

    #[test]
    fn permanent_kinds_never_retry() {
        assert!(is_permanent_provider_failure_kind(
            Some("invalid_request"),
            0,
            Some(400)
        ));
        assert!(is_permanent_provider_failure_kind(Some("refusal"), 0, None));
        assert!(is_permanent_provider_failure_kind(
            Some("permission"),
            0,
            None
        ));
        // TS #2472: safety filters deterministically reject identical
        // requests.
        assert!(is_permanent_provider_failure_kind(Some("safety"), 0, None));
        assert!(is_permanent_provider_failure_kind(
            Some("safety"),
            0,
            Some(400)
        ));
        // Auth retries once before it can be marked stale.
        assert!(!is_permanent_provider_failure_kind(
            Some("auth"),
            0,
            Some(401)
        ));
        assert!(is_permanent_provider_failure_kind(
            Some("auth"),
            1,
            Some(401)
        ));
        assert!(!is_permanent_provider_failure_kind(
            Some("invalid_request"),
            0,
            Some(404)
        ));
        // A 402's deterministic kind is permanent on the first attempt.
        assert!(is_permanent_provider_failure_kind(
            Some("payment_required"),
            0,
            Some(402)
        ));
        assert!(!is_permanent_provider_failure_kind(
            Some("server_error"),
            0,
            None
        ));
        // A dropped stream is transient at every rung of the ladder: the
        // provider ended the response mid-block without a stop signal, so
        // re-issuing the same request can succeed.
        for retries_performed in 0..3 {
            assert!(
                !is_permanent_provider_failure_kind(Some("stream_drop"), retries_performed, None),
                "a stream_drop is retryable at rung {retries_performed}"
            );
        }
        assert!(!is_permanent_provider_failure_kind(None, 0, None));
    }

    /// The stream-drop arc, end to end: the REAL provider against the
    /// dropped-mid-thinking SSE fixture (no stop signal, no `[DONE]`),
    /// through the real stream adapter and the retry driver — the retry
    /// fires and the completion settles on the retry's healthy stream
    /// (the fleet's death shape, replayed).
    #[tokio::test]
    async fn a_dropped_sse_stream_retries_and_completes_on_the_retry() {
        use crate::session_engine::provider_adapter::{json_round_trip, real_stream_fn};
        use pa_agent::stream::{LlmContext, StreamRequestOptions};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // The dropped body: a thinking delta, then the connection ends
        // mid-block (no `finish_reason`, no `[DONE]`).
        let dropped = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-test\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"Let me work through\"},\"finish_reason\":null}]}\n\n";
        // The healthy body the retry receives.
        let healthy = "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Recovered on the retry.\"}}]}\n\ndata: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for body in [dropped, healthy] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0u8; 8192];
                let _ = socket.read(&mut request).await.unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let model: pa_types::ai::Model = serde_json::from_value(serde_json::json!({
            "id": "glm-test", "name": "GLM test", "api": "openai-completions",
            "provider": "prime-inference", "baseUrl": format!("http://{addr}"),
            "reasoning": true, "input": ["text"],
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
            "contextWindow": 131_072, "maxTokens": 8192,
        }))
        .unwrap();
        let agent_model: pa_agent::types::Model = json_round_trip(&model).unwrap();
        let stream_fn = real_stream_fn(Some("test".to_string()), model);

        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_attempt = std::sync::Arc::clone(&attempts);
        let message = complete_with_provider_retry(
            &policy,
            None,
            |_| async { true },
            move || {
                let stream_fn = std::sync::Arc::clone(&stream_fn);
                let agent_model = agent_model.clone();
                let attempts = std::sync::Arc::clone(&attempts_for_attempt);
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let mut stream = stream_fn(
                        agent_model,
                        LlmContext::default(),
                        StreamRequestOptions::default(),
                    )
                    .await?;
                    stream.result().await
                }
            },
        )
        .await
        .unwrap();
        // The drop consumed one attempt; the retry completed the turn.
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(message.stop_reason, StopReason::Stop);
        let AssistantContent::Text(text) = &message.content[0] else {
            panic!("text content on the retried turn: {:?}", message.content);
        };
        assert_eq!(text.text, "Recovered on the retry.");
    }

    /// The one-shot completion arms (side questions, compaction,
    /// refinement) take the `stream_drop` class through the same retry
    /// ladder: a dropped stream retries and the completion settles on the
    /// retry.
    #[tokio::test]
    async fn stream_drop_failures_retry_in_the_one_shot_arms() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_attempt = std::sync::Arc::clone(&attempts);
        let message = complete_with_provider_retry(
            &policy,
            None,
            |_| async { true },
            move || {
                let attempts = std::sync::Arc::clone(&attempts_for_attempt);
                async move {
                    let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    if attempt < 2 {
                        Ok(error_message(Some("stream_drop"), None, None))
                    } else {
                        Ok(ok_message())
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(message.stop_reason, StopReason::Stop);
        let AssistantContent::Text(text) = &message.content[0] else {
            panic!("text content");
        };
        assert_eq!(text.text, "done");
    }

    #[tokio::test]
    async fn transient_failure_is_retried_until_success() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_attempt = std::sync::Arc::clone(&attempts);
        let waited = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let waited_for_wait = std::sync::Arc::clone(&waited);
        let message = complete_with_provider_retry(
            &policy,
            None,
            move |delay| {
                let waited = std::sync::Arc::clone(&waited_for_wait);
                async move {
                    waited.lock().unwrap().push(delay.as_millis() as u64);
                    true
                }
            },
            move || {
                let attempts = std::sync::Arc::clone(&attempts_for_attempt);
                async move {
                    let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    if attempt < 3 {
                        Ok(error_message(Some("server_error"), Some(500), None))
                    } else {
                        Ok(ok_message())
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
        // The waits sit in the ±20% jitter band around the 5ms/10ms
        // ladder steps (SANCTIONED DIVERGENCE, operator ruling
        // 2026-09-23).
        let waits = waited.lock().unwrap().clone();
        assert_eq!(waits.len(), 2, "two waits: {waits:?}");
        assert!(
            (4..=7).contains(&waits[0]) && (8..=14).contains(&waits[1]),
            "jittered waits {waits:?} outside the [4,7]/[8,14] bands"
        );
        assert_eq!(message.stop_reason, StopReason::Stop);
        let AssistantContent::Text(text) = &message.content[0] else {
            panic!("text content");
        };
        assert_eq!(text.text, "done");
    }

    #[tokio::test]
    async fn retries_exhaust_after_max_retries() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 2,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let mut attempts = 0;
        let message = complete_with_provider_retry(
            &policy,
            None,
            |_| async { true },
            || {
                attempts += 1;
                async { Ok(error_message(Some("server_error"), None, None)) }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts, 3);
        assert_eq!(message.stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn disabled_policy_never_retries() {
        let policy = ProviderRetryPolicy {
            enabled: false,
            max_retries: 5,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let mut attempts = 0;
        let message = complete_with_provider_retry(
            &policy,
            None,
            |_| async { true },
            || {
                attempts += 1;
                async { Ok(error_message(Some("server_error"), None, None)) }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts, 1);
        assert_eq!(message.stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn permanent_failure_returns_without_waiting() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_attempt = std::sync::Arc::clone(&attempts);
        let waited = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let waited_for_wait = std::sync::Arc::clone(&waited);
        let message = complete_with_provider_retry(
            &policy,
            None,
            move |_| {
                let waited = std::sync::Arc::clone(&waited_for_wait);
                async move {
                    waited.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    true
                }
            },
            move || {
                let attempts = std::sync::Arc::clone(&attempts_for_attempt);
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(error_message(Some("invalid_request"), None, Some(400)))
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(waited.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(message.stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn aborted_signal_racing_failure_stops_aborted() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let controller = pa_agent::abort::AbortController::new();
        controller.abort();
        let message = complete_with_provider_retry(
            &policy,
            Some(&controller.signal()),
            |_| async { true },
            || async { Ok(error_message(Some("server_error"), None, None)) },
        )
        .await
        .unwrap();
        assert_eq!(message.stop_reason, StopReason::Aborted);
    }

    #[tokio::test]
    async fn aborted_wait_stops_aborted() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let mut attempts = 0;
        let message = complete_with_provider_retry(
            &policy,
            None,
            |_| async { false },
            || {
                attempts += 1;
                async { Ok(error_message(Some("server_error"), None, None)) }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts, 1);
        assert_eq!(message.stop_reason, StopReason::Aborted);
    }

    #[tokio::test]
    async fn lifecycle_and_faux_failures_are_never_retried() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let mut lifecycle = error_message(None, None, None);
        lifecycle.diagnostics = Some(vec![AssistantMessageDiagnostic {
            kind: "agent_lifecycle_failure".to_string(),
            timestamp: 0,
            error: None,
            details: None,
        }]);
        let mut attempts = 0;
        let message = complete_with_provider_retry(
            &policy,
            None,
            |_| async { true },
            || {
                attempts += 1;
                async { Ok(lifecycle.clone()) }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts, 1);
        assert_eq!(message.stop_reason, StopReason::Error);

        let mut faux = error_message(None, None, None);
        faux.provider = "faux".to_string();
        faux.error_message = Some("No more faux responses queued".to_string());
        let mut attempts = 0;
        let message = complete_with_provider_retry(
            &policy,
            None,
            |_| async { true },
            || {
                attempts += 1;
                async { Ok(faux.clone()) }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts, 1);
        assert_eq!(message.stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn attempt_errors_propagate() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let error = complete_with_provider_retry(
            &policy,
            None,
            |_| async { true },
            || async { Err(anyhow::anyhow!("attempt failed")) },
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "attempt failed");
    }

    #[test]
    fn stream_failure_fields_read_from_diagnostics() {
        let message = error_message(Some("server_error"), Some(500), Some(120));
        assert_eq!(
            provider_stream_failure_kind(&message).as_deref(),
            Some("server_error")
        );
        assert_eq!(provider_stream_failure_status(&message), Some(500));
        assert_eq!(provider_stream_failure_retry_after_ms(&message), Some(120));
        let retry_after = provider_retry_delay(1, Some(120), &DEFAULT_PROVIDER_RETRY_POLICY);
        assert_eq!(retry_after, ProviderRetryDelay::Wait { delay_ms: 2000 });
    }

    /// Provider-controllable text alone never steers the retry policy;
    /// the rejection is permanent only together with the 404 status.
    #[test]
    fn unsupported_tool_rejections_are_terminal() {
        let mut unsupported = error_message(Some("invalid_request"), Some(404), None);
        unsupported.error_message = Some(
            "404 No endpoints found that support tool use. Try disabling \"ipython\".".to_string(),
        );
        assert!(is_unsupported_tool_failure(&unsupported));
        let mut upper = unsupported;
        upper.error_message = Some("404 NO ENDPOINTS FOUND THAT SUPPORT TOOL USE".to_string());
        assert!(is_unsupported_tool_failure(&upper));
        let mut blip = error_message(Some("invalid_request"), Some(404), None);
        blip.error_message = Some("404 model route not found".to_string());
        assert!(!is_unsupported_tool_failure(&blip));
        let mut transient = error_message(Some("server_error"), Some(503), None);
        transient.error_message = Some("503 No endpoints found that support tool use".to_string());
        assert!(!is_unsupported_tool_failure(&transient));
        let mut unclassified = error_message(Some("server_error"), None, None);
        unclassified.error_message = Some("No endpoints found that support tool use".to_string());
        assert!(!is_unsupported_tool_failure(&unclassified));
        let empty = error_message(Some("server_error"), Some(500), None);
        assert!(!is_unsupported_tool_failure(&empty));
    }

    #[tokio::test]
    async fn unsupported_tool_failures_never_retry() {
        let policy = ProviderRetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 5,
            max_retry_delay_ms: 50,
            max_delay_ms: UNBOUNDED_BACKOFF_MS,
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_attempt = std::sync::Arc::clone(&attempts);
        let message = complete_with_provider_retry(
            &policy,
            None,
            |_| async { true },
            move || {
                let attempts = std::sync::Arc::clone(&attempts_for_attempt);
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let mut unsupported = error_message(Some("invalid_request"), Some(404), None);
                    unsupported.error_message =
                        Some("404 No endpoints found that support tool use.".to_string());
                    Ok(unsupported)
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(message.stop_reason, StopReason::Error);
    }
}
