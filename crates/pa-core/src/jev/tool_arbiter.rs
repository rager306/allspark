//! Phase-2 consumer: the tool-selection arbiter (fork-only).
//!
//! The fork ships specialized tools the shell cannot match - `graph` (the
//! indexed code graph: definitions, callers, architecture), `lsp` (symbol
//! resolution through a real language server) and `edit` (the pa-edit engine
//! with no-op / seen-line guards) - yet a model still reaches for
//! `bash("grep -rn ...")` out of habit. Ported from oh-my-pi's
//! `bash-interceptor` table, with this fork's rules (see
//! `.research/omp-tool-steering.md` §2/§6: omp's interceptor *blocks* the
//! call; omp has no cross-tool redirect at all):
//!
//! - **Two layers, both advisory.** Layer 1 is a deterministic regex table
//!   ([`RULES`]) with no judge and no spend: a matching command whose
//!   specialized tool is on the roster yields a [`Suggestion`]. Layer 2 asks
//!   the judge one `choice` (`use_specialized | use_bash | unsure`) over the
//!   command plus the table's suggestion; a confident `use_specialized`
//!   (above [`STRONG_CONFIDENCE`]) strengthens the note's wording.
//! - **Never blocking, never substituting.** The command always runs exactly
//!   as written and the result is the command's own; the note is appended
//!   *after* it (`[tool-arbiter] ... nothing was blocked; the call ran
//!   unchanged`). The fork deliberately does not port omp's `throw ToolError`
//!   or invent the cross-tool redirect omp lacks (ADR §4e asks for a routing
//!   *hint*).
//! - **Conservative matching.** Only the command's *primary segment* (the
//!   text before the first `|`, `;`, `&&` or newline) is read: a pipeline's
//!   later stages are filters, not the action the model chose. Within it a
//!   rule fires only at a command position (so `echo grep` and `sudo rg` are
//!   silent), never when the tool takes its input from a redirect (`grep x <
//!   file`), and never when the rule's veto matches (`tail -f` is monitoring,
//!   not reading).
//! - **Fail-open**: no judge, a judge error, a timeout, or an unparseable
//!   pick leaves the table's own note; after
//!   [`MAX_CONSECUTIVE_FAILURES`] consecutive failures the judgment stops
//!   asking for the rest of the session (the table keeps working).
//! - **Gated like every other consumer**: `[jev] enabled` must admit the
//!   layer and `[jev] toolArbiter` must not be `off`. The table costs
//!   nothing to run, but a user who never enabled the layer still gets a
//!   byte-identical bash result - the arbiter is a jev surface, not an
//!   unconditional rewrite of the shell's output.
//! - **Promoted only on a measured win**: the corpus gate in
//!   [`crate::jev::eval`] scores the table on a frozen split before the
//!   consumer may be enabled by default (`RuFlo` ADR-391).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pa_jev::{Answer, JudgmentRequest, JudgmentState, Question};

use crate::jev::{ChainJudge, Judge, JudgeError};
use crate::settings::{JevEnabledSetting, JevToolArbiterSetting};

/// The per-call judgment bound: the command already finished, so a slow
/// judge must never hold the tool result open.
pub const ARBITER_TIMEOUT: Duration = Duration::from_millis(2500);
/// A `use_specialized` pick must be at least this confident to strengthen
/// the note (below it the table's own wording stands - the judge is not
/// sure enough to add anything).
pub const STRONG_CONFIDENCE: f64 = 0.8;
/// Consecutive judge failures after which the judgment layer suspends for the
/// rest of the session (the deterministic table keeps working).
pub const MAX_CONSECUTIVE_FAILURES: u32 = 2;
/// The command characters that become judgment state (a shell one-liner,
/// not a script: the state is billed per character).
pub const MAX_COMMAND_CHARS: usize = 400;
/// The note's marker: the model and the transcript can tell the hint from
/// the command's own output.
pub const NOTE_PREFIX: &str = "[tool-arbiter] ";
/// Every note says this: the arbiter observes, it never blocks.
pub const NOT_BLOCKED: &str = "nothing was blocked; the call ran unchanged";
/// The answer key of the arbiter question.
pub const QUESTION_ID: &str = "tool";
/// The judge's escape option (ADR rule 2: the menu always names one).
pub const UNSURE_OPTION: &str = "unsure";

/// Which specialized tools the session actually carries. The table never
/// suggests a tool the model cannot call (omp's rule: the block fires only
/// when the suggested tool is in `ctx.toolNames`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolAvailability {
    pub graph: bool,
    pub lsp: bool,
    pub edit: bool,
}

impl ToolAvailability {
    /// No specialized tools: the arbiter stays silent (the pre-jev path).
    pub const NONE: ToolAvailability = ToolAvailability {
        graph: false,
        lsp: false,
        edit: false,
    };

    /// Every specialized tool the fork ships.
    #[must_use]
    pub fn all() -> ToolAvailability {
        ToolAvailability {
            graph: true,
            lsp: true,
            edit: true,
        }
    }

