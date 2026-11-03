//! The admission plane's red-first battery: the 429-incident shape (nine
//! spawns behind a limit of six), the AIMD curve, the narrow bracket that
//! keeps spawn trees from deadlocking, and the dispatch stagger. Every
//! readiness signal here is an observable event (a resolved future, a
//! queue depth, a recorded grant) — never a sleep.

use super::*;
use std::sync::Mutex;
use std::time::Duration;

fn key() -> ProviderKey {
    ProviderKey::new("zai", "glm-5.3")
}

/// A stagger source that records every delay it hands out, so a test can
/// assert the dispatch fan-out without waiting for it.
#[derive(Debug, Default)]
struct RecordingStagger {
    handed: Mutex<Vec<u64>>,
}

impl RecordingStagger {
    fn handed(&self) -> Vec<u64> {
        self.handed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl StaggerSource for RecordingStagger {
    fn next_delay_ms(&self) -> u64 {
        let next = u64::try_from(self.handed().len()).unwrap_or(0) * 250;
        self.handed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(next);
        next
    }
}

/// Poll `condition` until it holds, bounded by a deadline. The condition is
/// an observable admission event (a queue depth, a recorded grant), so this
/// never depends on how long any work takes.
/// Assert-and-unwrap one admitted stream lease (the battery's old
/// `Option::expect` shape, now a three-way answer).
async fn admitted(registry: &ProviderAdmission, key: &ProviderKey, holder: &str) -> u64 {
    match registry
        .acquire_stream_within(key, holder, Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(lease) => lease,
        other => panic!("expected an admitted lease for {holder}, got {other:?}"),
    }
}

async fn until(condition: impl Fn() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::task::yield_now().await;
    }
}

/// Nine spawns behind a limit of six: six dispatch immediately, three park
/// in the key's FIFO queue, and each freed slot dispatches exactly one —
/// in arrival order. This is the incident's shape (nine parallel subagents
/// on one provider) against a shared limit instead of nine private ones.
#[tokio::test]
async fn nine_spawns_behind_a_limit_of_six_queue_three_and_dispatch_fifo() {
    let registry = ProviderAdmission::with_stagger(6, Arc::new(RecordingStagger::default()));
    let key = key();
    // The AIMD start limit is the settings default (six).
    assert_eq!(registry.start_limit(), 6);

    let mut immediate = Vec::new();
    for index in 0..6 {
        let lease = registry
            .spawn_gate_within(&key, &format!("spawn-{index}"), Duration::from_millis(200))
            .await
            .expect("the first six spawns admit immediately");
        immediate.push(lease.0);
    }
    assert_eq!(registry.snapshot(&key).in_flight, 6);
    assert_eq!(registry.snapshot(&key).queued, 0);

    // The remaining three park: their gates are still pending, so a
    // zero-budget probe sees the queue instead of a grant.
    for index in 6..9 {
        assert!(
            registry
                .spawn_gate_within(&key, &format!("spawn-{index}"), Duration::from_millis(0))
                .await
                .is_none(),
            "a spawn over the limit must queue, not admit"
        );
    }
    // Each zero-budget probe dequeued itself; re-park them for the FIFO
    // assertion below with a real budget.
    let mut queued = Vec::new();
    for index in 6..9 {
        let registry = Arc::clone(&registry);
        let key = key.clone();
        let spawn_id = format!("spawn-{index}");
        queued.push(tokio::spawn(async move {
            registry
                .spawn_gate_within(&key, &spawn_id, Duration::from_secs(5))
                .await
                .map(|(lease, stagger)| (spawn_id, lease, stagger))
        }));
    }
    until(
        || registry.snapshot(&key).queued == 3,
        "three queued spawns",
    )
    .await;

    // Release the six held slots one at a time: each frees exactly one
    // dispatch, and the dispatches arrive in FIFO order.
    for (released, lease) in immediate.into_iter().enumerate() {
        registry.release(lease);
        until(
            || registry.snapshot(&key).queued == 2usize.saturating_sub(released),
            "one dequeued spawn per released slot",
        )
        .await;
    }
    let mut order = Vec::new();
    for handle in queued {
        let (spawn_id, lease, _stagger) = handle
            .await
            .expect("the queued gate task must not panic")
            .expect("every queued spawn dispatches once its slot frees");
        registry.release(lease);
        order.push(spawn_id);
    }
    assert_eq!(order, vec!["spawn-6", "spawn-7", "spawn-8"], "FIFO order");
    assert_eq!(registry.snapshot(&key).queued, 0);
}

/// AIMD: a 429 series halves the limit down to the floor; a success series
/// grows it back one slot at a time up to the settings cap.
#[tokio::test]
async fn aimd_halves_on_429_down_to_the_floor_and_grows_back_to_the_cap() {
    let registry = ProviderAdmission::new(6);
    let key = key();
    assert_eq!(registry.snapshot(&key).limit, 6);

    // A 429 streak: 6 -> 3 -> 1 (the floor holds).
    for expected in [3, 1, 1] {
        let lease = admitted(&registry, &key, "worker-a").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
        assert_eq!(registry.snapshot(&key).limit, expected);
        assert!(registry.snapshot(&key).consecutive_429 > 0);
    }

    // A success streak: +1 per settled stream, capped at the settings cap.
    for expected in [2, 3, 4, 5, 6, 6] {
        let lease = admitted(&registry, &key, "worker-a").await;
        registry.settle(lease, StreamOutcome::Settled);
        assert_eq!(registry.snapshot(&key).limit, expected);
        assert_eq!(registry.snapshot(&key).consecutive_429, 0);
    }
    assert_eq!(
        registry.snapshot(&key).cap,
        6,
        "the cap is the settings value"
    );
}

/// A 5xx/timeout storm must not RAISE the concurrency limit (review #10).
///
/// The AIMD loop is the thing being tested, not the classifier: `Settled` grew the limit by
/// one per stream, so a provider failing every request was rewarded for failing. `Failed`
/// releases the slot and leaves the limit alone — and, unlike `RateLimited`, does not halve
/// it either, because the right concurrency for a broken provider is not knowable.
///
/// The breaker half is asserted too: a failure is a failure for the retry budget, or a
/// provider returning 500 to everything would never trip it.
#[tokio::test]
async fn a_failed_settle_neither_grows_nor_halves_the_limit() {
    // Cap 8, start 6 (START_LIMIT clamped): `Settled` would climb toward 8 in four steps, so
    // a `Failed` that climbed is visible instead of hidden by the ceiling.
    let registry = ProviderAdmission::new(8);
    let key = key();
    let start = registry.snapshot(&key).limit;
    assert_eq!(
        start, 6,
        "the fixture starts below its cap, or the test proves nothing"
    );

    for _ in 0..4 {
        let lease = admitted(&registry, &key, "worker-a").await;
        registry.settle(lease, StreamOutcome::Failed);
        let snapshot = registry.snapshot(&key);
        assert_eq!(
            snapshot.limit, start,
            "a failure must not move the limit in either direction"
        );
        assert_eq!(
            snapshot.consecutive_429, 0,
            "and must not be counted as a rate limit"
        );
    }
    // The failures DID reach the retry budget, which is what opens the breaker on a 5xx storm.
    let snapshot = registry.snapshot(&key);
    assert!(
        snapshot.breaker.budget_failures > 0,
        "a 5xx storm must land in the breaker's budget: {snapshot:?}"
    );

    // The slot was released every time, so the pool still admits.
    assert_eq!(snapshot.in_flight, 0, "every failure released its slot");
}

/// A backlog queued BEFORE the breaker tripped must not drain while it is half-open (#9).
///
/// The uncovered half of the finding. `breaker_gate` refuses half-open on the ACQUIRE path,
/// but `dispatch()` — reached from `settle` and `release` — never passes through it, and it
/// gated only on `Open`. A waiter parked while the breaker was CLOSED therefore got its grant
/// as soon as the cooldown expired: a non-probe lease beside the single probe, whose settle
/// lands in the `Open | HalfOpen => {}` no-op arm and teaches the breaker nothing.
///
/// Breakers are keyed by PROVIDER and limits by (provider, model), which is what makes the
/// scenario reachable: a waiter can sit at its limit on one model while the breaker for the
/// same provider trips from another model's failures.
#[tokio::test]
async fn a_backlog_queued_before_the_trip_does_not_drain_while_half_open() {
    let clock = ScriptedClock::new(1_000);
    let registry = ProviderAdmission::with_parts(
        1,
        Arc::new(JitteredStagger::new()),
        Arc::clone(&clock) as Arc<dyn ClockSource>,
    );
    // Fill the ONLY slot on the waiting model, so the next acquirer must park in its FIFO.
    let parked_key = ProviderKey::new("zai", "glm-parked");
    let holder = admitted(&registry, &parked_key, "slot-holder").await;

    // Park a second acquirer there while the breaker is still CLOSED.
    let parked = {
        let registry = Arc::clone(&registry);
        let key = parked_key.clone();
        tokio::spawn(async move {
            registry
                .acquire_stream_within(&key, "pre-trip-waiter", Duration::from_secs(30))
                .await
        })
    };
    until(
        || registry.snapshot(&parked_key).queued == 1,
        "the pre-trip waiter parks in the FIFO",
    )
    .await;

    // NOW trip the provider's breaker from a different model on the same provider.
    let tripping_key = ProviderKey::new("zai", "glm-tripping");
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &tripping_key, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }
    assert_eq!(
        registry.snapshot(&tripping_key).breaker.state,
        BreakerState::Open,
        "the fixture must open the provider breaker, or the test proves nothing"
    );

