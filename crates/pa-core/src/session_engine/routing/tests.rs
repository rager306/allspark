//! The router's pure decision battery: filter correctness, score ordering, and
//! the anti-thrash properties the level gate exists to keep.
use super::*;

fn candidate(selector: &str, provider: &str) -> RouteCandidate {
    RouteCandidate {
        selector: selector.to_string(),
        // One model id across providers: the failover ladder the router
        // reorders today.
        model_id: "mock-1".to_string(),
        provider: provider.to_string(),
        context_window: 200_000,
        accepts_images: false,
        cost_per_1m_input: Some(1.0),
        latency_ema_ms: Some(1_000.0),
        breaker_open: false,
        error_rate: None,
        quota_remaining: None,
        free_capacity: None,
        capability_fit: None,
        quality: None,
    }
}

fn ctx(current: Option<&str>) -> RouteContext {
    RouteContext {
        needed_tokens: 10_000,
        output_tokens: 4_096,
        carries_images: false,
        current: current.map(str::to_string),
        dwell_turns: MIN_DWELL_TURNS,
    }
}

#[test]
fn off_reports_the_incumbent_and_scores_nothing() {
    let candidates = vec![candidate("a/mock-1", "a"), candidate("b/mock-1", "b")];
    let decision =
        decide(&candidates, &ctx(Some("a/mock-1")), ModelRoutingLevel::Off).expect("a decision");
    assert_eq!(decision.chosen, "a/mock-1");
    assert_eq!(decision.reason, RouteReason::Disabled);
    assert_eq!(decision.runner_up, None);
    assert!(
        decision
            .candidates
            .iter()
            .all(|row| row.score == 0.0 && row.terms.is_empty()),
        "off must not compute a score: {decision:?}"
    );
}

#[test]
fn a_small_context_window_is_a_hard_filter() {
    let mut small = candidate("a/mock-1", "a");
    small.context_window = 8_000;
    let candidates = vec![small, candidate("b/mock-1", "b")];
    let decision =
        decide(&candidates, &ctx(Some("a/mock-1")), ModelRoutingLevel::On).expect("a decision");
    let rejected = decision
        .candidates
        .iter()
        .find(|row| row.selector == "a/mock-1")
        .expect("the small model is present");
    assert_eq!(rejected.rejections, vec![RouteRejection::ContextTooSmall]);
    assert_eq!(rejected.score, 0.0, "a rejected row is not scored");
    assert_eq!(decision.chosen, "b/mock-1");
    assert_eq!(decision.reason, RouteReason::IncumbentFailed);
}

#[test]
fn the_output_budget_counts_against_the_window() {
    // 199k input fits a 200k window; 199k input + 8k reserved output does not.
    let mut candidate = candidate("a/mock-1", "a");
    candidate.context_window = 200_000;
    let mut context = ctx(Some("a/mock-1"));
    context.needed_tokens = 199_000;
    context.output_tokens = 8_000;
    let decision = decide(&[candidate], &context, ModelRoutingLevel::On).expect("a decision");
    assert_eq!(decision.reason, RouteReason::AllRejected);
    assert_eq!(decision.chosen, "");
    assert!(decision.ladder().is_empty());
}

#[test]
fn an_image_turn_rejects_a_text_only_candidate() {
    let mut vision = candidate("a/mock-1", "a");
    vision.accepts_images = true;
    let candidates = vec![candidate("b/mock-1", "b"), vision];
    let mut context = ctx(None);
    context.carries_images = true;
    let decision = decide(&candidates, &context, ModelRoutingLevel::On).expect("a decision");
    assert_eq!(decision.chosen, "a/mock-1");
    let rejected = decision
        .candidates
        .iter()
        .find(|row| row.selector == "b/mock-1")
        .expect("the text-only candidate is present");
    assert_eq!(rejected.rejections, vec![RouteRejection::Modality]);
}

#[test]
fn an_open_breaker_and_a_spent_quota_both_reject() {
    let mut broken = candidate("a/mock-1", "a");
    broken.breaker_open = true;
    let mut spent = candidate("b/mock-1", "b");
    spent.quota_remaining = Some(0.0);
    let healthy = candidate("c/mock-1", "c");
    let decision = decide(
        &[broken, spent, healthy],
        &ctx(Some("c/mock-1")),
        ModelRoutingLevel::On,
    )
    .expect("a decision");
    assert_eq!(decision.chosen, "c/mock-1");
    let rejections: Vec<Vec<RouteRejection>> = decision
        .candidates
        .iter()
        .map(|row| row.rejections.clone())
        .collect();
    assert!(rejections.contains(&vec![RouteRejection::BreakerOpen]));
    assert!(rejections.contains(&vec![RouteRejection::QuotaExhausted]));
}

