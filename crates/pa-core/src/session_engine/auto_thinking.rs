//! FORK-ONLY (jev Phase 2): the auto-thinking consumer's session seam.
//!
//! The turn admission calls [`AgentSession::apply_auto_thinking`] once, right
//! before the prompt reaches the loop: the classifier reads the prompt, picks
//! a difficulty, and the session's thinking level is set for that turn. The
//! seam is deliberately thin and strictly advisory:
//!
//! - with no wiring installed (the default) nothing runs and the level the
//!   session was built with stands - the pre-jev behaviour;
//! - a classification failure (no judge, judge error, timeout) leaves the
//!   level untouched and the turn runs anyway (fail-open, ADR rule 5);
//! - a model without a reasoning surface skips classification entirely: the
//!   level would have no effect, so the spend would be pure waste;
//! - the level change goes through [`AgentSession::set_thinking_level`], the
//!   same bookkeeping `/thinking` uses, so the transcript records what the
//!   turn actually ran with.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use pa_agent::types::ThinkingLevel;

use crate::jev::auto_thinking::{Difficulty, DifficultyAnswer, DifficultyClassifier};
use crate::session_engine::AgentSession;

/// Consecutive judge failures after which the consumer suspends itself for
/// the rest of the session: an advisory surface must not keep paying a
/// timeout on every turn once the endpoint has proven unreliable (the
/// "removable" rule - a broken optional dependency degrades to the legacy
/// path, it does not tax it).
pub const MAX_CONSECUTIVE_FAILURES: u32 = 2;

/// What the admission path needs to classify one turn.
#[derive(Clone)]
pub struct AutoThinkingWiring {
    classifier: Arc<DifficultyClassifier>,
    /// The level the session returns to when the judge declines (`unsure`),
    /// fails, or the model has no reasoning surface.
    fallback: ThinkingLevel,
    /// Consecutive failures; at [`MAX_CONSECUTIVE_FAILURES`] the consumer
    /// suspends. Shared by every clone (one counter per session).
    failures: Arc<AtomicU32>,
}

impl std::fmt::Debug for AutoThinkingWiring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AutoThinkingWiring")
            .field("fallback", &self.fallback)
            .finish_non_exhaustive()
    }
}

impl AutoThinkingWiring {
    /// Build the wiring over a resolved classifier.
    #[must_use]
    pub fn new(classifier: Arc<DifficultyClassifier>, fallback: ThinkingLevel) -> Self {
        AutoThinkingWiring {
            classifier,
            fallback,
            failures: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Whether the consumer suspended itself after repeated judge failures.
    #[must_use]
    pub fn is_suspended(&self) -> bool {
        self.failures.load(Ordering::Relaxed) >= MAX_CONSECUTIVE_FAILURES
    }

    /// The consecutive-failure count (diagnostics + tests).
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.failures.load(Ordering::Relaxed)
    }

    /// The deterministic level: what the session uses when nothing better is
    /// known (and what a declined classification resolves to).
    #[must_use]
    pub fn fallback(&self) -> ThinkingLevel {
        self.fallback
    }

    /// The level a difficulty maps to.
    #[must_use]
    pub fn level_for(difficulty: Difficulty) -> ThinkingLevel {
        difficulty.thinking_level()
    }
}

/// The outcome of one admission-time classification (diagnostics + tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoThinkingOutcome {
    /// No wiring installed: the session level stands.
    NotConfigured,
    /// The model has no reasoning surface: classification was skipped.
    ModelWithoutReasoning,
    /// The judge answered; the level was set from it.
    Classified(Difficulty),
    /// The judge declined (`unsure`): the deterministic fallback level.
    Declined,
    /// The judge failed: the fallback level, and the turn ran anyway.
    Failed,
    /// Too many consecutive failures: the consumer stopped asking for the
    /// rest of the session (the fallback level stands).
    Suspended,
}

impl AutoThinkingOutcome {
    /// Whether the judge actually produced the level.
    #[must_use]
    pub fn judged(self) -> bool {
        matches!(self, AutoThinkingOutcome::Classified(_))
    }
}

impl AgentSession {
    /// Classify `prompt` and set this turn's thinking level.
    ///
    /// Advisory by contract: every failure resolves to the wiring's fallback
    /// level (or leaves the level alone without wiring) and never fails the
    /// turn. The caller ignores the result; tests read it.
    pub async fn apply_auto_thinking(&self, prompt: &str) -> AutoThinkingOutcome {
        let Some(wiring) = self.auto_thinking_wiring() else {
            return AutoThinkingOutcome::NotConfigured;
        };
        if wiring.is_suspended() {
            return AutoThinkingOutcome::Suspended;
        }
        let state = self.agent().state().await;
        if !state.model.reasoning {
            return AutoThinkingOutcome::ModelWithoutReasoning;
        }
        let answer = wiring.classifier.classify_answer(prompt).await;
        let (level, outcome) = if let Ok(DifficultyAnswer::Picked(difficulty)) = answer {
            wiring.failures.store(0, Ordering::Relaxed);
            (
                AutoThinkingWiring::level_for(difficulty),
                AutoThinkingOutcome::Classified(difficulty),
            )
        } else if matches!(answer, Ok(DifficultyAnswer::Unsure)) {
            // An explicit decline is a legitimate answer: the deterministic
            // level applies and the failure counter is untouched (a judge
            // that says "unsure" is working, not broken).
            (wiring.fallback, AutoThinkingOutcome::Declined)
        } else {
            // A failure keeps the deterministic level AND counts toward the
            // suspension bound: a judge that keeps timing out must stop
            // costing the turn its latency.
            let failures = wiring.failures.fetch_add(1, Ordering::Relaxed) + 1;
            if failures >= MAX_CONSECUTIVE_FAILURES {
                tracing::warn!(
                    failures,
                    "auto-thinking: judge failed repeatedly; suspending for this session"
                );
            }
            (wiring.fallback, AutoThinkingOutcome::Failed)
        };
        if level != state.thinking_level {
            if let Err(error) = self.set_thinking_level(level).await {
                tracing::warn!(error = %error, "auto-thinking: level change not persisted");
            }
        }
        outcome
    }
}