    // The `Open -> HalfOpen` transition happens INSIDE `breaker_gate`, so it takes an acquire
    // to reach it — that acquire is the probe. Without it the state stays `Open`, which already
    // blocks `dispatch`, and the scenario would not exist.
    clock.advance(BREAKER_COOLDOWN_MS);
    let probe = match registry
        .acquire_stream_within(&tripping_key, "probe", Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(lease) => lease,
        other => panic!("the probe must admit, got {other:?}"),
    };
    assert_eq!(
        registry.snapshot(&tripping_key).breaker.state,
        BreakerState::HalfOpen,
        "the probe acquire is what moves the breaker to half-open"
    );

    // Freeing the parked key's slot calls `dispatch` with the breaker HALF-OPEN and the probe in
    // flight. The backlog must stay parked: only the probe may be admitted in half-open.
    registry.release(holder);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        registry.snapshot(&parked_key).queued,
        1,
        "the pre-trip backlog must NOT drain while the breaker is half-open"
    );
    assert_eq!(
        registry.snapshot(&parked_key).in_flight,
        0,
        "and the freed slot must stay free"
    );
    parked.abort();
    registry.release(probe);
}

/// A settings cap below the compiled start limit clamps the start: the
/// ceiling is honored immediately instead of being grown into.
#[tokio::test]
async fn a_low_settings_cap_clamps_the_aimd_start() {
    let registry = ProviderAdmission::new(2);
    assert_eq!(registry.start_limit(), 2);
    let key = key();
    assert_eq!(registry.snapshot(&key).limit, 2);
    for _ in 0..2 {
        let lease = admitted(&registry, &key, "worker-a").await;
        registry.settle(lease, StreamOutcome::Settled);
    }
    assert_eq!(
        registry.snapshot(&key).limit,
        2,
        "the limit never grows past the settings cap"
    );
}

/// A half-open breaker admits ONLY its probe (review #9).
///
/// The queue that piled up during the outage used to drain as soon as the cooldown expired,
/// because `dispatch` gated on `Open` alone. Those backlog grants were not the probe, so their
/// settles hit the no-transition arm: the breaker learned nothing from them while fresh
/// traffic stayed refused, and full traffic could not resume.
#[tokio::test]
async fn a_half_open_breaker_drains_no_backlog() {
    let clock = ScriptedClock::new(1_000);
    let registry = ProviderAdmission::with_parts(
        12,
        Arc::new(JitteredStagger::new()),
        Arc::clone(&clock) as Arc<dyn ClockSource>,
    );
    let key = key();
    // Ten failures open the breaker, but only ONE may be a rate limit: each 429 halves the
    // limit, and at the floor `dispatch` refuses on `in_flight >= limit` instead of on the
    // breaker — which would make this test pass whether or not the breaker gates. The rest are
    // `Failed`, which count as failures (review #10) and leave the limit alone.
    for index in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &key, "storm-worker").await;
        if index == 0 {
            registry.settle(
                lease,
                StreamOutcome::RateLimited(RateLimitReason::RateLimit),
            );
        } else {
            registry.settle(lease, StreamOutcome::Failed);
        }
    }
    assert_eq!(
        registry.snapshot(&key).breaker.state,
        BreakerState::Open,
        "the fixture must open the breaker, or the test proves nothing"
    );
    assert!(
        registry.snapshot(&key).limit > 1,
        "and the limit must stay above the floor, or the refusal comes from the LIMIT rather \
         than the breaker: {:?}",
        registry.snapshot(&key)
    );

    // Past the cooldown the FIRST acquire is the single probe.
    clock.advance(BREAKER_COOLDOWN_MS);
    let probe = match registry
        .acquire_stream_within(&key, "probe", Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(lease) => lease,
        other => panic!("the probe must admit, got {other:?}"),
    };
    assert_eq!(
        registry.snapshot(&key).in_flight,
        1,
        "the probe holds the only slot; slots exist, so a backlog drain would show here"
    );

    // A backlog waiter arriving now is REFUSED, not queued and not admitted: `breaker_gate`
    // answers `Refuse` for every non-probe acquirer in half-open, so the backlog cannot drain
    // beside the probe. Asserting `Refused` specifically (not `Unadmitted`) is what
    // distinguishes "the breaker stopped it" from "the wait budget expired while it queued" —
    // the second shape would be the defect.
    let backlog = registry
        .acquire_stream_within(&key, "backlog-worker", Duration::from_millis(20))
        .await;
    assert_eq!(
        backlog,
        StreamAdmission::Refused,
        "a non-probe waiter must be refused by the breaker, not queued beside the probe"
    );
    assert_eq!(
        registry.snapshot(&key).in_flight,
        1,
        "and the probe is still the only holder"
    );
    // Release the probe so the slot returns; a lease id is a plain `u64`, so `drop` would be
    // a no-op and the pool would stay held for the rest of the binary.
    registry.release(probe);
}

/// The narrow bracket (oh-my-pi #3749): a spawn tree must not deadlock on
/// its own admission. With a limit of ONE, three generations nest: the
/// parent's stream slot must free for the child's spawn, and the child's
/// OWN stream slot must free for the grandchild's spawn — every release is
/// a STREAM boundary, never a process death.
#[tokio::test]
async fn a_nested_spawn_is_admitted_by_the_parent_stream_settle_not_its_end() {
    let registry = ProviderAdmission::new(1);
    let key = key();

    // Generation 1: the parent's turn is streaming, holding the only slot.
    let parent_stream = admitted(&registry, &key, "parent").await;
    assert_eq!(registry.snapshot(&key).in_flight, 1);

    // The parent's tool call spawns a child: the spawn queues behind the
    // exhausted limit.
    let child = {
        let registry = Arc::clone(&registry);
        let key = key.clone();
        tokio::spawn(async move {
            registry
                .spawn_gate_within(&key, "child", Duration::from_secs(5))
                .await
                .map(|(lease, _)| lease)
        })
    };
    until(|| registry.snapshot(&key).queued == 1, "the child queued").await;
    assert!(
        !child.is_finished(),
        "the child waits on the exhausted pool"
    );

    // The parent's stream SETTLES while its turn keeps running (the tool
    // call is executing, no stream in flight). That release dispatches the
    // child. A bracket held for the parent's whole lifetime would hang here
    // forever.
    registry.settle(parent_stream, StreamOutcome::Settled);
    let child_spawn = child
        .await
        .expect("the child gate task must not panic")
        .expect("the parent's stream settle dispatches the child");
    assert_eq!(registry.snapshot(&key).in_flight, 1);

    // The child admits (its create settled) and releases its spawn slot:
    // the reservation is the create round trip, never the child's lifetime.
    registry.release(child_spawn);
    assert_eq!(
        registry.snapshot(&key).in_flight,
        0,
        "the child's spawn reservation released at admission"
    );

    // Generation 2: the child's first turn streams, holding the only slot.
    let child_stream = admitted(&registry, &key, "child").await;

    // The child's tool call spawns a grandchild of the same provider: it
    // queues behind the child's own stream.
    let grandchild = {
        let registry = Arc::clone(&registry);
        let key = key.clone();
        tokio::spawn(async move {
            registry
                .spawn_gate_within(&key, "grandchild", Duration::from_secs(5))
                .await
                .map(|(lease, _)| lease)
        })
    };
    until(
        || registry.snapshot(&key).queued == 1,
        "the grandchild queued",
    )
    .await;
    assert!(
        !grandchild.is_finished(),
        "the grandchild must wait while the child's stream holds the only slot"
    );

    // The child's stream settles: the grandchild dispatches. Two levels
    // deep, no deadlock.
    registry.settle(child_stream, StreamOutcome::Settled);
    let grandchild_lease = grandchild
        .await
        .expect("the grandchild gate task must not panic")
        .expect("the child's stream settle dispatches the grandchild");
    registry.release(grandchild_lease);
    assert_eq!(registry.snapshot(&key).queued, 0);
    assert_eq!(registry.snapshot(&key).in_flight, 0);
}