#[test]
fn a_cheaper_and_roomier_challenger_wins_under_on() {
    let incumbent = candidate("a/mock-1", "a");
    let mut challenger = candidate("b/mock-1", "b");
    challenger.cost_per_1m_input = Some(0.1);
    challenger.quota_remaining = Some(1.0);
    let decision = decide(
        &[incumbent, challenger],
        &ctx(Some("a/mock-1")),
        ModelRoutingLevel::On,
    )
    .expect("a decision");
    assert_eq!(decision.chosen, "b/mock-1");
    assert_eq!(decision.reason, RouteReason::ChallengerWon);
    assert_eq!(decision.runner_up.as_deref(), Some("a/mock-1"));
    let chosen = decision.chosen_score().expect("the chosen row is scored");
    let runner_up = decision.runner_up_score().expect("the runner-up is scored");
    assert!(chosen > runner_up, "{chosen} > {runner_up}");
    assert_eq!(
        decision.ladder().first().copied(),
        Some("b/mock-1"),
        "the ladder leads with the chosen candidate"
    );
}

#[test]
fn auto_keeps_the_incumbent_inside_the_margin() {
    // A challenger that is marginally cheaper loses to the incumbent's cache
    // affinity under `auto` — the anti-thrash property.
    let incumbent = candidate("a/mock-1", "a");
    let mut challenger = candidate("b/mock-1", "b");
    challenger.cost_per_1m_input = Some(0.95);
    let decision = decide(
        &[incumbent, challenger],
        &ctx(Some("a/mock-1")),
        ModelRoutingLevel::Auto,
    )
    .expect("a decision");
    assert_eq!(decision.chosen, "a/mock-1");
    assert_eq!(decision.reason, RouteReason::IncumbentRetained);
}

#[test]
fn auto_moves_only_after_the_dwell_floor() {
    // A clearly better challenger: free, roomy, and reliable, against an
    // incumbent that is dear, nearly spent, AND flaky. The lead must clear the
    // margin comfortably so the test measures the dwell floor alone.
    let mut incumbent = candidate("a/mock-1", "a");
    incumbent.quota_remaining = Some(0.1);
    incumbent.error_rate = Some(0.5);
    let mut challenger = candidate("b/mock-1", "b");
    challenger.cost_per_1m_input = Some(0.0);
    challenger.quota_remaining = Some(1.0);
    challenger.error_rate = Some(0.0);
    let mut context = ctx(Some("a/mock-1"));
    context.dwell_turns = MIN_DWELL_TURNS - 1;
    let decision = decide(
        &[incumbent.clone(), challenger.clone()],
        &context,
        ModelRoutingLevel::Auto,
    )
    .expect("a decision");
    assert_eq!(
        decision.chosen, "a/mock-1",
        "a fresh incumbent keeps its turn"
    );
    context.dwell_turns = MIN_DWELL_TURNS;
    let decision =
        decide(&[incumbent, challenger], &context, ModelRoutingLevel::Auto).expect("a decision");
    assert_eq!(decision.chosen, "b/mock-1");
}

#[test]
fn dry_run_reports_the_incumbent_even_when_a_challenger_leads() {
    let incumbent = candidate("a/mock-1", "a");
    let mut challenger = candidate("b/mock-1", "b");
    challenger.cost_per_1m_input = Some(0.01);
    let decision = decide(
        &[incumbent, challenger],
        &ctx(Some("a/mock-1")),
        ModelRoutingLevel::DryRun,
    )
    .expect("a decision");
    assert_eq!(decision.chosen, "a/mock-1", "dry-run applies nothing");
    assert_eq!(decision.reason, RouteReason::IncumbentRetained);
    // …but the score is recorded, which is the whole point of the level.
    assert!(
        decision.candidates.iter().any(|row| row.score > 0.0),
        "dry-run still scores: {decision:?}"
    );
}

#[test]
fn no_incumbent_takes_the_best_survivor() {
    let incumbent = candidate("a/mock-1", "a");
    let mut challenger = candidate("b/mock-1", "b");
    challenger.cost_per_1m_input = Some(0.2);
    let decision = decide(
        &[incumbent, challenger],
        &ctx(None),
        ModelRoutingLevel::Auto,
    )
    .expect("a decision");
    assert_eq!(decision.reason, RouteReason::NoIncumbent);
    assert_eq!(decision.chosen, "b/mock-1");
}

