//! Typed-judgment wire types: the three question primitives (choice, score,
//! noul) and their answers, field-for-field with the System One contract
//! `{model, state, questions} -> {model, answers, usage}`.
//!
//! Questions serialize in the wire's own shape (`criteria` as an option map
//! for `choice`, an ordered level array for `score`, and an optional
//! `{true, false}` pair for `noul`). Answers decode tolerantly: the verified
//! API shape (a `type` echo, score probabilities keyed by level index) and
//! the OpenJev/System-One-server shape (no `type` echo, score probabilities
//! as an array plus a `legend`) both resolve to the same typed answer.

use std::collections::BTreeMap;

/// The content a judgment evaluates: plain text, or structured JSON (an
/// object or an array; scalars belong in [`JudgmentState::Text`]).
#[derive(Debug, Clone, PartialEq)]
pub enum JudgmentState {
    Text(String),
    Json(serde_json::Value),
}

impl JudgmentState {
    /// The canonical serialization the answer cache keys on (stable given
    /// `serde_json`'s insertion-ordered maps).
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            JudgmentState::Text(text) => text.clone(),
            JudgmentState::Json(value) => serde_json::to_string(value).unwrap_or_default(),
        }
    }

    /// The wire form: text passes through, JSON embeds as-is.
    #[must_use]
    pub fn to_wire(&self) -> serde_json::Value {
        match self {
            JudgmentState::Text(text) => serde_json::Value::String(text.clone()),
            JudgmentState::Json(value) => value.clone(),
        }
    }
}

impl From<&str> for JudgmentState {
    fn from(text: &str) -> Self {
        JudgmentState::Text(text.to_string())
    }
}

impl From<String> for JudgmentState {
    fn from(text: String) -> Self {
        JudgmentState::Text(text)
    }
}

/// Optional yes/no rubric of a `noul` question.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    /// What "yes" means (`true` is a Rust keyword; the wire name is the
    /// JSON key).
    #[serde(rename = "true", default, skip_serializing_if = "Option::is_none")]
    pub true_: Option<String>,
    #[serde(rename = "false", default, skip_serializing_if = "Option::is_none")]
    pub false_: Option<String>,
}

/// One typed question. Every kind carries `instructions`; `criteria` is the
/// wire's polymorphic field (option map / ordered levels / yes-no pair).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Pick one of at most [`MAX_CHOICE_OPTIONS`] options; the criteria map
    /// (ordered, so probabilities reproduce) sends option -> rubric, `None`
    /// when the name suffices.
    Choice {
        #[serde(default)]
        instructions: String,
        criteria: BTreeMap<String, Option<String>>,
    },
    /// Position on an ordered rubric of 2..=[`MAX_SCORE_LEVELS`] level
    /// descriptions, lowest first.
    Score {
        #[serde(default)]
        instructions: String,
        /// Wire alias: the verified API sends the level array as `criteria`;
        /// some System One servers send `legend`. Encoding always uses
        /// `criteria`.
        #[serde(rename = "criteria")]
        levels: Vec<String>,
    },
    /// Probability that a yes/no condition holds; no confidence comes back.
    Noul {
        #[serde(default)]
        instructions: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
}

/// The most options one `choice` question may carry (API contract).
pub const MAX_CHOICE_OPTIONS: usize = 255;
/// The rubric bounds of one `score` question (API contract: 2..=10 levels).
pub const MIN_SCORE_LEVELS: usize = 2;
pub const MAX_SCORE_LEVELS: usize = 10;

impl Question {
    /// The wire discriminant of this question's kind.
    #[must_use]
    pub fn kind(&self) -> QuestionKind {
        match self {
            Question::Choice { .. } => QuestionKind::Choice,
            Question::Score { .. } => QuestionKind::Score,
            Question::Noul { .. } => QuestionKind::Noul,
        }
    }

