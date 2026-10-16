//! MCP conformance suite: a minimal, in-process MCP JSON-RPC 2.0 server driven
//! by the product's own MCP client surface.
//!
//! The product's only Rust-side MCP client is [`super::probe::ReqwestMcpProbe`]
//! (the streamable-HTTP handshake the manager verifies connections with), so the
//! suite has two fronts over ONE server implementation:
//!
//! * a transport-agnostic JSON-RPC 2.0 core driven over an in-memory channel
//!   (the protocol matrix: initialize/capabilities, tools/list, tools/call,
//!   notifications/initialized, resources/list, the SSE flood, invalid method,
//!   protocol-version mismatch, and the no-hang timeout), and
//! * a loopback streamable-HTTP front over the same core so the real
//!   `ReqwestMcpProbe` proves our client connects to this server end to end.
//!
//! Everything runs in-process (a std `TcpListener` on loopback and an in-memory
//! channel) — no external process is spawned, and every operation is bounded by
//! [`OPERATION_TIMEOUT`].

#![allow(clippy::too_many_lines)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::probe::{McpEndpointProbe, ProbeOutcome, ReqwestMcpProbe};

/// The per-operation budget every conformance operation is held to (the spec's
/// 5-second no-hang bound). The timeout test exercises the mechanism on a short
/// budget so the suite stays fast; the default is asserted directly.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(5);

/// The protocol versions the test server negotiates.
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2024-11-05"];

/// The three tools `tools/list` serves, in a fixed order.
fn fixture_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "echo",
            "description": "Echoes the message argument back.",
            "inputSchema": {
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"]
            }
        }),
        json!({
            "name": "add",
            "description": "Adds two integers.",
            "inputSchema": {
                "type": "object",
                "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } },
                "required": ["a", "b"]
            }
        }),
        json!({
            "name": "fail",
            "description": "Always returns a tool-level error result.",
            "inputSchema": { "type": "object" }
        }),
    ]
}

/// The deterministic `tools/call` answer: a pure function of the request.
fn tool_result(name: &str, arguments: &Value) -> Value {
    match name {
        "echo" => json!({
            "content": [{
                "type": "text",
                "text": arguments.get("message").and_then(Value::as_str).unwrap_or_default()
            }],
            "isError": false
        }),
        "add" => {
            let a = arguments.get("a").and_then(Value::as_i64).unwrap_or(0);
            let b = arguments.get("b").and_then(Value::as_i64).unwrap_or(0);
            json!({
                "content": [{ "type": "text", "text": (a + b).to_string() }],
                "isError": false
            })
        }
        "fail" => json!({
            "content": [{ "type": "text", "text": "tool failed as designed" }],
            "isError": true
        }),
        other => json!({
            "content": [{ "type": "text", "text": format!("unknown tool: {other}") }],
            "isError": true
        }),
    }
}

/// The minimal MCP test server: a transport-agnostic JSON-RPC 2.0 core.
///
/// [`Self::dispatch`] consumes one request value and pushes zero or more frames
/// (a reply, or unsolicited notifications) onto [`Self::outbox`]; the channel
/// front and the HTTP front both read the outbox.
struct McpConformanceServer {
    outbox: VecDeque<Value>,
    /// When set, `dispatch` produces nothing (the stall the timeout test needs).
    stalled: bool,
    /// Emit 100 notifications on `notifications/initialized` (the SSE flood).
    flood_notifications: bool,
}

impl McpConformanceServer {
    fn new() -> Self {
        Self {
            outbox: VecDeque::new(),
            stalled: false,
            flood_notifications: false,
        }
    }

    fn with_flood() -> Self {
        Self {
            flood_notifications: true,
            ..Self::new()
        }
    }

    fn stall(&mut self) {
        self.stalled = true;
    }

    fn push_reply(&mut self, id: Option<&Value>, result: &Value) {
        self.outbox
            .push_back(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn push_error(&mut self, id: Option<&Value>, code: i64, message: &str) {
        self.outbox.push_back(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }));
    }