#[test]
fn a_single_candidate_is_its_own_answer() {
    let only = candidate("a/mock-1", "a");
    let decision =
        decide(&[only], &ctx(Some("a/mock-1")), ModelRoutingLevel::On).expect("a decision");
    assert_eq!(decision.chosen, "a/mock-1");
    assert_eq!(decision.runner_up, None);
    assert_eq!(decision.reason, RouteReason::IncumbentRetained);
    // A fact nobody measured is neutral, never free: an unpriced, unmeasured
    // latency sample must not read as "cheapest and fastest".
    let mut unfactored = candidate("b/mock-1", "b");
    unfactored.cost_per_1m_input = None;
    unfactored.latency_ema_ms = None;
    let decision = decide(&[unfactored], &ctx(None), ModelRoutingLevel::On).expect("a decision");
    let terms = &decision.candidates.first().expect("one row").terms;
    for name in [
        TERM_COST,
        TERM_LATENCY,
        TERM_RELIABILITY,
        TERM_HEADROOM,
        TERM_IDLE,
    ] {
        let value = terms
            .iter()
            .find(|(term, _)| *term == name)
            .map(|(_, value)| *value)
            .expect("the term");
        assert_eq!(value, UNKNOWN_TERM, "{name}");
    }
}

#[test]
fn the_cost_term_is_absolute_so_close_prices_stay_close() {
    // Two providers one cent apart must not span the whole cost weight: the
    // margin would be meaningless if a rounding difference outranked it.
    let mut a = candidate("a/mock-1", "a");
    a.cost_per_1m_input = Some(3.0);
    let mut b = candidate("b/mock-1", "b");
    b.cost_per_1m_input = Some(3.01);
    let decision = decide(&[a, b], &ctx(None), ModelRoutingLevel::On).expect("a decision");
    let cost = |selector: &str| {
        decision
            .candidates
            .iter()
            .find(|row| row.selector == selector)
            .and_then(|row| {
                row.terms
                    .iter()
                    .find(|(name, _)| *name == TERM_COST)
                    .map(|(_, value)| *value)
            })
            .expect("the cost term")
    };
    let gap = (cost("a/mock-1") - cost("b/mock-1")).abs();
    assert!(gap < 0.01, "a one-cent gap spanned {gap}");
    assert_eq!(cost("a/mock-1"), 0.5, "the reference price scores neutral");
}

#[test]
fn an_empty_candidate_set_has_no_decision() {
    assert_eq!(
        decide(&[], &ctx(Some("a/mock-1")), ModelRoutingLevel::On),
        None
    );
    assert_eq!(decide(&[], &ctx(None), ModelRoutingLevel::DryRun), None);
}

#[test]
fn scores_stay_inside_the_unit_range() {
    let mut expensive = candidate("a/mock-1", "a");
    expensive.cost_per_1m_input = Some(1_000.0);
    expensive.latency_ema_ms = Some(60_000.0);
    let mut cheap = candidate("b/mock-1", "b");
    cheap.cost_per_1m_input = Some(0.0);
    cheap.latency_ema_ms = Some(1.0);
    cheap.capability_fit = Some(1.0);
    cheap.quality = Some(1.0);
    cheap.quota_remaining = Some(1.0);
    cheap.free_capacity = Some(1.0);
    let decision =
        decide(&[expensive, cheap], &ctx(None), ModelRoutingLevel::On).expect("a decision");
    for row in &decision.candidates {
        assert!(
            (0.0..=1.0).contains(&row.score),
            "{} scored {}",
            row.selector,
            row.score
        );
    }
    // The best possible row: every term at `1.0` except a foreign-provider
    // affinity term (no incumbent was named, so it is neutral).
    let best = decision
        .candidates
        .iter()
        .find(|row| row.selector == "b/mock-1")
        .expect("the cheap row");
    // Every term at its best except the two whose facts the fixture does not
    // supply (reliability has no sample, affinity has no incumbent): those
    // score neutral.
    let expected = WEIGHT_FIT
        + WEIGHT_QUALITY
        + WEIGHT_COST
        + WEIGHT_LATENCY
        + WEIGHT_RELIABILITY * UNKNOWN_TERM
        + WEIGHT_HEADROOM
        + WEIGHT_IDLE
        + WEIGHT_AFFINITY * UNKNOWN_TERM;
    assert!(
        (best.score - expected).abs() < 1e-3,
        "{} != {expected}",
        best.score
    );
}

#[test]
fn the_weights_sum_to_one() {
    let total = WEIGHT_FIT
        + WEIGHT_QUALITY
        + WEIGHT_COST
        + WEIGHT_LATENCY
        + WEIGHT_RELIABILITY
        + WEIGHT_HEADROOM
        + WEIGHT_IDLE
        + WEIGHT_AFFINITY;
    assert!((total - 1.0).abs() < 1e-6, "weights sum to {total}");
}

