//! The offline evaluation harness every judgment consumer must pass before it
//! can be enabled by default (fork-only, Phase 2). See
//! `.research/adr-pa-jev-judge-layer.md` and the `RuFlo` lesson in
//! `.research/ruflo-jev-scan.md` §3: on a real 197-case corpus a local
//! decision router beat the baseline by +0.89 points and was NOT promoted.
//!
//! The harness has four parts, and the shapes - not the docs - enforce the
//! discipline:
//!
//! 1. A **frozen corpus**: hand-labelled cases, blind labels, split into
//!    `dev` (threshold tuning) and `test` (the decision). [`Corpus::fingerprint`]
//!    hashes the case list, so editing a case without re-freezing the
//!    expected hash fails the fixture test.
//! 2. A **baseline candidate**: the deterministic behaviour the consumer
//!    replaces (the current default), scored on the same cases.
//! 3. A **scripted judge**: the [`Judge`] side is a deterministic
//!    case-id -> answer table - no network, no clock, no sleep. The recorded
//!    answers are a *stand-in* for a measured live run, never a substitute
//!    for one (see [`scripted`]).
//! 4. A **gate**: [`TunedThresholds::tune`] accepts dev reports only (it
//!    errors on a test report - that is the structural "tune on dev, decide
//!    on test" rule), and [`gate`] accepts test reports only. A tie or a
//!    sub-threshold win is a rejection, never a promotion.
//!
//! One test touch per corpus: [`Corpus::unseal_test`] hands out one
//! [`TestSplit`] token, and a second unseal on the same corpus is an error.
//! Both the baseline and the candidate are scored inside that single touch.
//!
//! [`Judge`]: crate::jev::Judge

pub mod corpus;
pub mod scripted;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Write as _};
use std::time::Duration;

use serde::Serialize;

/// Which half of a corpus a case belongs to.
///
/// `Dev` tunes thresholds; `Test` decides. Nothing reads both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    Dev,
    Test,
}

impl Split {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Split::Dev => "dev",
            Split::Test => "test",
        }
    }
}

impl Display for Split {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One labelled corpus case: the consumer's input plus the blind label.
#[derive(Debug, Clone)]
pub struct Case<I, L> {
    /// Stable identity: the scripted judge keys its answers on it, and the
    /// fingerprint hashes it.
    pub id: String,
    /// What the consumer sees (a prompt, a query plus candidates, ...).
    pub input: I,
    /// The blind label (assigned before any candidate ran).
    pub label: L,
    pub split: Split,
    /// Free-form markers (`"trap"`, `"negative"`, ...) for per-slice reads.
    pub tags: Vec<String>,
}

/// A frozen corpus: named, versioned, hashable, split.
#[derive(Debug, Clone)]
pub struct Corpus<I, L> {
    name: String,
    version: String,
    cases: Vec<Case<I, L>>,
    test_unsealed: bool,
}

/// Everything that can go wrong before a gate decision exists.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    #[error("corpus {name} has no {split} cases")]
    EmptySplit { name: String, split: Split },
    #[error("corpus {name} repeats case id {id:?}")]
    DuplicateCase { name: String, id: String },
    #[error("corpus {name}: case {id:?} carries a {split} label the dev split never shows")]
    LabelNotTunable {
        name: String,
        id: String,
        split: Split,
    },
    #[error("corpus {name}: the test split was already unsealed (one touch per corpus)")]
    TestAlreadyTouched { name: String },
}

