//! Judgment wire dialects: how one judgment API is addressed, encoded, and
//! decoded. Classification is always by wire dialect (`model.api`), never by
//! provider identity - a Microsoft decision model behind a System One route
//! is a catalog entry, not a code path.
//!
//! Both shipped dialects share the System One body and response shape; they
//! differ only in route (the model entry's `baseUrl` already carries the
//! API root, so the dialect appends its path):
//!
//! - `typesafe`: `POST {base}/v1/systemone` - `TypeSafe` and every System
//!   One server (`OpenJev`, self-hosted).
//! - `openrouter-decisions`: `POST {base}/decisions` under `OpenRouter`'s
//!   `/api/alpha` root (entries carry `https://openrouter.ai/api/alpha`).

use std::collections::BTreeMap;

use crate::types::{Answer, DecodedJudgment, JudgmentRequest, Usage};

/// The judgment wire dialects `pa-jev` speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WireDialect {
    /// `TypeSafe`'s own System One route.
    Typesafe,
    /// `OpenRouter`'s Decisions route (same body under `/api/alpha`).
    OpenrouterDecisions,
}

impl WireDialect {
    /// The dialect serving a catalog `api` value, `None` for chat APIs.
    #[must_use]
    pub fn from_api(api: &str) -> Option<WireDialect> {
        match api {
            "typesafe" => Some(WireDialect::Typesafe),
            "openrouter-decisions" => Some(WireDialect::OpenrouterDecisions),
            _ => None,
        }
    }

    /// The dialect's catalog `api` spelling.
    #[must_use]
    pub fn api(self) -> &'static str {
        match self {
            WireDialect::Typesafe => "typesafe",
            WireDialect::OpenrouterDecisions => "openrouter-decisions",
        }
    }

    /// The path appended to the model entry's base URL.
    #[must_use]
    pub fn path(self) -> &'static str {
        match self {
            WireDialect::Typesafe => "/v1/systemone",
            WireDialect::OpenrouterDecisions => "/decisions",
        }
    }
}

/// Whether a catalog `api` value answers judgments natively.
#[must_use]
pub fn is_judgment_api(api: &str) -> bool {
    WireDialect::from_api(api).is_some()
}

/// The System One request body for `model` (shared by both dialects).
#[must_use]
pub fn encode_request(request: &JudgmentRequest, model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "state": request.state.to_wire(),
        "questions": request.questions,
    })
}

/// Decode a System One response payload; `None` when it is not this API's
/// response shape.
///
/// `input_price_per_mtok` prices a response that carries token counts but no
/// billed amount. It is passed in rather than read from a constant because
/// the chain mixes endpoints at different rates (see
/// [`crate::client::JevClientConfig::input_price_per_mtok`]).
#[must_use]
pub fn decode_response(
    payload: &serde_json::Value,
    input_price_per_mtok: f64,
) -> Option<DecodedJudgment> {
    let object = payload.as_object()?;
    let answers_object = object.get("answers")?.as_object()?;
    let mut answers = BTreeMap::new();
    for (id, value) in answers_object {
        // A `refusal`-shaped entry marks a declined question id.
        if value.as_object().is_some_and(|entry| {
            entry.get("type").and_then(serde_json::Value::as_str) == Some("refusal")
        }) {
            continue;
        }
        let answer = Answer::from_wire(value)?;
        answers.insert(id.clone(), answer);
    }
    let usage = decode_usage(object.get("usage"), input_price_per_mtok);
    Some(DecodedJudgment {
        model: object
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        answers,
        refused: Vec::new(),
        usage,
    })
}