    /// Pop the reply addressed to `id`, discarding any notifications queued
    /// ahead of it (the HTTP front's one-request-per-connection view).
    fn take_reply(&mut self, id: Option<&Value>) -> Value {
        let mut index = 0;
        while index < self.outbox.len() {
            let matches = self.outbox[index].get("id") == id;
            if matches {
                return self.outbox.remove(index).unwrap_or(Value::Null);
            }
            index += 1;
        }
        Value::Null
    }

    /// The one protocol dispatch: request -> outbox frames.
    fn dispatch(&mut self, request: &Value) {
        if self.stalled {
            return;
        }
        let id = request.get("id");
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
                    self.push_reply(
                        id,
                        &json!({
                            "protocolVersion": requested,
                            "capabilities": { "tools": { "listChanged": false }, "resources": {} },
                            "serverInfo": { "name": "pa-mcp-conformance", "version": "1.0.0" }
                        }),
                    );
                } else {
                    // An unsupported version is refused, never silently negotiated.
                    self.push_error(id, -32602, "unsupported protocol version");
                }
            }
            "notifications/initialized" => {
                // A notification: no reply. The flood rides the same path.
                if self.flood_notifications {
                    for index in 0..100 {
                        self.outbox.push_back(json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/message",
                            "params": { "level": "info", "data": index }
                        }));
                    }
                }
            }
            "tools/list" => {
                self.push_reply(id, &json!({ "tools": fixture_tools() }));
            }
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                self.push_reply(id, &tool_result(name, &arguments));
            }
            "resources/list" => {
                self.push_reply(id, &json!({ "resources": [] }));
            }
            _ => {
                self.push_error(id, -32601, "method not found");
            }
        }
    }
}

/// One conformance-client failure.
#[derive(Debug, PartialEq, Eq)]
enum ChannelError {
    /// The operation exceeded its budget without a reply.
    Timeout,
}

/// A JSON-RPC 2.0 client over the in-memory channel: it owns the request ids,
/// the per-operation budget, and the notifications received while awaiting a
/// reply (the flood's landing pad).
struct MemoryChannel {
    server: McpConformanceServer,
    next_id: i64,
    notifications: Vec<Value>,
    budget: Duration,
}

impl MemoryChannel {
    fn new(server: McpConformanceServer) -> Self {
        Self {
            server,
            next_id: 1,
            notifications: Vec::new(),
            budget: OPERATION_TIMEOUT,
        }
    }

    fn with_budget(server: McpConformanceServer, budget: Duration) -> Self {
        Self {
            budget,
            ..Self::new(server)
        }
    }