/// A validated corpus: every case id unique, both splits non-empty, every
/// test label also present in dev (a label dev never shows cannot be tuned
/// for - the tuner would pick thresholds blind).
///
/// # Errors
///
/// [`EvalError::EmptySplit`], [`EvalError::DuplicateCase`] or
/// [`EvalError::LabelNotTunable`].
pub fn validate<I, L: PartialEq + Display>(
    name: &str,
    cases: &[Case<I, L>],
) -> Result<(), EvalError> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for case in cases {
        if !seen.insert(case.id.as_str()) {
            return Err(EvalError::DuplicateCase {
                name: name.to_string(),
                id: case.id.clone(),
            });
        }
    }
    for split in [Split::Dev, Split::Test] {
        if !cases.iter().any(|case| case.split == split) {
            return Err(EvalError::EmptySplit {
                name: name.to_string(),
                split,
            });
        }
    }
    let dev_labels: BTreeSet<String> = cases
        .iter()
        .filter(|case| case.split == Split::Dev)
        .map(|case| case.label.to_string())
        .collect();
    for case in cases.iter().filter(|case| case.split == Split::Test) {
        if !dev_labels.contains(&case.label.to_string()) {
            return Err(EvalError::LabelNotTunable {
                name: name.to_string(),
                id: case.id.clone(),
                split: case.split,
            });
        }
    }
    Ok(())
}

impl<I, L> Corpus<I, L>
where
    I: Serialize,
    L: PartialEq + Display,
{
    /// Build and validate a corpus.
    ///
    /// # Errors
    ///
    /// Whatever [`validate`] rejects.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        cases: Vec<Case<I, L>>,
    ) -> Result<Corpus<I, L>, EvalError> {
        let name = name.into();
        validate(&name, &cases)?;
        Ok(Corpus {
            name,
            version: version.into(),
            cases,
            test_unsealed: false,
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.cases.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// The dev cases, in fixture order.
    #[must_use]
    pub fn dev(&self) -> Vec<&Case<I, L>> {
        self.cases
            .iter()
            .filter(|case| case.split == Split::Dev)
            .collect()
    }

    /// The test cases, in fixture order. Reading them without unsealing is
    /// fine (they are published fixtures); *scoring* them needs the token.
    #[must_use]
    pub fn test(&self) -> Vec<&Case<I, L>> {
        self.cases
            .iter()
            .filter(|case| case.split == Split::Test)
            .collect()
    }

    /// The frozen-corpus hash: sha256 over `id | split | label | input` for
    /// every case in fixture order. A fixture test pins it, so any edit to a
    /// case (or its order) is a conscious re-freeze, not a silent drift.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut canonical = String::new();
        for case in &self.cases {
            let input = serde_json::to_string(&case.input).unwrap_or_default();
            let _ = write!(
                canonical,
                "{}\u{1f}{}\u{1f}{}\u{1f}{input}\u{1e}",
                case.id,
                case.split.as_str(),
                case.label
            );
        }
        sha256_hex(canonical.as_bytes())
    }

    /// Open the one test touch this corpus instance allows.
    ///
    /// # Errors
    ///
    /// [`EvalError::TestAlreadyTouched`] on the second call.
    pub fn unseal_test(&mut self) -> Result<TestSplit<'_, I, L>, EvalError> {
        if self.test_unsealed {
            return Err(EvalError::TestAlreadyTouched {
                name: self.name.clone(),
            });
        }
        self.test_unsealed = true;
        Ok(TestSplit {
            corpus: self.name.clone(),
            fingerprint: self.fingerprint(),
            cases: self.test(),
        })
    }
}

impl<I, L> Corpus<I, L>
where
    I: Serialize + Send + Sync,
    L: Ord + Clone + Display + Send + Sync,
{
    /// Score `candidate` on the dev split (threshold tuning only).
    pub async fn evaluate_dev<C>(&self, candidate: &C) -> SplitReport<L>
    where
        C: Candidate<I, L>,
    {
        evaluate(
            &self.name,
            &self.fingerprint(),
            Split::Dev,
            &self.dev(),
            candidate,
        )
        .await
    }
}

/// The single test touch: the token that scores test cases.
pub struct TestSplit<'a, I, L> {
    corpus: String,
    fingerprint: String,
    cases: Vec<&'a Case<I, L>>,
}