/// The dispatch stagger: each dispatch carries its own randomized delay
/// from the injected source, and the production source stays inside the
/// documented 0-2s window.
#[tokio::test]
async fn dispatch_stagger_comes_from_the_injected_source_and_stays_in_range() {
    let recorder = Arc::new(RecordingStagger::default());
    let registry =
        ProviderAdmission::with_stagger(1, Arc::clone(&recorder) as Arc<dyn StaggerSource>);
    let key = key();

    // The immediate dispatch and the queued dispatch each take one delay
    // from the source.
    let (first, first_stagger) = registry
        .spawn_gate_within(&key, "spawn-a", Duration::from_millis(50))
        .await
        .expect("the first spawn admits");
    let queued = {
        let registry = Arc::clone(&registry);
        let key = key.clone();
        tokio::spawn(async move {
            registry
                .spawn_gate_within(&key, "spawn-b", Duration::from_secs(5))
                .await
        })
    };
    until(
        || registry.snapshot(&key).queued == 1,
        "the second spawn queued",
    )
    .await;
    registry.release(first);
    let (second, second_stagger) = queued
        .await
        .expect("the queued gate task must not panic")
        .expect("the freed slot dispatches the queued spawn");

    // The dispatches are spread: the two grants carry DIFFERENT delays, so
    // the herd arrives spread instead of synchronized.
    assert_ne!(
        first_stagger, second_stagger,
        "consecutive dispatches must not share a delay"
    );
    assert_eq!(
        recorder.handed(),
        vec![
            first_stagger.as_millis() as u64,
            second_stagger.as_millis() as u64
        ]
    );
    registry.release(second);

    // The production source is bounded by the documented window.
    let jitter = JitteredStagger::new();
    let delays: Vec<u64> = (0..64).map(|_| jitter.next_delay_ms()).collect();
    assert!(
        delays.iter().all(|delay| *delay <= SPAWN_STAGGER_MAX_MS),
        "every jittered delay stays inside 0..={SPAWN_STAGGER_MAX_MS}ms"
    );
    assert!(
        delays.iter().any(|delay| *delay > 0),
        "the jitter must actually spread the herd, not pin every delay to zero"
    );
    assert!(
        delays.windows(2).any(|pair| pair[0] != pair[1]),
        "the jitter must vary between dispatches"
    );
}

/// The key is the resolved `(provider, model)` pair: two models on one
/// provider have independent limits, and a selector round-trips.
#[tokio::test]
async fn admission_is_keyed_by_provider_and_model() {
    let registry = ProviderAdmission::new(1);
    let glm = ProviderKey::from_selector("zai/glm-5.3");
    let kimi = ProviderKey::from_selector("zai/kimi-k2");
    assert_eq!(glm.selector(), "zai/glm-5.3");
    assert_ne!(glm, kimi);

    let glm_lease = admitted(&registry, &glm, "worker-a").await;
    // The other model on the same provider is unaffected.
    let kimi_lease = admitted(&registry, &kimi, "worker-b").await;
    // ...while a second stream on glm's saturated key queues.
    assert_eq!(
        registry
            .acquire_stream_within(&glm, "worker-c", Duration::from_millis(0))
            .await,
        StreamAdmission::Unadmitted,
        "the saturated key queues its own traffic"
    );
    registry.release(glm_lease);
    registry.release(kimi_lease);
}

/// A worker that dies mid-stream must not shrink the pool forever: its
/// leases are reaped by holder.
#[tokio::test]
async fn a_dead_holder_has_its_leases_reaped() {
    let registry = ProviderAdmission::new(1);
    let key = key();
    let _lease = admitted(&registry, &key, "worker-dead").await;
    assert_eq!(registry.snapshot(&key).in_flight, 1);

    assert_eq!(registry.release_holder("worker-dead"), 1);
    assert_eq!(registry.snapshot(&key).in_flight, 0);
    assert!(matches!(
        registry
            .acquire_stream_within(&key, "worker-alive", Duration::from_millis(50))
            .await,
        StreamAdmission::Admitted(_)
    ));
    // A second reap is a no-op.
    assert_eq!(registry.release_holder("worker-dead"), 0);
}

/// An expired wait budget fails open: the caller proceeds unadmitted
/// instead of hanging the turn, and its queue slot is released.
#[tokio::test]
async fn an_expired_wait_budget_fails_open_and_releases_the_queue_slot() {
    let registry = ProviderAdmission::new(1);
    let key = key();
    let held = admitted(&registry, &key, "worker-a").await;

    assert_eq!(
        registry
            .acquire_stream_within(&key, "worker-b", Duration::from_millis(0))
            .await,
        StreamAdmission::Unadmitted,
        "a saturated key answers the expired budget with fail-open"
    );
    assert_eq!(
        registry.snapshot(&key).queued,
        0,
        "the abandoned waiter left the queue"
    );

    // The pool still works: the freed slot admits a later stream.
    registry.settle(held, StreamOutcome::Settled);
    assert!(matches!(
        registry
            .acquire_stream_within(&key, "worker-c", Duration::from_millis(50))
            .await,
        StreamAdmission::Admitted(_)
    ));
}

/// The AIMD signal a settled assistant message carries: a 429 (by status,
/// by classified kind, or in the error text) halves; anything else adds.
#[test]
fn a_message_carries_its_aimd_signal() {
    let mut message = AssistantMessage {
        content: Vec::new(),
        api: "openai".to_string(),
        provider: "zai".to_string(),
        model: "glm-5.3".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: pa_agent::types::Usage::zero(),
        stop_reason: pa_agent::types::StopReason::Stop,
        stop_reason_raw: None,
        error_message: None,
        timestamp: 0,
    };
    assert_eq!(outcome_for_message(&message), StreamOutcome::Settled);

    message.stop_reason = pa_agent::types::StopReason::Error;
    message.diagnostics = Some(vec![pa_agent::types::AssistantMessageDiagnostic {
        kind: "provider_stream_failure".to_string(),
        timestamp: 0,
        error: None,
        details: Some(serde_json::json!({ "status": 429, "kind": "rate_limit" })),
    }]);
    assert_eq!(
        outcome_for_message(&message),
        StreamOutcome::RateLimited(RateLimitReason::RateLimit)
    );

    // The text-only shape (no diagnostic) still classifies — and M3
    // types the cause: "too many concurrent requests" is a concurrency
    // cap, not a rolling rate cap.
    message.diagnostics = None;
    message.error_message = Some("API Error: 429 Too many concurrent requests".to_string());
    assert_eq!(
        outcome_for_message(&message),
        StreamOutcome::RateLimited(RateLimitReason::Concurrent)
    );

    // A non-429 FAILURE is not a settle (review #10). It used to be, and that made a 5xx
    // storm raise the concurrency limit against a provider failing every request while the
    // breaker counted those settles as clean samples. `Failed` releases the slot and counts
    // in the breaker, but sends no AIMD signal in either direction.
    message.error_message = Some("provider down: 503".to_string());
    assert_eq!(outcome_for_message(&message), StreamOutcome::Failed);

    // A 5xx STATUS is the same verdict even when the error text says nothing about it.
    message.error_message = None;
    message.diagnostics = Some(vec![pa_agent::types::AssistantMessageDiagnostic {
        kind: "provider_stream_failure".to_string(),
        timestamp: 0,
        error: None,
        details: Some(serde_json::json!({ "status": 503, "kind": "server_error" })),
    }]);
    assert_eq!(outcome_for_message(&message), StreamOutcome::Failed);

    // And a transport/timeout kind, which carries no status at all.
    message.diagnostics = Some(vec![pa_agent::types::AssistantMessageDiagnostic {
        kind: "provider_stream_failure".to_string(),
        timestamp: 0,
        error: None,
        details: Some(serde_json::json!({ "kind": "timeout" })),
    }]);
    assert_eq!(outcome_for_message(&message), StreamOutcome::Failed);

    // A message with NO failure at all is still a plain settle: the third arm must not
    // swallow the healthy case, or the limit can never grow.
    message.diagnostics = None;
    message.error_message = None;
    message.stop_reason = pa_agent::types::StopReason::Stop;
    assert_eq!(outcome_for_message(&message), StreamOutcome::Settled);
}

