//! [FORK-ONLY] The model router: one pure, forward-looking ordering of the
//! per-turn candidate ladder.
//!
//! The turn path already has a ladder — [`crate::models::failover_candidates`]
//! names the other providers serving the same model id, and the retry driver
//! walks it in catalog order. That ladder is *reactive*: it only moves after a
//! provider errored, and it can never reorder itself around a provider that is
//! spent, slow, or out of context room. This module supplies the missing
//! forward decision, and nothing else:
//!
//! * it is **pure** — no I/O, no clock, no globals, no randomness — so a
//!   decision replays exactly from its inputs and can be asserted in a unit
//!   test (the same contract `provider_failover` keeps);
//! * it **only reorders** — the caller still drives the retry loop, still owns
//!   the switch and the restore, and every hard filter runs before a score is
//!   computed, so no score can resurrect a candidate the filters rejected;
//! * it is **behind a level gate** (`routing.mode`): `off` never calls in,
//!   `dry-run` records the decision and applies nothing, `auto` applies it only
//!   when a challenger clears the hysteresis margin and the dwell floor, and
//!   `on` always orders by score.
//!
//! The score is deliberately small and legible: seven terms in `[0, 1]`, an
//! unknown term at the neutral `0.5`, and one weighted sum. Anti-thrash lives
//! in [`decide`]'s hysteresis, not in the weights — a switch costs the prompt
//! cache, and a session that reorders on noise pays that cost every turn.

use pa_types::ai::Model;

use crate::settings::ModelRoutingLevel;

/// The hysteresis margin: how much a challenger must beat the incumbent by
/// before a *soft* preference (cheaper, faster, roomier) may move the turn.
///
/// 0.05 is one twentieth of the score range — larger than the drift a single
/// noisy latency sample produces, small enough that a genuinely better
/// candidate (half the cost, twice the headroom) still wins. A hard failure
/// (breaker open, quota spent, context overflow) never consults it: [`decide`]
/// takes the best survivor at once when the incumbent was filtered out.
pub const HYSTERESIS_MARGIN: f32 = 0.05;

/// The dwell floor: the incumbent must have served at least this many turns
/// since the last switch before a soft preference may move it again.
pub const MIN_DWELL_TURNS: u32 = 2;

/// The score of a term nobody measured: neutral, so a missing fact neither
/// promotes nor demotes a candidate.
pub const UNKNOWN_TERM: f32 = 0.5;

/// The input price at which the cost term scores `0.5`, $/million tokens.
///
/// The term is an absolute, saturating mapping (`reference / (reference + value)`)
/// rather than a min-max fit over the candidate set. Range-fitting looks
/// attractive but it destroys the margin: normalizing to the set turns a
/// one-cent price difference into the full `0.20` of the cost weight, so two
/// near-identical providers would trade the turn forever and the hysteresis
/// could never hold. An absolute scale keeps closely-matched candidates close,
/// which is exactly what the margin is calibrated for.
pub const COST_REFERENCE_USD_PER_1M: f64 = 3.0;

/// The time-to-first-token at which the latency term scores `0.5`, in
/// milliseconds (the same saturating shape as the cost term).
pub const LATENCY_REFERENCE_MS: f64 = 2_000.0;

// The weight set. Sum = 1.00, so every score is in `[0, 1]` and a raw score
// compares directly against the margin.
//
// They encode one judgment: capability fit and quality dominate, cost and
// latency are tie-breakers, and pressure (quota headroom, idle capacity)
// rounds it out. Cache affinity is not a term of its own — it enters through
// the hysteresis, which is what actually prices a switch.
const WEIGHT_FIT: f32 = 0.30;
const WEIGHT_QUALITY: f32 = 0.20;
const WEIGHT_COST: f32 = 0.20;
const WEIGHT_LATENCY: f32 = 0.10;
const WEIGHT_HEADROOM: f32 = 0.10;
const WEIGHT_IDLE: f32 = 0.05;
const WEIGHT_AFFINITY: f32 = 0.05;

/// The stable spelling of one term name (log lines and the decision's `scores`
/// detail share it).
pub const TERM_FIT: &str = "fit";
pub const TERM_QUALITY: &str = "quality";
pub const TERM_COST: &str = "cost";
pub const TERM_LATENCY: &str = "latency";
pub const TERM_HEADROOM: &str = "headroom";
pub const TERM_IDLE: &str = "idle";
pub const TERM_AFFINITY: &str = "affinity";