    /// Read the roster's tool names (`active_tool_names`).
    #[must_use]
    pub fn from_tool_names(names: &[String]) -> ToolAvailability {
        let has = |name: &str| names.iter().any(|candidate| candidate == name);
        ToolAvailability {
            graph: has("graph"),
            lsp: has("lsp"),
            edit: has("edit"),
        }
    }

    /// Whether any specialized tool is present (an arbiter with none is
    /// inert: no rule can fire).
    #[must_use]
    pub fn any(&self) -> bool {
        self.graph || self.lsp || self.edit
    }

    /// Whether `tool` is on the roster.
    #[must_use]
    pub fn has(&self, tool: SuggestedTool) -> bool {
        match tool {
            SuggestedTool::GraphSearch => self.graph,
            SuggestedTool::LspFindSymbol => self.lsp,
            SuggestedTool::Edit => self.edit,
        }
    }
}

/// The tool a rule points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SuggestedTool {
    /// `graph` `search` (the indexed code graph).
    GraphSearch,
    /// `lsp` `find_symbol` (language-server symbol resolution).
    LspFindSymbol,
    /// The native `edit` tool (the pa-edit engine).
    Edit,
}

impl SuggestedTool {
    /// The wire name the note shows (`graph.search`, `lsp.find_symbol`,
    /// `edit`).
    #[must_use]
    pub fn tool_name(self) -> &'static str {
        match self {
            SuggestedTool::GraphSearch => "graph.search",
            SuggestedTool::LspFindSymbol => "lsp.find_symbol",
            SuggestedTool::Edit => "edit",
        }
    }
}

impl std::fmt::Display for SuggestedTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.tool_name())
    }
}

/// One deterministic rule: a regex over the shell command, the tool it
/// points at, and the one-line reason the note carries.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Stable identity (`grep-to-graph`), reported in the details and the
    /// corpus.
    pub id: &'static str,
    /// What the command must look like.
    pub pattern: &'static str,
    /// When this matches, the rule does NOT fire (`tail -f`).
    pub veto: Option<&'static str>,
    /// The tool the note suggests.
    pub tool: SuggestedTool,
    /// Why that tool is the better surface (one sentence, no trailing
    /// period).
    pub hint: &'static str,
}

/// The table, in priority order: the first rule whose tool is on the roster
/// and whose pattern matches wins. Ported from omp's
/// `bashInterceptor.patterns` (`exec/settings.ts:17-77`) with the fork's
/// tool names.
pub const RULES: &[Rule] = &[
    Rule {
        id: "grep-to-graph",
        pattern: r"\b(?:grep|egrep|fgrep|rg|ripgrep|ag|ack)\s",
        veto: None,
        tool: SuggestedTool::GraphSearch,
        hint: "the code graph answers symbol and literal queries with definitions, \
               callers and file:line instead of a shell scan",
    },
    Rule {
        id: "grep-to-lsp",
        pattern: r"\b(?:grep|egrep|fgrep|rg|ripgrep|ag|ack)\s",
        veto: None,
        tool: SuggestedTool::LspFindSymbol,
        hint: "`lsp` `find_symbol` resolves a definition by name through the \
               language server, which a text scan cannot follow",
    },
    Rule {
        id: "sed-to-edit",
        pattern: r"\b(?:sed|perl)\b[^|;&\n]*?\s-{1,2}[a-zA-Z]*(?:i|in-place)\b|\bawk\s+-i\s+inplace\b",
        veto: None,
        tool: SuggestedTool::Edit,
        hint: "`edit` applies the change through the pa-edit engine, which \
               refuses no-op and already-seen edits instead of rewriting the file blindly",
    },
    Rule {
        id: "find-to-graph",
        pattern: r"\b(?:find|fd)\s",
        veto: None,
        tool: SuggestedTool::GraphSearch,
        hint: "`graph` enumerates symbols and files from the index instead of \
               walking the tree on every call",
    },
    Rule {
        id: "read-to-edit",
        pattern: r"\b(?:cat|head|tail|bat)\s",
        veto: Some(r"\b(?:tail|cat|head|bat)\s+-\w*[fF]\b"),
        tool: SuggestedTool::Edit,
        hint: "`edit` (or the Python REPL) reads the file with line numbers, \
               which a shell pipe drops",
    },
];

/// The compiled table: `(pattern, veto, rule)` per [`RULES`] entry, built
/// once (regex compilation is the only cost the table has).
fn compiled() -> &'static [(
    fancy_regex::Regex,
    Option<fancy_regex::Regex>,
    &'static Rule,
)] {
    static COMPILED: OnceLock<
        Vec<(
            fancy_regex::Regex,
            Option<fancy_regex::Regex>,
            &'static Rule,
        )>,
    > = OnceLock::new();
    COMPILED.get_or_init(|| {
        RULES
            .iter()
            .map(|rule| {
                let pattern = fancy_regex::Regex::new(rule.pattern)
                    .expect("every arbiter rule pattern compiles (pinned by the tests)");
                let veto = rule
                    .veto
                    .map(|veto| fancy_regex::Regex::new(veto).expect("every veto compiles"));
                (pattern, veto, rule)
            })
            .collect()
    })
}

