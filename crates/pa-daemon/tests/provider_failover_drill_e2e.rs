//! FORK-ONLY provider-failover drill: fault shapes replayed against the real
//! provider transport and the real retry/failover driver.
//!
//! Every drill scripts a fault on a [`pa_ai::testing::MockInferenceServer`]
//! (a wire-level HTTP/1.1 + SSE server), runs a genuine
//! `openai-completions` stream through the shared transport, and then feeds
//! the resulting assistant message to the production retry chain
//! ([`pa_core::session_engine::provider_failover::run_turn_with_provider_failover`]).
//! The point is the seam the in-process `faux` provider cannot reach: real
//! HTTP status handling, `Retry-After` parsing, SSE decode, and the mid-stream
//! drop classification, each landing on the shared retry policy.
//!
//! Four drills:
//!   a. `429` + `Retry-After: 5` -> the policy waits ~5s and retries, then succeeds.
//!   b. `503` with no `Retry-After` -> the exponential backoff fires (no server wait).
//!   c. SSE truncation after 3 frames -> the retryable `stream_drop`; the retry
//!      re-issues and the first three chunks stay in the recovered message.
//!   d. Every candidate fails -> the last provider error reaches the caller.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pa_agent::types::{AssistantContent, AssistantMessage, StopReason};
use pa_ai::testing::MockInferenceServer;
use pa_ai::types::{Context, Model, StreamOptions};
use pa_core::session_engine::auto_retry::{AutoRetryEvent, RetryStartReason};
use pa_core::session_engine::provider_failover::{
    run_turn_with_provider_failover, ProviderFailoverPolicy,
};
use pa_core::session_engine::provider_retry::{ProviderRetryPolicy, UNBOUNDED_BACKOFF_MS};
use serde_json::json;

/// A model bound to a mock server's OpenAI-compatible root.
fn model_for(server: &MockInferenceServer) -> Model {
    serde_json::from_value(json!({
        "id": "mock-1",
        "name": "Mock 1",
        "api": "openai-completions",
        "provider": "prime-inference",
        "baseUrl": server.api_base_url(),
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 128_000,
        "maxTokens": 4096,
    }))
    .expect("mock model")
}

fn empty_context() -> Context {
    Context {
        system_prompt: None,
        messages: vec![],
        tools: None,
    }
}

/// One real provider attempt: a full wire round-trip through the shared
/// transport and SSE decoder, returning the terminal assistant message (its
/// `stop_reason` is `Error` on a classified failure). The pa-ai message
/// crosses the crate boundary by wire-shape round-trip, exactly as the
/// production provider adapter does.
async fn attempt(model: &Model) -> AssistantMessage {
    let options = StreamOptions {
        api_key: Some("sk-mock".to_string()),
        ..Default::default()
    };
    let stream = pa_ai::stream(model, &empty_context(), Some(options)).expect("stream starts");
    let result = stream.result().await;
    pa_core::session_engine::provider_adapter::json_round_trip(&result)
        .expect("assistant wire shapes match")
}

/// The single `data:` frame an OpenAI-compatible completion carries.
fn text_chunk(text: &str, finish: Option<&str>) -> String {
    json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "created": 1_750_000_000,
        "model": "mock-1",
        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": finish}],
    })
    .to_string()
}

/// The observed waits, retry events, and per-attempt terminal messages of one
/// drill.
#[derive(Default, Clone)]
struct Observed {
    waits_ms: Vec<u64>,
    events: Vec<AutoRetryEvent>,
    /// The terminal assistant message of every provider attempt, in order.
    attempts: Vec<AssistantMessage>,
}

impl Observed {
    fn quick_retries(&self) -> Vec<u64> {
        self.events
            .iter()
            .filter_map(|event| match event {
                AutoRetryEvent::Start {
                    reason: RetryStartReason::Quick,
                    delay_ms,
                    ..
                } => Some(*delay_ms),
                _ => None,
            })
            .collect()
    }

    fn backup_retries(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event {
                AutoRetryEvent::Start {
                    reason: RetryStartReason::Backup { backup_model },
                    ..
                } => Some(backup_model.clone()),
                _ => None,
            })
            .collect()
    }

    fn end(&self) -> Option<&AutoRetryEvent> {
        self.events
            .iter()
            .rev()
            .find(|event| matches!(event, AutoRetryEvent::End { .. }))
    }
}