/// Wire usage: token counts plus an optional billed amount. Judgment
/// pricing is input-only, so a missing amount prices from the input tokens.
// Token counts sit far below f64's 2^52 exact-integer range, so the cast
// loses nothing in practice.
#[allow(clippy::cast_precision_loss)]
fn decode_usage(value: Option<&serde_json::Value>, input_price_per_mtok: f64) -> Usage {
    let empty = serde_json::Map::new();
    let object = value
        .and_then(serde_json::Value::as_object)
        .unwrap_or(&empty);
    let input_tokens = object
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let output_tokens = object
        .get("output_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cost_usd = object
        .get("cost")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_else(|| input_tokens as f64 / 1.0e6 * input_price_per_mtok);
    Usage {
        input_tokens,
        output_tokens,
        cost_usd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JEV_INPUT_PRICE_PER_MTOK;
    use crate::types::{JudgmentState, NoulCriteria, Question};

    /// The verified doc fixture: one request carrying all three primitives.
    fn fixture_request() -> JudgmentRequest {
        JudgmentRequest {
            state: JudgmentState::from("my card was charged twice"),
            questions: BTreeMap::from([
                (
                    "dept".to_string(),
                    Question::Choice {
                        instructions: "route the ticket".to_string(),
                        criteria: BTreeMap::from([
                            (
                                "billing".to_string(),
                                Some("charges and refunds".to_string()),
                            ),
                            ("fraud".to_string(), None),
                        ]),
                    },
                ),
                (
                    "mood".to_string(),
                    Question::Score {
                        instructions: "sender tone".to_string(),
                        levels: vec![
                            "Calm".to_string(),
                            "Irritated".to_string(),
                            "Angry".to_string(),
                        ],
                    },
                ),
                (
                    "urgent".to_string(),
                    Question::Noul {
                        instructions: "the sender needs a response soon".to_string(),
                        criteria: Some(NoulCriteria {
                            true_: Some("a human waits on this".to_string()),
                            false_: None,
                        }),
                    },
                ),
            ]),
        }
    }

    #[test]
    fn request_encode_matches_the_wire_contract() {
        let body = encode_request(&fixture_request(), "jev-1.13.0");
        assert_eq!(
            body,
            serde_json::json!({
                "model": "jev-1.13.0",
                "state": "my card was charged twice",
                "questions": {
                    "dept": {
                        "type": "choice",
                        "instructions": "route the ticket",
                        "criteria": {"billing": "charges and refunds", "fraud": null}
                    },
                    "mood": {
                        "type": "score",
                        "instructions": "sender tone",
                        "criteria": ["Calm", "Irritated", "Angry"]
                    },
                    "urgent": {
                        "type": "noul",
                        "instructions": "the sender needs a response soon",
                        "criteria": {"true": "a human waits on this"}
                    }
                }
            })
        );
    }

    #[test]
    fn response_decode_matches_the_verified_shape() {
        let payload = serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {
                "dept": {
                    "type": "choice",
                    "choice": "billing",
                    "probabilities": {"billing": 0.99, "bug": 0.0, "account": 0.01},
                    "confidence": 0.99
                },
                "mood": {
                    "type": "score",
                    "score": 2.68,
                    "probabilities": {"0": 0.0, "1": 0.16, "2": 0.84},
                    "confidence": 0.68
                },
                "urgent": {"type": "noul", "noul": 0.95}
            },
            "usage": {"input_tokens": 296, "output_tokens": 20}
        });
        let decoded =
            decode_response(&payload, JEV_INPUT_PRICE_PER_MTOK).expect("verified shape decodes");
        assert_eq!(decoded.model.as_deref(), Some("jev-1.13.0"));
        let Answer::Choice {
            choice,
            probabilities,
            confidence,
        } = &decoded.answers["dept"]
        else {
            panic!("choice answer");
        };
        assert_eq!(choice, "billing");
        assert_eq!(probabilities["billing"].to_bits(), 0.99_f64.to_bits());
        assert_eq!(confidence.to_bits(), 0.99_f64.to_bits());
        let Answer::Score {
            score,
            probabilities,
            confidence,
        } = &decoded.answers["mood"]
        else {
            panic!("score answer");
        };
        assert_eq!(score.to_bits(), 2.68_f64.to_bits());
        // Score probabilities stay keyed by level index; noul carries no confidence.
        assert_eq!(probabilities.keys().collect::<Vec<_>>(), ["0", "1", "2"]);
        assert_eq!(confidence.to_bits(), 0.68_f64.to_bits());
        let Answer::Noul { noul } = decoded.answers["urgent"] else {
            panic!("noul answer");
        };
        assert_eq!(noul.to_bits(), 0.95_f64.to_bits());
        assert_eq!(decoded.usage.input_tokens, 296);
        assert_eq!(decoded.usage.output_tokens, 20);
    }

    #[test]
    fn usage_prices_input_only_when_no_billed_amount() {
        let payload = serde_json::json!({
            "answers": {},
            "usage": {"input_tokens": 1_000_000, "output_tokens": 0}
        });
        let decoded = decode_response(&payload, JEV_INPUT_PRICE_PER_MTOK).expect("decodes");
        assert!((decoded.usage.cost_usd - JEV_INPUT_PRICE_PER_MTOK).abs() < 1.0e-12);
        let billed = serde_json::json!({
            "answers": {},
            "usage": {"input_tokens": 1_000_000, "cost": 0.05}
        });
        let decoded = decode_response(&billed, JEV_INPUT_PRICE_PER_MTOK).expect("decodes");
        assert_eq!(
            decoded.usage.cost_usd.to_bits(),
            0.05_f64.to_bits(),
            "a billed amount wins"
        );
    }

    /// The fallback price belongs to the ENDPOINT, not the chain.
    ///
    /// The judge chain can hold `TypeSafe` (0.042/MTok) and `SubQ Snap`
    /// (0.05/MTok) at the same time, so a chain-wide constant prices one
    /// provider's tokens at another's rate. The same payload must therefore
    /// price differently per endpoint, and a billed amount still wins over
    /// either.
    #[test]
    fn the_fallback_price_follows_the_endpoint_not_the_chain() {
        let payload = serde_json::json!({
            "answers": {},
            "usage": {"input_tokens": 1_000_000, "output_tokens": 0}
        });
        let subq = decode_response(&payload, 0.05).expect("decodes");
        assert!((subq.usage.cost_usd - 0.05).abs() < 1.0e-12);
        let typesafe = decode_response(&payload, JEV_INPUT_PRICE_PER_MTOK).expect("decodes");
        assert!((typesafe.usage.cost_usd - JEV_INPUT_PRICE_PER_MTOK).abs() < 1.0e-12);
        assert_ne!(
            subq.usage.cost_usd.to_bits(),
            typesafe.usage.cost_usd.to_bits(),
            "two endpoints at different prices must not bill identically"
        );
    }

    #[test]
    fn openjev_score_shape_decodes() {
        // OpenJev/System One servers: no `type` echo, score probabilities as
        // an array, a `legend` string, and additive metadata fields.
        let payload = serde_json::json!({
            "answers": {
                "mood": {"score": 1, "legend": "Irritated", "probabilities": [0.1, 0.7, 0.2],
                          "confidence": 0.7, "abstain": 0.05, "calibrated": true},
                "urgent": {"noul": 0.91, "abstain": 0.02},
                "dept": {"choice": "billing", "probabilities": {"billing": 0.82, "fraud": 0.18},
                          "confidence": 0.78}
            },
            "usage": {"embed_calls": 1, "texts_embedded": 12, "state_bytes": 58}
        });
        let decoded =
            decode_response(&payload, JEV_INPUT_PRICE_PER_MTOK).expect("openjev shape decodes");
        let Answer::Score {
            score,
            probabilities,
            ..
        } = &decoded.answers["mood"]
        else {
            panic!("score answer");
        };
        assert_eq!(score.to_bits(), 1.0_f64.to_bits());
        assert_eq!(
            probabilities["1"].to_bits(),
            0.7_f64.to_bits(),
            "array probabilities key by index"
        );
        let Answer::Noul { noul } = decoded.answers["urgent"] else {
            panic!("noul answer");
        };
        assert_eq!(noul.to_bits(), 0.91_f64.to_bits());
        assert_eq!(
            decoded.answers["dept"].kind(),
            crate::types::QuestionKind::Choice
        );
        assert_eq!(
            decoded.usage.input_tokens, 0,
            "local usage fields are not tokens"
        );
    }

    #[test]
    fn non_response_shapes_are_rejected() {
        assert!(decode_response(&serde_json::json!({}), JEV_INPUT_PRICE_PER_MTOK).is_none());
        assert!(decode_response(
            &serde_json::json!({"answers": []}),
            JEV_INPUT_PRICE_PER_MTOK
        )
        .is_none());
        assert!(
            decode_response(
                &serde_json::json!({"answers": {"q": {"type": "choice"}}}),
                JEV_INPUT_PRICE_PER_MTOK
            )
            .is_none(),
            "a choice answer without its fields does not decode"
        );
    }

    #[test]
    fn dialects_route_to_their_paths() {
        assert_eq!(WireDialect::Typesafe.path(), "/v1/systemone");
        assert_eq!(WireDialect::OpenrouterDecisions.path(), "/decisions");
        assert_eq!(
            WireDialect::from_api("typesafe"),
            Some(WireDialect::Typesafe)
        );
        assert_eq!(
            WireDialect::from_api("openrouter-decisions"),
            Some(WireDialect::OpenrouterDecisions)
        );
        assert!(WireDialect::from_api("openai-completions").is_none());
        assert!(is_judgment_api("typesafe"));
        assert!(!is_judgment_api("anthropic-messages"));
    }
}
