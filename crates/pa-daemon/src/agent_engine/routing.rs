//! [FORK-ONLY] The model router at the daemon's turn boundary: build the
//! candidate set from the live session facts, ask
//! [`pa_core::session_engine::routing::decide`], record the decision, and — when
//! the level applies it — reorder the provider ladder the retry driver walks.
//!
//! **The seam is the ordering of `failover_candidates`, and nothing else.**
//! There is no second retry loop, no new failure path, and no live model switch
//! at the turn boundary. That restraint is the design, not an omission:
//!
//! * a live switch would have to re-derive `overflow_window` (computed once from
//!   the serving model and never recomputed), add a handoff note without
//!   breaking the prompt-cache prefix, and give the restore a contract; the
//!   research report (§6.4) names each of those, and getting one wrong costs
//!   more than the routing gains;
//! * the ladder holds the *same model id* on other providers, so a reorder
//!   cannot move the session to a model it was never pinned to — the operator's
//!   pin stays a ceiling;
//! * ordering the ladder is exactly what the retry driver already walks, so a
//!   reorder can only reorder, never introduce a candidate that catalog-order
//!   failover did not already have (the context-window gap the research notes
//!   is pre-existing, and the hard filter below only ever removes rows).
//!
//! What the operator gets from that: a spent, broken, or out-of-room provider is
//! skipped instead of being walked into, and a healthy incumbent is never
//! displaced for a soft preference (the hysteresis).

use pa_core::session_engine::routing::{
    decide, RouteCandidate, RouteContext, RouteDecision, RouteReason,
};
use pa_core::settings::ModelRoutingLevel;
use pa_types::ai::Model;

use crate::agent_engine::provider_admission::{AdmissionClient, BreakerState, ProviderKey};
use crate::agent_engine::AgentSessionEngine;

/// The router's per-session memory: the incumbent's dwell.
///
/// The state is per SESSION on purpose: two sessions on one account may be
/// pinned to different providers, and neither may move the other. The dwell is
/// the anti-thrash clock the `auto` level reads: it resets whenever a decision
/// moves the ladder off the serving model, so a move is always followed by at
/// least [`pa_core::session_engine::routing::MIN_DWELL_TURNS`] settled turns.
#[derive(Debug, Default)]
pub(crate) struct RouterState {
    /// Turns served since the incumbent last changed.
    pub(crate) dwell_turns: u32,
    /// Whether the incumbent has served a turn yet (see
    /// [`AgentSessionEngine::route_turn_candidates`]: the dwell counts turns
    /// SERVED, so the session's first boundary observes none).
    pub(crate) served_turn: bool,
}

/// The turn's routing inputs the boundary owns: what the request needs, and
/// what it carries.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurnRouteFacts {
    pub(crate) needed_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) carries_images: bool,
}

/// The health facts the admission registry contributes to a score.
#[derive(Debug, Clone, Copy)]
struct AdmissionFacts {
    breaker_open: bool,
    free_capacity: f32,
    /// The graded failure rate over the retry-budget window (`None` before the
    /// registry has enough samples to mean anything).
    error_rate: Option<f64>,
}

/// The router's telemetry seam: one lazily built client per worker, the
/// [`crate::model_allowlist::ModelRefusalTelemetry`] pattern. The client binds
/// to the session's cwd (the settings posture is scoped), so a session that
/// moved directories rebinds instead of reporting through the old project.
///
/// The client is kept for the worker's lifetime: the sinks batch
/// asynchronously, so building one per turn would drop every earlier event.
#[derive(Default)]
pub(crate) struct RouterTelemetry {
    client: std::sync::Mutex<Option<(std::path::PathBuf, pa_telemetry::TelemetryClient)>>,
}

impl std::fmt::Debug for RouterTelemetry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RouterTelemetry")
    }
}