/// The `details` of a failed attempt's `provider_stream_failure` diagnostic.
fn failure_details(message: &AssistantMessage) -> serde_json::Value {
    message
        .diagnostics
        .as_ref()
        .and_then(|diagnostics| {
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.kind == "provider_stream_failure")
        })
        .and_then(|diagnostic| diagnostic.details.clone())
        .unwrap_or(serde_json::Value::Null)
}

fn quick_policy() -> ProviderRetryPolicy {
    ProviderRetryPolicy {
        enabled: true,
        max_retries: 3,
        base_delay_ms: 2_000,
        max_retry_delay_ms: 60_000,
        max_delay_ms: UNBOUNDED_BACKOFF_MS,
    }
}

fn failover_policy() -> ProviderFailoverPolicy {
    ProviderFailoverPolicy {
        enabled: true,
        max_retries: 1,
        base_delay_ms: 1_000,
        max_delay_ms: 30_000,
    }
}

/// Run `attempt` through the production retry chain against one provider, with
/// no failover candidates: the pure quick-retry ladder. Waits are recorded, not
/// slept, so the drill asserts the policy's decisions instantly.
async fn run_quick(model: &Model, policy: &ProviderRetryPolicy) -> (AssistantMessage, Observed) {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let message = run_turn_with_provider_failover(
        policy,
        &ProviderFailoverPolicy {
            enabled: false,
            ..failover_policy()
        },
        &[],
        128_000,
        None,
        {
            let observed = Arc::clone(&observed);
            move || {
                let observed = Arc::clone(&observed);
                async move {
                    let message = attempt(model).await;
                    observed.lock().unwrap().attempts.push(message.clone());
                    Ok(message)
                }
            }
        },
        {
            let observed = Arc::clone(&observed);
            move |event| {
                observed.lock().unwrap().events.push(event);
                async { Ok(()) }
            }
        },
        {
            let observed = Arc::clone(&observed);
            move |delay| {
                observed
                    .lock()
                    .unwrap()
                    .waits_ms
                    .push(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX));
                async { true }
            }
        },
        |_next: &Model| async { Ok(()) },
        || async { Ok(None) },
        None,
    )
    .await
    .expect("retry driver settles");
    let observed = observed.lock().unwrap();
    (
        message,
        Observed {
            waits_ms: observed.waits_ms.clone(),
            events: observed.events.clone(),
            attempts: observed.attempts.clone(),
        },
    )
}

#[tokio::test]
async fn drill_a_rate_limit_retry_after_is_honored_then_succeeds() {
    let server = MockInferenceServer::new();
    // Turn 1: 429 carrying `Retry-After: 5` seconds.
    server.queue_response(
        429,
        &[("content-type", "application/json"), ("retry-after", "5")],
        json!({"error": {"message": "Too many concurrent requests", "type": "rate_limit_error"}})
            .to_string(),
    );
    // Turn 2: a healthy completion.
    server.queue_sse(vec![
        &text_chunk("recovered", None),
        &text_chunk("", Some("stop")),
    ]);
    let model = model_for(&server);

    let (message, observed) = run_quick(&model, &quick_policy()).await;

    // The provider's own wait (5000ms) was honored, not the 2s exponential base.
    assert_eq!(observed.waits_ms.len(), 1, "exactly one retry");
    assert!(
        (4_000..=6_000).contains(&observed.waits_ms[0]),
        "Retry-After wait {} outside the 5s band (±20% jitter)",
        observed.waits_ms[0]
    );
    assert_eq!(observed.quick_retries().len(), 1);
    // The FAILED first attempt classified as a rate limit with the parsed
    // wait (the terminal message is the successful retry, so read the first).
    let failed = &observed.attempts[0];
    assert_eq!(failed.stop_reason, StopReason::Error);
    let details = failure_details(failed);
    assert_eq!(details["kind"].as_str(), Some("rate_limit"));
    assert_eq!(details["retryAfterMs"].as_u64(), Some(5_000));
    assert_eq!(details["status"].as_u64(), Some(429));

    // The retry succeeded against the second scripted turn.
    assert_ne!(message.stop_reason, StopReason::Error);
    assert_eq!(server.request_count(), 2);
    let end = observed.end().expect("retry end");
    assert!(matches!(
        end,
        AutoRetryEvent::End {
            success: true,
            attempt: 1,
            ..
        }
    ));
}