    /// Whether this question satisfies the contract's shape bounds.
    ///
    /// # Errors
    ///
    /// The first violated bound, as a human-readable string.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Question::Choice { criteria, .. } => {
                if criteria.is_empty() {
                    Err("choice question needs at least one option".to_string())
                } else if criteria.len() > MAX_CHOICE_OPTIONS {
                    Err(format!(
                        "choice question carries {} options; the contract caps at {MAX_CHOICE_OPTIONS}",
                        criteria.len()
                    ))
                } else {
                    Ok(())
                }
            }
            Question::Score { levels, .. } => {
                if levels.len() < MIN_SCORE_LEVELS {
                    Err(format!(
                        "score question needs at least {MIN_SCORE_LEVELS} levels"
                    ))
                } else if levels.len() > MAX_SCORE_LEVELS {
                    Err(format!(
                        "score question carries {} levels; the contract caps at {MAX_SCORE_LEVELS}",
                        levels.len()
                    ))
                } else {
                    Ok(())
                }
            }
            Question::Noul { .. } => Ok(()),
        }
    }
}

/// The three question kinds, in wire spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuestionKind {
    Choice,
    Score,
    Noul,
}

impl QuestionKind {
    /// The wire discriminant (`"choice"` / `"score"` / `"noul"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            QuestionKind::Choice => "choice",
            QuestionKind::Score => "score",
            QuestionKind::Noul => "noul",
        }
    }
}

/// One typed answer, matching the answer shape the verified API returns.
///
/// - `choice`: the picked option, every option's probability (sums to 1),
///   and the distribution's concentration.
/// - `score`: the probability-weighted mean of the level *indices* (may land
///   between levels), probabilities keyed by level index, concentration.
/// - `noul`: the probability of yes; the contract sends no confidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Noul {
        noul: f64,
    },
}

impl Answer {
    /// The kind of question this answer resolves.
    #[must_use]
    pub fn kind(&self) -> QuestionKind {
        match self {
            Answer::Choice { .. } => QuestionKind::Choice,
            Answer::Score { .. } => QuestionKind::Score,
            Answer::Noul { .. } => QuestionKind::Noul,
        }
    }

    /// Decode one wire answer. Accepts the verified shape (a `type` echo)
    /// and the `OpenJev` shape (discriminated by the answer's own field);
    /// `None` when the value is neither.
    #[must_use]
    pub fn from_wire(value: &serde_json::Value) -> Option<Answer> {
        let object = value.as_object()?;
        match object.get("type").and_then(serde_json::Value::as_str) {
            Some("choice") => Answer::choice_from_wire(object),
            Some("score") => Answer::score_from_wire(object),
            Some("noul") => Answer::noul_from_wire(object),
            // An unknown discriminant is not an answer we can type.
            Some(_) => None,
            // No echo: discriminate by the answer's own field (OpenJev
            // servers slice their additive metadata and keep this shape).
            None => {
                if object.contains_key("choice") {
                    Answer::choice_from_wire(object)
                } else if object.contains_key("score") {
                    Answer::score_from_wire(object)
                } else {
                    Answer::noul_from_wire(object)
                }
            }
        }
    }

    fn choice_from_wire(object: &serde_json::Map<String, serde_json::Value>) -> Option<Answer> {
        Some(Answer::Choice {
            choice: object.get("choice")?.as_str()?.to_string(),
            probabilities: probability_map(object.get("probabilities")?)?,
            confidence: object.get("confidence")?.as_f64()?,
        })
    }

    fn score_from_wire(object: &serde_json::Map<String, serde_json::Value>) -> Option<Answer> {
        Some(Answer::Score {
            score: object.get("score")?.as_f64()?,
            probabilities: match object.get("probabilities") {
                // Verified shape: keyed by level index.
                Some(value) if value.is_object() => probability_map(value)?,
                // OpenJev shape: an array ordered by level index.
                Some(value) if value.is_array() => index_probability_map(value)?,
                _ => return None,
            },
            confidence: object.get("confidence")?.as_f64()?,
        })
    }

    fn noul_from_wire(object: &serde_json::Map<String, serde_json::Value>) -> Option<Answer> {
        Some(Answer::Noul {
            noul: object.get("noul")?.as_f64()?,
        })
    }
}