impl RouterTelemetry {
    /// Emit one `router_decision` (best-effort; no-op when telemetry is off).
    pub(crate) fn note_decision(&self, facts: &RouterDecisionFacts<'_>) {
        let RouterDecisionFacts {
            decision,
            serving,
            level,
            switched,
            dwell_turns,
            cwd,
            agent_dir,
        } = *facts;
        let settings = pa_core::settings::SettingsManager::create(cwd, agent_dir);
        // The same gating as the other turn-path seams: the env override wins,
        // else the merged settings switch. Asked per decision (a settings edit
        // takes effect without a restart) and it is a cached file read.
        if !pa_core::session_engine::telemetry::telemetry_switch(&settings).enabled() {
            return;
        }
        let mut slot = self
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_ref().is_none_or(|(bound_cwd, _)| bound_cwd != cwd) {
            let client = pa_core::session_engine::telemetry::build_client(&settings, agent_dir);
            *slot = Some((cwd.to_path_buf(), client));
        }
        let client = &slot.as_ref().expect("client bound").1;
        let chosen_model_category = decision
            .chosen
            .split_once('/')
            .map_or(decision.chosen.as_str(), |(_, model_id)| model_id);
        pa_telemetry::RouterDecision {
            level: level.as_str(),
            reason: decision.reason.as_str(),
            incumbent_model_category: pa_core::session_engine::telemetry::model_category(
                &serving.id,
            ),
            chosen_model_category: pa_core::session_engine::telemetry::model_category(
                chosen_model_category,
            ),
            candidate_count: u64::try_from(decision.candidates.len()).unwrap_or(u64::MAX),
            rejected_count: u64::try_from(
                decision
                    .candidates
                    .iter()
                    .filter(|candidate| !candidate.survives())
                    .count(),
            )
            .unwrap_or(u64::MAX),
            chosen_score_milli: decision.chosen_score().map(score_milli),
            runner_up_score_milli: decision.runner_up_score().map(score_milli),
            switched,
            dwell_turns: u64::from(dwell_turns),
        }
        .track(client);
    }
}

/// Everything one `router_decision` emission needs; a struct because the
/// signature would otherwise be eight positional arguments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RouterDecisionFacts<'a> {
    pub(crate) decision: &'a RouteDecision,
    pub(crate) serving: &'a Model,
    pub(crate) level: ModelRoutingLevel,
    pub(crate) switched: bool,
    pub(crate) dwell_turns: u32,
    pub(crate) cwd: &'a std::path::Path,
    pub(crate) agent_dir: &'a std::path::Path,
}

/// A score in thousandths of a point, the integer form the event carries.
fn score_milli(score: f32) -> u64 {
    let clamped = score.clamp(0.0, 1.0);
    (clamped * 1000.0).round() as u64
}

impl AgentSessionEngine {
    /// The router's level from settings (`routing.mode`; unset reads as `off`).
    pub(super) fn routing_level(&self) -> ModelRoutingLevel {
        pa_core::settings::SettingsManager::create(self.cwd(), &self.config.agent_dir)
            .get_routing_level()
    }

    /// The admission registry's health facts for one `(provider, model)`, when a
    /// local plane is attached. A worker's plane lives in the supervisor, and a
    /// turn-boundary round trip to it would put a socket on the turn path for a
    /// tie-breaking term, so a worker reports no fact here; the breaker is still
    /// enforced where it matters, at the stream acquire.
    fn admission_facts(&self, model: &Model) -> Option<AdmissionFacts> {
        let AdmissionClient::Local(registry) = &self.config.admission else {
            return None;
        };
        let snapshot =
            registry.snapshot(&ProviderKey::new(model.provider.clone(), model.id.clone()));
        if snapshot.limit == 0 {
            return None;
        }
        let in_flight = u32::try_from(snapshot.in_flight).unwrap_or(u32::MAX);
        let limit = u32::try_from(snapshot.limit).unwrap_or(u32::MAX);
        let free = 1.0 - (in_flight as f32 / limit as f32);
        // The graded twin of the breaker: the same window the breaker trips
        // on, read as a rate instead of a verdict. Below the window's minimum
        // sample count the rate is noise, so it stays unmeasured (`None`)
        // rather than reporting a 1-in-2 failure rate as if it meant
        // something.
        let error_rate = (snapshot.breaker.budget_samples
            >= crate::agent_engine::provider_admission::RETRY_BUDGET_MIN_SAMPLES)
            .then(|| {
                snapshot.breaker.budget_failures as f64 / snapshot.breaker.budget_samples as f64
            });
        Some(AdmissionFacts {
            breaker_open: snapshot.breaker.state == BreakerState::Open,
            free_capacity: free.clamp(0.0, 1.0),
            error_rate,
        })
    }

