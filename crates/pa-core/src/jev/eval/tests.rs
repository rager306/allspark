//! Phase-2 consumer tests: the corpus gate (the framework's own contract),
//! the auto-thinking classifier, and the graph re-rank - all offline, no
//! network, no sleep, readiness by observed state.
//!
//! The gate tests are the ones the ADR and the `RuFlo` lesson demand:
//! (a) a measured win on the test split promotes, (b) a tie does NOT, (c) a
//! +0.9-point win does NOT, and (d) thresholds can only come from dev.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pa_agent::types::ThinkingLevel;
use pa_jev::{Answer, JudgmentRequest, JudgmentResult, Question};

use crate::jev::auto_thinking::{
    difficulty_request, preprocess_prompt, should_run_auto_thinking, Difficulty,
    DifficultyClassifier, QUESTION_ID, UNSURE_OPTION,
};
use crate::jev::chain::{ChainJudge, JudgeCandidate, JudgeKind};
use crate::jev::eval::corpus::{
    arbiter_corpus, rerank_corpus, thinking_corpus, ArbiterCase, ToolRoute,
};
use crate::jev::eval::scripted::{ScriptEntry, ScriptedJudge};
use crate::jev::eval::{
    gate, Candidate, Case, Corpus, EvalError, GateDecision, GateRejection, Prediction,
    PredictionSource, Split, SplitReport, TunedThresholds, TuningError, TuningPolicy,
};
use crate::jev::graph_rerank::{
    question_id, should_run_graph_rerank, GraphReranker, RerankCandidate, DEMOTE_BELOW,
    MAX_RERANK_CANDIDATES,
};
use crate::jev::tool_arbiter::{
    should_run_tool_arbiter, ToolArbiter, ToolAvailability, ARBITER_TIMEOUT,
};
use crate::jev::{Judge, JudgeError};
use crate::settings::{
    JevAutoThinkingSetting, JevEnabledSetting, JevGraphRerankSetting, JevToolArbiterSetting,
};

// ───────────────────────── gate framework ─────────────────────────

/// A candidate that answers from a fixed table keyed by case id; unknown
/// cases fall back to `default`.
struct TableCandidate<L> {
    name: String,
    answers: BTreeMap<String, L>,
    default: L,
    elapsed: Option<Duration>,
    source: PredictionSource,
}

impl<L> TableCandidate<L> {
    fn new(name: &str, answers: &[(&str, L)], default: L) -> Self
    where
        L: Clone,
    {
        TableCandidate {
            name: name.to_string(),
            answers: answers
                .iter()
                .map(|(id, label)| ((*id).to_string(), label.clone()))
                .collect(),
            default,
            elapsed: None,
            source: PredictionSource::Judge,
        }
    }
}

#[async_trait::async_trait]
impl<L> Candidate<String, L> for TableCandidate<L>
where
    L: Clone + Send + Sync,
{
    fn name(&self) -> &str {
        &self.name
    }

    async fn predict(&self, case: &Case<String, L>) -> Prediction<L> {
        Prediction {
            label: self
                .answers
                .get(&case.id)
                .cloned()
                .unwrap_or_else(|| self.default.clone()),
            confidence: None,
            elapsed: self.elapsed,
            source: self.source,
        }
    }
}

/// A candidate that answers like a fixed baseline: one label for everything.
fn constant_candidate(name: &str, label: &str) -> TableCandidate<String> {
    TableCandidate {
        name: name.to_string(),
        answers: BTreeMap::new(),
        default: label.to_string(),
        elapsed: None,
        source: PredictionSource::Baseline,
    }
}

/// A toy corpus: four dev cases, four test cases, two labels.
fn toy_corpus() -> Corpus<String, String> {
    let cases = vec![
        Case {
            id: "d1".to_string(),
            input: "d1".to_string(),
            label: "a".to_string(),
            split: Split::Dev,
            tags: Vec::new(),
        },
        Case {
            id: "d2".to_string(),
            input: "d2".to_string(),
            label: "b".to_string(),
            split: Split::Dev,
            tags: Vec::new(),
        },
        Case {
            id: "d3".to_string(),
            input: "d3".to_string(),
            label: "a".to_string(),
            split: Split::Dev,
            tags: Vec::new(),
        },
        Case {
            id: "d4".to_string(),
            input: "d4".to_string(),
            label: "b".to_string(),
            split: Split::Dev,
            tags: Vec::new(),
        },
        Case {
            id: "t1".to_string(),
            input: "t1".to_string(),
            label: "a".to_string(),
            split: Split::Test,
            tags: Vec::new(),
        },
        Case {
            id: "t2".to_string(),
            input: "t2".to_string(),
            label: "b".to_string(),
            split: Split::Test,
            tags: Vec::new(),
        },
        Case {
            id: "t3".to_string(),
            input: "t3".to_string(),
            label: "a".to_string(),
            split: Split::Test,
            tags: Vec::new(),
        },
        Case {
            id: "t4".to_string(),
            input: "t4".to_string(),
            label: "b".to_string(),
            split: Split::Test,
            tags: Vec::new(),
        },
    ];
    Corpus::new("toy", "v1", cases).expect("toy corpus validates")
}

fn tiny_policy() -> TuningPolicy {
    TuningPolicy {
        accuracy_delta_points: vec![1.0, 2.0, 5.0],
        max_p95_latency_ratio: 2.0,
        dev_margin_points: 0.0,
        min_cases: 2,
    }
}

/// The four-label split report for the gate's arithmetic (no evaluation
/// needed: the gate reads only accuracy, macro-F1, counts and latency).
fn report(
    split: Split,
    correct: usize,
    cases: usize,
    elapsed_p95: Option<Duration>,
) -> SplitReport<String> {
    SplitReport {
        corpus: "toy".to_string(),
        fingerprint: "fp".to_string(),
        split,
        candidate: "candidate".to_string(),
        cases,
        correct,
        accuracy: correct as f64 / cases as f64,
        macro_f1: correct as f64 / cases as f64,
        per_label: BTreeMap::new(),
        misclassified: Vec::new(),
        judged: cases,
        fallback: 0,
        elapsed_p95,
    }
}

