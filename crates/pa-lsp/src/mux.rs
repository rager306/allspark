//! L14 — shared per-project LSP mux over a Unix socket.
//!
//! FORK-ONLY (allspark). One language server per project is enough: two
//! sessions working in the same project should share one `rust-analyzer`, not
//! spawn two and fight over the same index. The reference implementation
//! brokers this over a Unix socket keyed by a *server identity* — a hash of the
//! exact spawn parameters — so any process that computes the same identity
//! finds the same server.
//!
//! [`MuxIdentity`] is that key: `sha256(command, args, cwd, sorted env)`. Every
//! component that changes what the server *is* goes into the hash, and the
//! environment is sorted and hashed rather than stored, so credentials in the
//! environment never appear in a socket path.
//!
//! [`LspMux::daemon`] binds the identity's socket and serves requests through a
//! [`Handler`]; [`LspMux::acquire`] connects to it. A second `daemon` for the
//! same identity fails with [`MuxError::AlreadyRunning`] instead of stealing the
//! socket, which is what makes "one server per project" hold. The wire protocol
//! is a 4-byte big-endian length prefix followed by a JSON frame.
//!
//! ```no_run
//! # async fn run() -> Result<(), pa_lsp::mux::MuxError> {
//! use pa_lsp::mux::{LspMux, MuxIdentity, MuxResponse};
//! use std::collections::BTreeMap;
//!
//! let mux = LspMux::new("/tmp/lsp-mux");
//! let identity = MuxIdentity::new("rust-analyzer", &[], "/work/project", &BTreeMap::new());
//! let daemon = mux.daemon(&identity, std::sync::Arc::new(|req| {
//!     MuxResponse::ok(req.id, serde_json::json!({"echo": req.method}))
//! })).await?;
//!
//! let mut conn = mux.acquire(&identity).await?;
//! let response = conn.call("initialize", serde_json::json!({})).await?;
//! assert!(response.result.is_some());
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// Largest frame the mux will read, so a hostile peer cannot exhaust memory.
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// Errors from the mux daemon and its connections.
#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    /// A daemon already serves this identity's socket.
    #[error("a mux daemon is already running for this identity")]
    AlreadyRunning,
    /// No daemon is listening on the identity's socket.
    #[error("no mux daemon is listening for this identity")]
    NotRunning,
    /// A frame was larger than [`MAX_FRAME_BYTES`].
    #[error("mux frame exceeds the {MAX_FRAME_BYTES}-byte limit")]
    FrameTooLarge,
    /// A frame was not valid JSON for the mux protocol.
    #[error("mux protocol error: {0}")]
    Protocol(String),
    /// The connection closed before a response arrived.
    #[error("mux connection closed before a response")]
    Closed,
    /// A filesystem operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A frame could not be (de)serialized.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convenience alias for [`MuxError`].
pub type MuxResult<T> = std::result::Result<T, MuxError>;

/// A server identity: `sha256(command, args, cwd, sorted env)`.
///
/// Two spawn configurations produce the same identity — and so share one
/// server — only when the command, the ordered arguments, the working
/// directory, and the *set* of environment variables all match. The
/// environment is sorted before hashing, so its order does not matter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MuxIdentity {
    hash: String,
}

impl MuxIdentity {
    /// Compute the identity of a server spawn.
    #[must_use]
    pub fn new(
        command: &str,
        args: &[String],
        cwd: impl AsRef<Path>,
        env: &BTreeMap<String, String>,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"pa-lsp-mux-identity-v1\0");
        hash_component(&mut hasher, command.as_bytes());
        hasher.update((args.len() as u64).to_be_bytes());
        for arg in args {
            hash_component(&mut hasher, arg.as_bytes());
        }
        let cwd = cwd.as_ref().to_string_lossy();
        hash_component(&mut hasher, cwd.as_bytes());
        // `BTreeMap` iterates in key order, so the environment is already
        // canonical: the hash does not depend on the map's insertion order.
        hasher.update((env.len() as u64).to_be_bytes());
        for (key, value) in env {
            hash_component(&mut hasher, key.as_bytes());
            hash_component(&mut hasher, value.as_bytes());
        }
        let digest = hasher.finalize();
        let mut hash = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(hash, "{byte:02x}");
        }
        Self { hash }
    }

    /// Convenience constructor from `&str` arguments.
    #[must_use]
    pub fn from_strs(
        command: &str,
        args: &[&str],
        cwd: impl AsRef<Path>,
        env: &BTreeMap<String, String>,
    ) -> Self {
        let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        Self::new(command, &owned, cwd, env)
    }

    /// The lowercase-hex identity hash.
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// The socket file name for this identity (hash plus `.sock`).
    #[must_use]
    pub fn socket_name(&self) -> String {
        format!("{}.sock", self.hash)
    }
}

