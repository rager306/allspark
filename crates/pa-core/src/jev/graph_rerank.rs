//! Phase-2 consumer: the `graph` search re-rank (fork-only).
//!
//! `pa-graph`'s hybrid search fuses BM25 name/body channels with a degree
//! channel through RRF, which ranks *lexically and structurally* - it cannot
//! tell "this symbol's name matches the query" from "this symbol answers the
//! query". The re-rank asks the judge one `noul` question per candidate over
//! the top-K hits in a SINGLE request (the state is billed once, ADR rule 3)
//! and demotes the candidates the judge is confident are irrelevant.
//!
//! Rules this module keeps:
//!
//! - **K is bounded** ([`MAX_RERANK_CANDIDATES`] = 8): one judgment, a menu
//!   the judge can actually compare, and a cost that stays ~$0.00002.
//! - **Both thresholds must hold**: a candidate is demoted only when the
//!   judge's yes-probability is below [`DEMOTE_BELOW`] (a confident "no"),
//!   and every survivor keeps its RRF order otherwise - the judge never
//!   reorders, it filters. Nothing is ever added.
//! - **Fail-open**: any failure leaves the hits exactly as the search
//!   returned them, unmarked.
//! - **Honest marker**: a re-ranked result carries [`VERIFIED_MARKER`] in its
//!   details, a per-hit `judged` flag (the judge looked at it) and a per-hit
//!   `kept` flag (it cleared the threshold). `kept` is deliberately not called
//!   "verified": the judge is asked whether a symbol plausibly answers the
//!   query and its yes-probability only has to clear
//!   [`DEMOTE_BELOW`] (0.1) to keep a hit, so the flag means "not confidently
//!   rejected", not "confirmed correct" (audit F18). A demoted hit is reported
//!   with its probability, never silently deleted.
//! - **Promoted only on a measured win** (corpus gate, `RuFlo` ADR-391).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use pa_jev::{Answer, JudgmentRequest, JudgmentState, Question};

use crate::jev::{Judge, JudgeError};
use crate::settings::{JevEnabledSetting, JevGraphRerankSetting};

/// The largest top-K the re-rank will judge in one request.
pub const MAX_RERANK_CANDIDATES: usize = 8;
/// A candidate whose yes-probability is below this is demoted ("confidently
/// not an answer to the query"). The ADR's deny threshold.
pub const DEMOTE_BELOW: f64 = 0.1;
/// The per-search bound; a slow judge must not delay the tool result.
pub const RERANK_TIMEOUT: Duration = Duration::from_millis(3000);
/// The details marker a re-ranked result carries.
///
/// The wire string keeps its historical spelling: it is what transcripts and
/// the eval corpus already contain, and renaming it would make old results
/// unreadable to the corpus gate. The Rust name says what it is.
pub const VERIFIED_MARKER: &str = "verified_jev";

/// One candidate the judge sees: what the search knows, trimmed to what the
/// question needs (the state is billed per character).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerankCandidate {
    /// The candidate's position in the original result list (the answer key).
    pub index: usize,
    pub qualified_name: String,
    pub file_path: String,
    pub label: String,
}

/// The re-rank's outcome: the survivors in their original order, plus the
/// demoted candidates with the probability that demoted them.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankOutcome {
    /// Indices (into the input list) of the surviving candidates, ascending.
    pub kept: Vec<usize>,
    /// `(index, yes_probability)` for every demoted candidate, ascending.
    pub demoted: Vec<(usize, f64)>,
    /// The judge's raw yes-probability per judged index.
    pub probabilities: BTreeMap<usize, f64>,
}

impl RerankOutcome {
    /// Whether anything was demoted (an all-kept outcome changes no order
    /// but still counts as verified - the judge vouched for every hit).
    #[must_use]
    pub fn demoted_any(&self) -> bool {
        !self.demoted.is_empty()
    }
}

/// The re-ranker over one judge.
pub struct GraphReranker {
    judge: Arc<dyn Judge>,
    timeout: Duration,
    top_k: usize,
}

impl GraphReranker {
    /// Build the re-ranker over the session's judge chain.
    #[must_use]
    pub fn new(judge: Arc<dyn Judge>) -> Self {
        GraphReranker {
            judge,
            timeout: RERANK_TIMEOUT,
            top_k: MAX_RERANK_CANDIDATES,
        }
    }

    /// Override the per-search bound.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Override the judged top-K (clamped to [`MAX_RERANK_CANDIDATES`]).
    #[must_use]
    pub fn with_top_k(mut self, top_k: usize) -> Self {
        self.top_k = top_k.clamp(1, MAX_RERANK_CANDIDATES);
        self
    }

