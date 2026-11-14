//! FORK-ONLY: the jev-verifier step (the adversarial verifier role).
//!
//! Skeleton for the P1 pattern (source: Antigravity /swarm, 2026-11-10;
//! applicability study 2026-11-13): after `pa-edit` applies a change and
//! `pa-diff` renders the preview, this step asks the pinned judgment
//! endpoint for a `score` verdict over the diff. The verifier is a ROLE,
//! not a trailing step: its model pin, endpoint, and rubric live in the
//! environment of the daemon process — never in agent conversation.
//!
//! Gated OFF by default: with `ALLSPARK_JEV_VERIFIER` unset, every entry
//! point in this module is a no-op, so wiring a call site cannot change
//! behavior until the flag flips. Wiring point (post-skeleton, see the
//! tracked issue): the edit-apply site renders its pa-diff preview, then
//! calls [`maybe_verify_diff`] and logs the verdict as a telemetry row —
//! the verdict advises, it does not gate the apply (advice-first rollout).
//!
//! Secrets discipline (pattern: secrets never through the agent channel):
//! the endpoint credential is read from `ALLSPARK_JEV_VERIFIER_KEY` in the
//! process environment only. No tool argument, session row, or agent
//! message ever carries it.

#![allow(dead_code)] // skeleton: no call site yet — wiring lands with the tracked issue; the gate stays off until then

use std::collections::BTreeMap;
use std::time::Duration;

use pa_jev::{
    Answer, JevClient, JevClientConfig, JudgmentRequest, JudgmentState, Question, WireDialect,
};

/// The env flag that turns the step on (`1` / `true` / `on`, case-insensitive).
const ENABLE_ENV: &str = "ALLSPARK_JEV_VERIFIER";
/// Endpoint root for the judgment dialect (e.g. `https://typesafe.jev/v1`).
const BASE_URL_ENV: &str = "ALLSPARK_JEV_VERIFIER_URL";
/// Wire model id pinned for the verifier ROLE (a stronger model than the
/// builder's, per the pattern: asymmetry of intellect favors verification).
const MODEL_ENV: &str = "ALLSPARK_JEV_VERIFIER_MODEL";
/// Bearer credential; process-environment only, never agent-visible.
const KEY_ENV: &str = "ALLSPARK_JEV_VERIFIER_KEY";
/// The one question id this step sends.
const QUESTION_ID: &str = "diff-verdict";

/// The score rubric, lowest first (2..=10 levels allowed; 4 is enough for
/// an advice-first verdict).
pub(crate) const VERIFIER_LEVELS: [&str; 4] = [
    "regression: the change breaks working behavior or loses a guarantee",
    "no-progress: the change neither helps nor harms the stated task",
    "partial: the change advances the task but leaves gaps the task named",
    "clean: the change serves the stated task without visible regressions",
];

/// One verifier outcome, ready for the telemetry row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VerifierVerdict {
    /// Probability-weighted mean rubric position (0-based level index;
    /// `3.0` ~ clean, `0.0` ~ regression).
    pub score: f64,
    /// The endpoint's answer confidence.
    pub confidence: f64,
    /// The endpoint label that answered (`provider/model`).
    pub answered_by: String,
}

/// Whether the step is enabled in this process.
#[must_use]
pub(crate) fn enabled() -> bool {
    match std::env::var(ENABLE_ENV) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on"
        ),
        Err(_) => false,
    }
}

/// The verifier's endpoint config from the process environment, or `None`
/// when the step is off or the endpoint is under-specified.
#[must_use]
pub(crate) fn config_from_env() -> Option<JevClientConfig> {
    if !enabled() {
        return None;
    }
    let base_url = env_nonempty(BASE_URL_ENV)?;
    let model = env_nonempty(MODEL_ENV)?;
    let api_key = env_nonempty(KEY_ENV)?;
    let mut config = JevClientConfig::new(
        WireDialect::from_api("typesafe")?,
        base_url,
        model,
        "typesafe",
        api_key,
    );
    config.timeout = Duration::from_secs(20);
    Some(config)
}

/// The verifier's question: one `score` over the four-level rubric, with
/// the task and the rendered diff folded into the instructions.
#[must_use]
pub(crate) fn score_question(task: &str, preview: &str) -> Question {
    Question::Score {
        instructions: build_verifier_prompt(task, preview),
        levels: VERIFIER_LEVELS
            .iter()
            .map(|level| (*level).to_string())
            .collect(),
    }
}

/// The instructions one verifier question carries. Pure, so tests pin the
/// contract: the task and the diff both reach the endpoint, the rubric is
/// positional, and the advice-first stance is explicit.
#[must_use]
pub(crate) fn build_verifier_prompt(task: &str, preview: &str) -> String {
    format!(
        "You are the adversarial verifier for an autonomous coding run. \
         Another agent applied a change; judge the rendered diff against \
         the stated task. Position the change on the rubric (lowest first). \
         Judge only what the diff shows: a regression outranks a gap. \
         The verdict advises the run; it does not gate it.\n\n\
         TASK:\n{task}\n\nRENDERED DIFF (pa-diff preview):\n{preview}"
    )
}

/// The advice-first entry point a wiring site calls after an apply. `None`
/// when the step is off, the endpoint is unspecified, or the judgment
/// fails — the apply always stands on its own.
///
/// # Errors
///
/// Surfaces the judgment failure to the caller for debugLog; the caller
/// decides whether to log and continue (the default).
pub(crate) async fn verify_diff(
    client: &JevClient,
    task: &str,
    preview: &str,
) -> anyhow::Result<VerifierVerdict> {
    let mut questions = BTreeMap::new();
    questions.insert(QUESTION_ID.to_string(), score_question(task, preview));
    let request = JudgmentRequest {
        state: JudgmentState::Text(format!("task: {task}")),
        questions,
    };
    let result = client.judge(&request).await?;
    let answer = result
        .answers
        .get(QUESTION_ID)
        .ok_or_else(|| anyhow::anyhow!("jev-verifier: endpoint returned no {QUESTION_ID}"))?;
    match answer {
        Answer::Score {
            score, confidence, ..
        } => Ok(VerifierVerdict {
            score: *score,
            confidence: *confidence,
            answered_by: result.provider.clone(),
        }),
        other => Err(anyhow::anyhow!(
            "jev-verifier: expected a score answer, got {:?}",
            other.kind().as_str()
        )),
    }
}

/// Read a non-empty trimmed env value.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default() {
        // The flag is absent in the test process: the step must be a no-op.
        assert!(!enabled());
        assert!(config_from_env().is_none());
    }

    #[test]
    fn question_shape_holds_contract() {
        let question = score_question("fix the flaky test", "--- a/main.rs\n+++ b/main.rs");
        assert_eq!(question.kind().as_str(), "score");
        question
            .validate()
            .expect("the four-level rubric satisfies the 2..=10 contract");
        let prompt = build_verifier_prompt("fix the flaky test", "--- a/main.rs");
        assert!(prompt.contains("fix the flaky test"));
        assert!(prompt.contains("--- a/main.rs"));
        assert!(
            prompt.contains("does not gate"),
            "advice-first stance is explicit"
        );
    }

    #[test]
    fn rubric_is_ordered_lowest_first() {
        assert!(VERIFIER_LEVELS
            .first()
            .is_some_and(|level| level.contains("regression")));
        assert!(VERIFIER_LEVELS
            .last()
            .is_some_and(|level| level.contains("clean")));
    }
}
