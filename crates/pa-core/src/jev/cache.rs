//! The judgment answer cache (fork port of the omp `JudgmentCache`, Phase 1
//! in-memory).
//!
//! Every request decomposes into independent questions, so a later request
//! that re-asks any question about an identical state under the same model
//! is answered locally for free, and only its unanswered questions reach the
//! endpoint. The interface mirrors the ADR's SQLite-backed cache (lookup /
//! record keyed by canonical state + model + question identity), so the
//! on-disk backend replaces this struct without touching the callers.
//!
//! Deviation from the ADR (flagged to reviewers): the ADR sketches SQLite;
//! `pa-core` does not link rusqlite (it is `pa-graph`'s dependency), so
//! Phase 1 ships the in-memory store. A cache entry is tiny (one typed
//! answer), the layer is default-OFF, and sessions are short-lived - the
//! bound below keeps the worst case small.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use pa_jev::{Answer, JudgmentRequest, JudgmentResult, Question};

/// Soft entry cap; a full cache clears once (a session-scoped, low-volume
/// store - the cheap bound, not an eviction policy).
const MAX_ENTRIES: usize = 10_000;

/// Cached answers for one request, plus the state identity `record`
/// reuses.
#[derive(Debug, Clone, Default)]
pub struct CachedAnswers {
    /// The canonical state the answers were given about.
    pub state: String,
    pub answers: BTreeMap<String, Answer>,
}

/// One cache key component: the question's full identity on the wire.
fn question_key(question: &Question) -> String {
    serde_json::to_string(question).unwrap_or_default()
}

/// The in-memory answer cache. Interior-mutable; share by `Arc`.
#[derive(Default)]
pub struct AnswerCache {
    entries: Mutex<HashMap<(String, String, String, String), Answer>>,
}

impl AnswerCache {
    /// Answers already recorded for `request`'s questions under `model`.
    /// Best-effort: a lock failure reports no cached answers.
    #[must_use]
    pub fn lookup(&self, model: &str, request: &JudgmentRequest) -> CachedAnswers {
        let state = request.state.canonical();
        let mut answers = BTreeMap::new();
        if let Ok(entries) = self.entries.lock() {
            for (id, question) in &request.questions {
                let key = (
                    state.clone(),
                    model.to_string(),
                    id.clone(),
                    question_key(question),
                );
                if let Some(answer) = entries.get(&key) {
                    answers.insert(id.clone(), answer.clone());
                }
            }
        }
        CachedAnswers { state, answers }
    }

    /// Record one billed request: one entry per question it answered.
    /// Best-effort: failures never surface to the judgment.
    pub fn record(
        &self,
        model: &str,
        cached: &CachedAnswers,
        questions: &BTreeMap<String, Question>,
        result: &JudgmentResult,
    ) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if entries.len() >= MAX_ENTRIES {
            entries.clear();
        }
        for (id, question) in questions {
            let Some(answer) = result.answers.get(id) else {
                continue;
            };
            let key = (
                cached.state.clone(),
                model.to_string(),
                id.clone(),
                question_key(question),
            );
            entries.insert(key, answer.clone());
        }
    }
}