/// A finite `name -> probability` map.
fn probability_map(value: &serde_json::Value) -> Option<BTreeMap<String, f64>> {
    let object = value.as_object()?;
    let mut map = BTreeMap::new();
    for (name, probability) in object {
        map.insert(name.clone(), probability.as_f64()?);
    }
    Some(map)
}

/// An array of probabilities keyed by its indices (`"0"`, `"1"`, ...).
fn index_probability_map(value: &serde_json::Value) -> Option<BTreeMap<String, f64>> {
    let array = value.as_array()?;
    let mut map = BTreeMap::new();
    for (index, probability) in array.iter().enumerate() {
        map.insert(index.to_string(), probability.as_f64()?);
    }
    Some(map)
}

/// One judgment request: the state every question sees and the questions
/// keyed by caller-chosen ids (answers return under the same ids).
#[derive(Debug, Clone, PartialEq)]
pub struct JudgmentRequest {
    pub state: JudgmentState,
    pub questions: BTreeMap<String, Question>,
}

impl JudgmentRequest {
    /// Validate every question's shape bounds.
    ///
    /// # Errors
    ///
    /// The first violated bound, named with its question id.
    pub fn validate(&self) -> Result<(), String> {
        for (id, question) in &self.questions {
            question
                .validate()
                .map_err(|detail| format!("{id}: {detail}"))?;
        }
        Ok(())
    }
}

/// Billed usage of one judgment (or a merged run of chunks).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// USD; a billed amount the API reports wins, otherwise input-only
    /// pricing at [`JEV_INPUT_PRICE_PER_MTOK`].
    pub cost_usd: f64,
}

impl Usage {
    /// Sum two usage blocks (chunk merge).
    #[must_use]
    pub fn sum(self, other: Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens + other.input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            cost_usd: self.cost_usd + other.cost_usd,
        }
    }
}

use serde::{Deserialize, Serialize};

/// Jev's measured input-only pricing: $/million input tokens ($0.042/MTok;
/// output is free).
pub const JEV_INPUT_PRICE_PER_MTOK: f64 = 0.042;

/// One decoded judgment response, before per-question postconditions run.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedJudgment {
    /// The model that answered, when the response names one (the exact
    /// version matters: thresholds calibrate per release).
    pub model: Option<String>,
    /// Wire answers keyed by question id.
    pub answers: BTreeMap<String, Answer>,
    /// Question ids the endpoint declined to answer.
    pub refused: Vec<String>,
    pub usage: Usage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_canonicalizes_and_wires() {
        let text = JudgmentState::from("my card was charged twice");
        assert_eq!(text.canonical(), "my card was charged twice");
        assert_eq!(
            text.to_wire(),
            serde_json::json!("my card was charged twice")
        );
        let json = JudgmentState::Json(serde_json::json!({"card": "visa", "charged": 2}));
        assert_eq!(json.canonical(), r#"{"card":"visa","charged":2}"#);
        assert_eq!(
            json.to_wire(),
            serde_json::json!({"card": "visa", "charged": 2})
        );
    }

    #[test]
    fn choice_shape_bounds() {
        let one = Question::Choice {
            instructions: "route".to_string(),
            criteria: BTreeMap::from([("billing".to_string(), None)]),
        };
        assert!(one.validate().is_ok());
        let empty = Question::Choice {
            instructions: String::new(),
            criteria: BTreeMap::new(),
        };
        assert!(empty.validate().is_err());
        let too_many = Question::Choice {
            instructions: String::new(),
            criteria: (0..=MAX_CHOICE_OPTIONS)
                .map(|index| (index.to_string(), None))
                .collect(),
        };
        assert!(too_many.validate().is_err());
    }

    #[test]
    fn score_shape_bounds() {
        let two = Question::Score {
            instructions: String::new(),
            levels: vec!["low".to_string(), "high".to_string()],
        };
        assert!(two.validate().is_ok());
        let one = Question::Score {
            instructions: String::new(),
            levels: vec!["only".to_string()],
        };
        assert!(one.validate().is_err());
        let many = Question::Score {
            instructions: String::new(),
            levels: (0..=MAX_SCORE_LEVELS).map(|i| i.to_string()).collect(),
        };
        assert!(many.validate().is_err());
    }
}
