//! The agent-facing `jev` tool: typed decisions through the session's judge
//! chain. FORK-ONLY (not in upstream); gated by `[jev] enabled` (default
//! OFF) at session creation.
//!
//! Four actions over one vocabulary:
//!
//! - `ask`: the general form - a state plus explicit typed questions.
//! - `classify`: sugar - one choice question over an options menu.
//! - `route`: classify with an explicit destination menu; the menu always
//!   carries an `other` escape (the judge never invents a route).
//! - `check`: sugar - one noul question (a calibrated yes/no probability).
//!
//! Hard rules first (ADR): code builds the menu, the judge picks; every menu
//! names an explicit `other`/`none` option (`classify`/`route` append one
//! when missing); arithmetic and dates never go through the judge.

use std::sync::Arc;

use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback, ToolResultContent};
use pa_jev::{JudgmentRequest, JudgmentState, NoulCriteria, Question};

use crate::jev::{Judge, JudgeError};

/// The escape option `classify`/`route` append when the caller's menu
/// carries none.
const ESCAPE_OPTION: &str = "other";

/// The native `jev` tool over the session's resolved judge.
pub struct JevTool {
    judge: Arc<dyn Judge>,
    schema: serde_json::Value,
}

impl JevTool {
    /// Build the tool over `judge`.
    #[must_use]
    pub fn new(judge: Arc<dyn Judge>) -> Self {
        JevTool {
            judge,
            schema: params_schema(),
        }
    }

    async fn dispatch(
        &self,
        params: &serde_json::Value,
        signal: &AbortSignal,
    ) -> anyhow::Result<AgentToolResult> {
        let action = params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .map_or("", str::trim)
            .to_ascii_lowercase();
        let request = match action.as_str() {
            "ask" => Self::request_ask(params)?,
            "classify" | "route" => Self::request_classify(params)?,
            "check" => Self::request_check(params)?,
            other => anyhow::bail!(
                "unknown jev action {other:?}: expected one of ask, classify, route, check"
            ),
        };
        // Hard rules first (ADR): the tool enforces the contract's shape
        // bounds (a choice needs options, a score 2..=10 levels) before any
        // backend sees the request - a malformed menu is a caller bug, not
        // a judgment.
        request
            .validate()
            .map_err(|detail| anyhow::anyhow!("jev question shape is invalid: {detail}"))?;
        let outcome = tokio::select! {
            biased;
            () = signal.aborted() => Err(JudgeError::Aborted),
            outcome = self.judge.judge(&request) => outcome,
        };
        match outcome {
            Ok(result) => Ok(render_result(&result)),
            Err(error) => anyhow::bail!("jev judgment failed: {error}"),
        }
    }

