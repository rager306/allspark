//! Phase-2 consumer: the auto-thinking difficulty classifier (fork-only).
//!
//! One `choice` question over a three-way menu (`trivial | moderate | hard`
//! plus an explicit `unsure` escape) about the preprocessed user prompt; the
//! pick maps to a thinking level (`low | medium | high`). Ported from omp's
//! `auto-thinking/classifier.ts` with this fork's rules:
//!
//! - **Advisory, never blocking**: any failure (no judge, judge error,
//!   timeout, unparseable pick) leaves the turn's current level untouched and
//!   returns `None`. The caller runs the turn exactly as it would have.
//! - **Gated off by default**: `[jev] autoThinking` defaults to `off`; `auto`
//!   runs only behind a native (calibrated) judge, so an uncalibrated
//!   stand-in can never silently retune the reasoning budget.
//! - **Bounded**: the prompt is cleaned and middle-truncated to
//!   [`MAX_PROMPT_CHARS`] before it becomes judgment state, and the call is
//!   wrapped in [`AUTO_THINKING_TIMEOUT`].
//! - **Promoted only on a measured win**: the corpus gate in
//!   [`crate::jev::eval`] must promote the classifier on a frozen test split
//!   before this consumer may be enabled by default (`RuFlo` ADR-391).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use pa_agent::types::ThinkingLevel;
use pa_jev::{Answer, JudgmentRequest, JudgmentState, Question};

use crate::jev::{Judge, JudgeError};
use crate::settings::{JevAutoThinkingSetting, JevEnabledSetting};

/// Bound on the judgment state this consumer builds (omp's tiny-message cap).
pub const MAX_PROMPT_CHARS: usize = 2000;
/// The per-turn classification bound: a slow judge must not delay the turn.
pub const AUTO_THINKING_TIMEOUT: Duration = Duration::from_millis(4000);
/// The escape option: the judge may decline, and the deterministic default
/// applies (ADR rule 2 - the menu always names an escape).
pub const UNSURE_OPTION: &str = "unsure";

/// The classifier's three-way answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Difficulty {
    Trivial,
    Moderate,
    Hard,
}

impl Difficulty {
    /// Parse a wire option name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Difficulty> {
        match value.trim().to_ascii_lowercase().as_str() {
            "trivial" => Some(Difficulty::Trivial),
            "moderate" => Some(Difficulty::Moderate),
            "hard" => Some(Difficulty::Hard),
            _ => None,
        }
    }

    /// The wire option name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Difficulty::Trivial => "trivial",
            Difficulty::Moderate => "moderate",
            Difficulty::Hard => "hard",
        }
    }

    /// The thinking level this difficulty buys.
    #[must_use]
    pub fn thinking_level(self) -> ThinkingLevel {
        match self {
            Difficulty::Trivial => ThinkingLevel::Low,
            Difficulty::Moderate => ThinkingLevel::Medium,
            Difficulty::Hard => ThinkingLevel::High,
        }
    }

    /// The deterministic default: what the session does with no judge at all
    /// (and what `unsure`/an unparseable pick resolves to). The value the
    /// corpus baseline encodes.
    #[must_use]
    pub fn deterministic_default() -> Difficulty {
        Difficulty::Moderate
    }
}

impl std::fmt::Display for Difficulty {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What one classification resolved to: a pick, or the judge's explicit
/// `unsure` decline (a legitimate answer, not a failure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifficultyAnswer {
    Picked(Difficulty),
    /// The judge declined; the deterministic default applies and the
    /// failure counter stays put.
    Unsure,
}

/// The classifier over one judge.
pub struct DifficultyClassifier {
    judge: Arc<dyn Judge>,
    timeout: Duration,
}

impl DifficultyClassifier {
    /// Build the classifier over the session's judge chain.
    #[must_use]
    pub fn new(judge: Arc<dyn Judge>) -> Self {
        DifficultyClassifier {
            judge,
            timeout: AUTO_THINKING_TIMEOUT,
        }
    }

    /// Override the per-turn bound (tests use a zero bound: the ready judge
    /// still answers, a hanging one is abandoned without waiting).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Classify `prompt`; `None` on a decline or any failure (fail-open: the
    /// turn keeps its current level).
    pub async fn classify(&self, prompt: &str) -> Option<Difficulty> {
        match self.classify_answer(prompt).await {
            Ok(DifficultyAnswer::Picked(difficulty)) => Some(difficulty),
            // A decline and a failure both leave the level alone.
            Ok(DifficultyAnswer::Unsure) | Err(_) => None,
        }
    }

    /// Classify `prompt`, distinguishing the judge's explicit decline
    /// ([`DifficultyAnswer::Unsure`] - an answer, not a failure) from a real
    /// error.
    ///
    /// # Errors
    ///
    /// [`JudgeError`] from the chain, or [`JudgeError::Timeout`] when the
    /// bound elapses first.
    pub async fn classify_answer(&self, prompt: &str) -> Result<DifficultyAnswer, JudgeError> {
        let request = difficulty_request(prompt);
        let judged = tokio::time::timeout(self.timeout, self.judge.judge(&request)).await;
        let result = match judged {
            Ok(outcome) => outcome?,
            Err(_elapsed) => return Err(JudgeError::Timeout { attempts: 1 }),
        };
        let Some(answer) = result.answers.get(QUESTION_ID) else {
            return Err(JudgeError::Candidate {
                identity: self.judge.label().to_string(),
                message: "the judgment carried no difficulty answer".to_string(),
                status: None,
            });
        };
        match answer {
            Answer::Choice { choice, .. } => match Difficulty::parse(choice) {
                Some(difficulty) => Ok(DifficultyAnswer::Picked(difficulty)),
                None if choice.trim().eq_ignore_ascii_case(UNSURE_OPTION) => {
                    Ok(DifficultyAnswer::Unsure)
                }
                None => Err(JudgeError::Candidate {
                    identity: self.judge.label().to_string(),
                    message: format!("unusable difficulty pick {choice:?}"),
                    status: None,
                }),
            },
            other => Err(JudgeError::Candidate {
                identity: self.judge.label().to_string(),
                message: format!("expected a choice answer, got {:?}", other.kind()),
                status: None,
            }),
        }
    }
}