/// The wire spelling of an outcome round-trips (the worker/supervisor
/// settle contract): the outcome word carries a default reason, and the
/// typed reason rides its own wire field beside it.
#[test]
fn stream_outcomes_round_trip_through_their_wire_spelling() {
    // EVERY variant, not just the happy one. This assertion was `Settled`-only, which is
    // exactly how `Failed` could be added to `as_wire` and left out of `from_wire`: the write
    // side emitted a word the read side turned into `None`, and the supervisor's
    // `unwrap_or(Settled)` turned that back into a success (review #10).
    for outcome in [
        StreamOutcome::Settled,
        StreamOutcome::Failed,
        StreamOutcome::RateLimited(RateLimitReason::RateLimit),
    ] {
        assert_eq!(
            StreamOutcome::from_wire(outcome.as_wire()),
            Some(outcome),
            "{outcome:?} must survive its own wire spelling"
        );
    }
    assert_eq!(
        StreamOutcome::from_wire(StreamOutcome::Settled.as_wire()),
        Some(StreamOutcome::Settled)
    );
    // A bare `rate_limited` outcome word (a legacy settle, a stripped
    // field) reads as the plain retryable 429; the typed reason refines
    // it through its own wire spelling.
    assert_eq!(
        StreamOutcome::from_wire("rate_limited"),
        Some(StreamOutcome::RateLimited(RateLimitReason::RateLimit))
    );
    assert_eq!(StreamOutcome::from_wire("nonsense"), None);
    for reason in [
        RateLimitReason::QuotaExhausted,
        RateLimitReason::RateLimit,
        RateLimitReason::Concurrent,
        RateLimitReason::Capacity,
        RateLimitReason::Server,
        RateLimitReason::SpendCap,
    ] {
        assert_eq!(RateLimitReason::from_wire(reason.as_wire()), Some(reason));
    }
}

/// The end-to-end bracket through [`admit_stream_fn`]: a stream holds a
/// slot for exactly its own lifetime, so a SECOND stream on the same key
/// waits until the first is settled (or dropped) — and a settled 429
/// halves the shared limit.
#[tokio::test]
async fn a_wrapped_stream_holds_its_slot_only_for_its_own_lifetime() {
    use pa_agent::stream::StreamRequestOptions;
    use pa_agent::types::Model as AgentModel;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let registry = ProviderAdmission::new(6);
    let key = key();
    let started = Arc::new(AtomicUsize::new(0));
    // A stream that never ends on its own: its slot must free on DROP.
    let inner: StreamFn = {
        let started = Arc::clone(&started);
        Arc::new(
            move |_model: AgentModel, _context: LlmContext, _options: StreamRequestOptions| {
                started.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    let (_handle, consumer) = pa_agent::stream::event_stream();
                    Ok(Box::new(consumer) as Box<dyn ModelStream>)
                })
            },
        )
    };
    let key_of: KeyResolver = {
        let key = key.clone();
        Arc::new(move || key.clone())
    };
    let client = AdmissionClient::local(Arc::clone(&registry));
    let stream_fn = admit_stream_fn(Arc::clone(&inner), Arc::clone(&key_of), client);

    let model = AgentModel {
        id: "glm-5.3".to_string(),
        name: "glm".to_string(),
        api: "openai".to_string(),
        provider: "zai".to_string(),
        base_url: String::new(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 0,
        max_tokens: 0,
        max_tokens_explicit: false,
    };
    let context = LlmContext {
        system_prompt: None,
        system_prompt_cache_prefix: None,
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let options = StreamRequestOptions {
        transport: None,
        temperature: None,
        max_tokens: None,
        reasoning: pa_agent::types::ThinkingLevel::Off,
        session_id: None,
        service_tier: None,
        api_key: None,
        signal: pa_agent::abort::AbortSignal::never(),
        on_payload: None,
        on_response: None,
        headers: None,
    };

    // Two live streams: both take a slot (the limit is six).
    let first = stream_fn(model.clone(), context.clone(), options.clone())
        .await
        .expect("the first stream admits");
    assert_eq!(registry.snapshot(&key).in_flight, 1);
    let second = stream_fn(model.clone(), context.clone(), options.clone())
        .await
        .expect("the second stream admits");
    assert_eq!(registry.snapshot(&key).in_flight, 2);
    assert_eq!(started.load(Ordering::SeqCst), 2);

    // Dropping a stream returns its slot: the bracket is the stream's
    // lifetime, never the caller's.
    drop(first);
    until(
        || registry.snapshot(&key).in_flight == 1,
        "the dropped stream's slot returns",
    )
    .await;
    drop(second);
    until(
        || registry.snapshot(&key).in_flight == 0,
        "the last stream's slot returns",
    )
    .await;

    // A stream whose result carries a 429 halves the shared limit.
    let limited: StreamFn = Arc::new(
        |_model: AgentModel, _context: LlmContext, _options: StreamRequestOptions| {
            Box::pin(async move {
                let (handle, consumer) = pa_agent::stream::event_stream();
                let message = AssistantMessage {
                    content: Vec::new(),
                    api: "openai".to_string(),
                    provider: "zai".to_string(),
                    model: "glm-5.3".to_string(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: pa_agent::types::Usage::zero(),
                    stop_reason: pa_agent::types::StopReason::Error,
                    stop_reason_raw: None,
                    error_message: Some("API Error: 429 too many requests".to_string()),
                    timestamp: 0,
                };
                handle.end(Some(message));
                Ok(Box::new(consumer) as Box<dyn ModelStream>)
            })
        },
    );
    let limited_fn = admit_stream_fn(
        limited,
        key_of,
        AdmissionClient::local(Arc::clone(&registry)),
    );
    let mut stream = limited_fn(model, context, options)
        .await
        .expect("the limited stream admits");
    let message = stream.result().await.expect("the stream resolves");
    assert_eq!(
        outcome_for_message(&message),
        StreamOutcome::RateLimited(RateLimitReason::RateLimit)
    );
    // The settle rides `result()`; give the async settle its turn.
    until(
        || registry.snapshot(&key).limit == 3,
        "the 429 halves the shared limit",
    )
    .await;
    assert_eq!(registry.snapshot(&key).in_flight, 0);
}

// ===== M3/M4/M5 (ADR `ratelimit-admission`, wave 2b) =====

/// M3: the AIMD decrease is reason-aware — an account-local cause (a
/// spent quota or a spend cap) drops the limit to the floor at once,
/// a plain 429 only halves it.
#[tokio::test]
async fn account_local_reasons_drop_the_limit_to_the_floor_at_once() {
    let registry = ProviderAdmission::new(6);
    let key = key();

    // A spent quota: 6 -> 1 (the floor), not 6 -> 3.
    let lease = admitted(&registry, &key, "worker-a").await;
    registry.settle(
        lease,
        StreamOutcome::RateLimited(RateLimitReason::QuotaExhausted),
    );
    assert_eq!(registry.snapshot(&key).limit, 1);
    assert_eq!(registry.snapshot(&key).consecutive_429, 1);

    // Recovery: settled streams grow it back one slot at a time.
    let lease = admitted(&registry, &key, "worker-a").await;
    registry.settle(lease, StreamOutcome::Settled);
    assert_eq!(registry.snapshot(&key).limit, 2);

    // A spend cap is just as account-local.
    let lease = admitted(&registry, &key, "worker-b").await;
    registry.settle(lease, StreamOutcome::RateLimited(RateLimitReason::SpendCap));
    assert_eq!(registry.snapshot(&key).limit, 1);

    // A plain 429 halves (2 -> 1 here), never further than the floor.
    let lease = admitted(&registry, &key, "worker-a").await;
    registry.settle(
        lease,
        StreamOutcome::RateLimited(RateLimitReason::RateLimit),
    );
    assert_eq!(registry.snapshot(&key).limit, 1);
}

/// M4: the retry budget — rate-limited settles past 10% of the sliding
/// window (with enough samples) trip the provider's breaker OPEN, and
/// new stream acquires fail fast instead of queueing.
#[tokio::test]
async fn a_spent_retry_budget_opens_the_breaker_and_refuses_acquires() {
    let clock = ScriptedClock::new(1_000);
    let registry = ProviderAdmission::with_parts(
        6,
        Arc::new(JitteredStagger::new()),
        Arc::clone(&clock) as Arc<dyn ClockSource>,
    );
    let key = key();
    assert_eq!(
        registry.snapshot(&key).breaker.state,
        BreakerState::Closed,
        "a fresh provider starts closed"
    );

    // Ten rate-limited settles: the window fills past the 10% bound on
    // its minimum sample count and the breaker opens.
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &key, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }
    let snapshot = registry.snapshot(&key);
    assert_eq!(snapshot.breaker.state, BreakerState::Open);
    assert_eq!(snapshot.breaker.budget_failures, RETRY_BUDGET_MIN_SAMPLES);
    assert_eq!(snapshot.breaker.budget_samples, RETRY_BUDGET_MIN_SAMPLES);

    // New stream acquires fail fast: no queueing, no fail-open — the
    // caller must fail over instead.
    assert_eq!(
        registry
            .acquire_stream_within(&key, "worker-next", Duration::from_millis(50))
            .await,
        StreamAdmission::Refused
    );

    // A healthy provider's occasional 429 never trips: under the sample
    // floor the budget stays unspent.
    let other = ProviderKey::new("healthy", "m1");
    for _ in 0..(RETRY_BUDGET_MIN_SAMPLES - 1) {
        let lease = admitted(&registry, &other, "worker-a").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }
    assert_eq!(
        registry.snapshot(&other).breaker.state,
        BreakerState::Closed,
        "too few samples to judge the budget"
    );
}

/// M4: the OPEN breaker lifts through ONE half-open probe after the
/// cooldown — the first acquire is the probe, everything else still
/// fail-fasts, and a settled probe closes the breaker with a fresh
/// budget.
#[tokio::test]
async fn an_open_breaker_half_opens_after_the_cooldown_and_one_probe_closes_it() {
    let clock = ScriptedClock::new(1_000);
    let registry = ProviderAdmission::with_parts(
        6,
        Arc::new(JitteredStagger::new()),
        Arc::clone(&clock) as Arc<dyn ClockSource>,
    );
    let key = key();
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &key, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }
    assert_eq!(registry.snapshot(&key).breaker.state, BreakerState::Open);

    // Inside the cooldown everything fail-fasts.
    clock.advance(BREAKER_COOLDOWN_MS - 1);
    assert_eq!(
        registry
            .acquire_stream_within(&key, "worker-a", Duration::from_millis(50))
            .await,
        StreamAdmission::Refused
    );

    // Past the cooldown the FIRST acquire is the single probe...
    clock.advance(1);
    let probe = match registry
        .acquire_stream_within(&key, "probe", Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(lease) => lease,
        other => panic!("the probe must admit, got {other:?}"),
    };
    assert_eq!(
        registry.snapshot(&key).breaker.state,
        BreakerState::HalfOpen
    );
    // ...and everything else still fail-fasts while it is in flight.
    assert_eq!(
        registry
            .acquire_stream_within(&key, "worker-b", Duration::from_millis(50))
            .await,
        StreamAdmission::Refused
    );

    // A settled probe closes the breaker and clears the storm's
    // evidence: the budget starts fresh.
    registry.settle(probe, StreamOutcome::Settled);
    let snapshot = registry.snapshot(&key);
    assert_eq!(snapshot.breaker.state, BreakerState::Closed);
    assert_eq!(snapshot.breaker.budget_samples, 0);
    assert!(
        matches!(
            registry
                .acquire_stream_within(&key, "worker-c", Duration::from_millis(50))
                .await,
            StreamAdmission::Admitted(_)
        ),
        "a closed breaker admits again"
    );
}