    /// `ask`: explicit typed questions.
    fn request_ask(params: &serde_json::Value) -> anyhow::Result<JudgmentRequest> {
        let state = required_state(params)?;
        let questions = params
            .get("questions")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("jev ask needs a \"questions\" array"))?;
        let mut parsed = std::collections::BTreeMap::new();
        for question in questions {
            let id = question
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("every jev question needs an \"id\""))?;
            parsed.insert(id.to_string(), parse_question(question)?);
        }
        Ok(JudgmentRequest {
            state,
            questions: parsed,
        })
    }

    /// `classify`/`route`: one choice question over an options menu.
    fn request_classify(params: &serde_json::Value) -> anyhow::Result<JudgmentRequest> {
        let state = required_state(params)?;
        let instructions = params
            .get("question")
            .or_else(|| params.get("instructions"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("jev classify needs a \"question\""))?
            .to_string();
        let options = params
            .get("options")
            .or_else(|| params.get("routes"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "jev classify needs an \"options\" array (names or {{name, description}})"
                )
            })?;
        let mut criteria = std::collections::BTreeMap::new();
        for option in options {
            match option {
                serde_json::Value::String(name) => {
                    criteria.insert(name.clone(), None);
                }
                serde_json::Value::Object(fields) => {
                    let name = fields
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("every jev option needs a \"name\""))?;
                    let description = fields
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    criteria.insert(name.to_string(), description);
                }
                _ => anyhow::bail!("jev options must be names or {{name, description}} objects"),
            }
        }
        // The menu rule: an explicit escape option, appended when absent -
        // the judge picks from the menu, it never invents an entry. A menu
        // that already names an escape (`other`/`none`/`unknown`) keeps it
        // verbatim, for `classify` and `route` alike.
        if !criteria.keys().any(|name| is_escape_name(name)) {
            criteria.entry(ESCAPE_OPTION.to_string()).or_insert(None);
        }
        let id = params
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("classify")
            .to_string();
        Ok(JudgmentRequest {
            state,
            questions: std::collections::BTreeMap::from([(
                id,
                Question::Choice {
                    instructions,
                    criteria,
                },
            )]),
        })
    }

    /// `check`: one noul question.
    fn request_check(params: &serde_json::Value) -> anyhow::Result<JudgmentRequest> {
        let state = required_state(params)?;
        let instructions = params
            .get("question")
            .or_else(|| params.get("instructions"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("jev check needs a \"question\""))?
            .to_string();
        let criteria = NoulCriteria {
            true_: params
                .get("true")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            false_: params
                .get("false")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        };
        let has_criteria = criteria.true_.is_some() || criteria.false_.is_some();
        let id = params
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("check")
            .to_string();
        Ok(JudgmentRequest {
            state,
            questions: std::collections::BTreeMap::from([(
                id,
                Question::Noul {
                    instructions,
                    criteria: has_criteria.then_some(criteria),
                },
            )]),
        })
    }
}

/// Whether an option name already serves as the escape (`other`/`none`).
fn is_escape_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "other" | "none" | "unknown"
    )
}

/// The required `state` argument.
fn required_state(params: &serde_json::Value) -> anyhow::Result<JudgmentState> {
    let state = params
        .get("state")
        .ok_or_else(|| anyhow::anyhow!("jev needs a \"state\" (the content to judge)"))?;
    match state {
        serde_json::Value::String(text) => Ok(JudgmentState::Text(text.clone())),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            Ok(JudgmentState::Json(state.clone()))
        }
        _ => anyhow::bail!("jev \"state\" must be a string or a JSON object/array"),
    }
}

/// One `ask` question object into a typed question.
fn parse_question(question: &serde_json::Value) -> anyhow::Result<Question> {
    let instructions = question
        .get("instructions")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let kind = question
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("every jev question needs a \"type\""))?;
    match kind {
        "choice" => {
            let options = question
                .get("options")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| anyhow::anyhow!("a choice question needs \"options\""))?;
            let mut criteria = std::collections::BTreeMap::new();
            for option in options {
                match option {
                    serde_json::Value::String(name) => {
                        criteria.insert(name.clone(), None);
                    }
                    serde_json::Value::Object(fields) => {
                        let name = fields
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| anyhow::anyhow!("every jev option needs a \"name\""))?;
                        let description = fields
                            .get("description")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string);
                        criteria.insert(name.to_string(), description);
                    }
                    _ => {
                        anyhow::bail!("jev options must be names or {{name, description}} objects")
                    }
                }
            }
            Ok(Question::Choice {
                instructions,
                criteria,
            })
        }
        "score" => {
            let levels = question
                .get("levels")
                .or_else(|| question.get("criteria"))
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    anyhow::anyhow!("a score question needs \"levels\" (lowest first)")
                })?;
            let levels = levels
                .iter()
                .map(|level| {
                    level
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| anyhow::anyhow!("score levels must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Question::Score {
                instructions,
                levels,
            })
        }
        "noul" => {
            let criteria = NoulCriteria {
                true_: question
                    .get("true")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                false_: question
                    .get("false")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            };
            let has_criteria = criteria.true_.is_some() || criteria.false_.is_some();
            Ok(Question::Noul {
                instructions,
                criteria: has_criteria.then_some(criteria),
            })
        }
        other => {
            anyhow::bail!("unknown jev question type {other:?}: expected choice, score, or noul")
        }
    }
}