impl<I, L> TestSplit<'_, I, L>
where
    I: Send + Sync,
    L: Ord + Clone + Display + Send + Sync,
{
    #[must_use]
    pub fn corpus(&self) -> &str {
        &self.corpus
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.cases.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// Score `candidate` on the test split.
    pub async fn evaluate<C>(&self, candidate: &C) -> SplitReport<L>
    where
        C: Candidate<I, L>,
    {
        evaluate(
            &self.corpus,
            &self.fingerprint,
            Split::Test,
            &self.cases,
            candidate,
        )
        .await
    }
}

/// Where a prediction came from - the honest label the report keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionSource {
    /// The deterministic baseline (the behaviour being replaced).
    Baseline,
    /// The judgment layer answered.
    Judge,
    /// The judgment layer was asked and failed (or was gated off): the
    /// consumer took its deterministic path.
    Fallback,
}

/// One candidate's answer for one case.
#[derive(Debug, Clone)]
pub struct Prediction<L> {
    pub label: L,
    /// Probability the candidate assigned to its own pick, when it has one
    /// (a calibrated judge does; a deterministic baseline does not).
    pub confidence: Option<f64>,
    /// Measured wall time, when the harness measured one. The latency arm
    /// of the gate reads it; `None` disables that arm.
    pub elapsed: Option<Duration>,
    pub source: PredictionSource,
}

impl<L> Prediction<L> {
    /// A deterministic answer with no confidence and no timing.
    #[must_use]
    pub fn certain(label: L, source: PredictionSource) -> Prediction<L> {
        Prediction {
            label,
            confidence: None,
            elapsed: None,
            source,
        }
    }
}

/// A thing being evaluated: a baseline, or a consumer behind a judge.
#[async_trait::async_trait]
pub trait Candidate<I, L> {
    /// Stable name for the report (`baseline:always-moderate`, ...).
    fn name(&self) -> &str;

    /// Answer one case. Implementations never panic on a case: a failure is
    /// a [`Prediction`] on the consumer's deterministic path.
    async fn predict(&self, case: &Case<I, L>) -> Prediction<L>;
}

/// Per-label confusion statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelStats {
    /// Cases whose blind label is this one.
    pub support: usize,
    /// Cases the candidate answered with this one.
    pub predicted: usize,
    pub correct: usize,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
}

/// One case the candidate got wrong (the report's evidence).
#[derive(Debug, Clone, PartialEq)]
pub struct Misclassified<L> {
    pub id: String,
    pub expected: L,
    pub predicted: L,
    pub confidence: Option<f64>,
}

/// The score of one candidate on one split.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitReport<L> {
    pub corpus: String,
    /// The corpus fingerprint the cases were scored from.
    pub fingerprint: String,
    pub split: Split,
    pub candidate: String,
    pub cases: usize,
    pub correct: usize,
    pub accuracy: f64,
    pub macro_f1: f64,
    pub per_label: BTreeMap<L, LabelStats>,
    pub misclassified: Vec<Misclassified<L>>,
    /// How many cases the judgment layer actually answered.
    pub judged: usize,
    /// How many took the deterministic fallback.
    pub fallback: usize,
    /// p95 of the measured per-case elapsed times (`None` when unmeasured).
    pub elapsed_p95: Option<Duration>,
}

impl<L> SplitReport<L> {
    /// The accuracy difference in percentage points (candidate - baseline).
    #[must_use]
    pub fn accuracy_delta_points(&self, baseline: &SplitReport<L>) -> f64 {
        (self.accuracy - baseline.accuracy) * 100.0
    }
}

