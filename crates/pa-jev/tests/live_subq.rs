//! Live smoke against SubQ Snap (`https://api.subq.ai/v1/systemone`), the
//! System One-compatible decision model. Never runs in CI: the test is
//! `#[ignore]`d and double-gated on `SUBQ_LIVE=1` plus a `SUBQ_API_KEY`:
//!
//! ```text
//! SUBQ_LIVE=1 SUBQ_API_KEY=... cargo test -p pa-jev --test live_subq -- --ignored --nocapture
//! ```
//!
//! The request carries the doc-fixture shape (one `choice` with explicit
//! criteria, one bare `noul`), so a green run is a black-box check of the
//! wire encoder and decoder against a real endpoint - the compatibility the
//! catalog entry (`subquadratic/subq-snap-preview`, api `typesafe`)
//! promises.

use std::collections::BTreeMap;

use pa_jev::{
    Answer, JevClient, JevClientConfig, JudgmentRequest, JudgmentState, Question, WireDialect,
};

fn config_from_env() -> Option<JevClientConfig> {
    if std::env::var("SUBQ_LIVE").ok().as_deref() != Some("1") {
        return None;
    }
    let api_key = std::env::var("SUBQ_API_KEY").ok()?.trim().to_string();
    if api_key.is_empty() {
        return None;
    }
    let base_url = std::env::var("SUBQ_BASE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "https://api.subq.ai".to_string());
    let model = std::env::var("SUBQ_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "subq-snap-preview".to_string());
    Some(JevClientConfig::new(
        WireDialect::Typesafe,
        base_url,
        model,
        "subquadratic".to_string(),
        api_key,
    ))
}

#[tokio::test]
#[ignore = "networked: opt in with SUBQ_LIVE=1 and SUBQ_API_KEY"]
async fn subq_snap_answers_the_system_one_contract() {
    let Some(config) = config_from_env() else {
        eprintln!("SKIPPED: live SubQ smoke (set SUBQ_LIVE=1 and SUBQ_API_KEY)");
        return;
    };
    let client = JevClient::new(config);
    let request = JudgmentRequest {
        state: JudgmentState::Text(
            "A coding agent must decide whether a failed login retry is worth another attempt."
                .to_string(),
        ),
        questions: BTreeMap::from([
            (
                "verdict".to_string(),
                Question::Choice {
                    instructions: "Classify the failure.".to_string(),
                    criteria: BTreeMap::from([
                        (
                            "retry".to_string(),
                            Some("a transient network fault, worth retrying".to_string()),
                        ),
                        (
                            "reauth".to_string(),
                            Some("a credential problem, the user must log in again".to_string()),
                        ),
                        (
                            "permanent".to_string(),
                            Some("an entitlement or billing block, no retry helps".to_string()),
                        ),
                    ]),
                },
            ),
            (
                "ambiguous".to_string(),
                Question::Noul {
                    instructions: "Is the failure text too vague to classify confidently?"
                        .to_string(),
                    criteria: None,
                },
            ),
        ]),
    };
    let result = client.judge(&request).await.expect("the live call answers");
    let Answer::Choice {
        choice,
        probabilities,
        ..
    } = &result.answers["verdict"]
    else {
        panic!("the verdict answer is a choice");
    };
    assert!(
        ["retry", "reauth", "permanent"].contains(&choice.as_str()),
        "the pick came from the menu: {choice}"
    );
    assert!(
        probabilities.contains_key(choice),
        "the probabilities cover the pick"
    );
    let Answer::Noul { noul } = &result.answers["ambiguous"] else {
        panic!("the ambiguous answer is a noul");
    };
    assert!((0.0..=1.0).contains(noul), "noul is a probability: {noul}");
    assert!(
        result.usage.input_tokens > 0,
        "usage reports the billed input tokens"
    );
}
