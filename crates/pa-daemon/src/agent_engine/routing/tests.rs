//! The router's daemon seam: the level gate hands `off` back unchanged, the
//! ladder mapping only ever reorders (never invents) a row, and a hard filter
//! drops the row it rejected.
use super::*;
use pa_core::session_engine::routing::{RouteReason, ScoredCandidate};
use pa_types::ai::{ModelCost, ModelInput};
use pa_types::JsNumber;

fn model(provider: &str, id: &str, context_window: u64, input_cost: f64) -> Model {
    Model {
        id: id.to_string(),
        name: id.to_string(),
        api: "openai-completions".to_string(),
        provider: provider.to_string(),
        base_url: "http://127.0.0.1:9".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![ModelInput::Text],
        cost: ModelCost {
            input: JsNumber(input_cost),
            output: JsNumber(0.0),
            cache_read: JsNumber(0.0),
            cache_write: JsNumber(0.0),
        },
        context_window,
        max_tokens: 4_096,
        max_tokens_explicit: false,
        featured: None,
        headers: None,
        compat: None,
    }
}

fn row(selector: &str, score: f32) -> ScoredCandidate {
    ScoredCandidate {
        selector: selector.to_string(),
        score,
        rejections: Vec::new(),
        terms: Vec::new(),
    }
}

fn decision(reason: RouteReason, chosen: &str, rows: Vec<ScoredCandidate>) -> RouteDecision {
    RouteDecision {
        chosen: chosen.to_string(),
        runner_up: None,
        reason,
        candidates: rows,
    }
}

#[test]
fn a_decision_in_catalog_order_returns_the_caller_vector() {
    let serving = model("a", "mock-1", 200_000, 3.0);
    let ladder = vec![
        model("b", "mock-1", 200_000, 9.0),
        model("c", "mock-1", 200_000, 1.0),
    ];
    // The router agrees with catalog order: the caller's vector comes back
    // itself (the identity case `off` and `dry-run` rely on).
    let agreed = decision(
        RouteReason::IncumbentRetained,
        "a/mock-1",
        vec![
            row("a/mock-1", 0.9),
            row("b/mock-1", 0.5),
            row("c/mock-1", 0.1),
        ],
    );
    let next = apply_route_ladder(&serving, ladder.clone(), &agreed);
    assert_eq!(
        next, ladder,
        "the serving model and the order are preserved"
    );
}

#[test]
fn a_retained_decision_keeps_catalog_order() {
    let serving = model("a", "mock-1", 200_000, 3.0);
    let ladder = vec![
        model("b", "mock-1", 200_000, 9.0),
        model("c", "mock-1", 200_000, 0.5),
    ];
    // The incumbent stays even though `c` scored higher: applying the score
    // order here would reorder the ladder on exactly the noise the hysteresis
    // exists to ignore.
    let retained = decision(
        RouteReason::IncumbentRetained,
        "a/mock-1",
        vec![
            row("a/mock-1", 0.9),
            row("c/mock-1", 0.7),
            row("b/mock-1", 0.2),
        ],
    );
    let next = apply_route_ladder(&serving, ladder.clone(), &retained);
    assert_eq!(next, ladder);
}

#[test]
fn a_moved_decision_follows_the_score_order() {
    let serving = model("a", "mock-1", 200_000, 3.0);
    let ladder = vec![
        model("b", "mock-1", 200_000, 9.0),
        model("c", "mock-1", 200_000, 0.5),
    ];
    let moved = decision(
        RouteReason::ChallengerWon,
        "c/mock-1",
        vec![
            row("a/mock-1", 0.2),
            row("c/mock-1", 0.8),
            row("b/mock-1", 0.1),
        ],
    );
    let next = apply_route_ladder(&serving, ladder, &moved);
    assert_eq!(
        next.iter()
            .map(|model| format!("{}/{}", model.provider, model.id))
            .collect::<Vec<_>>(),
        vec!["c/mock-1", "b/mock-1"],
        "the challenge leads and the rest follow by score"
    );
}

#[test]
fn a_hard_failed_incumbent_leaves_the_caller_ladder_in_score_order() {
    let serving = model("a", "mock-1", 200_000, 3.0);
    let ladder = vec![
        model("b", "mock-1", 200_000, 9.0),
        model("c", "mock-1", 200_000, 0.5),
    ];
    let mut rejected = row("a/mock-1", 0.0);
    rejected.rejections = vec![pa_core::session_engine::routing::RouteRejection::BreakerOpen];
    let failed = decision(
        RouteReason::IncumbentFailed,
        "c/mock-1",
        vec![rejected, row("c/mock-1", 0.8), row("b/mock-1", 0.1)],
    );
    let next = apply_route_ladder(&serving, ladder, &failed);
    assert_eq!(
        next.iter()
            .map(|model| format!("{}/{}", model.provider, model.id))
            .collect::<Vec<_>>(),
        vec!["c/mock-1", "b/mock-1"]
    );
}

#[test]
fn a_filtered_row_leaves_the_ladder() {
    let serving = model("a", "mock-1", 200_000, 3.0);
    let ladder = vec![
        model("b", "mock-1", 200_000, 9.0),
        model("c", "mock-1", 200_000, 1.0),
    ];
    let mut rejected = row("b/mock-1", 0.0);
    rejected.rejections = vec![pa_core::session_engine::routing::RouteRejection::ContextTooSmall];
    let decision = decision(
        RouteReason::IncumbentRetained,
        "a/mock-1",
        vec![row("a/mock-1", 0.9), row("c/mock-1", 0.6), rejected],
    );
    let next = apply_route_ladder(&serving, ladder, &decision);
    assert_eq!(
        next.iter()
            .map(|model| model.provider.as_str())
            .collect::<Vec<_>>(),
        vec!["c"],
        "the filtered provider is skipped, never walked into"
    );
}

#[test]
fn all_rejected_keeps_the_caller_order() {
    let serving = model("a", "mock-1", 200_000, 3.0);
    let ladder = vec![model("b", "mock-1", 200_000, 9.0)];
    let mut rejected = row("b/mock-1", 0.0);
    rejected.rejections = vec![pa_core::session_engine::routing::RouteRejection::BreakerOpen];
    let decision = decision(RouteReason::AllRejected, "", vec![rejected]);
    let next = apply_route_ladder(&serving, ladder.clone(), &decision);
    assert_eq!(next, ladder);
}

#[test]
fn the_ladder_never_invents_a_row_the_caller_did_not_offer() {
    let serving = model("a", "mock-1", 200_000, 3.0);
    let ladder = vec![model("b", "mock-1", 200_000, 9.0)];
    // A decision naming a provider outside the caller's ladder (a stale
    // decision, or a candidate the allowlist has since dropped): the mapping
    // ignores it rather than conjuring a model.
    let decision = decision(
        RouteReason::ChallengerWon,
        "z/mock-1",
        vec![row("z/mock-1", 0.9), row("b/mock-1", 0.4)],
    );
    let next = apply_route_ladder(&serving, ladder.clone(), &decision);
    assert_eq!(next, ladder);
}

#[test]
fn a_score_becomes_integer_thousandths() {
    assert_eq!(score_milli(0.0), 0);
    assert_eq!(score_milli(0.5), 500);
    assert_eq!(score_milli(1.0), 1_000);
    assert_eq!(score_milli(1.5), 1_000, "clamped, never out of range");
    assert_eq!(score_milli(-1.0), 0);
}

#[test]
fn a_fresh_session_starts_with_no_switch_history() {
    assert_eq!(RouterState::default().dwell_turns, 0);
}