/// What the table decided for one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suggestion {
    pub tool: SuggestedTool,
    /// The rule that fired (diagnostics + the corpus).
    pub rule: &'static str,
    pub hint: &'static str,
}

impl Suggestion {
    /// The suggested tool's wire name.
    #[must_use]
    pub fn tool_name(self) -> &'static str {
        self.tool.tool_name()
    }
}

/// The deterministic layer: the first rule that matches `command`, points at
/// a tool `available` carries, and is not vetoed.
#[must_use]
pub fn suggest(command: &str, available: ToolAvailability) -> Option<Suggestion> {
    let command = command.trim();
    if command.is_empty() || !available.any() {
        return None;
    }
    // Each script line is its own command; within a line only the primary
    // segment (before the first `|`, `;`, `&&`) is read.
    for line in command.lines() {
        let primary = primary_segment(line);
        if primary.is_empty() {
            continue;
        }
        for (pattern, veto, rule) in compiled() {
            if !available.has(rule.tool) {
                continue;
            }
            let Ok(Some(found)) = pattern.find(primary) else {
                continue;
            };
            if !at_command_position(primary, found.start()) {
                continue;
            }
            if reads_redirected_stdin(primary) {
                continue;
            }
            if veto
                .as_ref()
                .is_some_and(|veto| matches!(veto.find(primary), Ok(Some(_))))
            {
                continue;
            }
            return Some(Suggestion {
                tool: rule.tool,
                rule: rule.id,
                hint: rule.hint,
            });
        }
    }
    None
}

/// The line's primary segment: everything before the first top-level `|`,
/// `;` or `&` (quotes respected, so `echo "a | b"` stays one segment). A
/// pipeline's later stages are filters over the first one - the arbiter
/// judges the action the model chose, not what it piped it into.
#[must_use]
pub fn primary_segment(command: &str) -> &str {
    let mut quote: Option<char> = None;
    for (index, character) in command.char_indices() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None => match character {
                '\'' | '"' => quote = Some(character),
                '|' | ';' | '&' | '\n' => return command.get(..index).unwrap_or_default().trim(),
                _ => {}
            },
        }
    }
    command.trim()
}

/// Whether the match at `start` sits where a command begins: the segment's
/// first word, or right after a shell operator. `echo grep` and `sudo rg`
/// are therefore silent (a wrapper or an argument, not a command).
fn at_command_position(command: &str, start: usize) -> bool {
    let prefix = command.get(..start).unwrap_or_default();
    let trimmed = prefix.trim_end();
    if trimmed.is_empty() {
        // Only whitespace before the match: the segment's first word.
        return true;
    }
    matches!(trimmed.chars().last(), Some(';' | '|' | '&' | '(' | '{'))
}

/// Whether the command feeds the tool from a redirect (`grep x < file`): the
/// scan reads a stream, so an indexed lookup would not answer it. A `<`
/// inside quotes (a comparison in an `awk` program) does not count.
fn reads_redirected_stdin(segment: &str) -> bool {
    let mut quote: Option<char> = None;
    for character in segment.chars() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None => match character {
                '\'' | '"' => quote = Some(character),
                '<' => return true,
                _ => {}
            },
        }
    }
    false
}

/// The judge's three-way answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbiterChoice {
    /// The specialized tool would have served the intent better.
    UseSpecialized,
    /// The shell command was the right call.
    UseBash,
    /// The judge declines (an answer, not a failure).
    Unsure,
}

impl ArbiterChoice {
    /// Parse a wire option name.
    #[must_use]
    pub fn parse(value: &str) -> Option<ArbiterChoice> {
        match value.trim().to_ascii_lowercase().as_str() {
            "use_specialized" | "specialized" => Some(ArbiterChoice::UseSpecialized),
            "use_bash" | "bash" => Some(ArbiterChoice::UseBash),
            UNSURE_OPTION => Some(ArbiterChoice::Unsure),
            _ => None,
        }
    }

    /// The wire option name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ArbiterChoice::UseSpecialized => "use_specialized",
            ArbiterChoice::UseBash => "use_bash",
            ArbiterChoice::Unsure => UNSURE_OPTION,
        }
    }
}

impl std::fmt::Display for ArbiterChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What the judge answered, with the confidence the note reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArbiterVerdict {
    pub choice: ArbiterChoice,
    pub confidence: f64,
}

impl ArbiterVerdict {
    /// A confident `use_specialized` - the only verdict that changes the
    /// note's wording.
    #[must_use]
    pub fn is_strong(self) -> bool {
        self.choice == ArbiterChoice::UseSpecialized && self.confidence > STRONG_CONFIDENCE
    }
}

/// The note appended to a bash result: the table's suggestion plus the
/// judge's verdict when one was obtained.
#[derive(Debug, Clone, PartialEq)]
pub struct ArbiterNote {
    pub suggestion: Suggestion,
    pub verdict: Option<ArbiterVerdict>,
    /// The text appended to the tool result (never a replacement).
    pub text: String,
}