/// Why one candidate was rejected before any score was computed.
///
/// A rejection is a fact, never a preference: the caller may surface it, and a
/// turn that ends with every candidate rejected reports them instead of a
/// generic failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteRejection {
    /// The context window cannot hold the request plus its output budget,
    /// whatever the score says.
    ContextTooSmall,
    /// The candidate cannot serve an attachment the turn carries.
    Modality,
    /// The provider's admission breaker is open (traffic is failing there now).
    BreakerOpen,
    /// The provider's quota window is spent until its reported reset.
    QuotaExhausted,
}

impl RouteRejection {
    /// The stable wire spelling (the `router_decision` telemetry vocabulary).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContextTooSmall => "context_too_small",
            Self::Modality => "modality",
            Self::BreakerOpen => "breaker_open",
            Self::QuotaExhausted => "quota_exhausted",
        }
    }

    /// Every rejection in the fixed vocabulary (the settings-source test
    /// checks the telemetry enum against it).
    #[must_use]
    pub fn all() -> [RouteRejection; 4] {
        [
            Self::ContextTooSmall,
            Self::Modality,
            Self::BreakerOpen,
            Self::QuotaExhausted,
        ]
    }
}

/// One candidate the router may choose, with the facts the decision reads.
///
/// Every `Option` is a fact the caller may or may not have; `None` scores as
/// [`UNKNOWN_TERM`] rather than as a good or a bad value, so a fact-free
/// candidate is never silently preferred.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteCandidate {
    /// The `provider/model-id` selector this candidate serves.
    pub selector: String,
    /// The model's catalog id (the same id across providers in a failover
    /// ladder; a different one for a capability-equivalent challenger).
    pub model_id: String,
    /// The provider the candidate would charge.
    pub provider: String,
    /// The model's context window in tokens.
    pub context_window: u64,
    /// Whether the model accepts image input.
    pub accepts_images: bool,
    /// Input price, $/million tokens (`None` when the catalog has no price).
    pub cost_per_1m_input: Option<f64>,
    /// Observed time-to-first-token, milliseconds (`None` before a sample).
    pub latency_ema_ms: Option<f64>,
    /// The admission registry's breaker state: `true` rejects the candidate at
    /// once (a hard filter, no score).
    pub breaker_open: bool,
    /// Fraction of the provider's quota still available, `0.0..=1.0`
    /// (`None` when the provider reports nothing, e.g. an unmetered key).
    pub quota_remaining: Option<f64>,
    /// The admission registry's free capacity fraction, `0.0..=1.0`
    /// (`1 - in_flight/limit`; `None` when the plane is unattached).
    pub free_capacity: Option<f32>,
    /// The capability-profile match for this turn's class in `[0, 1]`
    /// (`None` while no profile covers the model).
    pub capability_fit: Option<f32>,
    /// The quality prior for this turn's class in `[0, 1]` (`None` until a
    /// calibration exists).
    pub quality: Option<f32>,
}

/// The facts about THIS turn that the filters and the score need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteContext {
    /// The request's estimated input tokens (the live context).
    pub needed_tokens: u64,
    /// The output budget the request reserves on top of the input.
    pub output_tokens: u64,
    /// Whether the turn carries image blocks.
    pub carries_images: bool,
    /// The `provider/model-id` serving the session right now, if any: the
    /// incumbent the hysteresis protects.
    pub current: Option<String>,
    /// How many turns the incumbent has served since the last switch.
    pub dwell_turns: u32,
}

impl Default for RouteContext {
    fn default() -> Self {
        Self {
            needed_tokens: 0,
            output_tokens: 0,
            carries_images: false,
            current: None,
            dwell_turns: MIN_DWELL_TURNS,
        }
    }
}

/// One candidate's scored row.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredCandidate {
    pub selector: String,
    /// The weighted score; `0.0` for a rejected candidate (it is not scored).
    pub score: f32,
    /// The hard filters it failed (`empty` = it survived).
    pub rejections: Vec<RouteRejection>,
    /// The per-term values, in [`TERM_*`] order, for the diagnostic line.
    pub terms: Vec<(&'static str, f32)>,
}

impl ScoredCandidate {
    /// Whether the candidate cleared every hard filter.
    #[must_use]
    pub fn survives(&self) -> bool {
        self.rejections.is_empty()
    }
}