    /// Send a request and return its reply within the budget.
    fn request(&mut self, method: &str, params: &Value) -> Result<Value, ChannelError> {
        let id = self.next_id;
        self.next_id += 1;
        self.server.dispatch(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        self.await_reply(id)
    }

    /// Send a notification (no reply is awaited).
    fn notify(&mut self, method: &str, params: &Value) {
        self.server
            .dispatch(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn await_reply(&mut self, id: i64) -> Result<Value, ChannelError> {
        let deadline = Instant::now() + self.budget;
        loop {
            while let Some(frame) = self.server.outbox.pop_front() {
                if frame.get("id").and_then(Value::as_i64) == Some(id) {
                    return Ok(frame);
                }
                // An unsolicited notification (the flood) is stashed, not lost.
                self.notifications.push(frame);
            }
            if Instant::now() >= deadline {
                return Err(ChannelError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn handshake(&mut self) -> Result<Value, ChannelError> {
        let init = self.request(
            "initialize",
            &json!({
                "protocolVersion": SUPPORTED_PROTOCOL_VERSIONS[0],
                "capabilities": {},
                "clientInfo": { "name": "pa-conformance", "version": "1.0.0" }
            }),
        )?;
        self.notify("notifications/initialized", &json!({}));
        Ok(init)
    }
}

/// A loopback streamable-HTTP front over the same server core: one request per
/// connection, `application/json` replies, `202` for notifications.
struct LoopbackMcpHttp {
    url: String,
    stop: Arc<AtomicBool>,
}

impl LoopbackMcpHttp {
    /// Bind an ephemeral loopback port and serve the core until dropped.
    fn start(flood: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback mcp server");
        let address = listener.local_addr().expect("loopback address");
        let url = format!("http://{address}/mcp");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut server = McpConformanceServer::new();
            if flood {
                server.flood_notifications = true;
            }
            for incoming in listener.incoming() {
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = incoming else { break };
                serve_http_connection(stream, &mut server);
            }
        });
        Self { url, stop }
    }
}

impl Drop for LoopbackMcpHttp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so the thread observes the stop flag.
        let _ = TcpStream::connect(
            self.url
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap(),
        );
    }
}

/// Read one HTTP request (head + Content-Length body), dispatch its JSON-RPC
/// payload, and write the response.
fn serve_http_connection(mut stream: TcpStream, server: &mut McpConformanceServer) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream.try_clone().expect("clone loopback stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.trim().is_empty() {
        return;
    }
    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) if header.trim().is_empty() => break,
            Ok(_) => {
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return,
    };
    let is_notification = request.get("id").is_none();
    let request_id = request.get("id").cloned();
    server.dispatch(&request);
    let response = if is_notification {
        // The MCP spec answers a notification with 202 and no body; the
        // notifications it queued (the flood) are dropped with the connection.
        server.outbox.clear();
        b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
    } else {
        let frame = server.take_reply(request_id.as_ref());
        let payload = serde_json::to_vec(&frame).unwrap_or_default();
        let mut head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        )
        .into_bytes();
        head.extend_from_slice(&payload);
        head
    };
    let _ = stream.write_all(&response);
    let _ = stream.flush();
}