impl ArbiterNote {
    /// The `details` payload: what fired, whether the judge was consulted,
    /// and the note's own text (the transcript keeps the evidence).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "rule": self.suggestion.rule,
            "tool": self.suggestion.tool_name(),
            "judged": self.verdict.is_some(),
            "text": self.text,
        });
        if let Some(verdict) = self.verdict {
            value["verdict"] = serde_json::json!(verdict.choice.as_str());
            value["confidence"] = serde_json::json!(verdict.confidence);
            value["strong"] = serde_json::json!(verdict.is_strong());
        }
        value
    }
}

/// Build the note for `suggestion`, optionally strengthened by `verdict`.
#[must_use]
pub fn note_text(suggestion: Suggestion, verdict: Option<ArbiterVerdict>) -> String {
    match verdict {
        Some(verdict) if verdict.is_strong() => format!(
            "{NOTE_PREFIX}the judge agrees ({:.0}% confident): {}. Suggested tool: `{}`. ({NOT_BLOCKED})",
            verdict.confidence * 100.0,
            suggestion.hint,
            suggestion.tool_name()
        ),
        _ => format!(
            "{NOTE_PREFIX}hint: {}. Suggested tool: `{}`. ({NOT_BLOCKED})",
            suggestion.hint,
            suggestion.tool_name()
        ),
    }
}

/// Build the one-question judgment for `command` (a `choice` over the
/// three-way menu).
#[must_use]
pub fn arbiter_request(command: &str, suggestion: Suggestion) -> JudgmentRequest {
    let state = JudgmentState::Json(serde_json::json!({
        "command": truncate_command(command),
        "shell_tool": "bash",
        "specialized_tool": suggestion.tool_name(),
        "specialized_reason": suggestion.hint,
    }));
    let mut criteria = std::collections::BTreeMap::new();
    criteria.insert(
        ArbiterChoice::UseSpecialized.as_str().to_string(),
        Some(
            "the command's intent is answered better by the specialized tool: it \
             needs symbol resolution, definitions, callers, or a guarded edit"
                .to_string(),
        ),
    );
    criteria.insert(
        ArbiterChoice::UseBash.as_str().to_string(),
        Some(
            "the shell command is the right surface: an ad-hoc pipeline, a \
             process or build command, or something no specialized tool covers"
                .to_string(),
        ),
    );
    criteria.insert(
        UNSURE_OPTION.to_string(),
        Some("the command is too ambiguous or too truncated to place".to_string()),
    );
    JudgmentRequest {
        state,
        questions: std::collections::BTreeMap::from([(
            QUESTION_ID.to_string(),
            Question::Choice {
                instructions: "Would the specialized tool have served this command's intent \
                               better than the shell command?"
                    .to_string(),
                criteria,
            },
        )]),
    }
}

/// Bound the judgment state to [`MAX_COMMAND_CHARS`] (head-kept: the command
/// word and its arguments carry the intent).
#[must_use]
pub fn truncate_command(command: &str) -> String {
    let trimmed = command.trim();
    if trimmed.chars().count() <= MAX_COMMAND_CHARS {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(MAX_COMMAND_CHARS).collect();
    out.push('…');
    out
}

/// The armed state: what the session's roster carries, and the judge that
/// may be asked (set once, after the roster is final).
struct Armed {
    availability: ToolAvailability,
    judge: Option<Arc<dyn Judge>>,
}

/// The arbiter over one session's roster: the deterministic table plus the
/// optional judgment layer.
pub struct ToolArbiter {
    armed: Mutex<Armed>,
    timeout: Duration,
    failures: AtomicU32,
}

impl std::fmt::Debug for ToolArbiter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolArbiter")
            .field("availability", &self.availability())
            .field("judged", &self.has_judge())
            .field("failures", &self.consecutive_failures())
            .finish_non_exhaustive()
    }
}

impl ToolArbiter {
    /// An inert arbiter: no tools, no judge (the pre-arm state; it emits
    /// nothing until [`ToolArbiter::arm`] runs).
    #[must_use]
    pub fn new() -> ToolArbiter {
        ToolArbiter {
            armed: Mutex::new(Armed {
                availability: ToolAvailability::NONE,
                judge: None,
            }),
            timeout: ARBITER_TIMEOUT,
            failures: AtomicU32::new(0),
        }
    }

    /// Override the per-call judgment bound (tests use a zero bound: a ready
    /// judge still answers, a hanging one is abandoned without waiting).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Arm the arbiter once the roster is final: the tools it may suggest and
    /// the judge it may ask (`None` = the table alone).
    pub fn arm(&self, availability: ToolAvailability, judge: Option<Arc<dyn Judge>>) {
        let mut armed = self
            .armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        armed.availability = availability;
        armed.judge = judge;
    }

    /// The armed roster.
    #[must_use]
    pub fn availability(&self) -> ToolAvailability {
        self.armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .availability
    }