/// Score `candidate` over `cases` (one split).
pub async fn evaluate<I, L, C>(
    corpus: &str,
    fingerprint: &str,
    split: Split,
    cases: &[&Case<I, L>],
    candidate: &C,
) -> SplitReport<L>
where
    I: Send + Sync,
    L: Ord + Clone + Display + Send + Sync,
    C: Candidate<I, L>,
{
    let mut correct = 0usize;
    let mut judged = 0usize;
    let mut fallback = 0usize;
    let mut confusion: BTreeMap<(L, L), usize> = BTreeMap::new();
    let mut supports: BTreeMap<L, usize> = BTreeMap::new();
    let mut predicted_counts: BTreeMap<L, usize> = BTreeMap::new();
    let mut misclassified = Vec::new();
    let mut elapsed: Vec<Duration> = Vec::new();
    for case in cases {
        let prediction = candidate.predict(case).await;
        if prediction.source == PredictionSource::Judge {
            judged += 1;
        } else if prediction.source == PredictionSource::Fallback {
            fallback += 1;
        }
        if let Some(measured) = prediction.elapsed {
            elapsed.push(measured);
        }
        *supports.entry(case.label.clone()).or_default() += 1;
        *predicted_counts
            .entry(prediction.label.clone())
            .or_default() += 1;
        *confusion
            .entry((case.label.clone(), prediction.label.clone()))
            .or_default() += 1;
        if prediction.label == case.label {
            correct += 1;
        } else {
            misclassified.push(Misclassified {
                id: case.id.clone(),
                expected: case.label.clone(),
                predicted: prediction.label.clone(),
                confidence: prediction.confidence,
            });
        }
    }
    // Macro-F1 over every label the split shows or the candidate invents: a
    // hallucinated label costs, it does not vanish.
    let labels: BTreeSet<L> = supports
        .keys()
        .chain(predicted_counts.keys())
        .cloned()
        .collect();
    let mut per_label = BTreeMap::new();
    let mut f1_sum = 0.0;
    for label in &labels {
        let support = supports.get(label).copied().unwrap_or_default();
        let predicted = predicted_counts.get(label).copied().unwrap_or_default();
        let hit = confusion
            .get(&(label.clone(), label.clone()))
            .copied()
            .unwrap_or_default();
        let precision = ratio(hit, predicted);
        let recall = ratio(hit, support);
        let f1 = if precision + recall > 0.0 {
            2.0 * precision * recall / (precision + recall)
        } else {
            0.0
        };
        f1_sum += f1;
        per_label.insert(
            label.clone(),
            LabelStats {
                support,
                predicted,
                correct: hit,
                precision,
                recall,
                f1,
            },
        );
    }
    let macro_f1 = if labels.is_empty() {
        0.0
    } else {
        f1_sum / labels.len() as f64
    };
    SplitReport {
        corpus: corpus.to_string(),
        fingerprint: fingerprint.to_string(),
        split,
        candidate: candidate.name().to_string(),
        cases: cases.len(),
        correct,
        accuracy: ratio(correct, cases.len()),
        macro_f1,
        per_label,
        misclassified,
        judged,
        fallback,
        elapsed_p95: percentile_95(&mut elapsed),
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

/// The p95 of the measured per-case times (nearest-rank, no interpolation).
fn percentile_95(samples: &mut [Duration]) -> Option<Duration> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let rank = ((samples.len() as f64) * 0.95).ceil() as usize;
    let index = rank.clamp(1, samples.len()) - 1;
    samples.get(index).copied()
}

/// The tuning policy: the grid the dev split may pick a threshold from, plus
/// the latency ceiling the test split must hold.
#[derive(Debug, Clone)]
pub struct TuningPolicy {
    /// Candidate accuracy deltas (percentage points), smallest first.
    pub accuracy_delta_points: Vec<f64>,
    /// How much worse the candidate's p95 may be than the baseline's.
    pub max_p95_latency_ratio: f64,
    /// Safety margin subtracted from the dev delta before a grid value is
    /// accepted (a threshold the dev split only just meets will not hold).
    pub dev_margin_points: f64,
    /// The smallest corpus a decision may rest on.
    pub min_cases: usize,
}

impl Default for TuningPolicy {
    fn default() -> Self {
        TuningPolicy {
            accuracy_delta_points: vec![1.0, 2.0, 3.0, 5.0],
            max_p95_latency_ratio: 2.0,
            dev_margin_points: 1.0,
            min_cases: 12,
        }
    }
}

/// Why tuning could not produce thresholds.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TuningError {
    #[error("thresholds tune on the {actual} split, not {required}")]
    WrongSplit { required: Split, actual: Split },
    #[error("the candidate must beat the baseline on dev by more than the margin: dev delta {dev_delta:.2} points")]
    InsufficientDevDelta { dev_delta: f64 },
    #[error("the reports come from different corpora ({baseline} vs {candidate})")]
    CorpusMismatch { baseline: String, candidate: String },
    #[error("the dev split is too small to tune on: {cases} case(s), the policy needs {required}")]
    TooSmall { cases: usize, required: usize },
    #[error("the tuning policy carries no accuracy delta grid")]
    EmptyGrid,
}