/// M4: a rate-limited probe re-opens the breaker with a fresh cooldown,
/// and a probe that drops without a verdict re-opens it too (the breaker
/// must never wedge in half-open).
#[tokio::test]
async fn a_failed_or_dropped_probe_re_opens_the_breaker() {
    let clock = ScriptedClock::new(1_000);
    let registry = ProviderAdmission::with_parts(
        6,
        Arc::new(JitteredStagger::new()),
        Arc::clone(&clock) as Arc<dyn ClockSource>,
    );
    let key = key();
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &key, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }

    // The probe comes back rate-limited: re-open, cooldown restarts.
    clock.advance(BREAKER_COOLDOWN_MS);
    let probe = match registry
        .acquire_stream_within(&key, "probe", Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(lease) => lease,
        other => panic!("the probe must admit, got {other:?}"),
    };
    registry.settle(
        probe,
        StreamOutcome::RateLimited(RateLimitReason::RateLimit),
    );
    assert_eq!(registry.snapshot(&key).breaker.state, BreakerState::Open);
    clock.advance(BREAKER_COOLDOWN_MS - 1);
    assert_eq!(
        registry
            .acquire_stream_within(&key, "worker-a", Duration::from_millis(50))
            .await,
        StreamAdmission::Refused,
        "the re-opened breaker keeps its full cooldown"
    );

    // The next probe is dropped (an aborted turn releases its lease):
    // the breaker re-opens instead of wedging in half-open.
    clock.advance(1);
    let probe = match registry
        .acquire_stream_within(&key, "probe", Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(lease) => lease,
        other => panic!("the probe must admit, got {other:?}"),
    };
    registry.release(probe);
    assert_eq!(registry.snapshot(&key).breaker.state, BreakerState::Open);
}

/// M4: the breaker gates provider TRAFFIC, not spawn reservations — a
/// queued spawn still admits while the breaker is open (the child's own
/// first stream fail-fasts to failover).
#[tokio::test]
async fn an_open_breaker_does_not_gate_spawn_admissions() {
    let clock = ScriptedClock::new(1_000);
    let registry = ProviderAdmission::with_parts(
        6,
        Arc::new(JitteredStagger::new()),
        Arc::clone(&clock) as Arc<dyn ClockSource>,
    );
    let key = key();
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &key, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }
    assert_eq!(
        registry
            .acquire_stream_within(&key, "worker-a", Duration::from_millis(0))
            .await,
        StreamAdmission::Refused
    );
    assert!(
        registry
            .spawn_gate_within(&key, "spawn-1", Duration::from_millis(50))
            .await
            .is_some(),
        "a spawn reservation is the create round trip, not provider traffic"
    );
}

/// M5: every observability field is present in the per-provider rows —
/// the daemon state snapshot's compact section.
#[tokio::test]
async fn provider_rows_carry_every_observability_field() {
    let registry = ProviderAdmission::new(6);
    let glm = ProviderKey::new("zai", "glm-5.3");
    let kimi = ProviderKey::new("zai", "kimi-k2");
    let kimi_lease = admitted(&registry, &kimi, "worker-b").await;
    registry.release(kimi_lease);
    let lease = admitted(&registry, &glm, "worker-a").await;
    registry.settle(
        lease,
        StreamOutcome::RateLimited(RateLimitReason::RateLimit),
    );

    let rows = registry.provider_rows();
    assert_eq!(
        rows.iter()
            .map(|row| (row.provider.as_str(), row.model.as_str()))
            .collect::<Vec<_>>(),
        vec![("zai", "glm-5.3"), ("zai", "kimi-k2")],
        "rows are sorted per (provider, model)"
    );
    let glm_row = &rows[0];
    assert_eq!(glm_row.limit, 3);
    assert_eq!(glm_row.in_flight, 0);
    assert_eq!(glm_row.cap, 6);
    assert_eq!(glm_row.consecutive_429, 1);
    assert_eq!(glm_row.queued, 0);
    assert_eq!(glm_row.breaker.state, BreakerState::Closed);
    assert_eq!(glm_row.breaker.budget_failures, 1);
    assert_eq!(glm_row.breaker.budget_samples, 1);
    // The breaker is per PROVIDER: both models on zai share one standing.
    assert_eq!(rows[1].breaker, glm_row.breaker);
}

/// The open breaker's fail-fast stream: `admit_stream_fn` answers a
/// refusal with a rate-limit-shaped failure carrying the circuit-open
/// marker the M3 classifier reads as account-local rotation — so the
/// retry ladder settles it and the failover chain re-routes instead of
/// hammering the provider.
#[tokio::test]
async fn a_refused_acquire_fails_fast_as_a_rate_limit_for_failover() {
    use pa_agent::stream::StreamRequestOptions;
    use pa_agent::types::Model as AgentModel;

    let registry = ProviderAdmission::new(6);
    let key = key();
    // Trip the breaker.
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &key, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }

    // The refusal message itself classifies as account-local rotation.
    assert_eq!(
        outcome_for_message(&breaker_refusal_message(&key)),
        StreamOutcome::RateLimited(RateLimitReason::QuotaExhausted)
    );

    // The wrapped stream answers the refusal without calling the inner
    // stream: no provider traffic, one classified failure to fail over.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let inner: StreamFn = {
        let calls = Arc::clone(&calls);
        Arc::new(
            move |_model: AgentModel, _context: LlmContext, _options: StreamRequestOptions| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    let (_handle, consumer) = pa_agent::stream::event_stream();
                    Ok(Box::new(consumer) as Box<dyn ModelStream>)
                })
            },
        )
    };
    let key_of: KeyResolver = {
        let key = key.clone();
        Arc::new(move || key.clone())
    };
    let stream_fn = admit_stream_fn(inner, key_of, AdmissionClient::local(Arc::clone(&registry)));
    let model = AgentModel {
        id: "glm-5.3".to_string(),
        name: "glm".to_string(),
        api: "openai".to_string(),
        provider: "zai".to_string(),
        base_url: String::new(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 0,
        max_tokens: 0,
        max_tokens_explicit: false,
    };
    let mut stream = stream_fn(
        model,
        LlmContext {
            system_prompt: None,
            system_prompt_cache_prefix: None,
            messages: Vec::new(),
            tools: Vec::new(),
        },
        StreamRequestOptions {
            transport: None,
            temperature: None,
            max_tokens: None,
            reasoning: pa_agent::types::ThinkingLevel::Off,
            session_id: None,
            service_tier: None,
            api_key: None,
            signal: pa_agent::abort::AbortSignal::never(),
            on_payload: None,
            on_response: None,
            headers: None,
        },
    )
    .await
    .expect("the refused acquire answers a stream");
    let message = stream.result().await.expect("the refusal resolves");
    assert_eq!(message.stop_reason, pa_agent::types::StopReason::Error);
    assert!(
        message
            .error_message
            .as_deref()
            .is_some_and(|error| error.contains("circuit open")),
        "message: {:?}",
        message.error_message
    );
    assert_eq!(
        outcome_for_message(&message),
        StreamOutcome::RateLimited(RateLimitReason::QuotaExhausted)
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the inner stream never ran: no provider traffic while open"
    );
    // No lease was taken for the refusal: the pool is untouched.
    assert_eq!(registry.snapshot(&key).in_flight, 0);
}