    /// Re-rank `candidates` for `query`; `None` on any failure (fail-open:
    /// the caller keeps the search's own order).
    pub async fn rerank(
        &self,
        query: &str,
        candidates: &[RerankCandidate],
    ) -> Option<RerankOutcome> {
        self.rerank_or_error(query, candidates).await.ok()
    }

    /// Re-rank, surfacing the failure reason.
    ///
    /// # Errors
    ///
    /// [`JudgeError`] from the chain, or [`JudgeError::Timeout`] when the
    /// bound elapses first. An empty candidate list is `Ok` with nothing
    /// demoted (no request is made: nothing to judge, nothing to bill).
    pub async fn rerank_or_error(
        &self,
        query: &str,
        candidates: &[RerankCandidate],
    ) -> Result<RerankOutcome, JudgeError> {
        let judged: Vec<&RerankCandidate> = candidates.iter().take(self.top_k).collect();
        let every_index: Vec<usize> = candidates.iter().map(|candidate| candidate.index).collect();
        if judged.is_empty() {
            return Ok(RerankOutcome {
                kept: every_index,
                demoted: Vec::new(),
                probabilities: BTreeMap::new(),
            });
        }
        let request = rerank_request(query, &judged);
        let outcome = tokio::time::timeout(self.timeout, self.judge.judge(&request)).await;
        let result = match outcome {
            Ok(outcome) => outcome?,
            Err(_elapsed) => return Err(JudgeError::Timeout { attempts: 1 }),
        };
        let mut probabilities = BTreeMap::new();
        for candidate in &judged {
            let key = question_id(candidate.index);
            let Some(answer) = result.answers.get(&key) else {
                return Err(JudgeError::Candidate {
                    identity: self.judge.label().to_string(),
                    message: format!("the judgment skipped candidate {key}"),
                    status: None,
                });
            };
            let Answer::Noul { noul } = answer else {
                return Err(JudgeError::Candidate {
                    identity: self.judge.label().to_string(),
                    message: format!("expected a noul answer for {key}, got {:?}", answer.kind()),
                    status: None,
                });
            };
            probabilities.insert(candidate.index, *noul);
        }
        let mut kept = Vec::with_capacity(candidates.len());
        let mut demoted = Vec::new();
        for index in every_index {
            match probabilities.get(&index) {
                Some(&probability) if probability < DEMOTE_BELOW => {
                    demoted.push((index, probability));
                }
                // Beyond the judged top-K the candidate is kept unjudged: the
                // re-rank never drops what it did not look at.
                _ => kept.push(index),
            }
        }
        Ok(RerankOutcome {
            kept,
            demoted,
            probabilities,
        })
    }
}

/// The answer key of one candidate's question (`c0`, `c1`, ...).
#[must_use]
pub fn question_id(index: usize) -> String {
    format!("c{index}")
}

/// Build the one-request judgment for `candidates` (each a `noul`).
#[must_use]
pub fn rerank_request(query: &str, candidates: &[&RerankCandidate]) -> JudgmentRequest {
    let state = JudgmentState::Json(serde_json::json!({
        "query": query.trim(),
        "candidates": candidates
            .iter()
            .map(|candidate| serde_json::json!({
                "id": question_id(candidate.index),
                "symbol": candidate.qualified_name,
                "kind": candidate.label,
                "file": candidate.file_path,
            }))
            .collect::<Vec<_>>(),
    }));
    let questions = candidates
        .iter()
        .map(|candidate| {
            (
                question_id(candidate.index),
                Question::Noul {
                    instructions: format!(
                        "Does symbol {:?} (a {} in {}) plausibly answer the code query \
                         {:?} - would a developer looking for that query open this symbol?",
                        candidate.qualified_name,
                        candidate.label,
                        candidate.file_path,
                        query.trim()
                    ),
                    criteria: Some(pa_jev::NoulCriteria {
                        true_: Some(
                            "the symbol implements, defines, or directly relates to what \
                             the query names"
                                .to_string(),
                        ),
                        false_: Some(
                            "the symbol only shares a substring, a common word, or a \
                             structural neighbour with the query"
                                .to_string(),
                        ),
                    }),
                },
            )
        })
        .collect();
    JudgmentRequest { state, questions }
}

/// The graph re-rank gate: `[jev] graphRerank = auto` (the only on value)
/// plus the layer's own gate, and - like every judge-heavy consumer - a
/// native chain.
#[must_use]
pub fn should_run_graph_rerank(
    enabled: JevEnabledSetting,
    rerank: JevGraphRerankSetting,
    chain: Option<&crate::jev::ChainJudge>,
) -> bool {
    if rerank == JevGraphRerankSetting::Off {
        return false;
    }
    let Some(chain) = chain else {
        return false;
    };
    match enabled {
        JevEnabledSetting::Off => false,
        JevEnabledSetting::Auto | JevEnabledSetting::On => chain.starts_native(),
    }
}
