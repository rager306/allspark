//! Live promotion run for the jev phase-2 consumers: the frozen corpora
//! through the REAL judge (`SubQ` Snap over the `typesafe` dialect). Never
//! runs in CI: `#[ignore]`d and double-gated on `SUBQ_LIVE=1` plus a key:
//!
//! ```text
//! SUBQ_LIVE=1 SUBQ_API_KEY=... \
//!   cargo test -p pa-core --test jev_live_eval -- --ignored --nocapture
//! ```
//!
//! The scripted-judge runs in the lib tests prove the pipeline; only this
//! measured run over the same corpus fingerprints is the promotion
//! evidence the ADR demands before flipping a consumer default.

use std::sync::Arc;

use pa_core::jev::auto_thinking::{Difficulty, DifficultyClassifier};
use pa_core::jev::eval::corpus::{rerank_corpus, thinking_corpus, RerankCase};
use pa_core::jev::eval::{Candidate, Case, Prediction, PredictionSource};
use pa_core::jev::graph_rerank::{GraphReranker, RerankCandidate};
use pa_core::jev::{Judge, NativeJudge};
use pa_jev::{JevClient, JevClientConfig, WireDialect};

fn live_judge() -> Option<Arc<dyn Judge>> {
    if std::env::var("SUBQ_LIVE").ok().as_deref() != Some("1") {
        return None;
    }
    let api_key = std::env::var("SUBQ_API_KEY").ok()?.trim().to_string();
    if api_key.is_empty() {
        return None;
    }
    let config = JevClientConfig::new(
        WireDialect::Typesafe,
        "https://api.subq.ai",
        "subq-snap-preview",
        "subquadratic",
        api_key,
    );
    Some(Arc::new(NativeJudge::new(
        JevClient::new(config),
        "subquadratic/subq-snap-preview".to_string(),
        None,
    )))
}

// ---- auto-thinking --------------------------------------------------------

struct ThinkingBaseline;

#[async_trait::async_trait]
#[allow(clippy::unnecessary_literal_bound)] // the trait's `&str` name; ours are 'static literals
impl Candidate<String, Difficulty> for ThinkingBaseline {
    fn name(&self) -> &str {
        "baseline:always-moderate"
    }

    async fn predict(&self, _case: &Case<String, Difficulty>) -> Prediction<Difficulty> {
        Prediction::certain(
            Difficulty::deterministic_default(),
            PredictionSource::Baseline,
        )
    }
}

struct ThinkingLive {
    classifier: Arc<DifficultyClassifier>,
}

#[async_trait::async_trait]
#[allow(clippy::unnecessary_literal_bound)] // the trait's `&str` name; ours are 'static literals
impl Candidate<String, Difficulty> for ThinkingLive {
    fn name(&self) -> &str {
        "candidate:jev-auto-thinking"
    }

    async fn predict(&self, case: &Case<String, Difficulty>) -> Prediction<Difficulty> {
        match self.classifier.classify(&case.input).await {
            Some(difficulty) => Prediction::certain(difficulty, PredictionSource::Judge),
            None => Prediction::certain(
                Difficulty::deterministic_default(),
                PredictionSource::Fallback,
            ),
        }
    }
}

// ---- graph re-rank --------------------------------------------------------

struct RerankBaseline;

#[async_trait::async_trait]
#[allow(clippy::unnecessary_literal_bound)] // the trait's `&str` name; ours are 'static literals
impl Candidate<RerankCase, usize> for RerankBaseline {
    fn name(&self) -> &str {
        "baseline:rrf-top1"
    }

    async fn predict(&self, _case: &Case<RerankCase, usize>) -> Prediction<usize> {
        Prediction::certain(0, PredictionSource::Baseline)
    }
}

struct RerankLive {
    reranker: Arc<GraphReranker>,
}

#[async_trait::async_trait]
#[allow(clippy::unnecessary_literal_bound)] // the trait's `&str` name; ours are 'static literals
impl Candidate<RerankCase, usize> for RerankLive {
    fn name(&self) -> &str {
        "candidate:jev-rerank"
    }

    async fn predict(&self, case: &Case<RerankCase, usize>) -> Prediction<usize> {
        let candidates: Vec<RerankCandidate> = case
            .input
            .candidates
            .iter()
            .enumerate()
            .map(|(index, spec)| RerankCandidate {
                index,
                qualified_name: spec.symbol.clone(),
                file_path: spec.file.clone(),
                label: spec.label.clone(),
            })
            .collect();
        match self.reranker.rerank(&case.input.query, &candidates).await {
            Some(outcome) => Prediction::certain(
                outcome.kept.first().copied().unwrap_or_default(),
                PredictionSource::Judge,
            ),
            None => Prediction::certain(0, PredictionSource::Fallback),
        }
    }
}

// ---- the run --------------------------------------------------------------

#[tokio::test]
#[ignore = "networked: opt in with SUBQ_LIVE=1 and SUBQ_API_KEY"]
async fn live_judge_promotion_run_over_the_frozen_corpora() {
    let Some(judge) = live_judge() else {
        eprintln!("SKIPPED: live jev eval (set SUBQ_LIVE=1 and SUBQ_API_KEY)");
        return;
    };
    let mut thinking = thinking_corpus().expect("frozen thinking corpus");
    let mut rerank = rerank_corpus().expect("frozen rerank corpus");
    eprintln!(
        "corpora: thinking={} rerank={}",
        thinking.fingerprint(),
        rerank.fingerprint()
    );

    // auto-thinking: dev pair (tunes nothing here - the thresholds are the
    // scripted ones - but measures live accuracy), then the single test
    // touch for both sides.
    {
        let classifier = Arc::new(DifficultyClassifier::new(judge.clone()));
        let live = ThinkingLive { classifier };
        let test = thinking.unseal_test().expect("one test touch");
        let base_test = test.evaluate(&ThinkingBaseline).await;
        let cand_test = test.evaluate(&live).await;
        eprintln!(
            "auto-thinking TEST: baseline {:.3} candidate {:.3}",
            base_test.accuracy, cand_test.accuracy
        );
        let delta = (cand_test.accuracy - base_test.accuracy) * 100.0;
        eprintln!("auto-thinking live delta: {delta:+.1} points");
    }

    // graph re-rank: same discipline.
    {
        let reranker = Arc::new(GraphReranker::new(judge.clone()));
        let live = RerankLive { reranker };
        let test = rerank.unseal_test().expect("one test touch");
        let base_test = test.evaluate(&RerankBaseline).await;
        let cand_test = test.evaluate(&live).await;
        eprintln!(
            "graph-rerank TEST: baseline {:.3} candidate {:.3}",
            base_test.accuracy, cand_test.accuracy
        );
        let delta = (cand_test.accuracy - base_test.accuracy) * 100.0;
        eprintln!("graph-rerank live delta: {delta:+.1} points");
    }
}