/// Thresholds that only a dev split may produce. [`gate`] takes these, never
/// raw numbers - so "tune on test" is a type error, not a review comment.
#[derive(Debug, Clone, PartialEq)]
pub struct TunedThresholds {
    accuracy_delta_points: f64,
    max_p95_latency_ratio: f64,
    min_cases: usize,
    /// The corpus fingerprint the tuning saw (the gate refuses a report from
    /// another corpus).
    tuned_on: String,
    /// The dev delta the tuning actually observed (diagnostics).
    dev_delta_points: f64,
}

impl TunedThresholds {
    /// Pick the largest grid threshold the dev delta supports, minus the
    /// policy's safety margin.
    ///
    /// # Errors
    ///
    /// [`TuningError::WrongSplit`] when either report is a test report,
    /// [`TuningError::CorpusMismatch`], [`TuningError::TooSmall`],
    /// [`TuningError::EmptyGrid`], or [`TuningError::InsufficientDevDelta`]
    /// when even the smallest grid value is out of reach.
    pub fn tune<L>(
        baseline_dev: &SplitReport<L>,
        candidate_dev: &SplitReport<L>,
        policy: &TuningPolicy,
    ) -> Result<TunedThresholds, TuningError> {
        for report in [baseline_dev, candidate_dev] {
            if report.split != Split::Dev {
                return Err(TuningError::WrongSplit {
                    required: Split::Dev,
                    actual: report.split,
                });
            }
        }
        if baseline_dev.fingerprint != candidate_dev.fingerprint {
            return Err(TuningError::CorpusMismatch {
                baseline: baseline_dev.fingerprint.clone(),
                candidate: candidate_dev.fingerprint.clone(),
            });
        }
        if candidate_dev.cases < policy.min_cases {
            return Err(TuningError::TooSmall {
                cases: candidate_dev.cases,
                required: policy.min_cases,
            });
        }
        let grid = policy
            .accuracy_delta_points
            .iter()
            .copied()
            .filter(|value| value.is_finite() && *value > 0.0)
            .collect::<Vec<f64>>();
        let Some(smallest) = grid.iter().copied().reduce(f64::min) else {
            return Err(TuningError::EmptyGrid);
        };
        let dev_delta = candidate_dev.accuracy_delta_points(baseline_dev);
        let supported = dev_delta - policy.dev_margin_points;
        if supported < smallest {
            return Err(TuningError::InsufficientDevDelta { dev_delta });
        }
        let chosen = grid
            .iter()
            .copied()
            .filter(|value| *value <= supported)
            .reduce(f64::max)
            .unwrap_or(smallest);
        Ok(TunedThresholds {
            accuracy_delta_points: chosen,
            max_p95_latency_ratio: policy.max_p95_latency_ratio,
            min_cases: policy.min_cases,
            tuned_on: candidate_dev.fingerprint.clone(),
            dev_delta_points: dev_delta,
        })
    }

    #[must_use]
    pub fn accuracy_delta_points(&self) -> f64 {
        self.accuracy_delta_points
    }

    #[must_use]
    pub fn max_p95_latency_ratio(&self) -> f64 {
        self.max_p95_latency_ratio
    }