#[tokio::test]
async fn a_corpus_rejects_the_shapes_that_would_hide_a_broken_split() {
    let duplicate = vec![
        Case {
            id: "same".to_string(),
            input: "x".to_string(),
            label: "a".to_string(),
            split: Split::Dev,
            tags: Vec::new(),
        },
        Case {
            id: "same".to_string(),
            input: "y".to_string(),
            label: "a".to_string(),
            split: Split::Test,
            tags: Vec::new(),
        },
    ];
    assert!(matches!(
        Corpus::new("dup", "v1", duplicate),
        Err(EvalError::DuplicateCase { .. })
    ));

    let no_test = vec![Case {
        id: "only".to_string(),
        input: "x".to_string(),
        label: "a".to_string(),
        split: Split::Dev,
        tags: Vec::new(),
    }];
    assert!(matches!(
        Corpus::new("one-sided", "v1", no_test),
        Err(EvalError::EmptySplit {
            split: Split::Test,
            ..
        })
    ));

    // A test label the dev split never shows cannot be tuned for: the corpus
    // refuses to exist, so "tune on dev" is structural, not advisory.
    let unseen_label = vec![
        Case {
            id: "d".to_string(),
            input: "x".to_string(),
            label: "a".to_string(),
            split: Split::Dev,
            tags: Vec::new(),
        },
        Case {
            id: "t".to_string(),
            input: "y".to_string(),
            label: "z".to_string(),
            split: Split::Test,
            tags: Vec::new(),
        },
    ];
    assert!(matches!(
        Corpus::new("unseen", "v1", unseen_label),
        Err(EvalError::LabelNotTunable { .. })
    ));
}

#[tokio::test]
async fn the_test_split_is_a_one_shot_token() {
    let mut corpus = toy_corpus();
    let baseline = constant_candidate("baseline", "a");
    let first = corpus.unseal_test().expect("the first unseal succeeds");
    let report = first.evaluate(&baseline).await;
    assert_eq!(report.split, Split::Test);
    assert_eq!(report.cases, 4);
    drop(first);
    assert!(matches!(
        corpus.unseal_test(),
        Err(EvalError::TestAlreadyTouched { .. })
    ));
}

#[tokio::test]
async fn the_fingerprint_pins_the_case_list() {
    let corpus = toy_corpus();
    let fingerprint = corpus.fingerprint();
    assert_eq!(fingerprint.len(), 64, "sha256 hex");
    let mut edited = toy_corpus();
    edited.cases[0].label = "b".to_string();
    assert_ne!(
        edited.fingerprint(),
        fingerprint,
        "a label edit moves the hash"
    );
}

#[tokio::test]
async fn a_measured_win_on_the_test_split_promotes() {
    let mut corpus = toy_corpus();
    let baseline = constant_candidate("baseline:always-a", "a");
    // The candidate gets every case right (dev and test alike).
    let winner = TableCandidate::new(
        "candidate:oracle",
        &[
            ("d1", "a".to_string()),
            ("d2", "b".to_string()),
            ("d3", "a".to_string()),
            ("d4", "b".to_string()),
            ("t1", "a".to_string()),
            ("t2", "b".to_string()),
            ("t3", "a".to_string()),
            ("t4", "b".to_string()),
        ],
        "a".to_string(),
    );
    let baseline_dev = corpus.evaluate_dev(&baseline).await;
    let winner_dev = corpus.evaluate_dev(&winner).await;
    assert_eq!(baseline_dev.accuracy, 0.5);
    assert_eq!(winner_dev.accuracy, 1.0);
    let thresholds = TunedThresholds::tune(&baseline_dev, &winner_dev, &tiny_policy())
        .expect("dev supports a win");
    assert_eq!(
        thresholds.accuracy_delta_points(),
        5.0,
        "the largest grid value the dev win supports"
    );

    let test = corpus.unseal_test().expect("one touch");
    let baseline_test = test.evaluate(&baseline).await;
    let winner_test = test.evaluate(&winner).await;
    let decision = gate(&baseline_test, &winner_test, &thresholds);
    let GateDecision::Promote {
        accuracy_delta_points,
        ..
    } = decision
    else {
        panic!("a +50-point test win must promote, got {decision:?}");
    };
    assert!(
        (accuracy_delta_points - 50.0).abs() < 1e-9,
        "{accuracy_delta_points}"
    );
}

#[tokio::test]
async fn a_tie_never_promotes() {
    let mut corpus = toy_corpus();
    let baseline = constant_candidate("baseline:always-a", "a");
    let same = constant_candidate("candidate:same", "a");
    let baseline_dev = corpus.evaluate_dev(&baseline).await;
    let same_dev = corpus.evaluate_dev(&same).await;
    // Tuning itself refuses a candidate that does not beat the baseline on
    // dev (the safety margin leaves nothing to tune).
    let error = TunedThresholds::tune(&baseline_dev, &same_dev, &tiny_policy())
        .expect_err("a tie on dev cannot produce thresholds");
    assert!(matches!(error, TuningError::InsufficientDevDelta { .. }));

    // And with thresholds borrowed from a real win, the tie still fails the
    // test gate: accuracy delta 0 < 1.
    let thresholds = TunedThresholds {
        accuracy_delta_points: 1.0,
        max_p95_latency_ratio: 2.0,
        min_cases: 2,
        tuned_on: corpus.fingerprint(),
        dev_delta_points: 5.0,
    };
    let test = corpus.unseal_test().expect("one touch");
    let baseline_test = test.evaluate(&baseline).await;
    let same_test = test.evaluate(&same).await;
    let decision = gate(&baseline_test, &same_test, &thresholds);
    assert!(matches!(
        decision,
        GateDecision::Reject(GateRejection::AccuracyDelta { .. })
    ));
}

#[tokio::test]
async fn the_ruflo_lesson_a_sub_point_win_does_not_promote() {
    // The RuFlo ADR-391 case: +0.89 points on test, gate rejects.
    let thresholds = TunedThresholds {
        accuracy_delta_points: 1.0,
        max_p95_latency_ratio: 2.0,
        min_cases: 4,
        tuned_on: "fp".to_string(),
        dev_delta_points: 4.0,
    };
    // 50 of 100 vs 50.89 of 100 is not expressible in counts, so the corpus
    // is 1000 cases: 500 correct vs 508.9 -> use 509 (delta 0.9 points).
    let baseline = report(Split::Test, 500, 1000, None);
    let candidate = report(Split::Test, 509, 1000, None);
    let decision = gate(&baseline, &candidate, &thresholds);
    let GateDecision::Reject(GateRejection::AccuracyDelta { delta_points, .. }) = decision else {
        panic!("+0.9 points must not promote, got {decision:?}");
    };
    assert!((delta_points - 0.9).abs() < 1e-9, "{delta_points}");

    // One more correct case (+1.0 points) clears the same threshold: the
    // boundary is the threshold, not a rounding artefact.
    let candidate = report(Split::Test, 510, 1000, None);
    assert!(gate(&baseline, &candidate, &thresholds).is_promote());
}