#[test]
fn a_failing_provider_scores_below_a_stable_one() {
    // Two candidates identical but for observed reliability: the stable one
    // must win on score alone, before the breaker's hard filter ever trips.
    // That grace period is the point of the graded term.
    let stable = candidate("a/mock-1", "a");
    let mut flaky = candidate("b/mock-1", "b");
    flaky.error_rate = Some(0.3);
    let decision = decide(&[flaky, stable], &ctx(None), ModelRoutingLevel::On).expect("a decision");
    assert_eq!(decision.chosen, "a/mock-1");
    let score = |selector: &str| {
        decision
            .candidates
            .iter()
            .find(|row| row.selector == selector)
            .map(|row| row.score)
            .expect("the row")
    };
    assert!(
        score("a/mock-1") > score("b/mock-1"),
        "{} !> {}",
        score("a/mock-1"),
        score("b/mock-1")
    );
}

#[test]
fn the_reliability_term_is_calibrated_to_the_breaker_threshold() {
    // A clean provider scores full marks; the 10% retry budget that trips the
    // breaker scores neutral; both are on the same saturating curve, so the
    // score degrades exactly where the hard filter takes over.
    let term = |rate: Option<f64>| lower_is_better_term(rate, ERROR_RATE_REFERENCE);
    assert!((term(Some(0.0)) - 1.0).abs() < 1e-6);
    assert!((term(Some(ERROR_RATE_REFERENCE)) - 0.5).abs() < 1e-6);
    assert!(term(Some(1.0)) < 0.2, "a failing provider is near zero");
    assert_eq!(term(None), UNKNOWN_TERM);
}

#[test]
fn an_unmeasured_error_rate_is_neutral_not_perfect() {
    // A provider the registry has never sampled must not out-score one with a
    // measured clean record, and must not be punished for the silence.
    let mut unsampled = candidate("a/mock-1", "a");
    unsampled.error_rate = None;
    let mut measured_clean = candidate("b/mock-1", "b");
    measured_clean.error_rate = Some(0.0);
    let decision = decide(
        &[unsampled, measured_clean],
        &ctx(None),
        ModelRoutingLevel::On,
    )
    .expect("a decision");
    let term = |selector: &str| {
        decision
            .candidates
            .iter()
            .find(|row| row.selector == selector)
            .and_then(|row| {
                row.terms
                    .iter()
                    .find(|(name, _)| *name == TERM_RELIABILITY)
                    .map(|(_, value)| *value)
            })
            .expect("the reliability term")
    };
    assert_eq!(term("a/mock-1"), UNKNOWN_TERM);
    assert_eq!(term("b/mock-1"), 1.0);
}

#[test]
fn the_incumbent_provider_keeps_its_affinity_term() {
    let incumbent = candidate("a/mock-1", "a");
    let sibling = candidate("b/mock-1", "b");
    let decision = decide(
        &[incumbent, sibling],
        &ctx(Some("a/mock-1")),
        ModelRoutingLevel::On,
    )
    .expect("a decision");
    let affinity = |selector: &str| {
        decision
            .candidates
            .iter()
            .find(|row| row.selector == selector)
            .and_then(|row| {
                row.terms
                    .iter()
                    .find(|(name, _)| *name == TERM_AFFINITY)
                    .map(|(_, value)| *value)
            })
            .expect("the affinity term")
    };
    assert_eq!(affinity("a/mock-1"), 1.0);
    assert_eq!(affinity("b/mock-1"), 0.0);
}

#[test]
fn equal_scores_keep_the_input_order() {
    // Two identical candidates, no incumbent: the decision is the first one in
    // every run (the stable sort), so a replay never reorders.
    let first = candidate("a/mock-1", "a");
    let second = candidate("b/mock-1", "b");
    for _ in 0..5 {
        let decision = decide(
            &[first.clone(), second.clone()],
            &ctx(None),
            ModelRoutingLevel::On,
        )
        .expect("a decision");
        assert_eq!(decision.chosen, "a/mock-1");
        assert_eq!(decision.runner_up.as_deref(), Some("b/mock-1"));
    }
}

#[test]
fn every_vocabulary_value_is_distinct_and_stable() {
    let reasons: std::collections::HashSet<&str> = RouteReason::all()
        .iter()
        .map(|reason| reason.as_str())
        .collect();
    assert_eq!(reasons.len(), RouteReason::all().len());
    let rejections: std::collections::HashSet<&str> = RouteRejection::all()
        .iter()
        .map(|rejection| rejection.as_str())
        .collect();
    assert_eq!(rejections.len(), RouteRejection::all().len());
    for level in [
        ModelRoutingLevel::Off,
        ModelRoutingLevel::DryRun,
        ModelRoutingLevel::Auto,
        ModelRoutingLevel::On,
    ] {
        assert_eq!(ModelRoutingLevel::parse(level.as_str()), level);
    }
}