    #[must_use]
    pub fn tuned_on(&self) -> &str {
        &self.tuned_on
    }

    #[must_use]
    pub fn dev_delta_points(&self) -> f64 {
        self.dev_delta_points
    }
}

/// The gate's answer: promote the consumer, or keep the deterministic path.
#[derive(Debug, Clone, PartialEq)]
pub enum GateDecision {
    Promote {
        accuracy_delta_points: f64,
        macro_f1_delta: f64,
        p95_ratio: Option<f64>,
    },
    Reject(GateRejection),
}

impl GateDecision {
    #[must_use]
    pub fn is_promote(&self) -> bool {
        matches!(self, GateDecision::Promote { .. })
    }
}

/// Why a candidate was not promoted.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GateRejection {
    #[error("the gate decides on the {required} split, not {actual}")]
    WrongSplit { required: Split, actual: Split },
    #[error(
        "the test reports come from another corpus than the tuning ({tuned_on} vs {evaluated})"
    )]
    CorpusMismatch { tuned_on: String, evaluated: String },
    #[error("the test split is too small: {cases} case(s), the gate needs {required}")]
    InsufficientCases { cases: usize, required: usize },
    #[error(
        "accuracy delta {delta_points:+.2} points is below the tuned threshold {required_points:.2}"
    )]
    AccuracyDelta {
        delta_points: f64,
        required_points: f64,
    },
    #[error("p95 latency grew {ratio:.2}x, above the {limit:.2}x limit")]
    Latency { ratio: f64, limit: f64 },
}

/// Decide a candidate on the **test** split against the baseline, using
/// thresholds tuned on dev. A tie (delta 0) and a sub-threshold win are both
/// rejections - the `RuFlo` lesson.
#[must_use]
pub fn gate<L>(
    baseline_test: &SplitReport<L>,
    candidate_test: &SplitReport<L>,
    thresholds: &TunedThresholds,
) -> GateDecision {
    for report in [baseline_test, candidate_test] {
        if report.split != Split::Test {
            return GateDecision::Reject(GateRejection::WrongSplit {
                required: Split::Test,
                actual: report.split,
            });
        }
    }
    if baseline_test.fingerprint != candidate_test.fingerprint {
        return GateDecision::Reject(GateRejection::CorpusMismatch {
            tuned_on: thresholds.tuned_on.clone(),
            evaluated: candidate_test.fingerprint.clone(),
        });
    }
    if candidate_test.fingerprint != thresholds.tuned_on {
        return GateDecision::Reject(GateRejection::CorpusMismatch {
            tuned_on: thresholds.tuned_on.clone(),
            evaluated: candidate_test.fingerprint.clone(),
        });
    }
    if candidate_test.cases < thresholds.min_cases {
        return GateDecision::Reject(GateRejection::InsufficientCases {
            cases: candidate_test.cases,
            required: thresholds.min_cases,
        });
    }
    let delta_points = candidate_test.accuracy_delta_points(baseline_test);
    if delta_points < thresholds.accuracy_delta_points {
        return GateDecision::Reject(GateRejection::AccuracyDelta {
            delta_points,
            required_points: thresholds.accuracy_delta_points,
        });
    }
    let p95_ratio = match (baseline_test.elapsed_p95, candidate_test.elapsed_p95) {
        (Some(baseline), Some(candidate)) if !baseline.is_zero() => {
            let ratio = candidate.as_secs_f64() / baseline.as_secs_f64();
            if ratio > thresholds.max_p95_latency_ratio {
                return GateDecision::Reject(GateRejection::Latency {
                    ratio,
                    limit: thresholds.max_p95_latency_ratio,
                });
            }
            Some(ratio)
        }
        _ => None,
    };
    GateDecision::Promote {
        accuracy_delta_points: delta_points,
        macro_f1_delta: candidate_test.macro_f1 - baseline_test.macro_f1,
        p95_ratio,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