#[tokio::test]
async fn thresholds_come_from_dev_only() {
    let policy = tiny_policy();
    let dev = report(Split::Dev, 4, 4, None);
    let test = report(Split::Test, 4, 4, None);
    let error = TunedThresholds::tune(&dev, &test, &policy)
        .expect_err("tuning on a test report is a type-level mistake");
    assert!(matches!(
        error,
        TuningError::WrongSplit {
            required: Split::Dev,
            actual: Split::Test
        }
    ));
    // And the gate refuses a dev report: the decision is test-only.
    let thresholds = TunedThresholds {
        accuracy_delta_points: 1.0,
        max_p95_latency_ratio: 2.0,
        min_cases: 2,
        tuned_on: "fp".to_string(),
        dev_delta_points: 5.0,
    };
    let decision = gate(&dev, &dev, &thresholds);
    assert!(matches!(
        decision,
        GateDecision::Reject(GateRejection::WrongSplit {
            required: Split::Test,
            ..
        })
    ));
}

#[tokio::test]
async fn a_latency_blowup_rejects_a_winning_candidate() {
    // The RuFlo MiniLM lesson: +10.6 points, p95 x5.85, rejected.
    let thresholds = TunedThresholds {
        accuracy_delta_points: 1.0,
        max_p95_latency_ratio: 2.0,
        min_cases: 4,
        tuned_on: "fp".to_string(),
        dev_delta_points: 5.0,
    };
    let baseline = report(Split::Test, 500, 1000, Some(Duration::from_micros(660)));
    let candidate = report(Split::Test, 606, 1000, Some(Duration::from_micros(3_860)));
    let decision = gate(&baseline, &candidate, &thresholds);
    let GateDecision::Reject(GateRejection::Latency { ratio, limit }) = decision else {
        panic!("a 5.8x p95 blowup must reject, got {decision:?}");
    };
    assert!(ratio > 5.0 && limit == 2.0, "{ratio} / {limit}");
}

#[tokio::test]
async fn reports_and_tuning_refuse_cross_corpus_arithmetic() {
    let mut baseline = report(Split::Dev, 4, 4, None);
    let candidate = report(Split::Dev, 4, 4, None);
    baseline.fingerprint = "other".to_string();
    let error = TunedThresholds::tune(&baseline, &candidate, &tiny_policy())
        .expect_err("a different corpus cannot tune");
    assert!(matches!(error, TuningError::CorpusMismatch { .. }));
}

#[tokio::test]
async fn the_report_counts_judged_and_fallback_cases() {
    let corpus = toy_corpus();
    let mut judged = constant_candidate("candidate", "a");
    judged.source = PredictionSource::Judge;
    let dev = corpus.evaluate_dev(&judged).await;
    assert_eq!(dev.judged, 4);
    assert_eq!(dev.fallback, 0);
    let mut falling_back = constant_candidate("candidate", "a");
    falling_back.source = PredictionSource::Fallback;
    let dev = corpus.evaluate_dev(&falling_back).await;
    assert_eq!(dev.judged, 0);
    assert_eq!(dev.fallback, 4);
    // Every wrong answer is evidence, with its expected and predicted label.
    assert_eq!(dev.misclassified.len(), 2);
    assert!(dev.misclassified.iter().all(|miss| miss.expected == "b"));
}

// ─────────────────────── frozen corpora ───────────────────────

#[tokio::test]
async fn the_thinking_corpus_is_frozen_and_balanced() {
    let corpus = thinking_corpus().expect("the fixture validates");
    assert_eq!(corpus.name(), "auto-thinking");
    assert_eq!(corpus.len(), 28);
    assert_eq!(corpus.dev().len(), 16);
    assert_eq!(corpus.test().len(), 12);
    for difficulty in [Difficulty::Trivial, Difficulty::Moderate, Difficulty::Hard] {
        assert!(
            corpus.dev().iter().any(|case| case.label == difficulty),
            "dev shows {difficulty}"
        );
        assert!(
            corpus.test().iter().any(|case| case.label == difficulty),
            "test shows {difficulty}"
        );
    }
    // Pinned: editing a case (or its order) is a conscious re-freeze.
    assert_eq!(
        corpus.fingerprint(),
        THINKING_FINGERPRINT,
        "the auto-thinking corpus changed: re-freeze only with a measured re-run"
    );
}

#[tokio::test]
async fn the_rerank_corpus_is_frozen_and_balanced() {
    let corpus = rerank_corpus().expect("the fixture validates");
    assert_eq!(corpus.name(), "graph-rerank");
    assert_eq!(corpus.len(), 26);
    assert_eq!(corpus.dev().len(), 14);
    assert_eq!(corpus.test().len(), 12);
    // Every case's expected index is inside its candidate list, and index 0
    // is never the answer for the trap cases (the baseline must be able to
    // lose).
    for case in corpus.dev().iter().chain(corpus.test().iter()) {
        assert!(
            case.label < case.input.candidates.len(),
            "{} expects index {} of {}",
            case.id,
            case.label,
            case.input.candidates.len()
        );
    }
    assert_eq!(
        corpus.fingerprint(),
        RERANK_FINGERPRINT,
        "the graph re-rank corpus changed: re-freeze only with a measured re-run"
    );
}

#[tokio::test]
async fn the_arbiter_corpus_is_frozen_and_balanced() {
    let corpus = arbiter_corpus().expect("the fixture validates");
    assert_eq!(corpus.name(), "tool-arbiter");
    assert_eq!(corpus.len(), 15);
    assert_eq!(corpus.dev().len(), 9);
    assert_eq!(corpus.test().len(), 6);
    // Both splits must be able to lose: the `bash` label (the shell was the
    // right surface) and a specialized route are in each.
    for split in [corpus.dev(), corpus.test()] {
        assert!(split.iter().any(|case| case.label == ToolRoute::Bash));
        assert!(split.iter().any(|case| case.label != ToolRoute::Bash));
    }
    // Every case names a roster that can reach its own label: a suggestion
    // for an absent tool would be unreachable, and a `bash` case on an empty
    // roster would be trivially right.
    for case in corpus.dev().iter().chain(corpus.test().iter()) {
        let (graph, lsp, edit) = case.input.available;
        assert!(graph || lsp || edit, "{}: an empty roster", case.id);
        match case.label {
            ToolRoute::GraphSearch => assert!(graph, "{} needs graph", case.id),
            ToolRoute::LspFindSymbol => assert!(lsp, "{} needs lsp", case.id),
            ToolRoute::Edit => assert!(edit, "{} needs edit", case.id),
            ToolRoute::Bash => {}
        }
    }
    assert_eq!(
        corpus.fingerprint(),
        ARBITER_FINGERPRINT,
        "the tool-arbiter corpus changed: re-freeze only with a measured re-run"
    );
}

/// Pinned fingerprints (see the tests above).
const THINKING_FINGERPRINT: &str =
    "281525f1f02a619b4c7fc6d7e9f97572ebe38758fe68972a579e184dfc4b15cc";
