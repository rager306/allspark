//! A wire-level mock inference server: a real HTTP/1.1 listener on an
//! ephemeral loopback port that serves scripted responses (JSON or SSE) in
//! request order. Provider code paths then run end-to-end against genuine
//! sockets — request build, transport, SSE decode, failure classification —
//! with no network and no spawned process.
//!
//! Scripting is per-turn: queue the responses a test expects, hand the client
//! the [`MockInferenceServer::base_url`], and assert on
//! [`MockInferenceServer::requests`] afterwards. Every response carries
//! `Connection: close`, so each request lands on its own connection and the
//! queue stays strictly ordered.
//!
//! SSE bodies use `Transfer-Encoding: chunked` (the shape real providers
//! emit). [`MockInferenceServer::queue_sse_dropping`] stops mid-stream — the
//! first `drop_after` frames, then the socket closes without the terminating
//! zero-length chunk — which reproduces the silent lane-death the provider
//! classifies as a retryable `stream_drop`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One request the server received, captured for assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    /// The request target exactly as sent (path plus any query).
    pub path: String,
    /// Header names lower-cased, in wire order.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl RecordedRequest {
    /// The first header value for `name` (case-insensitive), if present.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The request body parsed as JSON, if it is valid JSON.
    #[must_use]
    pub fn body_json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.body).ok()
    }
}

/// A scripted response.
#[derive(Debug, Clone)]
enum ScriptedResponse {
    /// A fixed body with `Content-Length`.
    Body {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    /// An SSE body: `data: <payload>\n\n` per frame, terminated by `[DONE]`
    /// unless the payload is already the marker. `drop_after` cuts the
    /// connection after that many frames (mid-stream death).
    Sse {
        status: u16,
        headers: Vec<(String, String)>,
        frames: Vec<String>,
        drop_after: Option<usize>,
    },
    /// An SSE body announced with `Content-Length` and then closed cleanly
    /// after only the first `keep` frames: the reader hits a normal EOF with
    /// no terminal marker — the silent lane-death the provider classifies as
    /// a retryable `stream_drop` (a chunked body cut mid-flight surfaces a
    /// transport error instead, which is a different fault shape).
    SseTruncated {
        status: u16,
        headers: Vec<(String, String)>,
        frames: Vec<String>,
        keep: usize,
    },
    /// A held connection: the request is read and recorded, then the server
    /// never answers. The client's request timeout / abort race must settle
    /// the wait (a stalled peer, the shape `send` bounds with
    /// `timeout_ms`). Dropping the server releases the hold.
    Hang,
    /// An SSE response whose head and first `stall_after` frames are sent,
    /// then the connection is held open with no further bytes: the mid-stream
    /// stall a per-chunk idle timeout must bound (the head arrived, so the
    /// request timeout no longer applies; body EOF never comes).
    SseStall {
        status: u16,
        headers: Vec<(String, String)>,
        frames: Vec<String>,
        stall_after: usize,
    },
}

/// A wire-level mock inference server.
///
/// The listener lives on an ephemeral loopback port for the server's
/// lifetime; dropping it stops the accept loop.
pub struct MockInferenceServer {
    port: u16,
    queue: Arc<Mutex<Vec<ScriptedResponse>>>,
    /// Path-keyed responses, checked before the FIFO queue. This is what lets a
    /// test hold two dialects apart: a `/v1/messages` request and a
    /// `/v1/chat/completions` request consume different queues instead of the same
    /// one in arrival order.
    path_queues: Arc<Mutex<Vec<(String, ScriptedResponse)>>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown: Arc<AtomicBool>,
    /// Wakes a held [`ScriptedResponse::Hang`] connection when the server
    /// shuts down, so dropping the server never blocks on a hang.
    hang_wake: Arc<(Mutex<()>, std::sync::Condvar)>,
    /// The thread the accept loop runs on; joined on drop.
    thread: Option<std::thread::JoinHandle<()>>,
}

/// `MockServer` is the shorter spelling the drills use.
pub type MockServer = MockInferenceServer;

#[cfg(test)]
mod path_routing_tests {
    use super::*;
    use std::io::{Read, Write};

    fn raw_get(url: &str, path: &str) -> String {
        let mut stream = std::net::TcpStream::connect(url).expect("connect");
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nhost: {url}\r\nconnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .expect("write");
        let mut body = String::new();
        stream.read_to_string(&mut body).expect("read");
        body
    }

    fn body_of(raw: &str) -> String {
        raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
    }

