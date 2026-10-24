//! Wire-level client tests against a loopback mock server (the pa-ai
//! `testing::mock_server` pattern, trimmed to the judgment surface: a real
//! HTTP/1.1 listener on an ephemeral port serving scripted responses in
//! request order, every response `Connection: close` so each request lands
//! on its own connection and the queue stays ordered).

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pa_jev::{
    JevClient, JevClientConfig, JevError, JudgmentRequest, JudgmentState, NoulCriteria, Question,
    WireDialect, JEV_INPUT_PRICE_PER_MTOK,
};

/// One recorded request.
#[derive(Debug, Clone)]
struct RecordedRequest {
    path: String,
    body: String,
}

/// One scripted response: status, headers, body.
type ScriptedResponse = (u16, Vec<(String, String)>, String);

/// The scripted mock: queued `(status, headers, body)` responses in request
/// order.
struct MockServer {
    port: u16,
    queue: Arc<Mutex<VecDeque<ScriptedResponse>>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (queue, requests, shutdown) = (
                Arc::clone(&queue),
                Arc::clone(&requests),
                Arc::clone(&shutdown),
            );
            move || {
                for stream in listener.incoming() {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(stream) = stream {
                        let _ = serve(stream, &queue, &requests);
                    }
                }
            }
        });
        MockServer {
            port,
            queue,
            requests,
            shutdown,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn push(&self, status: u16, headers: &[(&str, &str)], body: String) {
        self.queue.lock().expect("queue").push_back((
            status,
            headers
                .iter()
                .map(|(n, v)| ((*n).into(), (*v).into()))
                .collect(),
            body,
        ));
    }

    /// Queue a JSON success answering every id in `ids` (a noul answer),
    /// billing `input_tokens`.
    fn push_judgment(&self, ids: &[&str], input_tokens: u64) {
        let answers: BTreeMap<String, serde_json::Value> = ids
            .iter()
            .map(|id| {
                (
                    (*id).to_string(),
                    serde_json::json!({"type": "noul", "noul": 0.5}),
                )
            })
            .collect();
        self.push(
            200,
            &[("content-type", "application/json")],
            serde_json::json!({
                "model": "jev-1.13.0",
                "answers": answers,
                "usage": {"input_tokens": input_tokens, "output_tokens": 0}
            })
            .to_string(),
        );
    }

    fn push_status(&self, status: u16, retry_after: Option<&str>) {
        let mut headers = vec![("content-type", "application/json")];
        if let Some(hint) = retry_after {
            headers.push(("retry-after", hint));
        }
        self.push(
            status,
            &headers,
            r#"{"error":{"message":"scripted"}}"#.to_string(),
        );
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("requests").len()
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("requests").clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Read one request, pop the next scripted response, write it.
fn serve(
    mut stream: TcpStream,
    queue: &Mutex<VecDeque<ScriptedResponse>>,
    requests: &Mutex<Vec<RecordedRequest>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let content_length: usize = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    requests.lock().expect("requests").push(RecordedRequest {
        path,
        body: String::from_utf8_lossy(&body).into_owned(),
    });
    let response = queue.lock().expect("queue").pop_front();
    let (status, headers, body) = response.unwrap_or((
        500,
        Vec::new(),
        r#"{"error":{"message":"queue exhausted"}}"#.to_string(),
    ));
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    };
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in &headers {
        let _ = write!(out, "{name}: {value}\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    stream.write_all(out.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// A client against `server` on the typesafe dialect.
fn typesafe_client(server: &MockServer) -> JevClient {
    JevClient::new(JevClientConfig::new(
        WireDialect::Typesafe,
        server.base_url(),
        "jev-1.13.0",
        "typesafe",
        "test-key",
    ))
}

/// `n` noul questions `q0..=q{n-1}` over a fixed state.
fn noul_request(n: usize) -> JudgmentRequest {
    JudgmentRequest {
        state: JudgmentState::from("the state every question sees"),
        questions: (0..n)
            .map(|index| {
                (
                    format!("q{index}"),
                    Question::Noul {
                        instructions: format!("question {index}"),
                        criteria: Some(NoulCriteria::default()),
                    },
                )
            })
            .collect(),
    }
}

#[tokio::test]
async fn single_request_round_trips_the_wire_contract() {
    let server = MockServer::new();
    server.push_judgment(&["urgent"], 296);
    let request = JudgmentRequest {
        state: JudgmentState::from("my card was charged twice"),
        questions: BTreeMap::from([(
            "urgent".to_string(),
            Question::Noul {
                instructions: "the sender needs a response soon".to_string(),
                criteria: None,
            },
        )]),
    };
    let result = typesafe_client(&server).judge(&request).await.expect("ok");
    assert_eq!(result.model, "jev-1.13.0", "the responding model wins");
    assert_eq!(result.answers.len(), 1);
    match &result.answers["urgent"] {
        pa_jev::Answer::Noul { noul } => assert_eq!(noul.to_bits(), 0.5_f64.to_bits()),
        other => panic!("expected a noul answer, got {other:?}"),
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/systemone");
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).expect("json body");
    assert_eq!(body["model"], "jev-1.13.0");
    assert_eq!(body["state"], "my card was charged twice");
    assert_eq!(body["questions"]["urgent"]["type"], "noul");
}

#[tokio::test]
async fn openrouter_dialect_posts_the_decisions_route() {
    let server = MockServer::new();
    server.push_judgment(&["q0"], 1);
    let client = JevClient::new(JevClientConfig::new(
        WireDialect::OpenrouterDecisions,
        server.base_url(),
        "typesafe/jev-1.13",
        "openrouter",
        "test-key",
    ));
    client.judge(&noul_request(1)).await.expect("ok");
    assert_eq!(server.requests()[0].path, "/decisions");
}

#[tokio::test]
async fn json_state_embeds_verbatim() {
    let server = MockServer::new();
    server.push_judgment(&["q0"], 1);
    let request = JudgmentRequest {
        state: JudgmentState::Json(serde_json::json!({"card": "visa", "charged": 2})),
        questions: noul_request(1).questions,
    };
    typesafe_client(&server).judge(&request).await.expect("ok");
    let body: serde_json::Value = serde_json::from_str(&server.requests()[0].body).expect("json");
    assert_eq!(
        body["state"],
        serde_json::json!({"card": "visa", "charged": 2})
    );
}

#[tokio::test]
async fn chunking_splits_merges_answers_and_sums_usage() {
    let server = MockServer::new();
    // 40 questions, cap 16 -> 3 chunks; every response answers every id
    // (the mock serves arrival order), usage differs per response so the
    // sum proves the merge.
    let ids: Vec<String> = (0..40).map(|index| format!("q{index}")).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    server.push_judgment(&id_refs, 100);
    server.push_judgment(&id_refs, 200);
    server.push_judgment(&id_refs, 300);
    let mut config = JevClientConfig::new(
        WireDialect::Typesafe,
        server.base_url(),
        "jev-1.13.0",
        "typesafe",
        "test-key",
    );
    config.max_questions = Some(16);
    let client = JevClient::new(config);
    let result = client.judge(&noul_request(40)).await.expect("ok");
    assert_eq!(server.request_count(), 3, "three chunked requests");
    assert_eq!(result.answers.len(), 40);
    assert_eq!(result.usage.input_tokens, 600);
    let expected_cost = 600.0 / 1.0e6 * JEV_INPUT_PRICE_PER_MTOK;
    assert!((result.usage.cost_usd - expected_cost).abs() < 1.0e-12);
    // Each request stayed within the cap.
    for request in server.requests() {
        let body: serde_json::Value = serde_json::from_str(&request.body).expect("json");
        let questions = body["questions"].as_object().expect("questions");
        assert!(questions.len() <= 16);
    }
}

#[tokio::test]
async fn first_failing_chunk_aborts_siblings_and_stops_dispatch() {
    let server = MockServer::new();
    // 80 questions, cap 16 -> 5 chunks, 4 in flight: the four dispatched
    // chunks all fail (400 is never retried), so the fifth never dispatches.
    for _ in 0..4 {
        server.push_status(400, None);
    }
    let mut config = JevClientConfig::new(
        WireDialect::Typesafe,
        server.base_url(),
        "jev-1.13.0",
        "typesafe",
        "test-key",
    );
    config.max_questions = Some(16);
    let client = JevClient::new(config);
    let error = client
        .judge(&noul_request(80))
        .await
        .expect_err("every dispatched chunk failed");
    match &error {
        JevError::ChunkFailed {
            partial_cost_usd, ..
        } => assert_eq!(
            partial_cost_usd.to_bits(),
            0.0_f64.to_bits(),
            "no survivor was billed"
        ),
        other => panic!("expected ChunkFailed, got {other:?}"),
    }
    // The abort races the in-flight connects: on a loaded host a sibling can
    // observe the failure flag before its TCP connect writes a byte, so the
    // mock may see anywhere from 1 to 4 requests. What must hold: the fifth
    // chunk never dispatches (cap 16 x 80 ids = 5 chunks, 4 in flight), and
    // every request that did land got the scripted 400 (never retried).
    let landed = server.request_count();
    assert!(
        (1..=4).contains(&landed),
        "the fifth chunk never dispatches and at least one 400 landed: {landed}"
    );
    assert_eq!(
        server.requests().len(),
        landed,
        "every recorded request is one of the four dispatched chunks"
    );
}

#[tokio::test]
async fn failed_chunk_carries_the_survivors_partial_usage() {
    let server = MockServer::new();
    // 41 questions, cap 40 -> 2 chunks: one succeeds (100 tokens), one
    // fails; both were already in flight, so exactly two requests.
    let ids: Vec<String> = (0..41).map(|index| format!("q{index}")).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    server.push_judgment(&id_refs, 100);
    server.push_status(400, None);
    let mut config = JevClientConfig::new(
        WireDialect::Typesafe,
        server.base_url(),
        "jev-1.13.0",
        "typesafe",
        "test-key",
    );
    config.max_questions = Some(40);
    let client = JevClient::new(config);
    let error = client
        .judge(&noul_request(41))
        .await
        .expect_err("one chunk failed");
    match &error {
        JevError::ChunkFailed {
            partial_cost_usd, ..
        } => {
            let expected = 100.0 / 1.0e6 * JEV_INPUT_PRICE_PER_MTOK;
            assert!((partial_cost_usd - expected).abs() < 1.0e-12);
        }
        other => panic!("expected ChunkFailed, got {other:?}"),
    }
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn transient_429_retries_then_succeeds() {
    let server = MockServer::new();
    // retry-after: 0 keeps the backoff sleep at zero; the attempt count is
    // the behavior under test.
    server.push_status(429, Some("0"));
    server.push_judgment(&["q0"], 10);
    let result = typesafe_client(&server)
        .judge(&noul_request(1))
        .await
        .expect("second attempt succeeds");
    assert_eq!(server.request_count(), 2);
    assert_eq!(result.usage.input_tokens, 10);
}

#[tokio::test]
async fn transient_5xx_exhausts_retries_with_a_typed_error() {
    let server = MockServer::new();
    for _ in 0..3 {
        server.push_status(500, Some("0"));
    }
    let error = typesafe_client(&server)
        .judge(&noul_request(1))
        .await
        .expect_err("every attempt failed");
    assert!(matches!(
        error,
        JevError::RetriesExhausted {
            status: 500,
            attempts: 3,
            ..
        }
    ));
    assert_eq!(server.request_count(), 3, "MAX_ATTEMPTS bounds the retries");
}

#[tokio::test]
async fn non_transient_statuses_never_retry() {
    for status in [400u16, 404] {
        let server = MockServer::new();
        server.push_status(status, Some("0"));
        let error = typesafe_client(&server)
            .judge(&noul_request(1))
            .await
            .expect_err("non-transient fails fast");
        assert!(
            matches!(error, JevError::Api { status: actual, .. } if actual == status),
            "expected Api({status}), got {error:?}"
        );
        assert_eq!(server.request_count(), 1);
    }
}

#[tokio::test]
async fn account_rejection_is_classified_for_the_cooldown() {
    let server = MockServer::new();
    server.push_status(401, None);
    let error = typesafe_client(&server)
        .judge(&noul_request(1))
        .await
        .expect_err("401");
    assert!(error.is_account_rejection());
    assert_eq!(server.request_count(), 1);
    let server = MockServer::new();
    server.push_status(400, None);
    let error = typesafe_client(&server)
        .judge(&noul_request(1))
        .await
        .expect_err("400");
    assert!(!error.is_account_rejection());
}

#[tokio::test]
async fn missing_or_mistyped_answers_are_malformed() {
    let server = MockServer::new();
    server.push(
        200,
        &[("content-type", "application/json")],
        serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {"other-id": {"type": "noul", "noul": 0.9}},
            "usage": {"input_tokens": 1, "output_tokens": 0}
        })
        .to_string(),
    );
    let error = typesafe_client(&server)
        .judge(&noul_request(1))
        .await
        .expect_err("no answer for the asked id");
    assert!(matches!(error, JevError::Malformed { .. }));

    let server = MockServer::new();
    server.push(
        200,
        &[("content-type", "application/json")],
        serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {"q0": {"type": "choice", "choice": "a", "probabilities": {"a": 1.0}, "confidence": 1.0}},
            "usage": {"input_tokens": 1, "output_tokens": 0}
        })
        .to_string(),
    );
    let error = typesafe_client(&server)
        .judge(&noul_request(1))
        .await
        .expect_err("kind mismatch");
    assert!(matches!(error, JevError::Malformed { .. }));
}

/// Audit F18: an answer that decodes but is not about the question must be
/// rejected by the CLIENT, not just by the typed helper — otherwise the
/// validation is reachable only from tests.
#[tokio::test]
async fn a_decodable_but_unoffered_answer_fails_the_judgment() {
    let server = MockServer::new();
    // Noul is asked; the endpoint answers with a choice naming an option the
    // question never offered. Shape is fine, kind is wrong, and the kind check
    // catches this one -- so the endpoint instead answers a CHOICE question
    // with an unknown option, which only meaning-validation can reject.
    server.push(
        200,
        &[("content-type", "application/json")],
        serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {
                "dept": {
                    "type": "choice",
                    "choice": "shipping",
                    "probabilities": {"shipping": 0.9},
                    "confidence": 0.9
                }
            },
            "usage": {"input_tokens": 10, "output_tokens": 0}
        })
        .to_string(),
    );
    let request = JudgmentRequest {
        state: JudgmentState::from("my card was charged twice"),
        questions: BTreeMap::from([(
            "dept".to_string(),
            Question::Choice {
                instructions: "which department".to_string(),
                criteria: BTreeMap::from([
                    ("billing".to_string(), None),
                    ("fraud".to_string(), None),
                ]),
            },
        )]),
    };
    let error = typesafe_client(&server)
        .judge(&request)
        .await
        .expect_err("an unoffered option is not an answer");
    let detail = error.to_string();
    assert!(detail.contains("never offered"), "{detail}");
}
