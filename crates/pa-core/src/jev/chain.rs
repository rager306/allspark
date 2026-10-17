//! The candidate ladder: one judgment walks the chain until a backend
//! answers. Mirrors the omp `ChainJudge` semantics:
//!
//! - a candidate rejected by its account (401/402/403) cools down for
//!   [`CANDIDATE_REJECTION_COOLDOWN`] (5 minutes), keyed by candidate
//!   identity - without this, every judgment re-pays the rejected request
//!   before reaching the next candidate;
//! - a candidate without a credential is skipped, recorded unavailable;
//! - an abort or a timeout rethrows immediately - a cancellation must not
//!   spend the next candidate;
//! - every candidate failing is a typed error naming the last failure and
//!   the last unavailability (consumers pick fail-open / fail-closed).
//!
//! Native-only rule: from the first native candidate on, prompted candidates
//! never run - a prompted model cannot reproduce calibrated probabilities,
//! and the silent substitution breaks the 0.7/0.9 thresholds. The reverse is
//! fine: with no native at all, prompted candidates may serve.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::jev::{Judge, JudgeError};

/// How long a rejected account stays skipped.
pub const CANDIDATE_REJECTION_COOLDOWN: Duration = Duration::from_mins(5);

/// Which backend a chain candidate routes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeKind {
    /// A native System One / Decisions API (calibrated probabilities).
    Native,
    /// A prompted chat model standing in as a judge (Phase 1: never built;
    /// the chain keeps the slot so the rule and its test are real).
    Prompted,
}

/// One chain candidate.
pub struct JudgeCandidate {
    /// Cooldown key: `provider/model-id` of the catalog entry.
    pub identity: String,
    pub kind: JudgeKind,
    /// `None`: the candidate has no credential and is always skipped.
    pub judge: Option<Arc<dyn Judge>>,
}

/// The process-shared rejection ledger (keyed by candidate identity, so a
/// re-resolved chain inherits another session's cooldowns - the omp
/// registry-carried map's equivalent).
fn shared_rejections() -> &'static Arc<Mutex<HashMap<String, Instant>>> {
    static REJECTIONS: OnceLock<Arc<Mutex<HashMap<String, Instant>>>> = OnceLock::new();
    REJECTIONS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// The judge that walks the candidate ladder. Cheap to share by `Arc`; the
/// ladder state (candidates + cooldowns) lives per instance.
pub struct ChainJudge {
    candidates: Vec<JudgeCandidate>,
    rejections: Arc<Mutex<HashMap<String, Instant>>>,
}

impl ChainJudge {
    /// Build the chain with a private rejection ledger. The native-only
    /// filter applies here: after the first native candidate, prompted
    /// candidates drop out.
    #[must_use]
    pub fn new(candidates: Vec<JudgeCandidate>) -> Self {
        ChainJudge::with_rejections(candidates, Arc::new(Mutex::new(HashMap::new())))
    }

    /// Build the chain over the process-shared rejection ledger.
    #[must_use]
    pub fn new_shared_rejections(candidates: Vec<JudgeCandidate>) -> Self {
        ChainJudge::with_rejections(candidates, Arc::clone(shared_rejections()))
    }

    /// Build the chain over an explicit rejection ledger (tests seed and
    /// inspect it).
    #[must_use]
    pub fn with_rejections(
        candidates: Vec<JudgeCandidate>,
        rejections: Arc<Mutex<HashMap<String, Instant>>>,
    ) -> Self {
        ChainJudge {
            candidates: enforce_native_only(candidates),
            rejections,
        }
    }

    /// The first candidate's kind (`None` on an empty chain) - the
    /// `jev.enabled = "auto"` gate reads it.
    #[must_use]
    pub fn first_candidate_kind(&self) -> Option<JudgeKind> {
        self.candidates.first().map(|candidate| candidate.kind)
    }

    /// Whether the chain starts native.
    #[must_use]
    pub fn starts_native(&self) -> bool {
        self.first_candidate_kind() == Some(JudgeKind::Native)
    }

    /// The candidates' identities, in attempt order (diagnostics).
    #[must_use]
    pub fn identities(&self) -> Vec<&str> {
        self.candidates
            .iter()
            .map(|candidate| candidate.identity.as_str())
            .collect()
    }
}

/// From the first native candidate on, only native candidates remain.
fn enforce_native_only(candidates: Vec<JudgeCandidate>) -> Vec<JudgeCandidate> {
    let Some(first_native) = candidates
        .iter()
        .position(|candidate| candidate.kind == JudgeKind::Native)
    else {
        return candidates;
    };
    candidates
        .into_iter()
        .enumerate()
        .filter(|(index, candidate)| *index < first_native || candidate.kind == JudgeKind::Native)
        .map(|(_, candidate)| candidate)
        .collect()
}

/// Whether `identity` is inside its rejection cooldown (an expired entry
/// clears on read).
fn is_cooled_down(rejections: &Mutex<HashMap<String, Instant>>, identity: &str) -> bool {
    let Ok(mut guard) = rejections.lock() else {
        return false;
    };
    match guard.get(identity).copied() {
        Some(until) if until > Instant::now() => true,
        Some(_) => {
            guard.remove(identity);
            false
        }
        None => false,
    }
}

/// Enter `identity` into its rejection cooldown.
fn record_rejection(rejections: &Mutex<HashMap<String, Instant>>, identity: &str) {
    if let Ok(mut guard) = rejections.lock() {
        guard.insert(
            identity.to_string(),
            Instant::now() + CANDIDATE_REJECTION_COOLDOWN,
        );
    }
}

#[async_trait::async_trait]
impl Judge for ChainJudge {
    fn label(&self) -> &'static str {
        "judge chain"
    }

    async fn judge(
        &self,
        request: &pa_jev::JudgmentRequest,
    ) -> Result<pa_jev::JudgmentResult, JudgeError> {
        let mut last_failure: Option<String> = None;
        let mut last_unavailable: Option<String> = None;
        for candidate in &self.candidates {
            let identity = candidate.identity.as_str();
            if is_cooled_down(&self.rejections, identity) {
                last_unavailable = Some(format!("{identity} rejected the account recently"));
                continue;
            }
            let Some(judge) = &candidate.judge else {
                last_unavailable = Some(format!("no API key for {identity}"));
                continue;
            };
            match judge.judge(request).await {
                Ok(result) => return Ok(result),
                Err(error) => {
                    // Aborts and timeouts rethrow: a cancellation (or a slow
                    // endpoint) must not spend the next candidate.
                    if matches!(error, JudgeError::Aborted | JudgeError::Timeout { .. }) {
                        return Err(error);
                    }
                    if error.is_account_rejection() {
                        record_rejection(&self.rejections, identity);
                    }
                    tracing::warn!(
                        candidate = identity,
                        error = %error,
                        "jev judge candidate failed"
                    );
                    last_failure = Some(error.to_string());
                }
            }
        }
        Err(JudgeError::AllCandidatesFailed {
            last_failure,
            last_unavailable,
        })
    }
}
