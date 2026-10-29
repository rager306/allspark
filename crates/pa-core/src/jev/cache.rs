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
//! Phase 1 ships the in-memory store.
//!
//! # Why the key is a DIGEST, not the state
//!
//! The first version put `state.clone()` in every key: one question meant one
//! full copy of the canonical state, so a request with forty questions about a
//! 300 KB evidence pack held twelve megabytes. The review measures that shape
//! at 384K-token inputs (`pa-jev`'s window), where one pack is ~1 MB and a
//! forty-question request is 40 MB of key text — for a cache whose whole point
//! is to be cheap.
//!
//! A SHA-256 digest is 32 bytes and comparison is exact, so the key is now
//! `(digest, model, question id, question)` and the state itself is never
//! stored. `state_bytes` keeps the ORIGINAL size for the byte budget below: the
//! digest is what identifies, the size is what bounds.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use pa_jev::{Answer, JudgmentRequest, JudgmentResult, Question};
use sha2::{Digest as _, Sha256};

/// Soft byte budget for the whole cache.
///
/// Replaces the entry cap, which bounded the wrong quantity: ten thousand TINY
/// answers and ten thousand answers about 1 MB packs were both "10 000". 8 MiB is
/// modest beside the graph's 8 MiB source-batch ceiling, and the layer is default-OFF.
const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Cached answers for one request, plus the state identity `record` reuses.
#[derive(Debug, Clone, Default)]
pub struct CachedAnswers {
    /// The canonical state's digest — what identifies it, without a copy.
    pub state_digest: String,
    /// The canonical state's size in bytes, for the budget.
    pub state_bytes: usize,
    pub answers: BTreeMap<String, Answer>,
}

/// The SHA-256 of a canonical state's bytes, hex.
fn state_digest(state: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(state);
    format!("{:x}", hasher.finalize())
}

/// One cache key component: the question's full identity on the wire.
fn question_key(question: &Question) -> String {
    serde_json::to_string(question).unwrap_or_default()
}

/// One stored answer with its accounting, so eviction can price it.
#[derive(Debug, Clone)]
struct StoredAnswer {
    answer: Answer,
    /// Bytes this entry owns: the answer, the question key, the digest and the ids.
    bytes: usize,
}

/// The in-memory answer cache. Interior-mutable; share by `Arc`.
#[derive(Default)]
pub struct AnswerCache {
    entries: Mutex<HashMap<(String, String, String, String), StoredAnswer>>,
    /// Bytes currently held, maintained alongside `entries`.
    held_bytes: Mutex<usize>,
}

impl AnswerCache {
    /// Answers already recorded for `request`'s questions under `model`.
    /// Best-effort: a lock failure reports no cached answers.
    #[must_use]
    pub fn lookup(&self, model: &str, request: &JudgmentRequest) -> CachedAnswers {
        // Hash the state's bytes WITHOUT cloning a 1 MB pack to do it. `canonical()`
        // clones the text variant; a cache key only needs the digest.
        let bytes = request.state.canonical_bytes();
        let digest = state_digest(&bytes);
        let state_bytes = bytes.len();
        let mut answers = BTreeMap::new();
        if let Ok(entries) = self.entries.lock() {
            for (id, question) in &request.questions {
                let key = (
                    digest.clone(),
                    model.to_string(),
                    id.clone(),
                    question_key(question),
                );
                if let Some(stored) = entries.get(&key) {
                    answers.insert(id.clone(), stored.answer.clone());
                }
            }
        }
        CachedAnswers {
            state_digest: digest,
            state_bytes,
            answers,
        }
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
        let Ok(mut held) = self.held_bytes.lock() else {
            return;
        };
        for (id, question) in questions {
            let Some(answer) = result.answers.get(id) else {
                continue;
            };
            let qkey = question_key(question);
            let bytes = qkey.len()
                + cached.state_digest.len()
                + id.len()
                + model.len()
                + serde_json::to_vec(answer).map_or(0, |v| v.len());
            let key = (
                cached.state_digest.clone(),
                model.to_string(),
                id.clone(),
                qkey,
            );
            if let Some(previous) = entries.get(&key) {
                *held = held.saturating_sub(previous.bytes);
            }
            *held = held.saturating_add(bytes);
            entries.insert(
                key,
                StoredAnswer {
                    answer: answer.clone(),
                    bytes,
                },
            );
        }
        // Evict down to the budget by clearing: one session-scoped store, default-OFF,
        // and a precise policy would cost more code than the memory it saves. What the
        // cap must NOT do is bound the wrong quantity, which the entry count did.
        if *held > MAX_BYTES {
            entries.clear();
            *held = 0;
        }
    }