const RERANK_FINGERPRINT: &str = "28addc2c5d82761d0768820c44933f0b7787f15825e071f8e3965dd9595f7548";
const ARBITER_FINGERPRINT: &str =
    "40c81032bb5c37959c368af615527bfad14d5dd9c0ca7dcdcace138022c9ec55";

// ─────────────────────── auto-thinking ───────────────────────

/// A judge that answers the difficulty question from a prompt-keyed table.
struct DifficultyJudge {
    answer: Mutex<Option<Answer>>,
    calls: AtomicUsize,
}

use std::sync::Mutex;

impl DifficultyJudge {
    fn new(answer: Option<Answer>) -> Arc<Self> {
        Arc::new(DifficultyJudge {
            answer: Mutex::new(answer),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Judge for DifficultyJudge {
    fn label(&self) -> &'static str {
        "scripted/difficulty"
    }

    async fn judge(&self, request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // The question must be the three-way choice plus the escape.
        let Question::Choice { criteria, .. } = &request.questions[QUESTION_ID] else {
            panic!("the difficulty question is a choice");
        };
        assert_eq!(criteria.len(), 4, "trivial/moderate/hard + unsure");
        assert!(criteria.contains_key(UNSURE_OPTION));
        let Some(answer) = self.answer.lock().expect("answer").clone() else {
            return Err(JudgeError::Candidate {
                identity: "scripted/difficulty".to_string(),
                message: "scripted failure".to_string(),
                status: None,
            });
        };
        Ok(JudgmentResult {
            api: "scripted".to_string(),
            provider: "scripted".to_string(),
            model: "scripted/difficulty".to_string(),
            answers: BTreeMap::from([(QUESTION_ID.to_string(), answer)]),
            usage: pa_jev::Usage::default(),
        })
    }
}

fn choice_answer(pick: &str) -> Answer {
    Answer::Choice {
        choice: pick.to_string(),
        probabilities: BTreeMap::from([(pick.to_string(), 0.9)]),
        confidence: 0.9,
    }
}

#[tokio::test]
async fn the_classifier_maps_each_bucket_to_a_thinking_level() {
    for (pick, level, difficulty) in [
        ("trivial", ThinkingLevel::Low, Difficulty::Trivial),
        ("moderate", ThinkingLevel::Medium, Difficulty::Moderate),
        ("hard", ThinkingLevel::High, Difficulty::Hard),
    ] {
        let judge = DifficultyJudge::new(Some(choice_answer(pick)));
        let classifier = DifficultyClassifier::new(judge.clone());
        let outcome = classifier.classify("do the thing").await;
        assert_eq!(outcome, Some(difficulty));
        assert_eq!(difficulty.thinking_level(), level);
        assert_eq!(judge.calls(), 1, "one judgment per turn");
    }
}

#[tokio::test]
async fn an_escape_or_a_broken_answer_is_advisory() {
    let judge = DifficultyJudge::new(Some(choice_answer(UNSURE_OPTION)));
    let classifier = DifficultyClassifier::new(judge);
    assert_eq!(classifier.classify("unclear").await, None);
    assert_eq!(
        classifier.classify_answer("unclear").await,
        Ok(crate::jev::auto_thinking::DifficultyAnswer::Unsure),
        "a decline is an answer, not a failure"
    );

    let judge = DifficultyJudge::new(Some(Answer::Noul { noul: 0.5 }));
    let classifier = DifficultyClassifier::new(judge);
    let error = classifier
        .classify_answer("wrong shape")
        .await
        .expect_err("a noul is not a difficulty");
    assert!(error.to_string().contains("choice"), "{error}");

    let judge = DifficultyJudge::new(None);
    let classifier = DifficultyClassifier::new(judge);
    assert_eq!(classifier.classify("judge down").await, None);
}

#[tokio::test]
async fn a_hanging_judge_is_abandoned_at_the_bound() {
    struct HangingJudge;
    #[async_trait::async_trait]
    impl Judge for HangingJudge {
        fn label(&self) -> &'static str {
            "hanging"
        }
        async fn judge(&self, _request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
            std::future::pending().await
        }
    }
    // A zero bound: the timer fires on the first poll, so the pending judge
    // is abandoned without any wall-clock wait (no sleep in the test).
    let classifier = DifficultyClassifier::new(Arc::new(HangingJudge)).with_timeout(Duration::ZERO);
    let error = classifier
        .classify_answer("slow")
        .await
        .expect_err("the bound elapses");
    assert!(matches!(error, JudgeError::Timeout { .. }), "{error}");
    assert_eq!(classifier.classify("slow").await, None, "fail-open");
}

/// The critical-path test (ADR): a judge endpoint that never answers cannot
/// hold the turn beyond the bound. Paused time proves the real bound fires
/// without sleeping.
#[tokio::test(start_paused = true)]
async fn a_stalled_judge_releases_the_turn_at_the_real_bound() {
    struct StalledJudge;
    #[async_trait::async_trait]
    impl Judge for StalledJudge {
        fn label(&self) -> &'static str {
            "stalled"
        }
        async fn judge(&self, _request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
            std::future::pending().await
        }
    }
    let classifier = DifficultyClassifier::new(Arc::new(StalledJudge));
    let started = tokio::time::Instant::now();
    let outcome = classifier.classify("a hard task").await;
    assert_eq!(
        outcome, None,
        "a stalled judge is advisory: the level stands"
    );
    assert!(
        started.elapsed() >= crate::jev::auto_thinking::AUTO_THINKING_TIMEOUT,
        "the bound fired: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn the_prompt_is_cleaned_and_bounded_before_it_is_billed() {
    let long = format!(
        "{}\u{1b}[1mbold\u{1b}[0m\n\n\n```\nfn secret() {{}}\n```\n{}",
        "a".repeat(4000),
        "z".repeat(4000)
    );
    let cleaned = preprocess_prompt(&long);
    assert!(
        cleaned.chars().count() <= 2000,
        "bounded: {}",
        cleaned.chars().count()
    );
    assert!(!cleaned.contains('\u{1b}'), "ANSI stripped");
    assert!(!cleaned.contains("secret"), "fenced blocks dropped");
    assert!(
        cleaned.contains("chars omitted"),
        "the omission is explicit"
    );
    assert!(
        cleaned.starts_with('a') && cleaned.ends_with('z'),
        "both ends survive"
    );

    // A prompt that is essentially one code block keeps its text (stripping
    // would leave nothing to classify).
    let code_only = "```\nlet x = compute_the_answer();\n```";
    assert!(preprocess_prompt(code_only).contains("compute_the_answer"));
    // A short prompt passes through (modulo whitespace collapse).
    assert_eq!(preprocess_prompt("  add   a test  "), "add a test");
}

#[tokio::test]
async fn the_classifier_request_is_one_choice_over_the_preprocessed_state() {
    let request = difficulty_request("refactor the admission layer");
    assert_eq!(request.questions.len(), 1);
    let state = request.state.canonical();
    assert!(state.contains("refactor the admission layer"), "{state}");
    request.validate().expect("the question shape is valid");
}

#[tokio::test]
async fn the_auto_thinking_gate_demands_a_native_judge() {
    let native = ChainJudge::new(vec![JudgeCandidate {
        identity: "typesafe/jev".to_string(),
        kind: JudgeKind::Native,
        judge: None,
    }]);
    let prompted = ChainJudge::new(vec![JudgeCandidate {
        identity: "local/model".to_string(),
        kind: JudgeKind::Prompted,
        judge: None,
    }]);
    // Default OFF: never.
    assert!(!should_run_auto_thinking(
        JevEnabledSetting::Off,
        JevAutoThinkingSetting::Auto,
        Some(&native)
    ));
    // `autoThinking = off` wins over an enabled layer.
    assert!(!should_run_auto_thinking(
        JevEnabledSetting::On,
        JevAutoThinkingSetting::Off,
        Some(&native)
    ));
    // No chain, no classification.
    assert!(!should_run_auto_thinking(
        JevEnabledSetting::On,
        JevAutoThinkingSetting::Auto,
        None
    ));
    // A prompted-only chain cannot retune the reasoning budget.
    assert!(!should_run_auto_thinking(
        JevEnabledSetting::On,
        JevAutoThinkingSetting::Auto,
        Some(&prompted)
    ));
    assert!(should_run_auto_thinking(
        JevEnabledSetting::On,
        JevAutoThinkingSetting::Auto,
        Some(&native)
    ));
    assert!(should_run_auto_thinking(
        JevEnabledSetting::Auto,
        JevAutoThinkingSetting::Auto,
        Some(&native)
    ));
}

#[tokio::test]
async fn the_auto_thinking_setting_parses_conservatively() {
    assert_eq!(
        JevAutoThinkingSetting::parse("auto"),
        JevAutoThinkingSetting::Auto
    );
    assert_eq!(
        JevAutoThinkingSetting::parse("ON"),
        JevAutoThinkingSetting::Auto
    );
    for typo in ["", "true", "yes", "1", "enabled"] {
        assert_eq!(
            JevAutoThinkingSetting::parse(typo),
            JevAutoThinkingSetting::Off,
            "{typo:?} reads as off"
        );
    }
    assert_eq!(
        JevGraphRerankSetting::parse("auto"),
        JevGraphRerankSetting::Auto
    );
    assert_eq!(
        JevGraphRerankSetting::parse("sometimes"),
        JevGraphRerankSetting::Off
    );
}

// ─────────────────────── graph re-rank ───────────────────────

fn candidates(count: usize) -> Vec<RerankCandidate> {
    (0..count)
        .map(|index| RerankCandidate {
            index,
            qualified_name: format!("module::symbol_{index}"),
            file_path: format!("src/file_{index}.rs"),
            label: "function".to_string(),
        })
        .collect()
}

/// A judge answering every candidate's `noul` from a table.
struct RerankJudge {
    probabilities: BTreeMap<String, f64>,
    calls: AtomicUsize,
    fail: bool,
}

impl RerankJudge {
    fn new(probabilities: &[(&str, f64)]) -> Arc<Self> {
        Arc::new(RerankJudge {
            probabilities: probabilities
                .iter()
                .map(|(id, probability)| ((*id).to_string(), *probability))
                .collect(),
            calls: AtomicUsize::new(0),
            fail: false,
        })
    }

    fn failing() -> Arc<Self> {
        Arc::new(RerankJudge {
            probabilities: BTreeMap::new(),
            calls: AtomicUsize::new(0),
            fail: true,
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Judge for RerankJudge {
    fn label(&self) -> &'static str {
        "scripted/rerank"
    }

    async fn judge(&self, request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(JudgeError::Candidate {
                identity: "scripted/rerank".to_string(),
                message: "scripted failure".to_string(),
                status: None,
            });
        }
        // Every question is a noul with both rubrics (the question shape).
        for question in request.questions.values() {
            let Question::Noul { criteria, .. } = question else {
                panic!("the re-rank asks noul questions only");
            };
            let criteria = criteria.as_ref().expect("rubrics present");
            assert!(criteria.true_.is_some() && criteria.false_.is_some());
        }
        let mut answers = BTreeMap::new();
        for id in request.questions.keys() {
            let probability =
                self.probabilities
                    .get(id)
                    .copied()
                    .ok_or_else(|| JudgeError::Candidate {
                        identity: "scripted/rerank".to_string(),
                        message: format!("no scripted probability for {id}"),
                        status: None,
                    })?;
            answers.insert(id.clone(), Answer::Noul { noul: probability });
        }
        Ok(JudgmentResult {
            api: "scripted".to_string(),
            provider: "scripted".to_string(),
            model: "scripted/rerank".to_string(),
            answers,
            usage: pa_jev::Usage::default(),
        })
    }
}

#[tokio::test]
async fn one_request_judges_the_whole_top_k_and_demotes_confident_misses() {
    let judge = RerankJudge::new(&[("c0", 0.92), ("c1", 0.03), ("c2", 0.88)]);
    let reranker = GraphReranker::new(judge.clone());
    let outcome = reranker
        .rerank("answer cache lookup", &candidates(3))
        .await
        .expect("the judge answered");
    assert_eq!(judge.calls(), 1, "one request for the whole top-K");
    assert_eq!(outcome.kept, vec![0, 2], "survivors keep their RRF order");
    assert_eq!(outcome.demoted.len(), 1);
    assert_eq!(outcome.demoted[0].0, 1);
    assert!(outcome.demoted[0].1 < DEMOTE_BELOW);
    assert!(outcome.demoted_any());
    assert_eq!(outcome.probabilities.len(), 3);
}

#[tokio::test]
async fn the_rerank_never_drops_what_it_did_not_look_at() {
    let judge = RerankJudge::new(&[("c0", 0.9), ("c1", 0.9), ("c2", 0.9), ("c3", 0.9)]);
    let reranker = GraphReranker::new(judge.clone()).with_top_k(2);
    let outcome = reranker
        .rerank("query", &candidates(6))
        .await
        .expect("answered");
    assert_eq!(judge.calls(), 1);
    assert_eq!(outcome.probabilities.len(), 2, "only the top-2 were judged");
    assert_eq!(
        outcome.kept,
        vec![0, 1, 2, 3, 4, 5],
        "the unjudged tail survives"
    );
    assert!(!outcome.demoted_any());
}

#[tokio::test]
async fn the_rerank_top_k_is_bounded() {
    let reranker = GraphReranker::new(RerankJudge::new(&[])).with_top_k(64);
    // The clamp keeps the request inside the one-menu bound.
    assert_eq!(MAX_RERANK_CANDIDATES, 8);
    let request = crate::jev::graph_rerank::rerank_request(
        "query",
        &candidates(MAX_RERANK_CANDIDATES).iter().collect::<Vec<_>>(),
    );
    assert_eq!(request.questions.len(), MAX_RERANK_CANDIDATES);
    request.validate().expect("eight nouls are a valid request");
    // An empty candidate list bills nothing and asks nothing.
    let outcome = reranker.rerank("query", &[]).await.expect("no-op");
    assert!(outcome.kept.is_empty() && !outcome.demoted_any());
}

#[tokio::test]
async fn a_judge_failure_leaves_the_order_untouched() {
    let reranker = GraphReranker::new(RerankJudge::failing());
    assert!(reranker.rerank("query", &candidates(3)).await.is_none());
}

#[tokio::test]
async fn a_partial_answer_is_a_failure_not_a_silent_keep() {
    // The judge answers c0 only: the re-rank must not guess about c1/c2.
    let judge = RerankJudge::new(&[("c0", 0.9)]);
    let reranker = GraphReranker::new(judge);
    let error = reranker
        .rerank_or_error("query", &candidates(3))
        .await
        .expect_err("a missing answer is a failure");
    assert!(error.to_string().contains("c1"), "{error}");
}

#[tokio::test]
async fn the_rerank_question_ids_are_the_candidate_indices() {
    assert_eq!(question_id(0), "c0");
    assert_eq!(question_id(7), "c7");
}

#[tokio::test]
async fn the_graph_rerank_gate_demands_a_native_judge() {
    let native = ChainJudge::new(vec![JudgeCandidate {
        identity: "typesafe/jev".to_string(),
        kind: JudgeKind::Native,
        judge: None,
    }]);
    assert!(!should_run_graph_rerank(
        JevEnabledSetting::Off,
        JevGraphRerankSetting::Auto,
        Some(&native)
    ));
    assert!(!should_run_graph_rerank(
        JevEnabledSetting::On,
        JevGraphRerankSetting::Off,
        Some(&native)
    ));
    assert!(!should_run_graph_rerank(
        JevEnabledSetting::On,
        JevGraphRerankSetting::Auto,
        None
    ));
    assert!(should_run_graph_rerank(
        JevEnabledSetting::On,
        JevGraphRerankSetting::Auto,
        Some(&native)
    ));
}

// ─────────────── the corpora against the scripted judge ───────────────

/// Score the auto-thinking corpus: the baseline is the deterministic
/// default (`moderate`), the candidate is the classifier behind a scripted
/// judge whose answers are the corpus labels.
///
/// This exercises the WHOLE path (gate -> request -> judge -> parse -> level)
/// on the frozen cases. The numbers are scripted-judge numbers: the scripted
/// judge replays the labels, so the win is the pipeline's, not a live
/// judge's. A promotion still needs a measured run (see `eval::scripted`).
struct ThinkingBaseline;

#[async_trait::async_trait]
impl Candidate<String, Difficulty> for ThinkingBaseline {
    fn name(&self) -> &'static str {
        "baseline:always-moderate"
    }

    async fn predict(&self, _case: &Case<String, Difficulty>) -> Prediction<Difficulty> {
        Prediction::certain(
            Difficulty::deterministic_default(),
            PredictionSource::Baseline,
        )
    }
}

struct ThinkingCandidate {
    classifier: Arc<DifficultyClassifier>,
}

#[async_trait::async_trait]
impl Candidate<String, Difficulty> for ThinkingCandidate {
    fn name(&self) -> &'static str {
        "candidate:jev-auto-thinking"
    }

    async fn predict(&self, case: &Case<String, Difficulty>) -> Prediction<Difficulty> {
        match self.classifier.classify(&case.input).await {
            Some(difficulty) => Prediction {
                label: difficulty,
                confidence: None,
                elapsed: None,
                source: PredictionSource::Judge,
            },
            None => Prediction::certain(
                Difficulty::deterministic_default(),
                PredictionSource::Fallback,
            ),
        }
    }
}

fn scripted_thinking_judge() -> Arc<dyn Judge> {
    let corpus = thinking_corpus().expect("fixture");
    let entries = corpus
        .dev()
        .into_iter()
        .chain(corpus.test())
        .map(|case| ScriptEntry {
            key: case.input.clone(),
            answers: BTreeMap::from([(
                QUESTION_ID.to_string(),
                choice_answer(case.label.as_str()),
            )]),
        })
        .collect();
    Arc::new(ScriptedJudge::new("scripted/auto-thinking", entries))
}

#[tokio::test]
async fn the_auto_thinking_corpus_runs_the_gate_end_to_end() {
    let mut corpus = thinking_corpus().expect("fixture");
    let baseline = ThinkingBaseline;
    let candidate = ThinkingCandidate {
        classifier: Arc::new(DifficultyClassifier::new(scripted_thinking_judge())),
    };
    let baseline_dev = corpus.evaluate_dev(&baseline).await;
    let candidate_dev = corpus.evaluate_dev(&candidate).await;
    // The deterministic baseline is right only on the moderate cases.
    let dev_moderate = corpus
        .dev()
        .iter()
        .filter(|case| case.label == Difficulty::Moderate)
        .count();
    assert_eq!(baseline_dev.correct, dev_moderate);
    assert_eq!(
        candidate_dev.correct,
        corpus.dev().len(),
        "the pipeline maps every case"
    );
    let thresholds = TunedThresholds::tune(
        &baseline_dev,
        &candidate_dev,
        &TuningPolicy {
            min_cases: 10,
            ..TuningPolicy::default()
        },
    )
    .expect("dev supports the win");

    let test = corpus.unseal_test().expect("one touch");
    let baseline_test = test.evaluate(&baseline).await;
    let candidate_test = test.evaluate(&candidate).await;
    assert_eq!(
        candidate_test.fallback, 0,
        "the scripted judge never failed"
    );
    let decision = gate(&baseline_test, &candidate_test, &thresholds);
    let GateDecision::Promote {
        accuracy_delta_points,
        ..
    } = decision
    else {
        panic!("the pipeline wins on the frozen test split, got {decision:?}");
    };
    // The win is the label spread: the baseline only gets `moderate` right.
    let test_moderate = corpus
        .test()
        .iter()
        .filter(|case| case.label == Difficulty::Moderate)
        .count();
    let expected =
        (corpus.test().len() - test_moderate) as f64 / corpus.test().len() as f64 * 100.0;
    assert!(
        (accuracy_delta_points - expected).abs() < 1e-6,
        "{accuracy_delta_points} vs {expected}"
    );
}

#[tokio::test]
async fn the_auto_thinking_candidate_falls_back_without_a_judge() {
    // A judge with no script for the state: every case takes the
    // deterministic path, so the candidate ties the baseline - and a tie
    // never promotes.
    let mut corpus = thinking_corpus().expect("fixture");
    let baseline = ThinkingBaseline;
    let candidate = ThinkingCandidate {
        classifier: Arc::new(DifficultyClassifier::new(Arc::new(ScriptedJudge::new(
            "scripted/empty",
            Vec::new(),
        )))),
    };
    let test = corpus.unseal_test().expect("one touch");
    let baseline_test = test.evaluate(&baseline).await;
    let candidate_test = test.evaluate(&candidate).await;
    assert_eq!(candidate_test.fallback, candidate_test.cases);
    assert_eq!(candidate_test.judged, 0);
    assert!((candidate_test.accuracy - baseline_test.accuracy).abs() < f64::EPSILON);
    let thresholds = TunedThresholds {
        accuracy_delta_points: 1.0,
        max_p95_latency_ratio: 2.0,
        min_cases: 4,
        tuned_on: corpus.fingerprint(),
        dev_delta_points: 10.0,
    };
    assert!(matches!(
        gate(&baseline_test, &candidate_test, &thresholds),
        GateDecision::Reject(GateRejection::AccuracyDelta { .. })
    ));
}

/// The graph re-rank baseline: the store's own order (top-1 wins).
struct RerankBaseline;

#[async_trait::async_trait]
impl Candidate<crate::jev::eval::corpus::RerankCase, usize> for RerankBaseline {
    fn name(&self) -> &'static str {
        "baseline:rrf-top1"
    }

    async fn predict(
        &self,
        _case: &Case<crate::jev::eval::corpus::RerankCase, usize>,
    ) -> Prediction<usize> {
        Prediction::certain(0, PredictionSource::Baseline)
    }
}

/// The re-rank candidate: the top-1 survivor of the judge's filter.
struct RerankCandidateEval {
    reranker: Arc<GraphReranker>,
}

#[async_trait::async_trait]
impl Candidate<crate::jev::eval::corpus::RerankCase, usize> for RerankCandidateEval {
    fn name(&self) -> &'static str {
        "candidate:jev-rerank"
    }