/// A disabled client admits everything and mints no lease: a standalone
/// host never talks to an admission plane.
#[tokio::test]
async fn a_disabled_client_admits_without_a_plane() {
    let client = AdmissionClient::Disabled;
    assert!(!client.is_enabled());
    let key = key();
    assert_eq!(
        client.acquire_stream(&key).await,
        StreamAdmission::Unadmitted
    );
    assert!(client.spawn_gate(&key, "spawn").await.is_none());
    // The calls are no-ops, never panics.
    client
        .settle(1, StreamOutcome::RateLimited(RateLimitReason::RateLimit))
        .await;
    client.release(1).await;
}

// ===== Admission adoption telemetry (M3-M5) =====

/// A registry wired to a mock-sink telemetry client: the adoption seam's
/// battery. The client delivers only on an explicit flush, so every
/// assertion below reads a drained queue — never a sleep.
fn telemetry_registry(
    cap: usize,
    clock: Arc<dyn ClockSource>,
) -> (
    Arc<ProviderAdmission>,
    Arc<pa_telemetry::MockSink>,
    pa_telemetry::TelemetryClient,
) {
    let mock = Arc::new(pa_telemetry::MockSink::new());
    let mut config = pa_telemetry::TelemetryClientConfig::new("install-1");
    config.flush_interval = Duration::from_mins(5);
    config.sinks = vec![mock.clone() as Arc<dyn pa_telemetry::TelemetrySink>];
    let client = pa_telemetry::TelemetryClient::spawn(config).expect("spawn telemetry client");
    let registry = ProviderAdmission::with_parts(cap, Arc::new(JitteredStagger::new()), clock);
    registry.attach_telemetry(client.clone());
    (registry, mock, client)
}

/// The full storm-and-recovery ride the seam: each AIMD change is one
/// `admission_demoted`/`admission_promoted`, each breaker flip one
/// `admission_breaker_transition` with the decision-moment failure count.
#[tokio::test]
async fn breaker_and_aimd_transitions_emit_adoption_events() {
    let clock = ScriptedClock::new(1_000);
    let (registry, mock, client) =
        telemetry_registry(6, Arc::clone(&clock) as Arc<dyn ClockSource>);
    let key = key();

    // The storm: ten rate-limited settles. AIMD halves 6 -> 3 -> 1 (two
    // demotions; the floor holds after that), then the spent budget
    // opens the breaker on the tenth sample.
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &key, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }
    // Past the cooldown the first acquire is the probe; its settle
    // promotes the limit one slot and closes the breaker.
    clock.advance(BREAKER_COOLDOWN_MS);
    let probe = match registry
        .acquire_stream_within(&key, "probe", Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(lease) => lease,
        other => panic!("the probe must admit, got {other:?}"),
    };
    registry.settle(probe, StreamOutcome::Settled);
    client.flush().await.expect("flush telemetry");

    let events = mock.events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "admission_demoted",
            "admission_demoted",
            "admission_breaker_transition",
            "admission_breaker_transition",
            "admission_promoted",
            "admission_breaker_transition",
        ],
        "one event per transition, in settle order"
    );
    // The plain 429s halve: 6 -> 3 -> 1.
    for (index, (old_limit, new_limit)) in [(0usize, (6i64, 3i64)), (1, (3, 1))] {
        assert_eq!(
            events[index].properties.get("provider"),
            Some(&serde_json::json!("zai"))
        );
        assert_eq!(
            events[index].properties.get("reason"),
            Some(&serde_json::json!("rate_limited"))
        );
        assert_eq!(
            events[index].properties.get("old_limit"),
            Some(&serde_json::json!(old_limit))
        );
        assert_eq!(
            events[index].properties.get("new_limit"),
            Some(&serde_json::json!(new_limit))
        );
    }
    // closed -> open at the spent budget: ten failures of ten samples.
    assert_eq!(
        events[2].properties.get("from_state"),
        Some(&serde_json::json!("closed"))
    );
    assert_eq!(
        events[2].properties.get("to_state"),
        Some(&serde_json::json!("open"))
    );
    assert_eq!(
        events[2].properties.get("failure_count"),
        Some(&serde_json::json!(RETRY_BUDGET_MIN_SAMPLES))
    );
    // open -> half_open when the cooldown passes and the probe dispatches.
    assert_eq!(
        events[3].properties.get("from_state"),
        Some(&serde_json::json!("open"))
    );
    assert_eq!(
        events[3].properties.get("to_state"),
        Some(&serde_json::json!("half_open"))
    );
    // The settled probe promotes 1 -> 2...
    assert_eq!(
        events[4].properties.get("reason"),
        Some(&serde_json::json!("settled"))
    );
    assert_eq!(
        events[4].properties.get("old_limit"),
        Some(&serde_json::json!(1))
    );
    assert_eq!(
        events[4].properties.get("new_limit"),
        Some(&serde_json::json!(2))
    );
    // ...and closes the breaker, reporting the storm's count as decided.
    assert_eq!(
        events[5].properties.get("from_state"),
        Some(&serde_json::json!("half_open"))
    );
    assert_eq!(
        events[5].properties.get("to_state"),
        Some(&serde_json::json!("closed"))
    );
    assert_eq!(
        events[5].properties.get("failure_count"),
        Some(&serde_json::json!(RETRY_BUDGET_MIN_SAMPLES))
    );
    client.shutdown().await.expect("shutdown telemetry");
}

/// M3 rides the event: an account-local cause (a spent quota) drops the
/// limit to the floor at once and the demotion names its reason.
#[tokio::test]
async fn an_account_local_demote_names_its_reason() {
    let (registry, mock, client) =
        telemetry_registry(6, ScriptedClock::new(1_000) as Arc<dyn ClockSource>);
    let key = key();
    let lease = admitted(&registry, &key, "worker-a").await;
    registry.settle(
        lease,
        StreamOutcome::RateLimited(RateLimitReason::QuotaExhausted),
    );
    client.flush().await.expect("flush telemetry");

    let events = mock.events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.name.as_str())
            .collect::<Vec<_>>(),
        vec!["admission_demoted"],
        "one sample is no breaker verdict, one floor drop is one demotion"
    );
    assert_eq!(
        events[0].properties.get("reason"),
        Some(&serde_json::json!("account_local"))
    );
    assert_eq!(
        events[0].properties.get("old_limit"),
        Some(&serde_json::json!(6))
    );
    assert_eq!(
        events[0].properties.get("new_limit"),
        Some(&serde_json::json!(1))
    );
    client.shutdown().await.expect("shutdown telemetry");
}

/// M5 rides the seam: one served snapshot ask emits the registry's
/// counter rollup — provider pairs, in-flight and queued sums, and
/// whether any breaker stands open.
#[tokio::test]
async fn provider_rows_emit_the_snapshot_rollup() {
    let (registry, mock, client) =
        telemetry_registry(6, ScriptedClock::new(1_000) as Arc<dyn ClockSource>);
    let glm = ProviderKey::new("zai", "glm-5.3");
    let kimi = ProviderKey::new("zai", "kimi-k2");
    let stormy = ProviderKey::new("stormy", "m1");

    // One lease held on glm (in flight), a settled round trip on kimi,
    // and a spent budget on stormy: its breaker opens at the floor.
    let _held = admitted(&registry, &glm, "worker-a").await;
    let kimi_lease = admitted(&registry, &kimi, "worker-b").await;
    registry.settle(kimi_lease, StreamOutcome::Settled);
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &stormy, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }
    // The stormy limit sits at the floor and its breaker is open, so the
    // held lease is a spawn reservation (spawns never gate on the
    // breaker): one slot in flight, and a second spawn parks in the queue.
    let (_stormy_held, _) = registry
        .spawn_gate_within(&stormy, "spawn-held", Duration::from_millis(50))
        .await
        .expect("the floor's one slot admits a spawn");
    let parked = tokio::spawn({
        let registry = Arc::clone(&registry);
        let stormy = stormy.clone();
        async move {
            registry
                .spawn_gate_within(&stormy, "spawn-queued", Duration::from_secs(5))
                .await
        }
    });
    until(
        || registry.snapshot(&stormy).queued == 1,
        "one queued spawn on stormy",
    )
    .await;

    let rows = registry.provider_rows();
    client.flush().await.expect("flush telemetry");
    assert_eq!(rows.len(), 3);

    let snapshots: Vec<_> = mock
        .events()
        .into_iter()
        .filter(|event| event.name == "admission_snapshot")
        .collect();
    assert_eq!(snapshots.len(), 1, "one ask, one rollup");
    let properties = &snapshots[0].properties;
    assert_eq!(
        properties.get("providers_count"),
        Some(&serde_json::json!(3))
    );
    assert_eq!(
        properties.get("total_in_flight"),
        Some(&serde_json::json!(2)),
        "glm's held lease plus stormy's floor lease"
    );
    assert_eq!(properties.get("total_queued"), Some(&serde_json::json!(1)));
    assert_eq!(
        properties.get("any_open_breakers"),
        Some(&serde_json::json!(true)),
        "stormy's breaker stands open"
    );
    parked.abort();
    client.shutdown().await.expect("shutdown telemetry");
}