fn hash_component(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

/// One request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MuxRequest {
    /// Monotonic request id, unique within a connection.
    pub id: u64,
    /// The method name (an LSP method, or a mux control method).
    pub method: String,
    /// The method parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// One response frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MuxResponse {
    /// The id of the request this answers.
    pub id: u64,
    /// The result, on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// The error message, on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MuxResponse {
    /// A successful response.
    #[must_use]
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failed response.
    #[must_use]
    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(error.into()),
        }
    }

    /// Whether the response carries a result rather than an error.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// The daemon's request handler. Runs for every incoming request, possibly
/// concurrently across connections.
pub type Handler = Arc<dyn Fn(&MuxRequest) -> MuxResponse + Send + Sync + 'static>;

/// A running mux daemon. Dropping it aborts the accept loop and unlinks the
/// socket.
pub struct MuxDaemon {
    socket: PathBuf,
    join: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for MuxDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MuxDaemon")
            .field("socket", &self.socket)
            .finish_non_exhaustive()
    }
}

impl MuxDaemon {
    /// The socket path the daemon is listening on.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// Stop the daemon: abort the accept loop and remove the socket.
    pub fn shutdown(self) {
        self.join.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

impl Drop for MuxDaemon {
    fn drop(&mut self) {
        self.join.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// A client connection to a mux daemon.
#[derive(Debug)]
pub struct Connection {
    stream: UnixStream,
    next_id: u64,
}

impl Connection {
    /// Send `method`/`params` and await the matching response.
    ///
    /// # Errors
    /// Returns [`MuxError::Closed`] if the daemon closes first, and propagates
    /// framing and I/O errors.
    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> MuxResult<MuxResponse> {
        self.next_id += 1;
        let request = MuxRequest {
            id: self.next_id,
            method: method.to_string(),
            params,
        };
        write_frame(&mut self.stream, &request).await?;
        let response: MuxResponse = read_frame(&mut self.stream)
            .await?
            .ok_or(MuxError::Closed)?;
        Ok(response)
    }

    /// A liveness probe against the daemon.
    ///
    /// # Errors
    /// Propagates the same errors as [`Connection::call`].
    pub async fn ping(&mut self) -> MuxResult<()> {
        let response = self.call("mux/ping", serde_json::Value::Null).await?;
        if response.is_ok() {
            Ok(())
        } else {
            Err(MuxError::Protocol(
                response.error.unwrap_or_else(|| "ping failed".to_string()),
            ))
        }
    }
}

/// A directory-backed factory for per-project mux daemons.
#[derive(Debug, Clone)]
pub struct LspMux {
    base: PathBuf,
}

impl LspMux {
    /// A mux whose sockets live under `base`.
    #[must_use]
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// The base directory holding every identity's socket.
    #[must_use]
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// The socket path for `identity`.
    #[must_use]
    pub fn socket_path(&self, identity: &MuxIdentity) -> PathBuf {
        self.base.join(identity.socket_name())
    }

    /// Start a daemon for `identity`, serving requests through `handler`.
    ///
    /// # Errors
    /// Returns [`MuxError::AlreadyRunning`] when a live daemon already serves
    /// this identity — the caller should [`LspMux::acquire`] it instead — and
    /// propagates bind failures.
    pub async fn daemon(&self, identity: &MuxIdentity, handler: Handler) -> MuxResult<MuxDaemon> {
        tokio::fs::create_dir_all(&self.base).await?;
        let socket = self.socket_path(identity);
        // A socket file that already answers a connect belongs to a live
        // daemon; do not steal it. A stale file (no answer) is unlinked below.
        if socket.exists() && UnixStream::connect(&socket).await.is_ok() {
            return Err(MuxError::AlreadyRunning);
        }
        let _ = tokio::fs::remove_file(&socket).await;
        let listener = UnixListener::bind(&socket)?;
        let join = tokio::spawn(accept_loop(listener, handler));
        Ok(MuxDaemon { socket, join })
    }

    /// Connect to the daemon serving `identity`.
    ///
    /// # Errors
    /// Returns [`MuxError::NotRunning`] when nothing is listening.
    pub async fn acquire(&self, identity: &MuxIdentity) -> MuxResult<Connection> {
        let socket = self.socket_path(identity);
        match UnixStream::connect(&socket).await {
            Ok(stream) => Ok(Connection { stream, next_id: 0 }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(MuxError::NotRunning),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                Err(MuxError::NotRunning)
            }
            Err(error) => Err(MuxError::Io(error)),
        }
    }
}

async fn accept_loop(listener: UnixListener, handler: Handler) {
    loop {
        // A failed accept on a bound listener is not fatal; keep serving.
        if let Ok((stream, _addr)) = listener.accept().await {
            let handler = Arc::clone(&handler);
            tokio::spawn(serve_connection(stream, handler));
        }
    }
}

async fn serve_connection(mut stream: UnixStream, handler: Handler) {
    loop {
        // A clean close or a framing error both end the connection.
        let Ok(Some(request)) = read_frame::<MuxRequest>(&mut stream).await else {
            return;
        };
        let response = if request.method == "mux/ping" {
            MuxResponse::ok(request.id, serde_json::json!("pong"))
        } else {
            handler(&request)
        };
        if write_frame(&mut stream, &response).await.is_err() {
            return;
        }
    }
}

/// Write a length-prefixed JSON frame.
async fn write_frame<T: Serialize>(stream: &mut UnixStream, value: &T) -> MuxResult<()> {
    let body = serde_json::to_vec(value)?;
    let len = u32::try_from(body.len()).map_err(|_| MuxError::FrameTooLarge)?;
    if len > MAX_FRAME_BYTES {
        return Err(MuxError::FrameTooLarge);
    }
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}

/// Read a length-prefixed JSON frame, or `None` on a clean end of stream.
async fn read_frame<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> MuxResult<Option<T>> {
    let mut len_bytes = [0u8; 4];
    match stream.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(MuxError::Io(error)),
    }
    let len = u32::from_be_bytes(len_bytes);
    if len > MAX_FRAME_BYTES {
        return Err(MuxError::FrameTooLarge);
    }
    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body).await?;
    let value = serde_json::from_slice(&body)?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn identity_is_deterministic() {
        let a = MuxIdentity::from_strs("rust-analyzer", &[], "/work", &env(&[("A", "1")]));
        let b = MuxIdentity::from_strs("rust-analyzer", &[], "/work", &env(&[("A", "1")]));
        assert_eq!(a, b);
        assert_eq!(a.hash().len(), 64);
        assert_eq!(
            std::path::Path::new(&a.socket_name()).extension(),
            Some(std::ffi::OsStr::new("sock"))
        );
    }

    #[test]
    fn identity_ignores_environment_order() {
        let a = MuxIdentity::new(
            "gopls",
            &["serve".to_string()],
            "/w",
            &env(&[("Z", "1"), ("A", "2")]),
        );
        // A `BTreeMap` is always in key order, so the two maps are equal; the
        // point is that construction order cannot leak into the hash.
        let b = MuxIdentity::new(
            "gopls",
            &["serve".to_string()],
            "/w",
            &env(&[("A", "2"), ("Z", "1")]),
        );
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn identity_changes_with_every_component() {
        let base = MuxIdentity::from_strs("cmd", &["a"], "/w", &env(&[("K", "V")]));
        assert_ne!(
            base,
            MuxIdentity::from_strs("other", &["a"], "/w", &env(&[("K", "V")]))
        );
        assert_ne!(
            base,
            MuxIdentity::from_strs("cmd", &["b"], "/w", &env(&[("K", "V")]))
        );
        assert_ne!(
            base,
            MuxIdentity::from_strs("cmd", &["a", "b"], "/w", &env(&[("K", "V")]))
        );
        assert_ne!(
            base,
            MuxIdentity::from_strs("cmd", &["a"], "/other", &env(&[("K", "V")]))
        );
        assert_ne!(
            base,
            MuxIdentity::from_strs("cmd", &["a"], "/w", &env(&[("K", "OTHER")]))
        );
        assert_ne!(
            base,
            MuxIdentity::from_strs("cmd", &["a"], "/w", &env(&[("OTHER", "V")]))
        );
    }

    #[test]
    fn argument_order_matters_but_is_not_confused_with_separators() {
        // "a b" as one argument must not hash the same as "a","b" as two.
        let one = MuxIdentity::from_strs("cmd", &["a b"], "/w", &env(&[]));
        let two = MuxIdentity::from_strs("cmd", &["a", "b"], "/w", &env(&[]));
        assert_ne!(one, two);
    }

    #[test]
    fn socket_path_is_stable_and_under_the_base() {
        let mux = LspMux::new("/tmp/mux-base");
        let id = MuxIdentity::from_strs("cmd", &[], "/w", &env(&[]));
        let path = mux.socket_path(&id);
        assert!(path.starts_with("/tmp/mux-base"));
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            id.socket_name()
        );
    }

    #[tokio::test]
    async fn daemon_and_acquire_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mux = LspMux::new(dir.path());
        let id = MuxIdentity::from_strs("rust-analyzer", &[], "/work", &env(&[]));
        let _daemon = mux
            .daemon(
                &id,
                Arc::new(|req: &MuxRequest| {
                    MuxResponse::ok(req.id, serde_json::json!({"method": req.method}))
                }),
            )
            .await
            .expect("daemon");

        let mut conn = mux.acquire(&id).await.expect("acquire");
        conn.ping().await.expect("ping");
        let response = conn
            .call("textDocument/hover", serde_json::json!({"line": 1}))
            .await
            .expect("call");
        assert!(response.is_ok());
        assert_eq!(
            response.result,
            Some(serde_json::json!({"method": "textDocument/hover"}))
        );
    }

    #[tokio::test]
    async fn one_daemon_serves_many_connections() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mux = LspMux::new(dir.path());
        let id = MuxIdentity::from_strs("gopls", &["serve"], "/work", &env(&[]));
        let _daemon = mux
            .daemon(
                &id,
                Arc::new(|req: &MuxRequest| MuxResponse::ok(req.id, serde_json::json!(req.id))),
            )
            .await
            .expect("daemon");

        // Two independent "sessions" acquire the same server.
        let mut a = mux.acquire(&id).await.expect("acquire a");
        let mut b = mux.acquire(&id).await.expect("acquire b");
        let ra = a.call("m", serde_json::Value::Null).await.expect("a");
        let rb = b.call("m", serde_json::Value::Null).await.expect("b");
        assert_eq!(ra.result, Some(serde_json::json!(1)));
        assert_eq!(
            rb.result,
            Some(serde_json::json!(1)),
            "each connection numbers from 1"
        );
    }

