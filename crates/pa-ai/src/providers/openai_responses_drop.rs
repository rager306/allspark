//! The Responses terminal-event pins: an OpenAI/Azure Responses body that closes before
//! `response.completed`/`response.incomplete` was cut off, and its text or tool call is partial.
//! The processor used to reject that only for xAI (review #4), so every other provider settled a
//! truncated answer as a completed turn and the retry ladder never ran.

use super::*;
use crate::event_stream::AssistantMessageEventExt;
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

/// A partial text delta: content arrived, no terminal response event.
const PARTIAL_TEXT: &str = "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"The first half\"}\n\n";

/// The terminal event a healthy Responses stream always carries.
const COMPLETED: &str = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n";

async fn openai_events(body: String) -> (Vec<&'static str>, AssistantMessageEvent) {
    let addr = serve_sse(body).await;
    let model: Model = serde_json::from_value(json!({
        "id": "gpt-test", "name": "GPT test", "api": "openai-responses",
        "provider": "openai", "baseUrl": format!("http://{addr}"),
        "reasoning": false, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 400_000, "maxTokens": 8192,
    }))
    .expect("test model");
    let options = OpenAIResponsesOptions::from_base(StreamOptions {
        api_key: Some("test".into()),
        ..Default::default()
    });
    let mut reader = stream_openai_responses(&model, &Context::default(), Some(&options));
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

/// A non-xAI Responses stream that closes before its terminal event is a truncated answer, not a
/// completed turn: it must terminate with an error so the retry ladder can re-issue the request.
#[tokio::test]
async fn an_openai_responses_stream_ended_before_its_terminal_event_is_a_failure() {
    let (event_types, terminal) = openai_events(PARTIAL_TEXT.to_string()).await;
    let AssistantMessageEvent::Error { error, .. } = terminal else {
        panic!("a truncated Responses stream must terminate with an error, got {event_types:?}");
    };
    assert_eq!(error.stop_reason, StopReason::Error);
    assert!(
        error
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("terminal response event"),
        "{:?}",
        error.error_message
    );
}

/// The healthy shape still completes: a stream that carries `response.completed` is a turn.
#[tokio::test]
async fn an_openai_responses_stream_with_its_terminal_event_completes() {
    let (event_types, terminal) = openai_events(format!("{PARTIAL_TEXT}{COMPLETED}")).await;
    let AssistantMessageEvent::Done { message, .. } = terminal else {
        panic!("a finished Responses stream must complete, got {event_types:?}");
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
}