#[tokio::test]
async fn drill_b_service_unavailable_retries_immediately_on_the_ladder() {
    let server = MockInferenceServer::new();
    server.queue_response(
        503,
        &[("content-type", "application/json")],
        json!({"error": {"message": "overloaded", "type": "server_error"}}).to_string(),
    );
    server.queue_sse(vec![
        &text_chunk("back up", None),
        &text_chunk("", Some("stop")),
    ]);
    let model = model_for(&server);

    let (message, observed) = run_quick(&model, &quick_policy()).await;

    // No Retry-After: the wait is the policy's exponential base, not a server
    // request. 2000ms ±20% jitter.
    assert_eq!(observed.waits_ms.len(), 1);
    assert!(
        (1_600..=2_400).contains(&observed.waits_ms[0]),
        "exponential wait {} outside the 2s base band",
        observed.waits_ms[0]
    );
    assert_ne!(message.stop_reason, StopReason::Error);
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn drill_c_stream_drop_retries_and_keeps_the_partial_chunks() {
    let server = MockInferenceServer::new();
    // Turn 1: three frames arrive, then the body ends with no terminal marker.
    server.queue_sse_truncated(
        vec![
            &text_chunk("part-1 ", None),
            &text_chunk("part-2 ", None),
            &text_chunk("part-3", None),
            &text_chunk("never-sent", None),
        ],
        3,
    );
    // Turn 2: a healthy completion.
    server.queue_sse(vec![
        &text_chunk("complete", None),
        &text_chunk("", Some("stop")),
    ]);
    let model = model_for(&server);

    let (message, observed) = run_quick(&model, &quick_policy()).await;

    // The dropped stream classified as the retryable `stream_drop`, carrying
    // the three chunks that did arrive.
    assert_eq!(observed.quick_retries().len(), 1, "one quick retry fired");
    let end = observed.end().expect("retry end");
    assert!(matches!(
        end,
        AutoRetryEvent::End {
            success: true,
            attempt: 1,
            ..
        }
    ));

    assert_ne!(message.stop_reason, StopReason::Error);
    let text: String = message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "complete");
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn drill_c_dropped_attempt_carries_the_three_chunks_in_its_context() {
    // The failure path alone: no retry budget, so the dropped message settles
    // as the terminal error and its partial content is the three chunks.
    let server = MockInferenceServer::new();
    server.queue_sse_truncated(
        vec![
            &text_chunk("part-1 ", None),
            &text_chunk("part-2 ", None),
            &text_chunk("part-3", None),
            &text_chunk("never-sent", None),
        ],
        3,
    );
    let model = model_for(&server);

    let policy = ProviderRetryPolicy {
        enabled: true,
        max_retries: 0,
        ..quick_policy()
    };
    let (message, observed) = run_quick(&model, &policy).await;

    assert_eq!(message.stop_reason, StopReason::Error);
    assert!(observed.waits_ms.is_empty(), "no retry budget spent");
    let details = message
        .diagnostics
        .as_ref()
        .and_then(|diagnostics| diagnostics.first())
        .and_then(|diagnostic| diagnostic.details.clone());
    assert_eq!(
        details.as_ref().and_then(|d| d["kind"].as_str()),
        Some("stream_drop")
    );
    let text: String = message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "part-1 part-2 part-3");
    // The end event discloses the failure.
    assert!(matches!(
        observed.end(),
        Some(AutoRetryEvent::End { success: false, .. })
    ));
}

#[tokio::test]
async fn drill_d_all_candidates_failing_surfaces_the_last_error() {
    let primary = MockInferenceServer::new();
    // Each provider gets two requests (initial + one quick retry), so script
    // the failure twice per server.
    for _ in 0..2 {
        primary.queue_response(
            503,
            &[("content-type", "application/json")],
            json!({"error": {"message": "primary down", "type": "server_error"}}).to_string(),
        );
    }
    let backup = MockInferenceServer::new();
    for _ in 0..2 {
        backup.queue_response(
            500,
            &[("content-type", "application/json")],
            json!({"error": {"message": "backup down", "type": "server_error"}}).to_string(),
        );
    }
    let primary_model = model_for(&primary);
    let backup_model = Model {
        provider: "prime-backup".to_string(),
        base_url: backup.api_base_url(),
        ..model_for(&backup)
    };

    let observed = Arc::new(Mutex::new(Observed::default()));
    let attempts = Arc::new(AtomicUsize::new(0));
    let message = run_turn_with_provider_failover(
        &quick_policy(),
        &failover_policy(),
        std::slice::from_ref(&backup_model),
        128_000,
        None,
        {
            let attempts = Arc::clone(&attempts);
            let primary_model = primary_model.clone();
            let backup_model = backup_model.clone();
            move || {
                // First two attempts on the primary (initial + one quick retry),
                // then the switch serves the backup for the remaining budget.
                let index = attempts.fetch_add(1, Ordering::SeqCst);
                let model = if index < 2 {
                    primary_model.clone()
                } else {
                    backup_model.clone()
                };
                async move { Ok(attempt(&model).await) }
            }
        },
        {
            let observed = Arc::clone(&observed);
            move |event| {
                observed.lock().unwrap().events.push(event);
                async { Ok(()) }
            }
        },
        {
            let observed = Arc::clone(&observed);
            move |delay| {
                observed
                    .lock()
                    .unwrap()
                    .waits_ms
                    .push(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX));
                async { true }
            }
        },
        |_next: &Model| async { Ok(()) },
        || async { Ok(None) },
        None,
    )
    .await
    .expect("failover driver settles");

    // Every provider failed: the terminal message is the LAST error (the
    // backup's), and the failover chain walked both providers.
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(message.provider, "prime-backup");
    assert!(
        message
            .error_message
            .as_deref()
            .is_some_and(|error| error.contains("backup down")),
        "final error: {:?}",
        message.error_message
    );
    let observed = observed.lock().unwrap();
    assert_eq!(observed.backup_retries().len(), 1, "one re-route fired");
    assert_eq!(observed.backup_retries()[0], "prime-backup/mock-1");
    assert!(matches!(
        observed.end(),
        Some(AutoRetryEvent::End { success: false, final_error: Some(error), .. })
            if error.contains("backup down")
    ));
    assert_eq!(primary.request_count(), 2);
    assert_eq!(backup.request_count(), 2);
}