    async fn predict(
        &self,
        case: &Case<crate::jev::eval::corpus::RerankCase, usize>,
    ) -> Prediction<usize> {
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
            Some(outcome) => Prediction {
                // The consumer's answer is the first survivor.
                label: outcome.kept.first().copied().unwrap_or_default(),
                confidence: None,
                elapsed: None,
                source: PredictionSource::Judge,
            },
            None => Prediction::certain(0, PredictionSource::Fallback),
        }
    }
}

/// The scripted re-rank judge: every candidate of a case gets a high
/// probability except the labelled one's distractors, which get a low one.
/// The script is derived from the corpus (the "blind label" is the only
/// input), so a case's answer never depends on the pipeline under test.
fn scripted_rerank_judge() -> Arc<dyn Judge> {
    let corpus = rerank_corpus().expect("fixture");
    let entries = corpus
        .dev()
        .into_iter()
        .chain(corpus.test())
        .map(|case| {
            let answers = case
                .input
                .candidates
                .iter()
                .enumerate()
                .map(|(index, _spec)| {
                    let probability = if index == case.label { 0.95 } else { 0.02 };
                    (question_id(index), Answer::Noul { noul: probability })
                })
                .collect();
            ScriptEntry {
                key: case.input.query.clone(),
                answers,
            }
        })
        .collect();
    Arc::new(ScriptedJudge::new("scripted/graph-rerank", entries))
}