    /// The candidate rows for one decision: the serving model plus the ladder
    /// the failover driver would walk, each with the health facts the local
    /// admission registry knows.
    fn route_candidates(&self, serving: &Model, ladder: &[Model]) -> Vec<RouteCandidate> {
        std::iter::once(serving)
            .chain(ladder.iter())
            .map(|model| {
                let mut candidate = pa_core::session_engine::routing::candidate_from_model(model);
                if let Some(facts) = self.admission_facts(model) {
                    candidate.breaker_open = facts.breaker_open;
                    candidate.free_capacity = Some(facts.free_capacity);
                    candidate.error_rate = facts.error_rate;
                }
                candidate
            })
            .collect()
    }

    /// Ask the router for this turn's ordering, record the decision, and return
    /// the ladder to walk.
    ///
    /// `ladder` is the same-model failover ladder in catalog order (the retry
    /// driver's own contract: candidates *after* the serving model). The
    /// returned vector keeps that contract; when the level applies nothing it is
    /// the input vector unchanged, byte for byte.
    pub(super) fn route_turn_candidates(
        &self,
        serving: &Model,
        ladder: Vec<Model>,
        facts: TurnRouteFacts,
    ) -> Vec<Model> {
        let level = self.routing_level();
        if !level.is_enabled() {
            return ladder;
        }
        let candidates = self.route_candidates(serving, &ladder);
        let serving_selector = format!("{}/{}", serving.provider, serving.id);
        let decision = {
            let mut state = self
                .router_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The dwell counts turns served by the incumbent. At a turn
            // boundary the previous turn has settled, so this boundary
            // observes it; the session's first boundary observes none.
            if state.served_turn {
                state.dwell_turns = state.dwell_turns.saturating_add(1);
            }
            state.served_turn = true;
            let context = RouteContext {
                needed_tokens: facts.needed_tokens,
                output_tokens: facts.output_tokens,
                carries_images: facts.carries_images,
                current: Some(serving_selector.clone()),
                dwell_turns: state.dwell_turns,
            };
            let decision = decide(&candidates, &context, level);
            if let Some(decision) = &decision {
                // Only `auto` gates on the dwell, so only `auto` resets it: a
                // move there must be followed by the floor's settled turns
                // before the next soft preference may move again. Under `on`
                // the score decides every turn by definition, and counting a
                // clock nothing reads would be bookkeeping for its own sake.
                if level == ModelRoutingLevel::Auto && decision.chosen != serving_selector {
                    state.dwell_turns = 0;
                }
            }
            decision
        };
        let Some(decision) = decision else {
            return ladder;
        };
        self.track_route_decision(&decision, serving, level);
        if !level.applies() {
            // `dry-run` records the decision and applies nothing: the ladder
            // stays exactly as the caller built it.
            return ladder;
        }
        apply_route_ladder(serving, ladder, &decision)
    }

    /// Record the decision: one log line always (it costs nothing and it is
    /// what an operator reads when the ladder surprises them), and one
    /// `router_decision` telemetry event when the session records telemetry.
    ///
    /// Fire-and-forget by construction: the client's `track` only queues, so
    /// the turn path never awaits a sink.
    fn track_route_decision(
        &self,
        decision: &RouteDecision,
        serving: &Model,
        level: ModelRoutingLevel,
    ) {
        let rejected = decision
            .candidates
            .iter()
            .filter(|candidate| !candidate.survives())
            .count();
        let serving_selector = format!("{}/{}", serving.provider, serving.id);
        let switched = !decision.chosen.is_empty() && decision.chosen != serving_selector;
        tracing::debug!(
            level = level.as_str(),
            reason = decision.reason.as_str(),
            chosen = %decision.chosen,
            runner_up = decision.runner_up.as_deref().unwrap_or(""),
            candidates = decision.candidates.len(),
            rejected,
            switched,
            "model router: turn decision"
        );
        // The adoption event rides the run's own telemetry client (one lazily
        // built client per worker, the model-refusal seam's pattern): the
        // router runs on the turn path, not in the supervisor, so it must not
        // build a client per turn.
        let dwell_turns = self
            .router_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .dwell_turns;
        self.router_telemetry.note_decision(&RouterDecisionFacts {
            decision,
            serving,
            level,
            switched,
            dwell_turns,
            cwd: &self.cwd(),
            agent_dir: &self.config.agent_dir,
        });
    }