/// §3.1: a slot held by a DEAD worker must be reclaimed at admission, not waited out.
///
/// The old recovery was the worker-exit reap — correct, and useless while the worker
/// is alive holding a slot nobody uses, or while a killed worker's slot is charged to
/// a token the reap already forgot. The next acquirer then waited `ACQUIRE_BUDGET`
/// (600 s) for a lease that would never be released.
#[tokio::test]
async fn a_dead_holders_slot_is_reclaimed_instead_of_waited_out() {
    let registry = ProviderAdmission::with_parts(
        1,
        Arc::new(RecordingStagger::default()),
        Arc::new(SystemClock),
    );
    // The pool's ceiling is 1, so the second acquire can only succeed by reclaiming.
    let live: Arc<Mutex<std::collections::HashSet<String>>> = Arc::new(Mutex::new(
        ["worker-alive".to_string()].into_iter().collect(),
    ));
    {
        let live = Arc::clone(&live);
        registry.set_holder_liveness(Arc::new(move |holder: String| {
            let live = Arc::clone(&live);
            tokio::spawn(async move { live.lock().expect("live set").contains(&holder) })
        }));
    }

    let held = admitted(&registry, &key(), "worker-dead").await;
    assert_ne!(held, 0, "the dead worker's lease is charged first");

    // `worker-dead` is NOT in the live set: the next acquire must reclaim its slot
    // rather than park for the wait budget.
    let reclaimed = registry
        .acquire_stream_within(&key(), "worker-alive", Duration::from_millis(50))
        .await;
    assert!(
        matches!(reclaimed, StreamAdmission::Admitted(_)),
        "a dead holder's slot must be reclaimed immediately, got {reclaimed:?}"
    );
}

/// The mirror: a LIVE holder's slot is never taken. Reclaiming those would let a
/// second waiter exceed the provider's concurrency, which is the incident the pool
/// exists to prevent.
#[tokio::test]
async fn a_live_holders_slot_is_never_reclaimed() {
    let registry = ProviderAdmission::with_parts(
        1,
        Arc::new(RecordingStagger::default()),
        Arc::new(SystemClock),
    );
    registry.set_holder_liveness(Arc::new(|_holder: String| tokio::spawn(async { true })));

    let _held = admitted(&registry, &key(), "worker-alive").await;
    let second = registry
        .acquire_stream_within(&key(), "worker-other", Duration::from_millis(50))
        .await;
    assert!(
        matches!(second, StreamAdmission::Unadmitted),
        "a live holder keeps its slot: {second:?}"
    );
}

/// No liveness source (a test registry, the daemonless path) keeps the old
/// behaviour exactly: the wait budget decides, and nothing is reclaimed on a guess.
#[tokio::test]
async fn without_a_liveness_source_nothing_is_reclaimed() {
    let registry = ProviderAdmission::with_parts(
        1,
        Arc::new(RecordingStagger::default()),
        Arc::new(SystemClock),
    );
    let _held = admitted(&registry, &key(), "worker-a").await;
    let second = registry
        .acquire_stream_within(&key(), "worker-b", Duration::from_millis(50))
        .await;
    assert!(matches!(second, StreamAdmission::Unadmitted), "{second:?}");
}

/// Review #39: the FIFO has a ceiling, and reaching it REFUSES instead of growing.
///
/// Unbounded, an outage turned every arrival into a 600-second queue entry — memory grew
/// with the outage, and the re-open dispatched the whole backlog as a herd. The ceiling is
/// generous (256 vs the measured single digits), so this test fills it deliberately.
#[tokio::test]
async fn a_full_admission_queue_refuses_instead_of_growing() {
    // One slot, so every acquirer after the first parks in the queue.
    let registry = ProviderAdmission::new(1);
    let key = key();
    let _held = admitted(&registry, &key, "slot-holder").await;

    // Fill the queue to the ceiling with waiters that never get a grant.
    let mut parked = Vec::new();
    for index in 0..MAX_QUEUED_WAITERS {
        let registry = Arc::clone(&registry);
        let key = key.clone();
        parked.push(tokio::spawn(async move {
            registry
                .acquire_stream_within(&key, &format!("waiter-{index}"), Duration::from_secs(60))
                .await
        }));
    }
    // Wait until they are actually queued: the spawns above are asynchronous.
    until(
        || registry.snapshot(&key).queued == MAX_QUEUED_WAITERS,
        "all waiters park in the FIFO",
    )
    .await;
    assert_eq!(
        registry.snapshot(&key).queued,
        MAX_QUEUED_WAITERS,
        "the fixture must fill the queue, or the ceiling is not exercised"
    );

    // One more: refused, not queued.
    let refused = registry
        .acquire_stream_within(&key, "one-too-many", Duration::from_millis(50))
        .await;
    assert_eq!(
        refused,
        StreamAdmission::Refused,
        "a full queue must fail over, not hold another entry"
    );
    assert_eq!(
        registry.snapshot(&key).queued,
        MAX_QUEUED_WAITERS,
        "and the queue must not have grown"
    );
    for handle in parked {
        handle.abort();
    }
}

/// Review #38: the budget is chosen by ACTION, and only the queueing actions get the long one.
///
/// The first version of this test compared the two constants, which is a test of a literal —
/// it would pass just as happily if `settle` used the acquire budget at runtime. This one
/// asserts the rule that actually routes: `request_budget(action)`.
#[test]
fn only_queueing_actions_get_the_ten_minute_budget() {
    for action in [
        "settle_stream",
        "release_stream",
        "snapshot",
        "not_an_action",
    ] {
        assert_eq!(
            request_budget(action),
            MUTATION_REQUEST_BUDGET,
            "{action} never waits in a queue, so it must not inherit the acquire budget"
        );
    }
    for action in ["acquire_stream", "spawn_gate"] {
        assert_eq!(
            request_budget(action),
            ADMISSION_REQUEST_BUDGET,
            "{action} may wait in the supervisor's FIFO, so it needs the full budget"
        );
    }
    assert!(
        request_budget("settle_stream") * 10 < request_budget("acquire_stream"),
        "the gap must be an order of magnitude, or the split buys nothing"
    );
}