/// The polarity flip (ADR rule 9): a judge that answers the question
/// backwards must LOSE to the baseline, not sneak through. This is the test
/// that catches a pipeline keying on position instead of on the answer.
fn polarity_flipped_rerank_judge() -> Arc<dyn Judge> {
    let corpus = rerank_corpus().expect("fixture");
    let entries = corpus
        .dev()
        .into_iter()
        .chain(corpus.test())
        .map(|case| {
            let answers = case
                .input
                .candidates
                .iter()
                .enumerate()
                .map(|(index, _spec)| {
                    let probability = if index == case.label { 0.02 } else { 0.95 };
                    (question_id(index), Answer::Noul { noul: probability })
                })
                .collect();
            ScriptEntry {
                key: case.input.query.clone(),
                answers,
            }
        })
        .collect();
    Arc::new(ScriptedJudge::new("scripted/graph-rerank-flipped", entries))
}

#[tokio::test]
async fn a_polarity_flipped_judge_loses_the_gate() {
    let corpus = rerank_corpus().expect("fixture");
    let baseline = RerankBaseline;
    let flipped = RerankCandidateEval {
        reranker: Arc::new(GraphReranker::new(polarity_flipped_rerank_judge())),
    };
    // On dev the flipped candidate is strictly worse than the baseline, so
    // tuning cannot even produce thresholds from it.
    let baseline_dev = corpus.evaluate_dev(&baseline).await;
    let flipped_dev = corpus.evaluate_dev(&flipped).await;
    assert!(
        flipped_dev.accuracy < baseline_dev.accuracy,
        "an inverted judge scores below the baseline: {} vs {}",
        flipped_dev.accuracy,
        baseline_dev.accuracy
    );
    assert!(
        TunedThresholds::tune(
            &baseline_dev,
            &flipped_dev,
            &TuningPolicy {
                min_cases: 10,
                ..TuningPolicy::default()
            },
        )
        .is_err(),
        "a losing candidate cannot tune thresholds"
    );
}

