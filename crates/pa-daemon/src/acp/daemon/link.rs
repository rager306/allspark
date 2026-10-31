//! The supervisor link: the socket connection, the frame reader, and the
//! request/response exchange matched by id.
//!
//! One writer task owns the socket's write half, one reader task classifies
//! every inbound frame into [`LinkFrame`], and a consumer loop (in the
//! parent module) owns the ordering, so a turn's chunks always precede its
//! boundary frames.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use pa_types::daemon::{
    DaemonCommand, DaemonCommandEnvelope, DaemonCommandFrameType, DaemonProtocolInfo,
    DaemonResponse, DAEMON_PROTOCOL_NAME, DAEMON_PROTOCOL_VERSION,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};

/// Response timeout for session-scoped commands.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) enum LinkFrame {
    Event(Value),
    Response(DaemonResponse),
    /// The daemon-global heartbeat-catalog broadcast the supervisor
    /// re-broadcasts to every client (`heartbeats_changed`).
    HeartbeatsChanged,
}

/// A client connection to the supervisor socket: JSONL command envelopes
/// out, responses matched by id, session events forwarded raw.
pub(crate) struct DaemonLink {
    pub(super) writer: mpsc::UnboundedSender<String>,
    pub(super) pending: Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<DaemonResponse>>>>,
    /// Set once the frame channel ends (the socket closed and every
    /// frame the reader read is delivered): every new request fails
    /// fast instead of waiting for a response no one will send.
    pub(super) closed: Arc<std::sync::atomic::AtomicBool>,
    pub(super) frames: Mutex<mpsc::UnboundedReceiver<LinkFrame>>,
    pub(super) protocol_version: u64,
    pub(super) next_request_id: std::sync::atomic::AtomicU64,
}

impl DaemonLink {
    pub(super) async fn connect(socket_path: &Path) -> anyhow::Result<Self> {
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            pa_types::platform::transport::connect_transport(socket_path),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timed out connecting to the daemon socket"))??;
        let (reader_half, writer_half) = stream.split();
        let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<LinkFrame>();
        let (hello_tx, hello_rx) = oneshot::channel::<Value>();
        let pending: Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<DaemonResponse>>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let closed: Arc<std::sync::atomic::AtomicBool> =
            Arc::new(std::sync::atomic::AtomicBool::new(false));

        tokio::spawn(async move {
            let mut writer = writer_half;
            while let Some(line) = line_rx.recv().await {
                let mut payload = line.into_bytes();
                payload.push(b'\n');
                if writer.write_all(&payload).await.is_err() {
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });
        // The reader only classifies frames; the consumer loop below owns
        // the ordering, so a turn's chunks always precede its boundary
        // frames.
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader_half);
            let mut line = String::new();
            let mut hello_tx = Some(hello_tx);
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                match value
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "daemon_hello" => {
                        if let Some(tx) = hello_tx.take() {
                            let _ = tx.send(value);
                        }
                    }
                    "response" => {
                        let Ok(response) = serde_json::from_value::<DaemonResponse>(value.clone())
                        else {
                            continue;
                        };
                        let _ = frame_tx.send(LinkFrame::Response(response));
                    }
                    "session_event" => {
                        let _ = frame_tx.send(LinkFrame::Event(value));
                    }
                    "heartbeats_changed" => {
                        let _ = frame_tx.send(LinkFrame::HeartbeatsChanged);
                    }
                    _ => {}
                }
            }
        });

        let hello = tokio::time::timeout(std::time::Duration::from_secs(3), hello_rx)
            .await
            .map_err(|_| anyhow::anyhow!("the daemon did not send its handshake"))?
            .map_err(|_| anyhow::anyhow!("the daemon connection closed before the handshake"))?;
        let protocol = hello
            .get("protocol")
            .cloned()
            .and_then(|p| serde_json::from_value::<DaemonProtocolInfo>(p).ok())
            .unwrap_or(DaemonProtocolInfo {
                name: DAEMON_PROTOCOL_NAME.to_string(),
                version: DAEMON_PROTOCOL_VERSION,
            });
        let version = protocol.version.min(DAEMON_PROTOCOL_VERSION);
        Ok(DaemonLink {
            writer: line_tx,
            pending,
            closed,
            frames: Mutex::new(frame_rx),
            protocol_version: version,
            next_request_id: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Send one command envelope and wait for the matching response
    /// under the session-scoped response timeout.
    pub(crate) async fn request(&self, command: DaemonCommand) -> anyhow::Result<DaemonResponse> {
        self.exchange(command, Some(REQUEST_TIMEOUT)).await
    }

    pub(crate) async fn request_ok(&self, command: DaemonCommand) -> anyhow::Result<()> {
        let response = self.request(command).await?;
        if !response.success {
            anyhow::bail!(response
                .error
                .unwrap_or_else(|| "unknown error".to_string()));
        }
        Ok(())
    }

    /// Send one command envelope with no fixed cap: the response or the
    /// link close ends the wait (turn-long commands — declared
    /// divergence, TS caps them at 24 h; the link-close signal is the
    /// liveness bound here, not a timer).
    pub(super) async fn request_until_close(
        &self,
        command: DaemonCommand,
    ) -> anyhow::Result<DaemonResponse> {
        self.exchange(command, None).await
    }

    /// One exchange: register the response slot, send the envelope, and
    /// wait for the answer.
    async fn exchange(
        &self,
        command: DaemonCommand,
        timeout: Option<std::time::Duration>,
    ) -> anyhow::Result<DaemonResponse> {
        use std::sync::atomic::Ordering;
        let id = format!(
            "acp-{}",
            self.next_request_id.fetch_add(1, Ordering::SeqCst) + 1
        );
        let envelope = DaemonCommandEnvelope {
            frame_type: DaemonCommandFrameType::Command,
            id: id.clone(),
            protocol: DaemonProtocolInfo {
                name: DAEMON_PROTOCOL_NAME.to_string(),
                version: self.protocol_version,
            },
            client_id: Some(format!("acp:{}", std::process::id())),
            command,
        };
        let line = serde_json::to_string(&envelope)?;
        let (tx, rx) = oneshot::channel::<DaemonResponse>();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        // A request that races the close loses either way: the flag
        // fails it here, or the pending clear already dropped its
        // sender. The flag is stored before the clear, so no request
        // parks unnoticed.
        if self.closed.load(Ordering::SeqCst) {
            self.pending.lock().unwrap().remove(&id);
            anyhow::bail!("the daemon connection is closed");
        }
        if self.writer.send(line).is_err() {
            // A closed writer leaves the pending slot behind otherwise; a
            // link that never answers again would grow one entry per
            // request.
            self.pending.lock().unwrap().remove(&id);
            anyhow::bail!("the daemon connection is closed");
        }
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, rx).await.map_err(|_| {
                self.pending.lock().unwrap().remove(&id);
                anyhow::anyhow!("timed out waiting for the daemon response")
            })?,
            None => rx.await,
        }
        .map_err(|_| anyhow::anyhow!("the daemon connection closed mid-request"))
    }
}
