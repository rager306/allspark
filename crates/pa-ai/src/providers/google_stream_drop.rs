//! The Google terminal-marker pins: a Gemini or Vertex body that closes without the API's
//! terminal marker (a candidate `finishReason`, or a prompt block) was cut off mid-flight and
//! must not settle as a completed turn (review #3).
//!
//! The defect this file pins: `output.stop_reason` starts at `Stop` and both provider loops
//! finalized on EOF, so a stream that ended after one partial text chunk produced a clean
//! `done` event with half an answer — and the retry ladder never saw a failure.

use super::*;
use crate::event_stream::AssistantMessageEventExt;
use crate::providers::google_vertex::stream_google_vertex;
use crate::providers::google_vertex::GoogleVertexOptions;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serve one SSE response body for the provider's POST and return the bound address.
async fn serve_sse(body: String) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0u8; 8192];
        let _ = socket.read(&mut request).await.unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    addr
}

/// The partial-text chunk every fixture starts with: content, no `finishReason`.
const PARTIAL_TEXT: &str = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"The first half of the answer\"}]},\"finishReason\":null}]}\n\n";

/// The terminal chunk: the API says the response is over.
const FINISH_STOP: &str = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\"}]}\n\n";

fn test_model(addr: SocketAddr, provider: &str, api: &str) -> Model {
    serde_json::from_value(json!({
        "id": "gemini-test", "name": "Gemini test", "api": api,
        "provider": provider, "baseUrl": format!("http://{addr}"),
        "reasoning": false, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 1_048_576, "maxTokens": 8192,
    }))
    .expect("test model")
}

/// Drain a stream to its terminal event, returning the event types seen before it.
async fn terminal_event(
    mut reader: AssistantMessageEventStream,
) -> (Vec<&'static str>, AssistantMessageEvent) {
    let mut event_types = Vec::new();
    loop {
        let Some(event) = reader.next_event().await else {
            panic!("stream ended without a terminal event");
        };
        if event.is_terminal() {
            return (event_types, event);
        }
        event_types.push(event.event_type());
    }
}

async fn gemini_events(body: String) -> (Vec<&'static str>, AssistantMessageEvent) {
    let addr = serve_sse(body).await;
    let model = test_model(addr, "google", "google-generative-ai");
    let options = GoogleOptions::from_base(StreamOptions {
        api_key: Some("test".into()),
        ..Default::default()
    });
    terminal_event(stream_google(&model, &Context::default(), Some(&options))).await
}

async fn vertex_events(body: String) -> (Vec<&'static str>, AssistantMessageEvent) {
    let addr = serve_sse(body).await;
    let model = test_model(addr, "google-vertex", "google-vertex");
    let options = GoogleVertexOptions::from_base(StreamOptions {
        api_key: Some("test".into()),
        ..Default::default()
    });
    terminal_event(stream_google_vertex(
        &model,
        &Context::default(),
        Some(&options),
    ))
    .await
}

/// The trailing `stream_drop` classification of a terminal error's message.
fn is_stream_drop(error: &AssistantMessage) -> bool {
    error
        .error_message
        .as_deref()
        .is_some_and(|message| message.contains("stream_drop"))
}

/// A Gemini body that closes on a partial text chunk carries no terminal marker: the provider
/// must report the retryable drop, not a completed turn.
#[tokio::test]
async fn a_gemini_stream_ended_before_the_finish_reason_is_a_drop() {
    let (event_types, terminal) = gemini_events(PARTIAL_TEXT.to_string()).await;
    let AssistantMessageEvent::Error { error, .. } = terminal else {
        panic!("a truncated Gemini stream must terminate with an error, got {event_types:?}");
    };
    assert_eq!(error.stop_reason, StopReason::Error);
    assert!(
        is_stream_drop(&error),
        "the failure must be the retryable stream_drop class: {:?}",
        error.error_message
    );
    assert!(
        error
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("text block"),
        "the disclosure names the block the drop landed in: {:?}",
        error.error_message
    );
}

/// Vertex finalizes through the same shared state and needs the same rule.
#[tokio::test]
async fn a_vertex_stream_ended_before_the_finish_reason_is_a_drop() {
    let (event_types, terminal) = vertex_events(PARTIAL_TEXT.to_string()).await;
    let AssistantMessageEvent::Error { error, .. } = terminal else {
        panic!("a truncated Vertex stream must terminate with an error, got {event_types:?}");
    };
    assert!(is_stream_drop(&error), "{:?}", error.error_message);
}

/// The healthy shape: the same text followed by `finishReason: STOP` completes.
#[tokio::test]
async fn a_gemini_stream_with_its_finish_reason_completes() {
    let (event_types, terminal) = gemini_events(format!("{PARTIAL_TEXT}{FINISH_STOP}")).await;
    let AssistantMessageEvent::Done { message, .. } = terminal else {
        panic!("a finished Gemini stream must complete, got {event_types:?}");
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
}

/// A blocked prompt answers with `promptFeedback.blockReason` and no candidate at all: the API
/// did say its piece, so this is a terminal stream and must NOT be retried as a drop.
#[tokio::test]
async fn a_blocked_prompt_is_terminal_not_a_drop() {
    let (event_types, terminal) =
        gemini_events("data: {\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}\n\n".to_string())
            .await;
    let AssistantMessageEvent::Done { message, .. } = terminal else {
        panic!("a blocked prompt is a finished response, got {event_types:?}");
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
}