#[tokio::test]
async fn the_graph_rerank_corpus_runs_the_gate_end_to_end() {
    let mut corpus = rerank_corpus().expect("fixture");
    let baseline = RerankBaseline;
    let candidate = RerankCandidateEval {
        reranker: Arc::new(GraphReranker::new(scripted_rerank_judge())),
    };
    let baseline_dev = corpus.evaluate_dev(&baseline).await;
    let candidate_dev = corpus.evaluate_dev(&candidate).await;
    // The baseline is right only where index 0 is the answer.
    let dev_top1 = corpus.dev().iter().filter(|case| case.label == 0).count();
    assert_eq!(baseline_dev.correct, dev_top1);
    assert_eq!(candidate_dev.correct, corpus.dev().len());
    let thresholds = TunedThresholds::tune(
        &baseline_dev,
        &candidate_dev,
        &TuningPolicy {
            min_cases: 10,
            ..TuningPolicy::default()
        },
    )
    .expect("dev supports the win");
    let test = corpus.unseal_test().expect("one touch");
    let baseline_test = test.evaluate(&baseline).await;
    let candidate_test = test.evaluate(&candidate).await;
    assert_eq!(candidate_test.fallback, 0);
    assert!(
        gate(&baseline_test, &candidate_test, &thresholds).is_promote(),
        "the re-rank wins on the frozen test split"
    );
}