    /// Whether a judge is armed (the second layer runs).
    #[must_use]
    pub fn has_judge(&self) -> bool {
        self.armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .judge
            .is_some()
    }

    /// The consecutive judge-failure count (diagnostics + tests).
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.failures.load(Ordering::Relaxed)
    }

    /// Whether the judgment layer suspended itself after repeated failures.
    #[must_use]
    pub fn is_suspended(&self) -> bool {
        self.consecutive_failures() >= MAX_CONSECUTIVE_FAILURES
    }

    /// The table's suggestion for `command` (layer 1, no judge, no spend).
    #[must_use]
    pub fn suggest(&self, command: &str) -> Option<Suggestion> {
        suggest(command, self.availability())
    }

    /// Ask the judge whether the specialized tool would have served the
    /// command better; `None` on any failure (fail-open) or while suspended.
    ///
    /// # Errors
    ///
    /// [`JudgeError`] from the chain, or [`JudgeError::Timeout`] when the
    /// bound elapses first.
    pub async fn judge_choice(
        &self,
        command: &str,
        suggestion: Suggestion,
    ) -> Result<Option<ArbiterVerdict>, JudgeError> {
        let judge = {
            let armed = self
                .armed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            armed.judge.as_ref().map(Arc::clone)
        };
        let Some(judge) = judge else {
            return Ok(None);
        };
        if self.is_suspended() {
            return Ok(None);
        }
        let request = arbiter_request(command, suggestion);
        let judged = tokio::time::timeout(self.timeout, judge.judge(&request)).await;
        let result = match judged {
            Ok(outcome) => outcome?,
            Err(_elapsed) => return Err(JudgeError::Timeout { attempts: 1 }),
        };
        let Some(answer) = result.answers.get(QUESTION_ID) else {
            return Err(JudgeError::Candidate {
                identity: judge.label().to_string(),
                message: "the judgment carried no arbiter answer".to_string(),
                status: None,
            });
        };
        let Answer::Choice {
            choice, confidence, ..
        } = answer
        else {
            return Err(JudgeError::Candidate {
                identity: judge.label().to_string(),
                message: format!("expected a choice answer, got {:?}", answer.kind()),
                status: None,
            });
        };
        let Some(choice) = ArbiterChoice::parse(choice) else {
            return Err(JudgeError::Candidate {
                identity: judge.label().to_string(),
                message: format!("unusable arbiter pick {choice:?}"),
                status: None,
            });
        };
        Ok(Some(ArbiterVerdict {
            choice,
            confidence: *confidence,
        }))
    }

    /// The full note for `command`, or `None` when no rule fires.
    ///
    /// Advisory by contract: every judgment failure resolves to the table's
    /// own note (and counts toward the suspension bound) and never fails the
    /// caller's tool result.
    pub async fn note(&self, command: &str) -> Option<ArbiterNote> {
        let suggestion = self.suggest(command)?;
        let verdict = match self.judge_choice(command, suggestion).await {
            Ok(verdict) => {
                if verdict.is_some() {
                    self.failures.store(0, Ordering::Relaxed);
                }
                verdict
            }
            Err(error) => {
                let failures = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    tracing::warn!(
                        failures,
                        error = %error,
                        "tool-arbiter: judge failed repeatedly; suspending for this session"
                    );
                }
                None
            }
        };
        Some(ArbiterNote {
            suggestion,
            verdict,
            text: note_text(suggestion, verdict),
        })
    }
}

impl Default for ToolArbiter {
    fn default() -> Self {
        ToolArbiter::new()
    }
}