    /// The turn's routing facts: the live context estimate plus the output the
    /// request reserves on the serving model, and whether the admitted prompt
    /// carries image blocks.
    pub(super) fn turn_route_facts(
        &self,
        prompt: &super::turn_types::TurnPrompt,
        serving: &Model,
    ) -> TurnRouteFacts {
        let carries_images = match prompt {
            super::turn_types::TurnPrompt::User { images, batch, .. } => {
                !images.is_empty() || batch.iter().any(|row| !row.images.is_empty())
            }
            super::turn_types::TurnPrompt::Injected(message) => {
                Self::custom_message_carries_images(message)
            }
        };
        let needed_tokens = {
            let guard = self.session.blocking_lock();
            match guard.as_deref() {
                Some(core) => {
                    let state = self.runtime.block_on(core.session.agent().state());
                    let messages: Vec<pa_types::session::AgentMessage> = state
                        .messages
                        .iter()
                        .filter_map(|message| serde_json::to_value(message).ok())
                        .filter_map(|value| serde_json::from_value(value).ok())
                        .collect();
                    pa_core::session_engine::compaction::estimate_context_tokens(&messages).tokens
                }
                // No built session: the live context is empty (the first turn
                // of a fresh session).
                None => 0,
            }
        };
        TurnRouteFacts {
            needed_tokens,
            output_tokens: pa_core::session_engine::compaction::request_output_budget(
                serving,
                self.effective_thinking(),
            ),
            carries_images,
        }
    }
}

/// Map a decision onto the retry driver's ladder.
///
/// Two behaviours, and the split is the whole point of the hysteresis:
///
/// * **the incumbent was retained** (or the router is `off`/`dry-run`): the
///   caller's catalog order stands, minus the rows a hard filter rejected. A
///   retained decision must NOT silently reorder the ladder by score — that
///   would apply exactly the churn the margin exists to prevent, just one row
///   down from the serving model.
/// * **a decision moved the turn** (a challenger won, or the incumbent was
///   hard-failed): the ladder follows score order, so the next attempt walks to
///   the best survivor rather than the catalog neighbour.
///
/// A row the caller never offered is never invented: every returned model is one
/// of the input models. [`RouteReason::AllRejected`] returns the input vector
/// unchanged, because there is no better order to offer and the retry driver
/// should fail the turn on its own terms.
#[must_use]
fn apply_route_ladder(serving: &Model, ladder: Vec<Model>, decision: &RouteDecision) -> Vec<Model> {
    let serving_selector = format!("{}/{}", serving.provider, serving.id);
    let selector = |model: &Model| format!("{}/{}", model.provider, model.id);
    let survivors: Vec<String> = decision
        .candidates
        .iter()
        .filter(|candidate| candidate.survives())
        .map(|candidate| candidate.selector.clone())
        .collect();
    match decision.reason {
        RouteReason::AllRejected => ladder,
        // The incumbent stays: keep the caller's order, drop the rejected rows.
        RouteReason::IncumbentRetained | RouteReason::Disabled | RouteReason::NoIncumbent => {
            let next: Vec<Model> = ladder
                .iter()
                .filter(|model| survivors.contains(&selector(model)))
                .cloned()
                .collect();
            if next.len() == ladder.len() {
                ladder
            } else {
                next
            }
        }
        // The turn moved: score order, the serving model excluded (the caller's
        // ladder contract), every other survivor following.
        RouteReason::ChallengerWon | RouteReason::IncumbentFailed => {
            let mut next: Vec<Model> = decision
                .ladder()
                .into_iter()
                .filter(|chosen| *chosen != serving_selector)
                .filter_map(|chosen| {
                    ladder
                        .iter()
                        .find(|model| selector(model) == chosen)
                        .cloned()
                })
                .collect();
            if next.len() == ladder.len()
                && next
                    .iter()
                    .zip(ladder.iter())
                    .all(|(left, right)| left == right)
            {
                // The router agreed with catalog order: hand back the caller's
                // vector itself.
                return ladder;
            }
            next.shrink_to_fit();
            next
        }
    }
}

#[cfg(test)]
mod engine_tests;

#[cfg(test)]
mod tests;