// ─────────────────────── tool arbiter ───────────────────────

/// The arbiter baseline: never suggest anything (`bash` for every case) -
/// exactly what the fork does today.
struct ArbiterBaseline;

#[async_trait::async_trait]
impl Candidate<ArbiterCase, ToolRoute> for ArbiterBaseline {
    fn name(&self) -> &'static str {
        "baseline:never-suggest"
    }

    async fn predict(&self, _case: &Case<ArbiterCase, ToolRoute>) -> Prediction<ToolRoute> {
        Prediction::certain(ToolRoute::Bash, PredictionSource::Baseline)
    }
}

/// Arm an arbiter for one case's roster (the table alone: the corpus scores
/// the deterministic layer, which is the one that always runs).
fn arbiter_for(case: &ArbiterCase) -> Arc<ToolArbiter> {
    let (graph, lsp, edit) = case.available;
    let arbiter = ToolArbiter::new().with_timeout(ARBITER_TIMEOUT);
    arbiter.arm(ToolAvailability { graph, lsp, edit }, None);
    Arc::new(arbiter)
}

/// A candidate that arms the arbiter per case from the case's own roster (a
/// session's roster is fixed, but the corpus spans rosters so the table's
/// "never suggest an absent tool" rule is exercised).
struct RosterAwareArbiter;

#[async_trait::async_trait]
impl Candidate<ArbiterCase, ToolRoute> for RosterAwareArbiter {
    fn name(&self) -> &'static str {
        "candidate:jev-tool-arbiter"
    }

    async fn predict(&self, case: &Case<ArbiterCase, ToolRoute>) -> Prediction<ToolRoute> {
        let arbiter = arbiter_for(&case.input);
        match arbiter.note(&case.input.command).await {
            Some(note) => Prediction {
                label: ToolRoute::of(note.suggestion.tool),
                confidence: None,
                elapsed: None,
                source: PredictionSource::Judge,
            },
            None => Prediction::certain(ToolRoute::Bash, PredictionSource::Fallback),
        }
    }
}

#[tokio::test]
async fn the_tool_arbiter_corpus_runs_the_gate_end_to_end() {
    let mut corpus = arbiter_corpus().expect("fixture");
    let baseline = ArbiterBaseline;
    let candidate = RosterAwareArbiter;
    let baseline_dev = corpus.evaluate_dev(&baseline).await;
    let candidate_dev = corpus.evaluate_dev(&candidate).await;
    // The baseline is right only on the cases where the shell was the
    // surface; the table routes every specialized case.
    let dev_bash = corpus
        .dev()
        .iter()
        .filter(|case| case.label == ToolRoute::Bash)
        .count();
    assert_eq!(baseline_dev.correct, dev_bash);
    assert_eq!(
        candidate_dev.correct,
        corpus.dev().len(),
        "the table routes every dev case: {:?}",
        candidate_dev.misclassified
    );
    let thresholds = TunedThresholds::tune(
        &baseline_dev,
        &candidate_dev,
        &TuningPolicy {
            min_cases: 6,
            ..TuningPolicy::default()
        },
    )
    .expect("dev supports the win");
    let test = corpus.unseal_test().expect("one touch");
    let baseline_test = test.evaluate(&baseline).await;
    let candidate_test = test.evaluate(&candidate).await;
    assert_eq!(
        candidate_test.misclassified,
        Vec::new(),
        "every test case routes as labelled"
    );
    assert!(matches!(
        gate(&baseline_test, &candidate_test, &thresholds),
        GateDecision::Promote { .. }
    ));
}

#[tokio::test]
async fn a_judge_that_never_answers_leaves_the_table_standing() {
    // The judgment layer is armed but its judge always fails: every case
    // falls back to the table's own note (the arbiter's deterministic
    // layer), which still routes as labelled - and the fallback counter
    // proves the layer was actually asked.
    let corpus = arbiter_corpus().expect("fixture");
    let failing: Arc<dyn Judge> = Arc::new(ScriptedJudge::new("scripted/empty", Vec::new()));
    let mut judged = 0usize;
    let mut correct = 0usize;
    for case in corpus.dev().iter().chain(corpus.test().iter()) {
        let (graph, lsp, edit) = case.input.available;
        let arbiter = ToolArbiter::new();
        arbiter.arm(
            ToolAvailability { graph, lsp, edit },
            Some(Arc::clone(&failing)),
        );
        let prediction = match arbiter.note(&case.input.command).await {
            Some(note) => {
                assert!(note.verdict.is_none(), "a failing judge invents nothing");
                judged += 1;
                ToolRoute::of(note.suggestion.tool)
            }
            None => ToolRoute::Bash,
        };
        if prediction == case.label {
            correct += 1;
        }
    }
    assert!(judged > 0, "the judge was consulted");
    assert_eq!(
        correct,
        corpus.dev().len() + corpus.test().len(),
        "a broken judge degrades to the table, not to silence"
    );
}

#[test]
fn the_tool_arbiter_gate_demands_a_native_judge() {
    let native = ChainJudge::new(vec![JudgeCandidate {
        identity: "typesafe/jev".to_string(),
        kind: JudgeKind::Native,
        judge: None,
    }]);
    let prompted = ChainJudge::new(vec![JudgeCandidate {
        identity: "local/model".to_string(),
        kind: JudgeKind::Prompted,
        judge: None,
    }]);
    assert!(!should_run_tool_arbiter(
        JevEnabledSetting::Off,
        JevToolArbiterSetting::Auto,
        Some(&native)
    ));
    assert!(!should_run_tool_arbiter(
        JevEnabledSetting::On,
        JevToolArbiterSetting::Off,
        Some(&native)
    ));
    assert!(!should_run_tool_arbiter(
        JevEnabledSetting::On,
        JevToolArbiterSetting::Auto,
        None
    ));
    assert!(!should_run_tool_arbiter(
        JevEnabledSetting::On,
        JevToolArbiterSetting::Auto,
        Some(&prompted)
    ));
    assert!(should_run_tool_arbiter(
        JevEnabledSetting::On,
        JevToolArbiterSetting::Auto,
        Some(&native)
    ));
}