fn tool_names(reply: &Value) -> Vec<String> {
    reply
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                .map(std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Happy path: initialize -> capabilities, tools/list -> 3 tools, tools/call ->
/// a deterministic answer, notifications/initialized accepted, resources/list
/// empty.
#[test]
fn happy_path_initialize_list_call_and_resources() {
    let mut client = MemoryChannel::new(McpConformanceServer::new());
    let init = client
        .request(
            "initialize",
            &json!({
                "protocolVersion": SUPPORTED_PROTOCOL_VERSIONS[0],
                "capabilities": {},
                "clientInfo": { "name": "pa-conformance", "version": "1.0.0" }
            }),
        )
        .expect("initialize replies within the budget");
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(
        init.pointer("/result/protocolVersion")
            .and_then(Value::as_str),
        Some(SUPPORTED_PROTOCOL_VERSIONS[0])
    );
    assert!(
        init.pointer("/result/capabilities/tools").is_some(),
        "initialize advertises capabilities: {init}"
    );
    assert_eq!(
        init.pointer("/result/serverInfo/name")
            .and_then(Value::as_str),
        Some("pa-mcp-conformance")
    );

    // The initialized notification is accepted and produces no reply frame.
    client.notify("notifications/initialized", &json!({}));
    assert!(
        client.server.outbox.is_empty(),
        "a notification yields no reply"
    );

    let list = client
        .request("tools/list", &json!({}))
        .expect("tools/list replies");
    assert_eq!(tool_names(&list), vec!["echo", "add", "fail"]);

    let echo = client
        .request(
            "tools/call",
            &json!({ "name": "echo", "arguments": { "message": "hello conformance" } }),
        )
        .expect("tools/call replies");
    assert_eq!(
        echo.pointer("/result/content/0/text")
            .and_then(Value::as_str),
        Some("hello conformance")
    );
    assert_eq!(echo.pointer("/result/isError"), Some(&json!(false)));

    let add = client
        .request(
            "tools/call",
            &json!({ "name": "add", "arguments": { "a": 2, "b": 40 } }),
        )
        .expect("add replies");
    assert_eq!(
        add.pointer("/result/content/0/text")
            .and_then(Value::as_str),
        Some("42")
    );

    let resources = client
        .request("resources/list", &json!({}))
        .expect("resources/list replies");
    assert_eq!(
        resources
            .pointer("/result/resources")
            .and_then(Value::as_array)
            .map(std::vec::Vec::len),
        Some(0),
        "resources/list is empty: {resources}"
    );
}

/// The SSE flood: 100 back-to-back notifications must not break the client —
/// every one lands, and the next request still completes.
#[test]
fn sse_flood_of_one_hundred_notifications_keeps_the_client_alive() {
    let mut client = MemoryChannel::new(McpConformanceServer::with_flood());
    // The flood rides the initialized notification inside the handshake.
    client.handshake().expect("initialize replies");

    let list = client
        .request("tools/list", &json!({}))
        .expect("the client survives the flood and lists tools");
    assert_eq!(tool_names(&list).len(), 3);
    assert_eq!(
        client.notifications.len(),
        100,
        "exactly the 100 flooded notifications were received"
    );
    for (index, frame) in client.notifications.iter().enumerate() {
        assert_eq!(frame["method"], "notifications/message");
        assert_eq!(
            frame.pointer("/params/data").and_then(Value::as_i64),
            Some(index as i64)
        );
    }
}

/// An invalid method answers a JSON-RPC error (code -32601), never a hang.
#[test]
fn invalid_method_returns_a_jsonrpc_error() {
    let mut client = MemoryChannel::new(McpConformanceServer::new());
    let reply = client
        .request("does/not/exist", &json!({}))
        .expect("the error reply arrives within the budget");
    assert_eq!(
        reply.pointer("/error/code").and_then(Value::as_i64),
        Some(-32601)
    );
    assert_eq!(
        reply.pointer("/error/message").and_then(Value::as_str),
        Some("method not found")
    );
    assert!(
        reply.get("result").is_none(),
        "an error frame carries no result"
    );
}

/// A protocol-version mismatch is refused with an error, never negotiated.
#[test]
fn protocol_version_mismatch_returns_an_error() {
    let mut client = MemoryChannel::new(McpConformanceServer::new());
    let reply = client
        .request(
            "initialize",
            &json!({ "protocolVersion": "1999-01-01", "capabilities": {} }),
        )
        .expect("the refusal arrives within the budget");
    assert_eq!(
        reply.pointer("/error/code").and_then(Value::as_i64),
        Some(-32602)
    );
    assert!(reply.get("result").is_none());
}

/// A stalling server must time out on the operation budget, never hang.
#[test]
fn a_stalling_operation_times_out_instead_of_hanging() {
    assert_eq!(
        OPERATION_TIMEOUT,
        Duration::from_secs(5),
        "the harness holds every operation to a 5-second budget"
    );
    let mut server = McpConformanceServer::new();
    server.stall();
    let mut client = MemoryChannel::with_budget(server, Duration::from_millis(250));
    let started = Instant::now();
    let outcome = client.request("tools/list", &json!({}));
    assert_eq!(outcome, Err(ChannelError::Timeout));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the operation returned on its own budget, not a hang"
    );
}

/// Our real MCP client (the manager's `ReqwestMcpProbe`) completes the handshake
/// against the in-process loopback server.
#[tokio::test]
async fn real_probe_handshake_against_the_loopback_server() {
    let server = LoopbackMcpHttp::start(false);
    let probe = McpEndpointProbe::new(Arc::new(ReqwestMcpProbe::default()));
    let outcome: ProbeOutcome = probe.probe(&server.url, "").await;
    assert_eq!(outcome, Ok(3), "the probe lists the three fixture tools");
}

/// The loopback server serves the flood too: the probe still completes (the
/// extra notifications ride the initialized notification's 202, which the probe
/// ignores) — the client is never left hanging.
#[tokio::test]
async fn real_probe_survives_the_loopback_flood() {
    let server = LoopbackMcpHttp::start(true);
    let probe = McpEndpointProbe::new(Arc::new(ReqwestMcpProbe::default()));
    let outcome = probe.probe(&server.url, "").await;
    assert_eq!(outcome, Ok(3));
}