/// The arbiter's gate: `[jev] enabled` must admit the layer, `[jev]
/// toolArbiter` must not be `off`, and the chain must start native - the same
/// native-only rule as every other judge-heavy consumer (an uncalibrated
/// stand-in would strengthen notes against thresholds it cannot reproduce).
#[must_use]
pub fn should_run_tool_arbiter(
    enabled: JevEnabledSetting,
    arbiter: JevToolArbiterSetting,
    chain: Option<&ChainJudge>,
) -> bool {
    if arbiter == JevToolArbiterSetting::Off {
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

/// Whether the layer's settings alone admit an arbiter (the early
/// construction gate: the object is created before the roster and the judge
/// resolve, then armed by [`should_run_tool_arbiter`]).
#[must_use]
pub fn arbiter_admitted(enabled: JevEnabledSetting, arbiter: JevToolArbiterSetting) -> bool {
    enabled != JevEnabledSetting::Off && arbiter != JevToolArbiterSetting::Off
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;

    use pa_jev::{JudgmentResult, Usage};

    /// A judge that answers the arbiter question from a fixed verdict and
    /// counts its calls (readiness by observed state, never a sleep).
    struct VerdictJudge {
        answer: Mutex<Option<Answer>>,
        calls: AtomicUsize,
    }

    impl VerdictJudge {
        fn new(choice: &str, confidence: f64) -> Arc<VerdictJudge> {
            Arc::new(VerdictJudge {
                answer: Mutex::new(Some(Answer::Choice {
                    choice: choice.to_string(),
                    probabilities: BTreeMap::from([(choice.to_string(), confidence)]),
                    confidence,
                })),
                calls: AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl Judge for VerdictJudge {
        fn label(&self) -> &'static str {
            "scripted/arbiter"
        }

        async fn judge(&self, _request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let answer = self
                .answer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .ok_or_else(|| JudgeError::Candidate {
                    identity: "scripted/arbiter".to_string(),
                    message: "the script is empty".to_string(),
                    status: None,
                })?;
            Ok(JudgmentResult {
                api: "scripted".to_string(),
                provider: "scripted".to_string(),
                model: "scripted/arbiter".to_string(),
                answers: BTreeMap::from([(QUESTION_ID.to_string(), answer)]),
                usage: Usage::default(),
            })
        }
    }

    fn armed(judge: Option<Arc<dyn Judge>>) -> ToolArbiter {
        let arbiter = ToolArbiter::new();
        arbiter.arm(ToolAvailability::all(), judge);
        arbiter
    }

    #[test]
    fn every_rule_pattern_compiles_and_the_table_has_no_duplicate_ids() {
        let mut ids = std::collections::BTreeSet::new();
        for (pattern, veto, rule) in compiled() {
            assert!(
                ids.insert(rule.id),
                "the rule id {:?} appears twice",
                rule.id
            );
            assert!(
                pattern.as_str() == rule.pattern,
                "the compiled pattern is the rule's"
            );
            if let Some(veto) = veto {
                assert_eq!(
                    Some(veto.as_str()),
                    rule.veto,
                    "the veto compiled as written"
                );
            }
        }
        assert_eq!(compiled().len(), RULES.len());
    }

    #[test]
    fn the_table_routes_each_shell_habit_to_its_specialized_tool() {
        for (command, tool) in [
            (r#"grep -rn "fn main" src"#, SuggestedTool::GraphSearch),
            (r#"rg "Judge" crates/"#, SuggestedTool::GraphSearch),
            ("ack TODO src", SuggestedTool::GraphSearch),
            (r#"find . -name "*.rs""#, SuggestedTool::GraphSearch),
            ("fd cache", SuggestedTool::GraphSearch),
            (r"sed -i 's/a/b/' src/lib.rs", SuggestedTool::Edit),
            (r"perl -pi -e 's/a/b/' src/lib.rs", SuggestedTool::Edit),
            ("awk -i inplace '{print}' src/lib.rs", SuggestedTool::Edit),
            ("cat src/lib.rs", SuggestedTool::Edit),
            ("head -n 20 src/lib.rs", SuggestedTool::Edit),
        ] {
            let suggestion = suggest(command, ToolAvailability::all())
                .unwrap_or_else(|| panic!("{command:?} must fire a rule"));
            assert_eq!(suggestion.tool, tool, "{command:?}");
        }
    }

    #[test]
    fn an_unrelated_command_gets_no_note() {
        for command in [
            "",
            "   ",
            "cargo test -p pa-core --lib",
            "git status --porcelain",
            "make fmt",
            "ls -la",
        ] {
            assert!(
                suggest(command, ToolAvailability::all()).is_none(),
                "{command:?} is not a specialized-tool habit"
            );
        }
    }

    #[test]
    fn a_tool_the_roster_lacks_is_never_suggested() {
        // No `graph`: the same scan routes to `lsp` instead (the note must
        // point at a tool the model can actually call).
        let lsp_only = ToolAvailability {
            graph: false,
            lsp: true,
            edit: false,
        };
        let suggestion = suggest("grep -rn foo src", lsp_only).expect("lsp covers it");
        assert_eq!(suggestion.tool, SuggestedTool::LspFindSymbol);
        // Nothing at all: no note, whatever the command.
        assert!(suggest("grep -rn foo src", ToolAvailability::NONE).is_none());
        // `edit` absent: the sed habit has no reachable tool.
        assert!(suggest(
            "sed -i 's/a/b/' x.rs",
            ToolAvailability {
                graph: true,
                lsp: true,
                edit: false,
            }
        )
        .is_none());
    }

    #[test]
    fn the_roster_reads_the_active_tool_names() {
        let names = vec![
            "python_repl".to_string(),
            "graph".to_string(),
            "edit".to_string(),
        ];
        let availability = ToolAvailability::from_tool_names(&names);
        assert_eq!(
            availability,
            ToolAvailability {
                graph: true,
                lsp: false,
                edit: true,
            }
        );
        assert!(availability.any());
        assert!(!ToolAvailability::from_tool_names(&[]).any());
    }

    #[test]
    fn only_the_primary_segment_is_read() {
        // A pipeline's later stage is a filter, not the action: `cat | grep`
        // and `git log | grep` stay shell work.
        assert_eq!(primary_segment("cat a.rs | grep fn"), "cat a.rs");
        assert!(suggest("git log --oneline | grep jev", ToolAvailability::all()).is_none());
        // A quoted pipe does not split the segment.
        assert_eq!(primary_segment(r#"echo "a | b""#), r#"echo "a | b""#);
        // ... and a command after `;` or `&&` is not read either (the model
        // did not start the command there).
        assert!(suggest("make fmt && grep -rn x src", ToolAvailability::all()).is_none());
    }

    #[test]
    fn a_match_inside_a_string_or_after_a_wrapper_is_silent() {
        assert!(suggest(r#"echo "grep -rn foo""#, ToolAvailability::all()).is_none());
        assert!(suggest("sudo rg pattern src", ToolAvailability::all()).is_none());
        assert!(suggest("time find . -name x", ToolAvailability::all()).is_none());
        // A command at the start of a script line is read.
        assert_eq!(
            suggest("cd crates\nrg pattern", ToolAvailability::all()).map(|s| s.tool),
            Some(SuggestedTool::GraphSearch)
        );
    }

    #[test]
    fn monitoring_and_redirected_reads_are_vetoed() {
        assert!(suggest("tail -f log/app.log", ToolAvailability::all()).is_none());
        assert!(suggest("cat -n src/lib.rs", ToolAvailability::all()).is_some());
        assert!(suggest("grep pattern < input.txt", ToolAvailability::all()).is_none());
    }

    #[test]
    fn the_note_is_advisory_and_says_so() {
        let suggestion = suggest("grep -rn foo src", ToolAvailability::all()).expect("fires");
        let plain = note_text(suggestion, None);
        assert!(plain.starts_with(NOTE_PREFIX), "{plain}");
        assert!(plain.contains("graph.search"), "{plain}");
        assert!(plain.contains(NOT_BLOCKED), "{plain}");
        // A confident `use_specialized` strengthens the wording...
        let strong = note_text(
            suggestion,
            Some(ArbiterVerdict {
                choice: ArbiterChoice::UseSpecialized,
                confidence: 0.95,
            }),
        );
        assert!(strong.contains("the judge agrees"), "{strong}");
        assert!(strong.contains("95%"), "{strong}");
        // ... and nothing else does (a weak pick, a `use_bash`, an `unsure`).
        for verdict in [
            ArbiterVerdict {
                choice: ArbiterChoice::UseSpecialized,
                confidence: STRONG_CONFIDENCE,
            },
            ArbiterVerdict {
                choice: ArbiterChoice::UseBash,
                confidence: 0.99,
            },
            ArbiterVerdict {
                choice: ArbiterChoice::Unsure,
                confidence: 0.99,
            },
        ] {
            assert_eq!(note_text(suggestion, Some(verdict)), plain);
        }
        // The call was never blocked or rewritten: the note says it ran.
        assert!(plain.contains("ran unchanged"), "{plain}");
    }

    #[test]
    fn the_choice_parses_only_its_three_options() {
        assert_eq!(
            ArbiterChoice::parse("use_specialized"),
            Some(ArbiterChoice::UseSpecialized)
        );
        assert_eq!(
            ArbiterChoice::parse(" USE_BASH "),
            Some(ArbiterChoice::UseBash)
        );
        assert_eq!(ArbiterChoice::parse("unsure"), Some(ArbiterChoice::Unsure));
        for other in ["", "specialized_tool", "yes", "true"] {
            assert_eq!(ArbiterChoice::parse(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_request_is_one_choice_over_the_bounded_command() {
        let suggestion = suggest("grep -rn foo src", ToolAvailability::all()).expect("fires");
        let request = arbiter_request(&"x".repeat(MAX_COMMAND_CHARS * 2), suggestion);
        request.validate().expect("the question shape is valid");
        assert_eq!(request.questions.len(), 1);
        let state = request.state.canonical();
        assert!(state.contains("graph.search"), "{state}");
        assert!(
            state.contains("chars") || state.matches('x').count() <= MAX_COMMAND_CHARS + 1,
            "the command is bounded"
        );
        // The criteria carry the escape option (ADR rule 2).
        let Question::Choice { criteria, .. } = request
            .questions
            .get(QUESTION_ID)
            .expect("the arbiter question")
        else {
            panic!("the arbiter asks a choice");
        };
        assert!(criteria.contains_key(UNSURE_OPTION));
        assert_eq!(criteria.len(), 3);
    }

    #[tokio::test]
    async fn an_inert_arbiter_emits_nothing() {
        let arbiter = ToolArbiter::new();
        assert!(!arbiter.availability().any());
        assert!(!arbiter.has_judge());
        assert!(arbiter.note("grep -rn foo src").await.is_none());
    }

    #[tokio::test]
    async fn a_table_only_arbiter_notes_without_asking_anyone() {
        let arbiter = armed(None);
        let note = arbiter
            .note("grep -rn foo src")
            .await
            .expect("the table fires");
        assert!(note.verdict.is_none());
        assert!(note.text.contains("graph.search"));
        assert!(note.text.contains(NOT_BLOCKED));
        assert_eq!(note.suggestion.rule, "grep-to-graph");
        let details = note.to_json();
        assert_eq!(details["tool"], "graph.search");
        assert_eq!(details["judged"], false);
        assert_eq!(details["rule"], "grep-to-graph");
    }

    #[tokio::test]
    async fn a_confident_specialized_verdict_strengthens_the_note() {
        let judge = VerdictJudge::new("use_specialized", 0.93);
        let arbiter = armed(Some(judge.clone()));
        let note = arbiter.note("rg Judge crates").await.expect("fires");
        assert_eq!(judge.calls(), 1, "one judgment per call");
        let verdict = note.verdict.expect("the judge answered");
        assert!(verdict.is_strong());
        assert!(note.text.contains("the judge agrees"), "{}", note.text);
        assert_eq!(note.to_json()["strong"], true);
        assert_eq!(note.to_json()["confidence"], 0.93);
    }

    #[tokio::test]
    async fn a_bash_verdict_leaves_the_table_note_alone() {
        let judge = VerdictJudge::new("use_bash", 0.9);
        let arbiter = armed(Some(judge.clone()));
        let note = arbiter.note("grep -rn foo src").await.expect("fires");
        assert_eq!(judge.calls(), 1);
        assert_eq!(note.verdict.map(|v| v.choice), Some(ArbiterChoice::UseBash));
        assert!(!note.text.contains("the judge agrees"));
        assert!(note.text.contains(NOT_BLOCKED));
    }

    #[tokio::test]
    async fn a_judge_failure_keeps_the_table_note_and_suspends_the_layer() {
        // The judge answers with an unusable pick: a failure, not a decline.
        let judge = VerdictJudge::new("nonsense", 0.9);
        let arbiter = armed(Some(judge.clone()));
        let first = arbiter
            .note("grep -rn foo src")
            .await
            .expect("the table fires");
        assert!(first.verdict.is_none(), "a failure never invents a verdict");
        assert!(first.text.contains("graph.search"));
        assert_eq!(arbiter.consecutive_failures(), 1);
        let second = arbiter.note("grep -rn bar src").await.expect("fires");
        assert!(second.verdict.is_none());
        assert!(arbiter.is_suspended());
        // Suspended: the table still notes, and the judge is not asked again.
        let calls = judge.calls();
        let third = arbiter.note("grep -rn baz src").await.expect("fires");
        assert!(third.verdict.is_none());
        assert_eq!(judge.calls(), calls, "a suspended layer asks nothing");
    }

    #[tokio::test]
    async fn a_success_resets_the_failure_count() {
        let judge = VerdictJudge::new("use_specialized", 0.9);
        let arbiter = armed(Some(judge));
        let note = arbiter.note("grep -rn foo src").await.expect("fires");
        assert!(note.verdict.is_some());
        assert_eq!(arbiter.consecutive_failures(), 0);
        assert!(!arbiter.is_suspended());
    }

    #[tokio::test]
    async fn a_hanging_judge_is_abandoned_without_holding_the_result() {
        struct HangingJudge;

        #[async_trait::async_trait]
        impl Judge for HangingJudge {
            fn label(&self) -> &'static str {
                "hanging"
            }

            async fn judge(
                &self,
                _request: &JudgmentRequest,
            ) -> Result<JudgmentResult, JudgeError> {
                std::future::pending().await
            }
        }

        let arbiter = armed(Some(Arc::new(HangingJudge))).with_timeout(Duration::ZERO);
        let note = arbiter
            .note("grep -rn foo src")
            .await
            .expect("the table fires");
        assert!(note.verdict.is_none());
        assert_eq!(arbiter.consecutive_failures(), 1, "the timeout counted");
    }

    #[test]
    fn the_gate_demands_a_native_judge() {
        use crate::jev::chain::{JudgeCandidate, JudgeKind};
        use crate::settings::JevToolArbiterSetting;

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
        assert!(
            !should_run_tool_arbiter(
                JevEnabledSetting::On,
                JevToolArbiterSetting::Auto,
                Some(&prompted)
            ),
            "a prompted stand-in never strengthens a note"
        );
        assert!(should_run_tool_arbiter(
            JevEnabledSetting::Auto,
            JevToolArbiterSetting::Auto,
            Some(&native)
        ));
    }

    #[test]
    fn the_setting_parses_conservatively() {
        use crate::settings::JevToolArbiterSetting;

        assert_eq!(
            JevToolArbiterSetting::parse("auto"),
            JevToolArbiterSetting::Auto
        );
        assert_eq!(
            JevToolArbiterSetting::parse("ON"),
            JevToolArbiterSetting::Auto
        );
        for typo in ["", "true", "yes", "1", "enabled"] {
            assert_eq!(
                JevToolArbiterSetting::parse(typo),
                JevToolArbiterSetting::Off,
                "{typo:?} reads as off"
            );
        }
        assert_eq!(
            JevToolArbiterSetting::default(),
            JevToolArbiterSetting::Auto,
            "the default is auto, like the other Phase-2 consumers"
        );
        assert!(arbiter_admitted(
            JevEnabledSetting::Auto,
            JevToolArbiterSetting::Auto
        ));
        assert!(!arbiter_admitted(
            JevEnabledSetting::Off,
            JevToolArbiterSetting::Auto
        ));
    }
}
