//! The scripted judge: a deterministic [`Judge`] that answers from a table
//! and never touches the network.
//!
//! This is the harness's stand-in for a measured live run. It exists so a
//! consumer's *plumbing* (question shape, answer parsing, gating, fallback,
//! batching) is testable offline and reproducibly; it is NOT evidence that a
//! live judge would score the same. The corpus gate's numbers are therefore
//! reported as "scripted-judge results" and a promotion still requires a
//! measured run on the same frozen corpus (see [`super`]).
//!
//! Matching rule: an entry answers a request when its `key` occurs verbatim
//! in the request state's canonical form (the case's prompt, or the query
//! inside the re-rank state). First match in insertion order wins, so a
//! script is deterministic regardless of map iteration.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use pa_jev::{Answer, JudgmentRequest, JudgmentResult, Usage};

use crate::jev::{Judge, JudgeError};

/// One scripted answer set: a state key plus the answers keyed by question id.
#[derive(Debug, Clone)]
pub struct ScriptEntry {
    /// Substring that identifies the case in the request state.
    pub key: String,
    pub answers: BTreeMap<String, Answer>,
}

/// A deterministic, offline judge over a script.
pub struct ScriptedJudge {
    label: String,
    entries: Vec<ScriptEntry>,
    calls: AtomicUsize,
    requests: Mutex<Vec<JudgmentRequest>>,
}

impl ScriptedJudge {
    /// Build a scripted judge; `label` is the identity the chain/report shows.
    #[must_use]
    pub fn new(label: &str, entries: Vec<ScriptEntry>) -> ScriptedJudge {
        ScriptedJudge {
            label: label.to_string(),
            entries,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// One scripted `choice` answer for the state containing `key`.
    #[must_use]
    pub fn choice(label: &str, key: &str, question: &str, pick: &str, confidence: f64) -> Self {
        ScriptedJudge::new(
            label,
            vec![ScriptEntry {
                key: key.to_string(),
                answers: BTreeMap::from([(
                    question.to_string(),
                    Answer::Choice {
                        choice: pick.to_string(),
                        probabilities: BTreeMap::from([(pick.to_string(), confidence)]),
                        confidence,
                    },
                )]),
            }],
        )
    }

    /// One scripted `noul` per `(question, probability)` pair for the state
    /// containing `key` - the re-rank's one-request shape.
    #[must_use]
    pub fn nouls(label: &str, key: &str, probabilities: &[(&str, f64)]) -> Self {
        ScriptedJudge::new(
            label,
            vec![ScriptEntry {
                key: key.to_string(),
                answers: probabilities
                    .iter()
                    .map(|(question, probability)| {
                        ((*question).to_string(), Answer::Noul { noul: *probability })
                    })
                    .collect(),
            }],
        )
    }

    /// How many judgments reached this judge (batching assertions).
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The requests this judge saw, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<JudgmentRequest> {
        self.requests
            .lock()
            .map(|requests| requests.clone())
            .unwrap_or_default()
    }

    /// The answer set whose key matches `state`, if any.
    fn scripted_for(&self, state: &str) -> Option<&ScriptEntry> {
        self.entries
            .iter()
            .find(|entry| !entry.key.is_empty() && state.contains(entry.key.as_str()))
    }
}

#[async_trait::async_trait]
impl Judge for ScriptedJudge {
    fn label(&self) -> &str {
        &self.label
    }

    async fn judge(&self, request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(request.clone());
        }
        let state = request.state.canonical();
        let Some(entry) = self.scripted_for(&state) else {
            return Err(JudgeError::Candidate {
                identity: self.label.clone(),
                message: format!("no scripted answer for state {:.60}", state.trim()),
                status: None,
            });
        };
        let mut answers = BTreeMap::new();
        for id in request.questions.keys() {
            let Some(answer) = entry.answers.get(id) else {
                return Err(JudgeError::Candidate {
                    identity: self.label.clone(),
                    message: format!("the script has no answer for question {id:?}"),
                    status: None,
                });
            };
            answers.insert(id.clone(), answer.clone());
        }
        Ok(JudgmentResult {
            api: "scripted".to_string(),
            provider: "scripted".to_string(),
            model: self.label.clone(),
            answers,
            usage: Usage::default(),
        })
    }
}