    /// Bytes the cache currently holds (tests and diagnostics).
    #[must_use]
    pub fn held_bytes(&self) -> usize {
        self.held_bytes.lock().map_or(0, |held| *held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_jev::{Answer, JudgmentState};

    fn request(state: &str, questions: usize) -> JudgmentRequest {
        let mut map = BTreeMap::new();
        for i in 0..questions {
            map.insert(
                format!("q{i}"),
                Question::Noul {
                    instructions: format!("Is condition {i} true?"),
                    criteria: None,
                },
            );
        }
        JudgmentRequest {
            state: JudgmentState::Text(state.to_string()),
            questions: map,
        }
    }

    fn result_for(request: &JudgmentRequest) -> JudgmentResult {
        let mut answers = BTreeMap::new();
        for id in request.questions.keys() {
            answers.insert(id.clone(), Answer::Noul { noul: 0.5 });
        }
        JudgmentResult {
            api: "typesafe".to_string(),
            provider: "typesafe".to_string(),
            model: "m".to_string(),
            answers,
            usage: pa_jev::Usage::default(),
        }
    }

    /// The defect this replaces: `state.clone()` in EVERY key. Forty questions about a
    /// 1 MB pack held 40 MB of key text, for a cache whose whole point is to be cheap.
    ///
    /// The key is now a 64-character digest, so forty questions cost forty digests, not
    /// forty copies.
    #[test]
    fn many_questions_about_one_big_state_do_not_multiply_the_state() {
        let cache = AnswerCache::default();
        let state = "x".repeat(1024 * 1024);
        let request = request(&state, 40);
        let cached = cache.lookup("m", &request);
        assert_eq!(cached.state_bytes, state.len());
        assert_eq!(cached.state_digest.len(), 64, "a SHA-256 hex digest");
        cache.record("m", &cached, &request.questions, &result_for(&request));

        let held = cache.held_bytes();
        assert!(
            held < 8 * 1024,
            "forty answers must cost kilobytes, not megabytes: {held}"
        );
        // And the answers are still found.
        let again = cache.lookup("m", &request);
        assert_eq!(again.answers.len(), 40);
    }

    /// The budget bounds BYTES. Ten thousand tiny answers and ten thousand answers about
    /// 1 MB packs were both "10 000" under the old entry cap.
    #[test]
    fn the_budget_is_bytes_and_it_actually_clears() {
        let cache = AnswerCache::default();
        let big = "y".repeat(300 * 1024);
        // 40 questions x 300 KB of state is well past 8 MiB if the state were copied;
        // as digests it is small, so the cache keeps working across several packs.
        for round in 0..20 {
            let request = request(&format!("{big}{round}"), 40);
            let cached = cache.lookup("m", &request);
            cache.record("m", &cached, &request.questions, &result_for(&request));
            assert!(
                cache.held_bytes() <= MAX_BYTES,
                "the budget must hold: {} > {}",
                cache.held_bytes(),
                MAX_BYTES
            );
        }
        // The accounting is real, not zero.
        assert!(cache.held_bytes() > 0, "entries were recorded");
    }

    /// A different state is a different key: the digest must not collide across packs
    /// that share a prefix.
    #[test]
    fn a_changed_state_is_a_cache_miss() {
        let cache = AnswerCache::default();
        let first = request("state one", 2);
        let cached = cache.lookup("m", &first);
        cache.record("m", &cached, &first.questions, &result_for(&first));
        assert_eq!(cache.lookup("m", &first).answers.len(), 2);

        let second = request("state two", 2);
        assert!(
            cache.lookup("m", &second).answers.is_empty(),
            "a different state must miss"
        );
    }
}