/// Render one judgment outcome: one line per answer plus the cost line.
fn render_result(result: &pa_jev::JudgmentResult) -> AgentToolResult {
    use std::fmt::Write as _;
    let mut text = String::new();
    for (id, answer) in &result.answers {
        match answer {
            pa_jev::Answer::Choice {
                choice,
                probabilities,
                confidence,
            } => {
                let probability = probabilities.get(choice).copied().unwrap_or_default();
                let _ = writeln!(
                    text,
                    "{id}: {choice} (p={probability:.2}, confidence={confidence:.2})"
                );
            }
            pa_jev::Answer::Score {
                score, confidence, ..
            } => {
                let _ = writeln!(text, "{id}: score={score:.2} (confidence={confidence:.2})");
            }
            pa_jev::Answer::Noul { noul } => {
                let _ = writeln!(text, "{id}: {noul:.2}");
            }
        }
    }
    let _ = writeln!(
        text,
        "(jev model={}, input={} tokens, cost=${:.6})",
        result.model, result.usage.input_tokens, result.usage.cost_usd
    );
    AgentToolResult {
        content: vec![ToolResultContent::text(text)],
        details: serde_json::to_value(Details {
            model: &result.model,
            usage: &result.usage,
            answers: &result.answers,
        })
        .unwrap_or(serde_json::Value::Null),
        terminate: None,
        is_error: false,
    }
}

/// The tool result's `details` payload.
#[derive(serde::Serialize)]
struct Details<'a> {
    model: &'a str,
    usage: &'a pa_jev::Usage,
    answers: &'a std::collections::BTreeMap<String, pa_jev::Answer>,
}

impl AgentTool for JevTool {
    fn name(&self) -> &'static str {
        "jev"
    }

    fn description(&self) -> &'static str {
        "Typed calibrated decisions over a state: classify into a menu, route with an escape, \
         check a yes/no probability, or ask explicit choice/score/noul questions. Cheap and \
         fast; use for gray-zone judgment calls, not arithmetic or lookups."
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.schema
    }

    fn execute(
        self: Arc<Self>,
        _tool_call_id: String,
        params: serde_json::Value,
        signal: AbortSignal,
        _on_update: AgentToolUpdateCallback,
    ) -> pa_agent::BoxFut<'static, anyhow::Result<AgentToolResult>> {
        Box::pin(async move { self.dispatch(&params, &signal).await })
    }
}

/// The tool's JSON-Schema parameters.
#[must_use]
pub fn params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["ask", "classify", "route", "check"],
                "description": "The judgment shape: ask (explicit questions), classify (menu pick), route (menu pick over destinations), check (yes/no probability)."
            },
            "state": {
                "type": ["string", "object", "array"],
                "description": "The content to judge; every question in one call sees the same state."
            },
            "question": {
                "type": "string",
                "description": "classify/route/check: the single question's instructions."
            },
            "options": {
                "type": "array",
                "items": { "type": ["string", "object"] },
                "description": "classify/route: the menu - option names or {name, description} rubrics. An explicit other/none escape is appended when missing."
            },
            "routes": {
                "type": "array",
                "items": { "type": ["string", "object"] },
                "description": "route: alias of options (destination names or {name, description})."
            },
            "true": {
                "type": "string",
                "description": "check: what yes means (optional rubric)."
            },
            "false": {
                "type": "string",
                "description": "check: what no means (optional rubric)."
            },
            "id": {
                "type": "string",
                "description": "classify/route/check: the answer key (default classify/check)."
            },
            "questions": {
                "type": "array",
                "description": "ask: explicit questions - {id, type, instructions, options?|levels?|true?/false?}.",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "type": { "type": "string", "enum": ["choice", "score", "noul"] },
                        "instructions": { "type": "string" },
                        "options": {
                            "type": "array",
                            "items": { "type": ["string", "object"] },
                            "description": "choice: option names or {name, description}."
                        },
                        "levels": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "score: 2-10 level descriptions, lowest first."
                        },
                        "true": { "type": "string", "description": "noul: what yes means." },
                        "false": { "type": "string", "description": "noul: what no means." }
                    },
                    "required": ["id", "type"]
                }
            }
        },
        "required": ["action", "state"]
    })
}