    #[test]
    fn a_path_keyed_response_is_served_only_to_that_path() {
        let server = MockInferenceServer::new();
        server.queue_json_for_path("/v1/messages", 200, r#"{"dialect":"messages"}"#);
        server.queue_json_for_path("/v1/chat/completions", 200, r#"{"dialect":"chat"}"#);

        let url = format!("127.0.0.1:{}", server.port());
        let messages = body_of(&raw_get(&url, "/v1/messages"));
        let chat = body_of(&raw_get(&url, "/v1/chat/completions"));

        assert_eq!(
            messages, r#"{"dialect":"messages"}"#,
            "the /v1/messages queue"
        );
        assert_eq!(
            chat, r#"{"dialect":"chat"}"#,
            "the /v1/chat/completions queue"
        );
    }

    #[test]
    fn an_unkeyed_path_falls_through_to_the_global_queue() {
        let server = MockInferenceServer::new();
        server.queue_json(200, r#"{"global":true}"#);
        let url = format!("127.0.0.1:{}", server.port());
        let raw = raw_get(&url, "/v1/anything");
        assert!(body_of(&raw).contains(r#""global":true"#));
    }

    #[test]
    fn a_path_queue_that_runs_dry_falls_back_to_the_global_queue() {
        let server = MockInferenceServer::new();
        server.queue_json_for_path("/v1/messages", 200, r#"{"first":true}"#);
        server.queue_json(200, r#"{"global-fallback":true}"#);
        let url = format!("127.0.0.1:{}", server.port());

        let first = body_of(&raw_get(&url, "/v1/messages"));
        let second = body_of(&raw_get(&url, "/v1/messages"));
        assert_eq!(first, r#"{"first":true}"#);
        assert!(
            second.contains(r#""global-fallback":true"#),
            "a dry path queue must degrade to the global queue, not to an error: {second}"
        );
    }

    #[test]
    fn the_key_is_the_path_so_a_query_string_does_not_claim_a_keyed_response() {
        // The key is the path exactly as queued; a query string is part of the request
        // target, so this request falls through to the global queue.
        let server = MockInferenceServer::new();
        server.queue_json_for_path("/v1/messages", 200, r#"{"exact":true}"#);
        server.queue_json(200, r#"{"fallback":true}"#);
        let url = format!("127.0.0.1:{}", server.port());
        let raw = raw_get(&url, "/v1/messages?stream=true");
        assert!(body_of(&raw).contains(r#""fallback":true"#), "{raw}");
    }
}

impl MockInferenceServer {
    /// Bind the server on `127.0.0.1:0` and start serving.
    ///
    /// # Panics
    ///
    /// Panics if the loopback listener cannot be bound.
    #[must_use]
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock inference server");
        let port = listener
            .local_addr()
            .expect("mock inference server address")
            .port();
        let queue: Arc<Mutex<Vec<ScriptedResponse>>> = Arc::new(Mutex::new(Vec::new()));
        let path_queues: Arc<Mutex<Vec<(String, ScriptedResponse)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let hang_wake = Arc::new((Mutex::new(()), std::sync::Condvar::new()));
        let queue_for_thread = Arc::clone(&queue);
        let path_queues_for_thread = Arc::clone(&path_queues);
        let requests_for_thread = Arc::clone(&requests);
        let shutdown_for_thread = Arc::clone(&shutdown);
        let hang_wake_for_thread = Arc::clone(&hang_wake);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if shutdown_for_thread.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let queue = Arc::clone(&queue_for_thread);
                let path_queues = Arc::clone(&path_queues_for_thread);
                let requests = Arc::clone(&requests_for_thread);
                // One connection = one request (`Connection: close`); handle
                // inline so responses stay in arrival order.
                let _ = serve(
                    stream,
                    &queue,
                    &path_queues,
                    &requests,
                    &shutdown_for_thread,
                    &hang_wake_for_thread,
                );
            }
        });
        MockInferenceServer {
            port,
            queue,
            path_queues,
            requests,
            shutdown,
            hang_wake,
            thread: Some(thread),
        }
    }

    /// The bound port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The base URL a provider's `baseUrl` should point at
    /// (`http://127.0.0.1:{port}`); the provider appends `/chat/completions`.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The OpenAI-compatible API root a provider's `baseUrl` should be set to
    /// (`{base_url}/v1`): the provider appends `/chat/completions`.
    #[must_use]
    pub fn api_base_url(&self) -> String {
        format!("{}/v1", self.base_url())
    }

    /// The full `chat/completions` endpoint.
    #[must_use]
    pub fn completions_url(&self) -> String {
        format!("{}/chat/completions", self.api_base_url())
    }

    /// Queue one raw response (status, headers, body) served on the next turn.
    /// Queue a response that is served only to `path`, ahead of the arrival-ordered
    /// queue. This is how a test holds two dialects apart: a `/v1/messages` request and a
    /// `/v1/chat/completions` request consume different queues, so a test can say
    /// "the next `/v1/messages` request fails while `/v1/chat/completions` succeeds".
    ///
    /// Unscripted requests still answer 500 loudly (a test bug), and a path with an
    /// empty key-queue falls through to the global queue, so a partial setup degrades
    /// to the old behaviour rather than to an error.
    ///
    /// # Panics
    ///
    pub fn queue_response(&self, status: u16, headers: &[(&str, &str)], body: impl Into<String>) {
        self.push(ScriptedResponse::Body {
            status,
            headers: owned_headers(headers),
            body: body.into(),
        });
    }

    /// Queue a JSON response (`content-type: application/json`).
    pub fn queue_json(&self, status: u16, body: impl Into<String>) {
        self.queue_response(status, &[("content-type", "application/json")], body);
    }

    /// Queue an SSE response: each payload becomes one `data: <payload>\n\n`
    /// frame and the stream is closed with `data: [DONE]\n\n`.
    pub fn queue_sse(&self, chunks: Vec<&str>) {
        self.push(ScriptedResponse::Sse {
            status: 200,
            headers: Vec::new(),
            frames: chunks.into_iter().map(str::to_string).collect(),
            drop_after: None,
        });
    }

    /// Queue an SSE response that dies mid-stream: the first `drop_after`
    /// frames are sent, then the connection closes without the terminating
    /// chunk or `[DONE]` marker.
    pub fn queue_sse_dropping(&self, chunks: Vec<&str>, drop_after: usize) {
        self.push(ScriptedResponse::Sse {
            status: 200,
            headers: Vec::new(),
            frames: chunks.into_iter().map(str::to_string).collect(),
            drop_after: Some(drop_after),
        });
    }

    /// Queue an SSE response that ends cleanly after the first `keep`
    /// frames, announced by `Content-Length` — no terminal marker and no
    /// transport error, the silent `stream_drop` shape.
    pub fn queue_sse_truncated(&self, chunks: Vec<&str>, keep: usize) {
        self.push(ScriptedResponse::SseTruncated {
            status: 200,
            headers: Vec::new(),
            frames: chunks.into_iter().map(str::to_string).collect(),
            keep,
        });
    }

    /// Queue an SSE response from complete frames (each frame is written
    /// verbatim; the caller owns the trailing blank line and any `[DONE]`).
    pub fn queue_sse_raw(&self, frames: Vec<&str>) {
        self.push(ScriptedResponse::Sse {
            status: 200,
            headers: Vec::new(),
            frames: frames.into_iter().map(str::to_string).collect(),
            drop_after: None,
        });
    }

    /// Queue a held connection: the request is read and recorded, then the
    /// server never writes a response (the stalled-peer shape). The client's
    /// request timeout / abort race settles the wait; dropping the server
    /// releases the hold.
    pub fn queue_hang(&self) {
        self.push(ScriptedResponse::Hang);
    }

    /// Queue an SSE response that sends its head and the first `stall_after`
    /// frames, then holds the connection open without any further bytes: the
    /// headers-then-silence shape a body-read idle timeout must bound.
    pub fn queue_sse_stalling(&self, chunks: Vec<&str>, stall_after: usize) {
        self.push(ScriptedResponse::SseStall {
            status: 200,
            headers: Vec::new(),
            frames: chunks.into_iter().map(str::to_string).collect(),
            stall_after,
        });
    }

    /// The responses still queued.
    ///
    /// # Panics
    ///
    /// Panics if the queue mutex is poisoned.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queue.lock().expect("mock queue lock").len()
    }

    /// Every request the server has received, in arrival order.
    ///
    /// # Panics
    ///
    /// Panics if the requests mutex is poisoned.
    #[must_use]
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("mock requests lock").clone()
    }

    /// How many requests the server has received.
    ///
    /// # Panics
    ///
    /// Panics if the requests mutex is poisoned.
    #[must_use]
    pub fn request_count(&self) -> usize {
        self.requests.lock().expect("mock requests lock").len()
    }

    fn push(&self, response: ScriptedResponse) {
        self.queue.lock().expect("mock queue lock").push(response);
    }

    fn push_for_path(&self, path: &str, response: ScriptedResponse) {
        self.path_queues
            .lock()
            .expect("mock path-queue lock")
            .push((path.to_string(), response));
    }

    /// Queue a JSON response served only to `path` (see `push_for_path`).
    pub fn queue_json_for_path(&self, path: &str, status: u16, body: impl Into<String>) {
        self.push_for_path(
            path,
            ScriptedResponse::Body {
                status,
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: body.into(),
            },
        );
    }

    /// Queue an SSE response served only to `path` (see `push_for_path`).
    pub fn queue_sse_for_path(&self, path: &str, chunks: Vec<&str>) {
        self.push_for_path(
            path,
            ScriptedResponse::Sse {
                status: 200,
                headers: Vec::new(),
                frames: chunks.into_iter().map(str::to_string).collect(),
                drop_after: None,
            },
        );
    }
}

impl Default for MockInferenceServer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MockInferenceServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake the blocking `accept` with a throwaway connection so the loop
        // observes the shutdown flag and exits.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        // Release any held `Hang` connection so `serve` returns and the
        // accept loop can observe the shutdown flag.
        self.hang_wake.1.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn owned_headers(headers: &[(&str, &str)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// Read one request, pop the next scripted response, and write it.
fn serve(
    mut stream: TcpStream,
    queue: &Mutex<Vec<ScriptedResponse>>,
    path_queues: &Mutex<Vec<(String, ScriptedResponse)>>,
    requests: &Mutex<Vec<RecordedRequest>>,
    shutdown: &AtomicBool,
    hang_wake: &(Mutex<()>, std::sync::Condvar),
) -> std::io::Result<()> {
    let Some(request) = read_request(&stream)? else {
        return Ok(());
    };
    requests
        .lock()
        .expect("mock requests lock")
        .push(request.clone());
    let response = {
        // Path-keyed responses win, then fall back to the arrival-ordered queue. The key is
        // the path exactly as the test queued it (a `/v1/messages` request never claims a
        // response queued for `/v1/chat/completions`).
        let mut keyed = path_queues.lock().expect("mock path-queue lock");
        if let Some(position) = keyed.iter().position(|(path, _)| path == &request.path) {
            Some(keyed.remove(position).1)
        } else {
            drop(keyed);
            let mut queue = queue.lock().expect("mock queue lock");
            // `then(|| queue.remove(0))` would panic on an empty queue: `then` is eager.
            (!queue.is_empty()).then(|| queue.remove(0))
        }
    };
    match response {
        Some(ScriptedResponse::Hang) => {
            hold_until_shutdown(shutdown, hang_wake);
            Ok(())
        }
        Some(ScriptedResponse::SseStall {
            status,
            headers,
            frames,
            stall_after,
        }) => {
            write_stalling_sse(&mut stream, status, &headers, &frames, stall_after)?;
            hold_until_shutdown(shutdown, hang_wake);
            Ok(())
        }
        Some(response) => write_response(&mut stream, response),
        // An unscripted request is a test bug: answer loudly instead of
        // hanging the client.
        None => write_response(
            &mut stream,
            ScriptedResponse::Body {
                status: 500,
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: r#"{"error":{"message":"mock inference server queue exhausted","type":"server_error"}}"#
                    .to_string(),
            },
        ),
    }
}

/// Hold a connection open without answering until the server shuts down:
/// the stalled-peer shape a request timeout must bound. Re-checks the
/// shutdown flag so a spurious wake does not return early.
fn hold_until_shutdown(shutdown: &AtomicBool, hang_wake: &(Mutex<()>, std::sync::Condvar)) {
    let (lock, condvar) = hang_wake;
    let mut guard = lock.lock().expect("mock hang lock");
    while !shutdown.load(Ordering::SeqCst) {
        let (next, _timeout) = condvar
            .wait_timeout(guard, std::time::Duration::from_millis(50))
            .expect("mock hang wait");
        // `wait_timeout` re-acquires the lock; the loop re-checks the flag.
        guard = next;
    }
}

/// Parse the request line, headers, and `Content-Length`-delimited body.
fn read_request(stream: &TcpStream) -> std::io::Result<Option<RecordedRequest>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            // Connection closed before a complete request line.
            return Ok(None);
        }
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let mut lines = head.split("\r\n");
    let Some(request_line) = lines.next() else {
        return Ok(None);
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || path.is_empty() {
        return Ok(None);
    }
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let content_length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body_bytes)?;
    }
    Ok(Some(RecordedRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
    }))
}

fn write_response(stream: &mut TcpStream, response: ScriptedResponse) -> std::io::Result<()> {
    match response {
        // `serve` intercepts `Hang` and `SseStall` before reaching here (they
        // hold the connection instead of finishing); these arms keep the
        // match exhaustive.
        ScriptedResponse::Hang | ScriptedResponse::SseStall { .. } => Ok(()),
        ScriptedResponse::Body {
            status,
            headers,
            body,
        } => {
            let mut head = format!(
                "HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                reason_phrase(status),
                body.len()
            );
            push_headers(&mut head, &headers);
            stream.write_all(head.as_bytes())?;
            stream.write_all(body.as_bytes())?;
            stream.flush()
        }
        ScriptedResponse::SseTruncated {
            status,
            headers,
            frames,
            keep,
        } => {
            let payload = sse_payload(frames.iter().take(keep));
            let mut head = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
                reason_phrase(status),
                payload.len()
            );
            push_headers(&mut head, &headers);
            stream.write_all(head.as_bytes())?;
            stream.write_all(payload.as_bytes())?;
            stream.flush()
        }
        ScriptedResponse::Sse {
            status,
            headers,
            frames,
            drop_after,
        } => {
            let mut head = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nCache-Control: no-cache\r\nConnection: close\r\n",
                reason_phrase(status),
            );
            push_headers(&mut head, &headers);
            stream.write_all(head.as_bytes())?;
            // `[DONE]` terminates the stream unless the caller already
            // supplied it or the stream is being dropped mid-flight.
            let mut payloads = frames;
            let already_done = payloads
                .last()
                .is_some_and(|frame| frame.trim() == "[DONE]");
            if drop_after.is_none() && !already_done {
                payloads.push("[DONE]".to_string());
            }
            let limit = drop_after.unwrap_or(payloads.len());
            for payload in payloads.iter().take(limit) {
                let frame = format!("data: {payload}\n\n");
                stream.write_all(format!("{:x}\r\n", frame.len()).as_bytes())?;
                stream.write_all(frame.as_bytes())?;
                stream.write_all(b"\r\n")?;
                stream.flush()?;
            }
            if drop_after.is_some() {
                // Mid-stream death: close without the terminating
                // zero-length chunk so the reader sees a truncated body.
                return Ok(());
            }
            stream.write_all(b"0\r\n\r\n")?;
            stream.flush()
        }
    }
}

/// Write the chunked SSE head plus the first `stall_after` frames, then
/// return without the terminating zero-length chunk: the caller holds the
/// connection open so the body stalls mid-stream.
fn write_stalling_sse(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(String, String)],
    frames: &[String],
    stall_after: usize,
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {}
Content-Type: text/event-stream
Transfer-Encoding: chunked
Cache-Control: no-cache
Connection: close
",
        reason_phrase(status),
    );
    push_headers(&mut head, headers);
    stream.write_all(head.as_bytes())?;
    for frame in frames.iter().take(stall_after) {
        let frame = format!("data: {frame}\n\n");
        stream.write_all(format!("{:x}\r\n", frame.len()).as_bytes())?;
        stream.write_all(frame.as_bytes())?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
    }
    Ok(())
}

/// Append the header block (each `name: value\r\n`) followed by the blank
/// line that ends the response head.
fn push_headers(head: &mut String, headers: &[(String, String)]) {
    use std::fmt::Write as _;
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
}

/// The `data: <frame>\n\n` SSE payload for a run of frames.
fn sse_payload<'a>(frames: impl Iterator<Item = &'a String>) -> String {
    use std::fmt::Write as _;
    let mut payload = String::new();
    for frame in frames {
        let _ = write!(payload, "data: {frame}\n\n");
    }
    payload
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

/// Convenience: the header map view a test often wants.
#[must_use]
pub fn header_map(request: &RecordedRequest) -> HashMap<String, String> {
    request.headers.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> reqwest::Client {
        reqwest::Client::builder().build().expect("reqwest client")
    }

    #[tokio::test]
    async fn sse_streaming_round_trip() {
        let server = MockInferenceServer::new();
        server.queue_sse(vec![
            r#"{"choices":[{"delta":{"content":"hello"}}]}"#,
            r#"{"choices":[{"delta":{"content":" world"},"finish_reason":"stop"}]}"#,
        ]);
        let response = client()
            .post(server.completions_url())
            .json(&serde_json::json!({"model": "mock-1"}))
            .send()
            .await
            .expect("send");
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = response.text().await.expect("body");
        assert!(body.contains("data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}"));
        assert!(body.contains("\"content\":\" world\""));
        assert!(body.trim_end().ends_with("data: [DONE]"));
    }

    #[tokio::test]
    async fn records_method_path_headers_and_body() {
        let server = MockInferenceServer::new();
        server.queue_json(200, r#"{"ok":true}"#);
        let payload = serde_json::json!({"model": "mock-1", "messages": [{"role": "user"}]});
        let response = client()
            .post(server.completions_url())
            .header("authorization", "Bearer sk-test")
            .json(&payload)
            .send()
            .await
            .expect("send");
        assert_eq!(response.status().as_u16(), 200);

        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/chat/completions");
        assert_eq!(request.header("authorization"), Some("Bearer sk-test"));
        assert_eq!(request.body_json(), Some(payload));
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn rotates_responses_by_turn() {
        let server = MockInferenceServer::new();
        server.queue_json(429, r#"{"error":{"message":"slow down"}}"#);
        server.queue_json(200, r#"{"ok":true}"#);
        assert_eq!(server.queued(), 2);

        let first = client()
            .post(server.completions_url())
            .body("{}")
            .send()
            .await
            .expect("first");
        assert_eq!(first.status().as_u16(), 429);
        assert_eq!(
            first.text().await.expect("body"),
            r#"{"error":{"message":"slow down"}}"#
        );
        assert_eq!(server.queued(), 1);

        let second = client()
            .post(server.completions_url())
            .body("{}")
            .send()
            .await
            .expect("second");
        assert_eq!(second.status().as_u16(), 200);
        assert_eq!(second.text().await.expect("body"), r#"{"ok":true}"#);
        assert_eq!(server.queued(), 0);
        assert_eq!(server.request_count(), 2);
    }

    #[tokio::test]
    async fn queue_exhaustion_answers_500() {
        let server = MockInferenceServer::new();
        let response = client()
            .post(server.completions_url())
            .body("{}")
            .send()
            .await
            .expect("send");
        assert_eq!(response.status().as_u16(), 500);
        let body = response.text().await.expect("body");
        assert!(body.contains("queue exhausted"));
    }

    #[tokio::test]
    async fn dropping_sse_mid_stream_truncates_the_body() {
        let server = MockInferenceServer::new();
        server.queue_sse_dropping(
            vec![
                r#"{"choices":[{"delta":{"content":"a"}}]}"#,
                r#"{"choices":[{"delta":{"content":"b"}}]}"#,
                r#"{"choices":[{"delta":{"content":"c"}}]}"#,
                r#"{"choices":[{"delta":{"content":"never"}}]}"#,
            ],
            3,
        );
        let response = client()
            .post(server.completions_url())
            .body("{}")
            .send()
            .await
            .expect("send");
        assert_eq!(response.status().as_u16(), 200);
        // The first three frames arrive, then the body read fails: no
        // terminating chunk and no `[DONE]`.
        let error = response.text().await.expect_err("truncated body");
        assert_ne!(error.to_string(), "");
    }

    #[tokio::test]
    async fn truncated_sse_ends_cleanly_without_a_terminal_marker() {
        let server = MockInferenceServer::new();
        server.queue_sse_truncated(
            vec![
                r#"{"choices":[{"delta":{"content":"a"}}]}"#,
                r#"{"choices":[{"delta":{"content":"b"}}]}"#,
                r#"{"choices":[{"delta":{"content":"c"}}]}"#,
                r#"{"choices":[{"delta":{"content":"never"}}]}"#,
            ],
            3,
        );
        let response = client()
            .post(server.completions_url())
            .body("{}")
            .send()
            .await
            .expect("send");
        assert_eq!(response.status().as_u16(), 200);
        // The body ends cleanly after three frames: a normal EOF with no
        // terminal marker (the `stream_drop` shape).
        let body = response.text().await.expect("body");
        assert_eq!(body.matches("data: ").count(), 3);
        assert!(!body.contains("never"));
        assert!(!body.contains("[DONE]"));
    }

    #[test]
    fn header_map_view_lowercases_names() {
        let request = RecordedRequest {
            method: "GET".to_string(),
            path: "/x".to_string(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: String::new(),
        };
        assert_eq!(
            header_map(&request).get("content-type").map(String::as_str),
            Some("application/json")
        );
    }
}