    #[tokio::test]
    async fn a_second_daemon_for_the_same_identity_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mux = LspMux::new(dir.path());
        let id = MuxIdentity::from_strs("cmd", &[], "/w", &env(&[]));
        let _first = mux
            .daemon(
                &id,
                Arc::new(|req: &MuxRequest| MuxResponse::ok(req.id, serde_json::Value::Null)),
            )
            .await
            .expect("first");
        let second = mux
            .daemon(
                &id,
                Arc::new(|req: &MuxRequest| MuxResponse::ok(req.id, serde_json::Value::Null)),
            )
            .await;
        assert!(matches!(second, Err(MuxError::AlreadyRunning)));
    }

    #[tokio::test]
    async fn acquire_without_a_daemon_reports_not_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mux = LspMux::new(dir.path());
        let id = MuxIdentity::from_strs("cmd", &[], "/w", &env(&[]));
        assert!(matches!(mux.acquire(&id).await, Err(MuxError::NotRunning)));
    }

    #[tokio::test]
    async fn handler_errors_propagate_as_failed_responses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mux = LspMux::new(dir.path());
        let id = MuxIdentity::from_strs("cmd", &[], "/w", &env(&[]));
        let _daemon = mux
            .daemon(
                &id,
                Arc::new(|req: &MuxRequest| {
                    if req.method == "boom" {
                        MuxResponse::err(req.id, "boom")
                    } else {
                        MuxResponse::ok(req.id, serde_json::Value::Null)
                    }
                }),
            )
            .await
            .expect("daemon");
        let mut conn = mux.acquire(&id).await.expect("acquire");
        let response = conn
            .call("boom", serde_json::Value::Null)
            .await
            .expect("call");
        assert!(!response.is_ok());
        assert_eq!(response.error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn dropping_the_daemon_removes_the_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mux = LspMux::new(dir.path());
        let id = MuxIdentity::from_strs("cmd", &[], "/w", &env(&[]));
        let socket = mux.socket_path(&id);
        let daemon = mux
            .daemon(
                &id,
                Arc::new(|req: &MuxRequest| MuxResponse::ok(req.id, serde_json::Value::Null)),
            )
            .await
            .expect("daemon");
        assert!(socket.exists());
        daemon.shutdown();
        assert!(!socket.exists());
    }
}