#[tokio::test]
async fn drill_a_wait_exceeding_the_cap_gives_up() {
    // A server-requested wait above the policy cap settles instead of
    // sleeping: the same `Retry-After` shape with a value the cap rejects.
    let server = MockInferenceServer::new();
    server.queue_response(
        429,
        &[("content-type", "application/json"), ("retry-after", "120")],
        json!({"error": {"message": "slow down", "type": "rate_limit_error"}}).to_string(),
    );
    let model = model_for(&server);
    let policy = ProviderRetryPolicy {
        enabled: true,
        max_retries: 3,
        base_delay_ms: 2_000,
        max_retry_delay_ms: 60_000,
        max_delay_ms: UNBOUNDED_BACKOFF_MS,
    };

    let (message, observed) = run_quick(&model, &policy).await;

    assert_eq!(message.stop_reason, StopReason::Error);
    assert!(observed.waits_ms.is_empty(), "no wait slept past the cap");
    assert_eq!(server.request_count(), 1, "no retry issued");
    let end = observed.end().expect("give-up end");
    assert!(matches!(
        end,
        AutoRetryEvent::End { success: false, final_error: Some(error), .. }
            if error.contains("above retry.provider.maxRetryDelayMs")
    ));
}

#[tokio::test]
async fn transport_death_waits_only_for_the_configured_timeout() {
    // A connection that accepts and then closes without a response surfaces a
    // connection failure, which the policy treats as retryable; the drill pins
    // that the classification reaches the retry decision (the server has no
    // second scripted turn, so the retry's queue exhaustion answers 500).
    let server = MockInferenceServer::new();
    // Drop the connection without writing anything by queuing nothing and
    // closing: an exhausted queue answers 500, so this drill pins the
    // unscripted-request guard instead.
    let model = model_for(&server);
    let policy = ProviderRetryPolicy {
        enabled: true,
        max_retries: 1,
        base_delay_ms: 10,
        max_retry_delay_ms: 60_000,
        max_delay_ms: UNBOUNDED_BACKOFF_MS,
    };
    let (message, observed) = run_quick(&model, &policy).await;
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(
        message
            .diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.first())
            .and_then(|diagnostic| diagnostic.details.as_ref())
            .and_then(|details| details["kind"].as_str()),
        Some("server_error")
    );
    assert_eq!(observed.waits_ms.len(), 1, "the 500 was retried once");
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn waits_are_recorded_never_slept() {
    // Guard: the drill's `wait` callback must not sleep, or the suite would
    // take minutes. A five-second server wait settles instantly here.
    let server = MockInferenceServer::new();
    server.queue_response(
        429,
        &[("content-type", "application/json"), ("retry-after", "5")],
        json!({"error": {"message": "wait", "type": "rate_limit_error"}}).to_string(),
    );
    server.queue_sse(vec![&text_chunk("ok", None), &text_chunk("", Some("stop"))]);
    let model = model_for(&server);
    let started = std::time::Instant::now();
    let (message, observed) = run_quick(&model, &quick_policy()).await;
    assert_ne!(message.stop_reason, StopReason::Error);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(observed.waits_ms[0] >= 4_000);
}
