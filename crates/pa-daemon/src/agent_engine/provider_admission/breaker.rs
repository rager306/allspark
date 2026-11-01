//! The per-PROVIDER circuit breaker (M4): one retry budget shared by every
//! model of one provider, the single half-open probe, and the fail-fast refusal
//! a stream is answered with while the breaker is open.
//!
//! The records here are driven by the gate in [`super::admit`] — which holds the
//! registry lock and so stays beside the registry — and by the settle path in
//! [`super::settle`]. Their fields are `pub(super)` because those two are
//! SIBLING modules: private visibility reaches the defining module and its
//! descendants, and the two halves of `impl ProviderAdmission` are neither.

use std::collections::{HashMap, VecDeque};

use pa_agent::types::AssistantMessage;

use super::{BreakerSnapshot, ProviderBreaker, ProviderKey, RETRY_BUDGET_MIN_SAMPLES};

/// The breaker half of a snapshot row: the provider's standing, or the
/// closed default for a provider the breaker has never seen.
pub(super) fn breaker_snapshot(
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
pub(super) enum BreakerGate {
    /// Normal traffic.
    Pass,
    /// This acquirer is the single half-open probe.
    Probe,
    /// Fail fast: the breaker is open (or one probe is already in flight).
    Refuse,
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
    pub(super) fn failures(&self) -> usize {
        self.window.iter().filter(|failed| **failed).count()
    }

    /// Whether the retry budget is spent past the SRE bound (10% of the
    /// window, once enough samples accumulated).
    pub(super) fn budget_exceeded(&self) -> bool {
        self.window.len() >= RETRY_BUDGET_MIN_SAMPLES && self.failures() * 10 > self.window.len()
    }
}

/// The open breaker's fail-fast message: a rate-limit-shaped refusal
/// carrying the marker the M3 classifier reads as account-local
/// rotation (the admission plane's own wording, matched by
/// [`pa_core::session_engine::provider_retry::classify_rate_limit_reason`]).
pub(super) fn breaker_refusal_message(key: &ProviderKey) -> AssistantMessage {
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