/// The router's answer for one turn.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteDecision {
    /// The candidate the router would serve this turn (empty when every
    /// candidate was rejected).
    pub chosen: String,
    /// The next-best survivor, when there is one.
    pub runner_up: Option<String>,
    /// Why `chosen` won (a fixed, loggable phrase).
    pub reason: RouteReason,
    /// Every candidate, survivors first (best score first) and then the
    /// rejected ones; a rejected row keeps its rejection reasons.
    pub candidates: Vec<ScoredCandidate>,
}

impl RouteDecision {
    /// The chosen row's score (`None` when every candidate was rejected).
    #[must_use]
    pub fn chosen_score(&self) -> Option<f32> {
        self.candidates
            .iter()
            .find(|candidate| candidate.selector == self.chosen)
            .map(|candidate| candidate.score)
    }

    /// The runner-up's score, when there is one.
    #[must_use]
    pub fn runner_up_score(&self) -> Option<f32> {
        let runner_up = self.runner_up.as_deref()?;
        self.candidates
            .iter()
            .find(|candidate| candidate.selector == runner_up)
            .map(|candidate| candidate.score)
    }

    /// The ordered survivor selectors: the ladder the retry driver should walk.
    /// The chosen candidate leads; every other survivor follows in score order.
    #[must_use]
    pub fn ladder(&self) -> Vec<&str> {
        self.candidates
            .iter()
            .filter(|candidate| candidate.survives())
            .map(|candidate| candidate.selector.as_str())
            .collect()
    }
}

/// Why the router picked what it picked. The vocabulary is closed so the
/// telemetry property stays a fixed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteReason {
    /// The incumbent stays: a healthy, fit model is not displaced for a soft
    /// preference (the cache loss would exceed the routing gain).
    IncumbentRetained,
    /// A challenger beat the incumbent by more than the margin.
    ChallengerWon,
    /// The incumbent was hard-failed (filtered out), so the best survivor
    /// serves at once.
    IncumbentFailed,
    /// No incumbent was named: the best survivor serves.
    NoIncumbent,
    /// Every candidate was rejected.
    AllRejected,
    /// The router is off: the pinned model serves and nothing was scored.
    Disabled,
}

impl RouteReason {
    /// The stable wire spelling (the `router_decision` telemetry vocabulary).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IncumbentRetained => "incumbent_retained",
            Self::ChallengerWon => "challenger_won",
            Self::IncumbentFailed => "incumbent_failed",
            Self::NoIncumbent => "no_incumbent",
            Self::AllRejected => "all_rejected",
            Self::Disabled => "disabled",
        }
    }

    /// Every reason in the fixed vocabulary.
    #[must_use]
    pub fn all() -> [RouteReason; 6] {
        [
            Self::IncumbentRetained,
            Self::ChallengerWon,
            Self::IncumbentFailed,
            Self::NoIncumbent,
            Self::AllRejected,
            Self::Disabled,
        ]
    }
}

/// `[0, 1]` from an optional `[0, 1]` fact; `None` is neutral.
#[must_use]
fn unit_term(value: Option<f32>) -> f32 {
    value.map_or(UNKNOWN_TERM, |value| value.clamp(0.0, 1.0))
}

/// A "lower is better" term on an absolute, saturating scale: `reference /
/// (reference + value)`, so `0` scores `1.0`, the reference scores `0.5`, and a
/// very large value approaches `0.0` without ever leaving the range.
///
/// An unmeasured value (`None`, or a negative/non-finite sample) scores
/// [`UNKNOWN_TERM`]: a missing fact is neutral, never free.
#[must_use]
fn lower_is_better_term(value: Option<f64>, reference: f64) -> f32 {
    match value {
        Some(value) if value.is_finite() && value >= 0.0 => {
            (reference / (reference + value)) as f32
        }
        _ => UNKNOWN_TERM,
    }
}