/// The answer key of the difficulty question.
pub const QUESTION_ID: &str = "difficulty";

/// Build the one-question judgment request for `prompt`.
#[must_use]
pub fn difficulty_request(prompt: &str) -> JudgmentRequest {
    let state = JudgmentState::Json(serde_json::json!({
        "request": preprocess_prompt(prompt),
    }));
    let mut criteria = BTreeMap::new();
    criteria.insert(
        "trivial".to_string(),
        Some(
            "a one-line answer already in the conversation, a lookup, or a \
             mechanical edit: no design choice, no debugging"
                .to_string(),
        ),
    );
    criteria.insert(
        "moderate".to_string(),
        Some(
            "a multi-file change or a routine debugging pass with a clear \
             target and a known shape"
                .to_string(),
        ),
    );
    criteria.insert(
        "hard".to_string(),
        Some(
            "open-ended design, an unclear root cause, a subtle concurrency or \
             performance problem, or a change whose blast radius must be \
             reasoned about"
                .to_string(),
        ),
    );
    criteria.insert(
        UNSURE_OPTION.to_string(),
        Some("the request is too ambiguous or too truncated to place".to_string()),
    );
    JudgmentRequest {
        state,
        questions: BTreeMap::from([(
            QUESTION_ID.to_string(),
            Question::Choice {
                instructions: "How much reasoning does this request need before acting?"
                    .to_string(),
                criteria,
            },
        )]),
    }
}

/// Clean and bound a prompt for judgment: SGR escapes and fenced code blocks
/// drop (they dominate the head without carrying the task), whitespace
/// collapses, and the result is middle-truncated to [`MAX_PROMPT_CHARS`]
/// keeping two thirds of the head and one third of the tail.
#[must_use]
pub fn preprocess_prompt(prompt: &str) -> String {
    let cleaned = strip_fenced_blocks(&strip_ansi(prompt));
    let collapsed = collapse_whitespace(&cleaned);
    truncate_middle(&collapsed, MAX_PROMPT_CHARS)
}

/// Below this many characters the fence-stripped text is too thin to
/// classify (the prompt was essentially one code block): the original wins.
const MIN_STRIPPED_CHARS: usize = 12;

/// Drop SGR escape sequences (`ESC [ ... m`) pasted in from terminals.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            out.push(character);
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next();
        for next in chars.by_ref() {
            if next == 'm' {
                break;
            }
            if !(next.is_ascii_digit() || next == ';') {
                // Not an SGR sequence after all: keep the payload.
                out.push(next);
            }
        }
    }
    out
}

/// Drop fenced code blocks (3+ backticks, an unterminated trailing fence
/// included) when enough prose remains.
fn strip_fenced_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        out.push_str(&rest[..start]);
        out.push(' ');
        let after = &rest[start..];
        let fence_end = after[3..].find("```").map(|index| index + 3);
        let Some(end) = fence_end else {
            rest = "";
            break;
        };
        rest = &after[end + 3..];
    }
    out.push_str(rest);
    if out.trim().chars().count() >= MIN_STRIPPED_CHARS {
        out
    } else {
        text.to_string()
    }
}

/// Collapse horizontal whitespace runs and blank-line runs, then trim.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_lines = 0usize;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_lines += 1;
            if blank_lines > 1 {
                continue;
            }
        } else {
            blank_lines = 0;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
    }
    out.trim().to_string()
}

/// Middle-truncate to `max_chars`, keeping 2/3 from the head and 1/3 from the
/// tail around an explicit omission marker.
fn truncate_middle(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let mut omitted = total - max_chars;
    let mut head_chars = 0usize;
    let mut tail_chars = 0usize;
    // The marker's width depends on the omitted count, which depends on the
    // widths: two passes converge (only the digit count can change).
    for _pass in 0..2 {
        let marker = format!("\n[… {omitted} chars omitted …]\n");
        let kept = max_chars.saturating_sub(marker.chars().count());
        head_chars = (kept.div_ceil(3) * 2).min(kept);
        tail_chars = kept - head_chars;
        omitted = total - head_chars - tail_chars;
    }
    let marker = format!("\n[… {omitted} chars omitted …]\n");
    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text
        .chars()
        .skip(total.saturating_sub(tail_chars))
        .collect();
    format!("{head}{marker}{tail}")
}

/// The auto-thinking gate: `[jev] enabled` must admit the layer, `[jev]
/// autoThinking` must be `auto`, and `auto` (the only value) requires a
/// native judge - the same native-only rule as every other judge-heavy
/// consumer.
#[must_use]
pub fn should_run_auto_thinking(
    enabled: JevEnabledSetting,
    auto_thinking: JevAutoThinkingSetting,
    chain: Option<&crate::jev::ChainJudge>,
) -> bool {
    if auto_thinking == JevAutoThinkingSetting::Off {
        return false;
    }
    let Some(chain) = chain else {
        return false;
    };
    match enabled {
        JevEnabledSetting::Off => false,
        // The consumer's own `auto` demands the calibrated backend even when
        // the layer itself is `on`: an uncalibrated stand-in would retune the
        // reasoning budget against thresholds it cannot reproduce.
        JevEnabledSetting::Auto | JevEnabledSetting::On => chain.starts_native(),
    }
}