/// A closed breaker releases every model queue of that provider (review #6).
///
/// The breaker is provider-wide while the queues are per `(provider, model)`.
/// `settle` dispatched only the key whose lease settled, so a backlog parked on
/// ANOTHER model of the same provider stayed stranded after recovery — it was
/// refused while the breaker was open, and nothing woke it once the probe closed
/// the breaker.
#[tokio::test]
async fn closing_the_breaker_releases_other_model_queues() {
    let clock = ScriptedClock::new(1_000);
    let registry = ProviderAdmission::with_parts(
        1,
        Arc::new(JitteredStagger::new()),
        Arc::clone(&clock) as Arc<dyn ClockSource>,
    );

    // Model A: the only slot is held, so its next acquirer parks.
    let parked_key = ProviderKey::new("zai", "glm-parked");
    let holder = admitted(&registry, &parked_key, "slot-holder").await;
    let parked = {
        let registry = Arc::clone(&registry);
        let key = parked_key.clone();
        tokio::spawn(async move {
            registry
                .acquire_stream_within(&key, "pre-trip-waiter", Duration::from_secs(30))
                .await
        })
    };
    until(
        || registry.snapshot(&parked_key).queued == 1,
        "the waiter parks on model A",
    )
    .await;

    // Model B (same provider) trips the provider-wide breaker.
    let tripping_key = ProviderKey::new("zai", "glm-tripping");
    for _ in 0..RETRY_BUDGET_MIN_SAMPLES {
        let lease = admitted(&registry, &tripping_key, "storm-worker").await;
        registry.settle(
            lease,
            StreamOutcome::RateLimited(RateLimitReason::RateLimit),
        );
    }
    assert_eq!(
        registry.snapshot(&tripping_key).breaker.state,
        BreakerState::Open,
        "the fixture must open the provider breaker"
    );

    // Free model A's slot while the breaker is open: the backlog stays parked,
    // which is what makes the recovery below the thing under test.
    registry.release(holder);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        registry.snapshot(&parked_key).queued,
        1,
        "an open breaker parks the backlog"
    );

    // Cooldown, then a probe on model B; a clean settle closes the breaker.
    clock.advance(BREAKER_COOLDOWN_MS);
    let probe = match registry
        .acquire_stream_within(&tripping_key, "probe", Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(lease) => lease,
        other => panic!("the probe must admit, got {other:?}"),
    };
    assert_eq!(
        registry.snapshot(&tripping_key).breaker.state,
        BreakerState::HalfOpen
    );
    registry.settle(probe, StreamOutcome::Settled);
    assert_eq!(
        registry.snapshot(&tripping_key).breaker.state,
        BreakerState::Closed,
        "a clean probe closes the breaker"
    );

    // The parked waiter on model A must now be admitted. Before the fix it stayed
    // queued forever: nothing dispatched a key other than the probe's own.
    let admitted_after_recovery = tokio::time::timeout(Duration::from_secs(2), parked)
        .await
        .expect("the other model's queue is released once the breaker closes")
        .expect("the parked task joins");
    assert!(
        matches!(admitted_after_recovery, StreamAdmission::Admitted(_)),
        "the parked waiter must be admitted, got {admitted_after_recovery:?}"
    );
}

fn agent_model() -> pa_agent::types::Model {
    pa_agent::types::Model {
        id: "glm-5.3".to_string(),
        name: "glm".to_string(),
        api: "openai".to_string(),
        provider: "zai".to_string(),
        base_url: String::new(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 0,
        max_tokens: 0,
        max_tokens_explicit: false,
    }
}

fn llm_context() -> LlmContext {
    LlmContext {
        system_prompt: None,
        system_prompt_cache_prefix: None,
        messages: Vec::new(),
        tools: Vec::new(),
    }
}

fn stream_options() -> pa_agent::stream::StreamRequestOptions {
    pa_agent::stream::StreamRequestOptions {
        transport: None,
        temperature: None,
        max_tokens: None,
        reasoning: pa_agent::types::ThinkingLevel::Off,
        session_id: None,
        service_tier: None,
        api_key: None,
        signal: pa_agent::abort::AbortSignal::never(),
        on_payload: None,
        on_response: None,
        headers: None,
    }
}

/// A stream dropped without settling still returns its slot (review #7).
///
/// `StreamLease::settle`/`release` used to clear the lease id BEFORE awaiting, so
/// `Drop` saw zero and skipped its detached release. A cancelled future — the
/// abort path, an aborted turn, a dropped stream — then left the slot held on a
/// LIVE worker, which the supervisor's holder reaping never reclaims because the
/// holder is alive. Each cancellation shrank the provider's capacity by one.
#[tokio::test]
async fn a_stream_dropped_before_it_ends_returns_its_slot() {
    use pa_agent::stream::StreamRequestOptions;
    use pa_agent::types::Model as AgentModel;

    let registry = ProviderAdmission::new(1);
    let key = key();
    let inner: StreamFn = Arc::new(
        move |_model: AgentModel, _context: LlmContext, _options: StreamRequestOptions| {
            Box::pin(async move {
                let (_handle, consumer) = pa_agent::stream::event_stream();
                Ok(Box::new(consumer) as Box<dyn ModelStream>)
            })
        },
    );
    let key_of: KeyResolver = {
        let key = key.clone();
        Arc::new(move || key.clone())
    };
    let client = AdmissionClient::local(Arc::clone(&registry));
    let stream_fn = admit_stream_fn(inner, key_of, client);

    let stream = stream_fn(agent_model(), llm_context(), stream_options())
        .await
        .expect("the wrapped stream opens");
    assert_eq!(
        registry.snapshot(&key).in_flight,
        1,
        "the stream holds the only slot"
    );

    // The abort shape: the stream is dropped while it is still open.
    drop(stream);

    until(
        || registry.snapshot(&key).in_flight == 0,
        "a stream dropped before it ends returns its slot",
    )
    .await;
}

/// A settled stream does not release a second time (review #7).
///
/// The `resolved` flag, not a zeroed id, keeps `Drop` quiet: a double release
/// decrements a counter another lease owns, and at the floor it frees a slot
/// nobody holds.
#[tokio::test]
async fn a_settled_stream_does_not_release_the_slot_twice() {
    use pa_agent::stream::StreamRequestOptions;
    use pa_agent::types::Model as AgentModel;

    let registry = ProviderAdmission::new(1);
    let key = key();
    let inner: StreamFn = Arc::new(
        move |_model: AgentModel, _context: LlmContext, _options: StreamRequestOptions| {
            Box::pin(async move {
                let (_handle, consumer) = pa_agent::stream::event_stream();
                Ok(Box::new(consumer) as Box<dyn ModelStream>)
            })
        },
    );
    let key_of: KeyResolver = {
        let key = key.clone();
        Arc::new(move || key.clone())
    };
    let client = AdmissionClient::local(Arc::clone(&registry));
    let stream_fn = admit_stream_fn(inner, key_of, client);

    let first = stream_fn(agent_model(), llm_context(), stream_options())
        .await
        .expect("the first stream opens");
    // A second acquirer parks on the FIFO behind the single slot.
    let queued = {
        let registry = Arc::clone(&registry);
        let key = key.clone();
        tokio::spawn(async move {
            registry
                .acquire_stream_within(&key, "queued-worker", Duration::from_secs(30))
                .await
        })
    };
    until(
        || registry.snapshot(&key).queued == 1,
        "the second acquirer parks",
    )
    .await;

    // Dropping the live stream is what a settled stream's Drop looks like once
    // `resolved` is set: exactly one release must reach the pool, so the queued
    // acquirer takes it and the counter lands on 1, not 0.
    drop(first);
    let admitted = tokio::time::timeout(Duration::from_secs(2), queued)
        .await
        .expect("the queued acquirer is served")
        .expect("the task joins");
    assert!(
        matches!(admitted, StreamAdmission::Admitted(_)),
        "the queued acquirer must get the slot, got {admitted:?}"
    );
    assert_eq!(
        registry.snapshot(&key).in_flight,
        1,
        "exactly one release: the queued lease holds the slot"
    );
}

/// The lease keeps its id until the settle RESOLVES (review #7).
///
/// The defect was ordering, not arithmetic: `settle` cleared the id and then
/// awaited, so any cancellation of that await left `Drop` with nothing to release
/// and the slot held on a live worker. Cancelling a `Local` settle is not
/// reachable — that path is synchronous — so the invariant is pinned directly:
/// while the settle is in flight the lease still owns its id, and only a resolved
/// settle sets `resolved`.
#[tokio::test]
async fn a_lease_keeps_its_id_until_the_settle_resolves() {
    let registry = ProviderAdmission::new(1);
    let key = key();
    let client = AdmissionClient::local(Arc::clone(&registry));
    let id = match registry
        .acquire_stream_within(&key, "worker-a", Duration::from_millis(50))
        .await
    {
        StreamAdmission::Admitted(id) => id,
        other => panic!("the acquire must admit, got {other:?}"),
    };
    let lease = StreamLease {
        id,
        client,
        resolved: false,
    };
    assert_eq!(
        lease.id(),
        id,
        "a lease owns its id from construction until its settle resolves"
    );
    assert_ne!(id, 0, "a real lease id is never zero");
}

/// A settle against a link that cannot answer must NOT report success (#7).
///
/// This is the flag's contract: `settle`/`release` answer whether the supervisor
/// confirmed the mutation. A dead link (worker lost the supervisor, supervisor
/// restarting) answers `false`, and only then does `StreamLease` keep the lease
/// unresolved so `Drop` gives the slot back. Reporting `true` here is what turned
/// a failed round trip into a permanently lost slot on a live worker: holder
/// reaping runs on worker EXIT, which a live worker never reaches.
#[tokio::test]
async fn a_settle_against_a_dead_link_reports_no_confirmation() {
    let dead = Arc::new(crate::supervisor_link::SupervisorLink::new(
        std::path::PathBuf::from("/nonexistent-provider-admission-test.sock"),
    ));
    let client = AdmissionClient::link(dead, "worker-a".to_string());
    assert!(
        !client.settle(7, StreamOutcome::Settled).await,
        "a link that never answers cannot confirm a settle"
    );
    assert!(
        !client.release(7).await,
        "a link that never answers cannot confirm a release"
    );
}