/// The hard filters: a rejection here is decisive, and no score rescues the
/// candidate. `None` means the candidate survives.
#[must_use]
fn filter(candidate: &RouteCandidate, ctx: &RouteContext) -> Option<RouteRejection> {
    // The window must hold the input AND the output budget the request
    // reserves: a model that fits the prompt but not its own answer fails the
    // very first call, so it is never a candidate.
    let needed = ctx.needed_tokens.saturating_add(ctx.output_tokens);
    if candidate.context_window == 0 || needed > candidate.context_window {
        return Some(RouteRejection::ContextTooSmall);
    }
    if ctx.carries_images && !candidate.accepts_images {
        return Some(RouteRejection::Modality);
    }
    // An open breaker means traffic is failing there right now: routing to it
    // burns the turn.
    if candidate.breaker_open {
        return Some(RouteRejection::BreakerOpen);
    }
    // A spent quota is a hard stop until its reported reset: unlike a plain
    // rate limit (which a short wait clears), retrying a spent window spends
    // the turn's retry budget for nothing.
    if candidate
        .quota_remaining
        .is_some_and(|remaining| remaining <= 0.0)
    {
        return Some(RouteRejection::QuotaExhausted);
    }
    None
}

/// Whether `level` may move the serving target off a healthy incumbent. `auto`
/// demands the margin and the dwell floor; `on` trusts the score; `dry-run` and
/// `off` never move.
#[must_use]
fn may_displace_incumbent(level: ModelRoutingLevel, leads_by: f32, dwell_turns: u32) -> bool {
    match level {
        ModelRoutingLevel::On => true,
        ModelRoutingLevel::Auto => leads_by > HYSTERESIS_MARGIN && dwell_turns >= MIN_DWELL_TURNS,
        ModelRoutingLevel::DryRun | ModelRoutingLevel::Off => false,
    }
}

/// Decide which candidate serves this turn.
///
/// Hard filters first, then a weighted score, then hysteresis. `None` when the
/// candidate set is empty (the caller keeps its own order). `Some` with
/// [`RouteReason::AllRejected`] when every candidate failed a hard filter: the
/// caller then keeps its own order and lets the retry driver fail the turn with
/// the rejections on the record.
///
/// The hysteresis rule, in order:
/// 1. `off` ⇒ the incumbent is reported unchanged and nothing is scored;
/// 2. no incumbent named ⇒ the best survivor serves ([`RouteReason::NoIncumbent`]);
/// 3. the incumbent was hard-filtered ⇒ the best survivor serves at once
///    ([`RouteReason::IncumbentFailed`]);
/// 4. the incumbent is the best survivor ⇒ it stays
///    ([`RouteReason::IncumbentRetained`]);
/// 5. a challenger leads and [`may_displace_incumbent`] allows the move ⇒ the
///    challenger serves ([`RouteReason::ChallengerWon`]);
/// 6. otherwise the incumbent stays: a marginal lead is noise, and the prompt
///    cache costs more than the lead is worth.
#[must_use]
pub fn decide(
    candidates: &[RouteCandidate],
    ctx: &RouteContext,
    level: ModelRoutingLevel,
) -> Option<RouteDecision> {
    if candidates.is_empty() {
        return None;
    }
    // `off` never consults a score: the pinned model serves, and the caller's
    // own ladder (catalog order) keeps failover identical to the pre-router
    // behaviour.
    if !level.is_enabled() {
        let current = ctx.current.clone().unwrap_or_else(|| {
            candidates
                .first()
                .map(|candidate| candidate.selector.clone())
                .unwrap_or_default()
        });
        return Some(RouteDecision {
            chosen: current,
            runner_up: None,
            reason: RouteReason::Disabled,
            candidates: candidates
                .iter()
                .map(|candidate| ScoredCandidate {
                    selector: candidate.selector.clone(),
                    score: 0.0,
                    rejections: Vec::new(),
                    terms: Vec::new(),
                })
                .collect(),
        });
    }
    let incumbent_provider = ctx.current.as_deref().and_then(|current| {
        candidates
            .iter()
            .find(|candidate| candidate.selector == current)
            .map(|candidate| candidate.provider.as_str())
    });

    let mut scored: Vec<ScoredCandidate> = candidates
        .iter()
        .map(|candidate| {
            let affinity = match incumbent_provider {
                // The incumbent's own provider keeps the prompt cache warm; a
                // sibling provider serving the same model is a colder cache; a
                // foreign model a different one entirely.
                Some(provider) if provider == candidate.provider => 1.0,
                Some(_) => 0.0,
                None => UNKNOWN_TERM,
            };
            let terms = vec![
                (TERM_FIT, unit_term(candidate.capability_fit)),
                (TERM_QUALITY, unit_term(candidate.quality)),
                (
                    TERM_COST,
                    lower_is_better_term(candidate.cost_per_1m_input, COST_REFERENCE_USD_PER_1M),
                ),
                (
                    TERM_LATENCY,
                    lower_is_better_term(candidate.latency_ema_ms, LATENCY_REFERENCE_MS),
                ),
                (
                    TERM_HEADROOM,
                    unit_term(candidate.quota_remaining.map(|remaining| remaining as f32)),
                ),
                (TERM_IDLE, unit_term(candidate.free_capacity)),
                (TERM_AFFINITY, affinity),
            ];
            let rejection = filter(candidate, ctx);
            // A rejected candidate is not scored: printing a score beside a
            // hard rejection invites a reader to think it mattered.
            let score = match rejection {
                Some(_) => 0.0,
                None => {
                    WEIGHT_FIT * terms[0].1
                        + WEIGHT_QUALITY * terms[1].1
                        + WEIGHT_COST * terms[2].1
                        + WEIGHT_LATENCY * terms[3].1
                        + WEIGHT_HEADROOM * terms[4].1
                        + WEIGHT_IDLE * terms[5].1
                        + WEIGHT_AFFINITY * terms[6].1
                }
            };
            ScoredCandidate {
                selector: candidate.selector.clone(),
                score,
                rejections: rejection.into_iter().collect(),
                terms,
            }
        })
        .collect();
    // Survivors first, best score first. `sort_by` is stable, so among equal
    // scores the input order decides: the same facts always replay the same
    // decision (the property the whole module exists to keep).
    scored.sort_by(|left, right| {
        right
            .survives()
            .cmp(&left.survives())
            .then_with(|| right.score.total_cmp(&left.score))
    });
    let survivors: Vec<&ScoredCandidate> = scored
        .iter()
        .filter(|candidate| candidate.survives())
        .collect();
    let Some(best) = survivors.first() else {
        return Some(RouteDecision {
            chosen: String::new(),
            runner_up: None,
            reason: RouteReason::AllRejected,
            candidates: scored,
        });
    };
    let Some(current) = ctx.current.as_deref() else {
        return Some(RouteDecision {
            chosen: best.selector.clone(),
            // The next-best survivor that is not the one just chosen.
            runner_up: survivors
                .iter()
                .find(|candidate| candidate.selector != best.selector)
                .map(|candidate| candidate.selector.clone()),
            reason: RouteReason::NoIncumbent,
            candidates: scored,
        });
    };
    let Some(incumbent) = survivors
        .iter()
        .find(|candidate| candidate.selector == current)
    else {
        return Some(RouteDecision {
            chosen: best.selector.clone(),
            runner_up: survivors
                .iter()
                .find(|candidate| candidate.selector != best.selector)
                .map(|candidate| candidate.selector.clone()),
            reason: RouteReason::IncumbentFailed,
            candidates: scored,
        });
    };
    let incumbent_score = incumbent.score;
    // The runner-up is always the best survivor other than the chosen row: a
    // reader must never see the same selector on both sides.
    let runner_up_of = |chosen: &str| {
        survivors
            .iter()
            .find(|candidate| candidate.selector != chosen)
            .map(|candidate| candidate.selector.clone())
    };
    if best.selector == current
        || !may_displace_incumbent(level, best.score - incumbent_score, ctx.dwell_turns)
    {
        return Some(RouteDecision {
            chosen: current.to_string(),
            runner_up: runner_up_of(current),
            reason: RouteReason::IncumbentRetained,
            candidates: scored,
        });
    }
    Some(RouteDecision {
        chosen: best.selector.clone(),
        runner_up: runner_up_of(&best.selector),
        reason: RouteReason::ChallengerWon,
        candidates: scored,
    })
}

/// Build a candidate from a resolved catalog model, so every call site maps the
/// same facts the same way. Health facts (breaker, quota, latency, capacity)
/// stay at their neutral `None` here and are filled by the caller that owns
/// them.
#[must_use]
pub fn candidate_from_model(model: &Model) -> RouteCandidate {
    RouteCandidate {
        selector: format!("{}/{}", model.provider, model.id),
        model_id: model.id.clone(),
        provider: model.provider.clone(),
        context_window: model.context_window,
        accepts_images: model.input.contains(&pa_types::ai::ModelInput::Image),
        // The catalog quotes $/million already (`ModelCost` is documented that
        // way), so no scaling is needed.
        cost_per_1m_input: Some(model.cost.input.0),
        latency_ema_ms: None,
        breaker_open: false,
        quota_remaining: None,
        free_capacity: None,
        capability_fit: None,
        quality: None,
    }
}

#[cfg(test)]
mod tests;
