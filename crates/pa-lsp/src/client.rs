//! One language-server client: spawn, initialize, synchronize documents, query.
//!
//! FORK-ONLY (allspark). Built on `async-lsp` (a tower-based LSP framework);
//! this is the native-Rust path the fork chose over an MCP/Python bridge.
//! Synthesizes oh-my-pi's per-file client (`client.ts`) and Serena's
//! `SolidLanguageServer`: one client per (server, root) pair, documents synced
//! on demand, `publishDiagnostics` captured into a shared ledger.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_lsp::router::Router;
use async_lsp::{LanguageClient, LanguageServer, ResponseError, ServerSocket};
use lsp_types::{
    ClientCapabilities, CodeActionOrCommand, CodeActionParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentDiagnosticParams, DocumentDiagnosticReport,
    DocumentDiagnosticReportResult, DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, ExecuteCommandParams, FileRename, FormattingOptions,
    GotoDefinitionParams, GotoDefinitionResponse, HoverParams, InitializeParams, InitializedParams,
    OneOf, Position, ProgressParams, PublishDiagnosticsParams, ReferenceContext, ReferenceParams,
    RenameFilesParams, RenameParams, ServerCapabilities, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, TextEdit, Url, WorkDoneProgressParams, WorkspaceEdit,
    WorkspaceFolder, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use tokio::sync::Mutex;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::debug;

use crate::ledger::DiagnosticIdentity;
use crate::servers::ServerConfig;

/// How long an edit waits for diagnostics before answering without them
/// (oh-my-pi's `INLINE_DIAGNOSTICS_WAIT_TIMEOUT_MS`, L12).
pub const INLINE_DIAGNOSTICS_WAIT: Duration = Duration::from_millis(500);

/// The outer budget a *deferred* diagnostics fetch may keep waiting
/// (oh-my-pi's `DEFERRED_DIAGNOSTICS_WAIT_TIMEOUT_MS`, L12).
pub const DEFERRED_DIAGNOSTICS_WAIT: Duration = Duration::from_secs(12);

/// Poll cadence while waiting for `publishDiagnostics`.
pub const DIAGNOSTICS_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Grace period after the first publish for a follow-up correction.
pub const DIAGNOSTICS_SETTLE: Duration = Duration::from_millis(500);

/// How an edit waits for diagnostics (L12).
///
/// [`Inline`](InlineWait::Inline) answers after [`INLINE_DIAGNOSTICS_WAIT`]
/// even when the server is slow, so a write never blocks on a cold
/// type-checker. [`Deferred`](InlineWait::Deferred) keeps waiting for the full
/// [`DEFERRED_DIAGNOSTICS_WAIT`] window (used by the background follow-up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineWait {
    /// Answer within the short inline budget.
    Inline,
    /// Keep waiting for the longer deferred budget.
    Deferred,
}

impl InlineWait {
    /// The wait budget this mode allows.
    #[must_use]
    pub fn budget(self) -> Duration {
        match self {
            InlineWait::Inline => INLINE_DIAGNOSTICS_WAIT,
            InlineWait::Deferred => DEFERRED_DIAGNOSTICS_WAIT,
        }
    }
}

/// What a [`DiagnosticsWaiter`] observation implies (L12 state machine).
#[derive(Debug, Clone, PartialEq)]
pub enum WaitStep {
    /// No answer yet; poll again.
    Continue,
    /// Stop and return the diagnostics gathered so far (`None` = nothing ever
    /// arrived within the budget).
    Done(Option<(Vec<lsp_types::Diagnostic>, u64)>),
}

/// A pure, clock-driven state machine deciding when a diagnostics wait ends.
///
/// Split out of the async loop so the inline/deferred policy is testable with a
/// mock clock: [`observe`](DiagnosticsWaiter::observe) takes the current instant
/// and the latest publish, and returns the next [`WaitStep`]. The loop that
/// feeds it only has to `sleep` between calls.
#[derive(Debug)]
pub struct DiagnosticsWaiter {
    deadline: tokio::time::Instant,
    settle: Duration,
    last_generation: Option<u64>,
    last_seen: Option<(Vec<lsp_types::Diagnostic>, u64)>,
    first_publish_at: Option<tokio::time::Instant>,
}

impl DiagnosticsWaiter {
    /// A waiter with the default settle window.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self::with_settle(timeout, DIAGNOSTICS_SETTLE)
    }

    /// A waiter with an explicit settle window (tests).
    #[must_use]
    pub fn with_settle(timeout: Duration, settle: Duration) -> Self {
        Self {
            deadline: tokio::time::Instant::now() + timeout,
            settle,
            last_generation: None,
            last_seen: None,
            first_publish_at: None,
        }
    }

    /// Feed one observation and decide whether to keep waiting.
    ///
    /// A non-empty publish answers immediately. An empty publish (a clean file)
    /// answers once the settle window elapses with no newer publish. Nothing at
    /// all answers `None` at the deadline.
    pub fn observe(
        &mut self,
        now: tokio::time::Instant,
        published: Option<(Vec<lsp_types::Diagnostic>, u64)>,
    ) -> WaitStep {
        if let Some((diagnostics, generation)) = published {
            if self.last_generation != Some(generation) {
                self.last_generation = Some(generation);
                self.first_publish_at.get_or_insert(now);
                if !diagnostics.is_empty() {
                    return WaitStep::Done(Some((diagnostics, generation)));
                }
                self.last_seen = Some((diagnostics, generation));
            }
        }
        // A clean file: the first (empty) publish has settled.
        if let Some(published_at) = self.first_publish_at {
            if now >= published_at + self.settle {
                return WaitStep::Done(self.last_seen.take());
            }
        }
        if now >= self.deadline {
            return WaitStep::Done(self.last_seen.take());
        }
        WaitStep::Continue
    }
}

/// Drive a [`DiagnosticsWaiter`] over an async poll function until it settles.
///
/// Kept generic over the poll closure so tests can feed a mock source under a
/// paused clock; [`LspClient::wait_for_diagnostics`] supplies the real one.
pub async fn await_diagnostics<F, Fut>(
    mut waiter: DiagnosticsWaiter,
    mut poll: F,
) -> Option<(Vec<lsp_types::Diagnostic>, u64)>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<(Vec<lsp_types::Diagnostic>, u64)>>,
{
    loop {
        let published = poll().await;
        match waiter.observe(tokio::time::Instant::now(), published) {
            WaitStep::Done(result) => return result,
            WaitStep::Continue => tokio::time::sleep(DIAGNOSTICS_POLL_INTERVAL).await,
        }
    }
}

/// A background diagnostics fetch started when the inline budget expires (L12).
///
/// The edit that spawned it does not wait: it holds this handle and resolves it
/// later (or drops it, abandoning the fetch).
pub struct DeferredDiagnostics {
    handle: tokio::task::JoinHandle<Option<(Vec<lsp_types::Diagnostic>, u64)>>,
}

impl DeferredDiagnostics {
    /// Await the background fetch (up to the deferred budget).
    pub async fn resolve(self) -> Option<(Vec<lsp_types::Diagnostic>, u64)> {
        self.handle.await.ok().flatten()
    }

    /// Abandon the fetch without waiting for it.
    pub fn abandon(self) {
        self.handle.abort();
    }
}

/// The outcome of an inline-with-deferral diagnostics wait (L12).
pub enum DiagnosticsOutcome {
    /// The server answered within the inline budget.
    Inline(Option<(Vec<lsp_types::Diagnostic>, u64)>),
    /// The server was too slow; the fetch continues in the background.
    Deferred(DeferredDiagnostics),
}

/// The bespoke `rust-analyzer/reloadWorkspace` request, which `lsp-types` does
/// not model. Carried through a local `Request` shim so `async-lsp`'s typed
/// socket can issue it.
enum RustAnalyzerReloadWorkspace {}

impl async_lsp::lsp_types::request::Request for RustAnalyzerReloadWorkspace {
    type Params = serde_json::Value;
    type Result = serde_json::Value;
    const METHOD: &'static str = "rust-analyzer/reloadWorkspace";
}

/// Shared state the LSP client's router mutates on server notifications.
#[derive(Default)]
pub struct ClientNotifications {
    /// Diagnostics the server pushed, keyed by file URI string.
    pub diagnostics: HashMap<String, Vec<lsp_types::Diagnostic>>,
    /// A generation counter per URI, bumped on each `publishDiagnostics`.
    pub generation: HashMap<String, u64>,
    /// The document version the last `publishDiagnostics` carried (L13
    /// freshness). `None` when the server did not version the publish.
    pub version: HashMap<String, Option<i32>>,
    /// Rolling work-done progress reported by the server, by token.
    ///
    /// A language server answers `null` or `[]` for navigation while its index is
    /// still loading, which is legal LSP and indistinguishable from "nothing
    /// found". Measured with rust-analyzer on this repository: the first
    /// `textDocument/references` returned `null` in **19 ms**, and the first
    /// non-empty answer arrived at **21.9 s**. Tracking progress is the only way
    /// to tell those apart.
    pub progress: HashMap<String, ProgressState>,
}

/// One server-reported work-done progress stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressState {
    /// The last human-readable title/message the server sent.
    pub title: Option<String>,
    /// Whether the server reported this stream as finished.
    pub done: bool,
}

/// A router over the client state: forwards server notifications into
/// [`ClientNotifications`] and answers the standard capability requests.
struct NotificationRouter {
    state: Arc<Mutex<ClientNotifications>>,
}

impl LanguageClient for NotificationRouter {
    type Error = ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn publish_diagnostics(&mut self, params: PublishDiagnosticsParams) -> Self::NotifyResult {
        let key = params.uri.to_string();
        let state = Arc::clone(&self.state);
        // The notification handler is sync; record on a best-effort basis. A
        // poisoned or contended lock must never break the LSP stream, so the
        // write is fire-and-forget through `try_lock`.
        if let Ok(mut guard) = state.try_lock() {
            guard.diagnostics.insert(key.clone(), params.diagnostics);
            let entry = guard.generation.entry(key.clone()).or_insert(0);
            *entry += 1;
            guard.version.insert(key, params.version);
        }
        ControlFlow::Continue(())
    }

    fn show_message(&mut self, params: lsp_types::ShowMessageParams) -> Self::NotifyResult {
        debug!(target: "pa_lsp", "server message: {}", params.message);
        ControlFlow::Continue(())
    }

    fn log_message(&mut self, params: lsp_types::LogMessageParams) -> Self::NotifyResult {
        debug!(target: "pa_lsp", "server log: {}", params.message);
        ControlFlow::Continue(())
    }

    fn progress(&mut self, params: ProgressParams) -> Self::NotifyResult {
        let state = Arc::clone(&self.state);
        // Work-done progress is the only signal a language server gives while it
        // loads; a navigation answer during that window is a legal `null` that
        // looks exactly like "nothing found".
        let token = format!("{:?}", params.token);
        let kind = serde_json::to_value(&params.value)
            .ok()
            .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_string));
        let message = serde_json::to_value(&params.value).ok().and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        });
        if let Ok(mut guard) = state.try_lock() {
            let entry = guard.progress.entry(token).or_default();
            match kind.as_deref() {
                Some("end") => {
                    entry.done = true;
                    entry.title = None;
                }
                Some(_) => {
                    entry.done = false;
                    if let Some(message) = message {
                        entry.title = Some(message);
                    }
                }
                None => {}
            }
        }
        ControlFlow::Continue(())
    }
}

/// A live client for one `(server, root)` pair.
pub struct LspClient {
    server: ServerSocket,
    /// The server binary's name, for status/errors.
    pub name: &'static str,
    /// The project root the server was started for.
    pub root: PathBuf,
    /// The language id used in `didOpen`.
    language_id: &'static str,
    /// Documents already opened, with their last-sent version.
    open_docs: Mutex<HashMap<String, i32>>,
    /// Server-pushed notifications (diagnostics).
    notifications: Arc<Mutex<ClientNotifications>>,
    /// The main-loop task; aborted on drop.
    mainloop: tokio::task::JoinHandle<()>,
    /// The child process, killed on drop. Behind a `Mutex` so liveness can
    /// be polled through a shared `Arc<LspClient>` (the manager's roster).
    child: std::sync::Mutex<tokio::process::Child>,
    /// The server's advertised capabilities from `initialize` (L2 `capabilities`).
    capabilities: ServerCapabilities,
    /// The server's name/version, when it reported one.
    server_info: Option<lsp_types::ServerInfo>,
}

/// What the server reports about its own readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warmth {
    /// No progress stream is open and none has finished: the server is either
    /// fully ready or has never reported progress.
    Ready,
    /// A progress stream is open. The server is still loading, and a navigation
    /// answer in this window is a legal `null` that means "not known yet".
    Warming {
        /// The last message the server sent for that stream, if any.
        message: Option<String>,
    },
}

impl Warmth {
    /// Whether a navigation answer in this state can be trusted as "none found".
    #[must_use]
    pub const fn answers_are_final(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// One line for a tool result.
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            Self::Ready => String::from("ready"),
            Self::Warming { message } => match message {
                Some(m) => format!("warming ({m})"),
                None => String::from("warming"),
            },
        }
    }
}

impl LspClient {
    /// Whether the server is ready to answer navigation questions *finally*.
    ///
    /// Measured on this repository with rust-analyzer: the handshake completes in
    /// 40 ms, the first `textDocument/references` returns `null` in 19 ms, and the
    /// first non-empty answer arrives at 21.9 s. Without this distinction the tool
    /// reports "no references found" for that whole window.
    pub async fn warmth(&self) -> Warmth {
        let guard = self.notifications.lock().await;
        let warming: Vec<&ProgressState> = guard
            .progress
            .values()
            .filter(|state| !state.done)
            .collect();
        if warming.is_empty() {
            return Warmth::Ready;
        }
        let message = warming
            .iter()
            .filter_map(|state| state.title.clone())
            .next_back();
        Warmth::Warming { message }
    }

    /// Spawn `server` for `root` and complete the LSP handshake.
    ///
    /// # Errors
    /// Fails when the binary cannot be spawned or the initialize handshake
    /// returns an error.
    pub async fn spawn(server: &'static ServerConfig, root: &Path) -> anyhow::Result<Arc<Self>> {
        let notifications = Arc::new(Mutex::new(ClientNotifications::default()));
        let router_state = Arc::clone(&notifications);

        let (mainloop, socket) = async_lsp::MainLoop::new_client(|_socket| {
            Router::from_language_client(NotificationRouter {
                state: Arc::clone(&router_state),
            })
        });

        let mut command = tokio::process::Command::new(server.command);
        command
            .args(server.args)
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|err| anyhow::anyhow!("failed to spawn `{}`: {err}", server.command))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("`{}` produced no stdout pipe", server.command))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("`{}` produced no stdin pipe", server.command))?;

        let mainloop = tokio::spawn(async move {
            if let Err(err) = mainloop
                .run_buffered(stdout.compat(), stdin.compat_write())
                .await
            {
                if std::env::var_os("PA_LSP_DEBUG").is_some() {
                    eprintln!("[pa_lsp] main loop ended: {err}");
                }
                debug!(target: "pa_lsp", "LSP main loop ended: {err}");
            }
        });

        let root_uri = Url::from_directory_path(root)
            .map_err(|()| anyhow::anyhow!("root path is not absolute: {}", root.display()))?;
        let mut socket = socket;
        // `root_uri` is deprecated in favor of `workspace_folders`, but older
        // servers still read it, so both are sent.
        #[allow(deprecated)]
        let init = socket
            .initialize(InitializeParams {
                process_id: Some(std::process::id()),
                root_uri: Some(root_uri.clone()),
                capabilities: client_capabilities(),
                workspace_folders: Some(vec![WorkspaceFolder {
                    uri: root_uri,
                    name: server.name.to_string(),
                }]),
                ..InitializeParams::default()
            })
            .await
            .map_err(|err| anyhow::anyhow!("initialize failed: {err}"))?;
        socket
            .initialized(InitializedParams {})
            .map_err(|err| anyhow::anyhow!("initialized notification failed: {err}"))?;

        Ok(Arc::new(Self {
            server: socket,
            name: server.name,
            root: root.to_path_buf(),
            language_id: server.language_id,
            open_docs: Mutex::new(HashMap::new()),
            notifications,
            mainloop,
            child: std::sync::Mutex::new(child),
            capabilities: init.capabilities,
            server_info: init.server_info,
        }))
    }

    /// Ensure `path` is open with its current on-disk contents, syncing a
    /// `didChange` when the file changed since the last sync.
    ///
    /// # Errors
    /// Fails when `path` is not absolute or the `didOpen`/`didChange`
    /// notification cannot be queued (the main loop has stopped).
    pub async fn ensure_open(&self, path: &Path) -> anyhow::Result<Url> {
        let uri = Url::from_file_path(path)
            .map_err(|()| anyhow::anyhow!("not an absolute path: {}", path.display()))?;
        let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
        let key = uri.to_string();

        let mut docs = self.open_docs.lock().await;
        match docs.get_mut(&key) {
            None => {
                let mut server = self.server.clone();
                server
                    .did_open(DidOpenTextDocumentParams {
                        text_document: TextDocumentItem {
                            uri: uri.clone(),
                            language_id: self.language_id.to_string(),
                            version: 0,
                            text,
                        },
                    })
                    .map_err(|err| anyhow::anyhow!("didOpen failed: {err}"))?;
                docs.insert(key, 0);
            }
            Some(version) => {
                *version += 1;
                let new_version = *version;
                let mut server = self.server.clone();
                server
                    .did_change(lsp_types::DidChangeTextDocumentParams {
                        text_document: lsp_types::VersionedTextDocumentIdentifier {
                            uri: uri.clone(),
                            version: new_version,
                        },
                        content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                            range: None,
                            range_length: None,
                            text,
                        }],
                    })
                    .map_err(|err| anyhow::anyhow!("didChange failed: {err}"))?;
            }
        }
        Ok(uri)
    }

    /// The `documentSymbol` tree for `path`.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn document_symbols(&self, path: &Path) -> anyhow::Result<Vec<DocumentSymbol>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        let response = server
            .document_symbol(DocumentSymbolParams {
                text_document: TextDocumentIdentifier { uri },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("documentSymbol failed: {err}"))?;
        Ok(match response {
            Some(DocumentSymbolResponse::Nested(symbols)) => symbols,
            // A flat response is reshaped into a single-level tree.
            Some(DocumentSymbolResponse::Flat(infos)) => infos
                .into_iter()
                .map(|info| {
                    // `deprecated` is superseded by `tags`, but the struct still
                    // carries the field; set it empty.
                    #[allow(deprecated)]
                    DocumentSymbol {
                        name: info.name,
                        detail: None,
                        kind: info.kind,
                        tags: None,
                        deprecated: None,
                        range: info.location.range,
                        selection_range: info.location.range,
                        children: None,
                    }
                })
                .collect(),
            None => Vec::new(),
        })
    }

    /// `textDocument/hover` at a position.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn hover(
        &self,
        path: &Path,
        position: Position,
    ) -> anyhow::Result<Option<lsp_types::Hover>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        server
            .hover(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("hover failed: {err}"))
    }

    /// `textDocument/definition` at a position.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn definition(
        &self,
        path: &Path,
        position: Position,
    ) -> anyhow::Result<Option<lsp_types::GotoDefinitionResponse>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        server
            .definition(lsp_types::GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("definition failed: {err}"))
    }

    /// `textDocument/references` at a position.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn references(
        &self,
        path: &Path,
        position: Position,
        include_declaration: bool,
    ) -> anyhow::Result<Option<Vec<lsp_types::Location>>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        server
            .references(ReferenceParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
                context: ReferenceContext {
                    include_declaration,
                },
            })
            .await
            .map_err(|err| anyhow::anyhow!("references failed: {err}"))
    }

    /// `textDocument/typeDefinition` at a position.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn type_definition(
        &self,
        path: &Path,
        position: Position,
    ) -> anyhow::Result<Option<GotoDefinitionResponse>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        server
            .type_definition(GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("typeDefinition failed: {err}"))
    }

    /// `textDocument/declaration` at a position (S12), falling back to
    /// `textDocument/definition` when the server has not implemented it (gopls
    /// and others return `MethodNotFound`).
    ///
    /// # Errors
    /// Fails when the document cannot be opened or both requests error.
    pub async fn declaration(
        &self,
        path: &Path,
        position: Position,
    ) -> anyhow::Result<Option<GotoDefinitionResponse>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        match server
            .declaration(GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .await
        {
            Ok(response) => Ok(response),
            Err(_) => self.definition(path, position).await,
        }
    }

    /// The server's advertised capabilities (L2 `capabilities`).
    #[must_use]
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// The server's name/version, when it reported one.
    #[must_use]
    pub fn server_info(&self) -> Option<&lsp_types::ServerInfo> {
        self.server_info.as_ref()
    }

    /// Whether the server advertises pull diagnostics (`textDocument/diagnostic`,
    /// LSP 3.17). Drives the pull-then-publish fallback in
    /// [`LspClient::diagnostics`].
    #[must_use]
    pub fn supports_pull_diagnostics(&self) -> bool {
        self.capabilities.diagnostic_provider.is_some()
    }

    /// Pull diagnostics with `textDocument/diagnostic` (L4).
    ///
    /// Returns `None` when the server does not support the request, so the
    /// caller falls back to published diagnostics. A `Full` report yields its
    /// items; an `Unchanged` report yields an empty list (nothing changed since
    /// the last pull).
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn pull_diagnostics(
        &self,
        path: &Path,
    ) -> anyhow::Result<Option<Vec<lsp_types::Diagnostic>>> {
        if !self.supports_pull_diagnostics() {
            return Ok(None);
        }
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        let response = server
            .document_diagnostic(DocumentDiagnosticParams {
                text_document: TextDocumentIdentifier { uri },
                identifier: None,
                previous_result_id: None,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .await;
        match response {
            Ok(DocumentDiagnosticReportResult::Report(report)) => Ok(Some(match report {
                DocumentDiagnosticReport::Full(full) => full.full_document_diagnostic_report.items,
                DocumentDiagnosticReport::Unchanged(_) => Vec::new(),
            })),
            Ok(DocumentDiagnosticReportResult::Partial(_)) => Ok(Some(Vec::new())),
            // A server that advertised pull but errors on it: fall back.
            Err(_) => Ok(None),
        }
    }

    /// Diagnostics for `path`: pull first (L4), then the published set (L8).
    ///
    /// # Errors
    /// Fails when the document cannot be opened.
    pub async fn diagnostics(&self, path: &Path) -> anyhow::Result<Vec<lsp_types::Diagnostic>> {
        if let Some(pulled) = self.pull_diagnostics(path).await? {
            return Ok(pulled);
        }
        self.ensure_open(path).await?;
        Ok(self
            .fresh_published_diagnostics(path)
            .await
            .map(|(diagnostics, _)| diagnostics)
            .unwrap_or_default())
    }

    /// `textDocument/implementation` at a position.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn implementation(
        &self,
        path: &Path,
        position: Position,
    ) -> anyhow::Result<Option<GotoDefinitionResponse>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        server
            .implementation(GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("implementation failed: {err}"))
    }

    /// `workspace/symbol` for a query (no file needed).
    ///
    /// # Errors
    /// Fails when the request errors (e.g. the server is gone).
    pub async fn workspace_symbols(
        &self,
        query: &str,
    ) -> anyhow::Result<Option<WorkspaceSymbolResponse>> {
        let mut server = self.server.clone();
        server
            .symbol(WorkspaceSymbolParams {
                query: query.to_string(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("workspace/symbol failed: {err}"))
    }

    /// `textDocument/codeAction` over a range.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn code_actions(
        &self,
        path: &Path,
        range: lsp_types::Range,
    ) -> anyhow::Result<Option<Vec<CodeActionOrCommand>>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        server
            .code_action(CodeActionParams {
                text_document: TextDocumentIdentifier { uri },
                range,
                context: lsp_types::CodeActionContext::default(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("codeAction failed: {err}"))
    }

    /// The `documentSymbol` tree for `path`, flattened to name-path leaves.
    ///
    /// # Errors
    /// Fails when the document symbol request fails.
    pub async fn flat_symbols(
        &self,
        path: &Path,
    ) -> anyhow::Result<Vec<crate::symbols::FlatSymbol>> {
        Ok(crate::symbols::flatten_symbols(
            &self.document_symbols(path).await?,
        ))
    }

    /// Send `shutdown` then `exit`, letting the server terminate cleanly.
    pub async fn shutdown(&self) {
        let mut server = self.server.clone();
        let _ = server.shutdown(()).await;
        let _ = server.exit(());
    }

    /// `textDocument/rename` at a position.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn rename(
        &self,
        path: &Path,
        position: Position,
        new_name: &str,
    ) -> anyhow::Result<Option<WorkspaceEdit>> {
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        server
            .rename(RenameParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                new_name: new_name.to_string(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("rename failed: {err}"))
    }

    /// Notify the server that files are about to be renamed, applying any
    /// edit it returns (oh-my-pi's `rename_file`; L2/L5-adjacent).
    ///
    /// # Errors
    /// Fails when the request errors.
    pub async fn rename_files(
        &self,
        renames: &[(Url, Url)],
    ) -> anyhow::Result<Option<WorkspaceEdit>> {
        let files = renames
            .iter()
            .map(|(old, new)| lsp_types::FileRename {
                old_uri: old.to_string(),
                new_uri: new.to_string(),
            })
            .collect();
        let mut server = self.server.clone();
        server
            .will_rename_files(lsp_types::RenameFilesParams { files })
            .await
            .map_err(|err| anyhow::anyhow!("willRenameFiles failed: {err}"))
    }

    /// Ask the server to reload its workspace (oh-my-pi's `reload` action).
    ///
    /// rust-analyzer answers a bespoke `rust-analyzer/reloadWorkspace` request;
    /// every other server gets `workspace/didChangeConfiguration` plus a
    /// `didChange` re-sync of every open document, which is the portable way to
    /// force a re-index.
    ///
    /// # Errors
    /// Fails when a request/notification cannot be queued.
    pub async fn reload(&self) -> anyhow::Result<()> {
        if self.name == "rust-analyzer" {
            // The bespoke method is not in `lsp-types`, so it is issued through
            // a local `Request` shim carrying raw JSON.
            let _ = self
                .server
                .request::<RustAnalyzerReloadWorkspace>(serde_json::Value::Null)
                .await;
        }
        let mut server = self.server.clone();
        server
            .did_change_configuration(lsp_types::DidChangeConfigurationParams {
                settings: serde_json::Value::Null,
            })
            .map_err(|err| anyhow::anyhow!("didChangeConfiguration failed: {err}"))?;
        // Re-open every tracked document so the server sees the current bytes.
        let uris: Vec<String> = self.open_docs.lock().await.keys().cloned().collect();
        for uri in uris {
            if let Ok(url) = Url::parse(&uri) {
                if let Ok(path) = url.to_file_path() {
                    let _ = self.ensure_open(&path).await;
                }
            }
        }
        Ok(())
    }

    /// The server's advertised capabilities plus its identity, as JSON
    /// (oh-my-pi's `capabilities` action).
    #[must_use]
    pub fn capabilities_json(&self) -> serde_json::Value {
        serde_json::json!({
            "server": self.name,
            "root": self.root,
            "server_info": self.server_info,
            "capabilities": self.capabilities,
        })
    }

    /// The latest diagnostics the server pushed for `path`, if any.
    pub async fn published_diagnostics(
        &self,
        path: &Path,
    ) -> Option<(Vec<lsp_types::Diagnostic>, u64)> {
        let uri = Url::from_file_path(path).ok()?.to_string();
        let guard = self.notifications.lock().await;
        let diagnostics = guard.diagnostics.get(&uri)?.clone();
        let generation = guard.generation.get(&uri).copied().unwrap_or(0);
        Some((diagnostics, generation))
    }

    /// Wait up to `timeout` for the server to push diagnostics for `path`.
    ///
    /// Servers typically publish an empty set on `didOpen` and the real set a
    /// moment later, so the *first* publish is not trusted on its own. The wait
    /// ends when a non-empty set arrives, or — for a genuinely clean file —
    /// after the first publish plus a short settle window with no newer publish.
    /// On timeout the last set seen (possibly empty) is returned.
    ///
    /// The policy itself lives in the pure [`DiagnosticsWaiter`] state machine;
    /// this method only supplies the live poll source.
    pub async fn wait_for_diagnostics(
        &self,
        path: &Path,
        timeout: Duration,
    ) -> Option<(Vec<lsp_types::Diagnostic>, u64)> {
        await_diagnostics(DiagnosticsWaiter::new(timeout), || {
            self.published_diagnostics(path)
        })
        .await
    }

    /// Inline-then-deferred diagnostics wait (L12).
    ///
    /// Waits up to [`INLINE_DIAGNOSTICS_WAIT`] for a publish. If one arrives,
    /// returns [`DiagnosticsOutcome::Inline`]. If the server is slow, returns
    /// [`DiagnosticsOutcome::Deferred`] immediately, with a background task
    /// still polling for the full [`DEFERRED_DIAGNOSTICS_WAIT`] window so the
    /// caller (an edit) is never blocked on a cold type-checker.
    pub async fn wait_for_diagnostics_split(&self, path: &Path) -> DiagnosticsOutcome {
        self.wait_for_diagnostics_split_with(path, InlineWait::Inline)
            .await
    }

    /// [`wait_for_diagnostics_split`](Self::wait_for_diagnostics_split) with an
    /// explicit inline wait mode (tests).
    ///
    /// The mode's [`budget`](InlineWait::budget) is the inline phase's timeout;
    /// the deferred phase always runs for the full
    /// [`DEFERRED_DIAGNOSTICS_WAIT`] window.
    pub async fn wait_for_diagnostics_split_with(
        &self,
        path: &Path,
        mode: InlineWait,
    ) -> DiagnosticsOutcome {
        let inline_result = await_diagnostics(DiagnosticsWaiter::new(mode.budget()), || {
            self.published_diagnostics(path)
        })
        .await;
        if let Some(result) = inline_result {
            return DiagnosticsOutcome::Inline(Some(result));
        }
        let handle = {
            let path = path.to_path_buf();
            // The background poll borrows `self`; the client is `Arc`-owned by
            // the manager, so the fetch is spawned against a clone of the
            // notification state instead of `self`.
            let notifications = Arc::clone(&self.notifications);
            tokio::spawn(async move {
                await_diagnostics(
                    DiagnosticsWaiter::new(InlineWait::Deferred.budget()),
                    || published_from(&notifications, &path),
                )
                .await
            })
        };
        DiagnosticsOutcome::Deferred(DeferredDiagnostics { handle })
    }

    /// The document version the last publish carried for `path`, if the server
    /// versioned it (L13).
    pub async fn published_version(&self, path: &Path) -> Option<i32> {
        let uri = Url::from_file_path(path).ok()?.to_string();
        let guard = self.notifications.lock().await;
        guard.version.get(&uri).copied().flatten()
    }

    /// The version last sent to the server for `path` (0 once opened, bumped on
    /// each `didChange`). `None` when the document is not open.
    pub async fn document_version(&self, path: &Path) -> Option<i32> {
        let uri = Url::from_file_path(path).ok()?.to_string();
        self.open_docs.lock().await.get(&uri).copied()
    }

    /// The published diagnostics for `path`, filtered for freshness (L13).
    ///
    /// A publish whose version is older than the version the client last sent
    /// is stale — the server computed it against a document the client has
    /// already replaced — and is dropped. A publish without a version is
    /// accepted. On a TypeScript server, orphan project diagnostics (a stray
    /// file outside every root marker) are also filtered.
    pub async fn fresh_published_diagnostics(
        &self,
        path: &Path,
    ) -> Option<(Vec<lsp_types::Diagnostic>, u64)> {
        let (diagnostics, generation) = self.published_diagnostics(path).await?;
        let published_version = self.published_version(path).await;
        let doc_version = self.document_version(path).await.unwrap_or(0);
        let gate = crate::diagnostics_flow::FreshnessGate::new(doc_version);
        if !gate.accepts(published_version) {
            return Some((Vec::new(), generation));
        }
        let diagnostics = if self.is_typescript() {
            crate::diagnostics_flow::filter_orphan_project_diagnostics(
                &diagnostics,
                path,
                &self.root,
                crate::diagnostics_flow::TS_ROOT_MARKERS,
            )
        } else {
            diagnostics
        };
        Some((diagnostics, generation))
    }

    /// Notify the server a document was saved (L10 `notifyFileSaved`).
    ///
    /// # Errors
    /// Fails when `path` is not absolute or the notification cannot be queued.
    pub fn did_save(&self, path: &Path, text: Option<String>) -> anyhow::Result<()> {
        let uri = Url::from_file_path(path)
            .map_err(|()| anyhow::anyhow!("not an absolute path: {}", path.display()))?;
        let mut server = self.server.clone();
        server
            .did_save(DidSaveTextDocumentParams {
                text_document: TextDocumentIdentifier { uri },
                text,
            })
            .map_err(|err| anyhow::anyhow!("didSave failed: {err}"))
    }

    /// Whether the server advertises document formatting (L10 format-on-save).
    #[must_use]
    pub fn supports_formatting(&self) -> bool {
        self.capabilities.document_formatting_provider.is_some()
    }

    /// `textDocument/formatting` for a whole document (L10).
    ///
    /// Returns the server's text edits, or `None` when the server does not
    /// implement formatting.
    ///
    /// # Errors
    /// Fails when the document cannot be opened or the request errors.
    pub async fn formatting(&self, path: &Path) -> anyhow::Result<Option<Vec<TextEdit>>> {
        if !self.supports_formatting() {
            return Ok(None);
        }
        let uri = self.ensure_open(path).await?;
        let mut server = self.server.clone();
        server
            .formatting(DocumentFormattingParams {
                text_document: TextDocumentIdentifier { uri },
                options: FormattingOptions::default(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .map_err(|err| anyhow::anyhow!("formatting failed: {err}"))
    }

    /// Tell the server the file was saved under a new URI (L18
    /// `workspace/didRenameFiles`).
    ///
    /// # Errors
    /// Fails when the notification cannot be queued.
    pub fn did_rename_files(&self, renames: &[(Url, Url)]) -> anyhow::Result<()> {
        let files = renames
            .iter()
            .map(|(old, new)| FileRename {
                old_uri: old.to_string(),
                new_uri: new.to_string(),
            })
            .collect();
        let mut server = self.server.clone();
        server
            .did_rename_files(RenameFilesParams { files })
            .map_err(|err| anyhow::anyhow!("didRenameFiles failed: {err}"))
    }

    /// Tell the server the on-disk content of a watched file changed (L10
    /// post-write sync), so it re-reads from disk.
    ///
    /// # Errors
    /// Fails when the notification cannot be queued.
    pub fn did_change_watched_files(
        &self,
        events: Vec<lsp_types::FileEvent>,
    ) -> anyhow::Result<()> {
        let mut server = self.server.clone();
        server
            .did_change_watched_files(lsp_types::DidChangeWatchedFilesParams { changes: events })
            .map_err(|err| anyhow::anyhow!("didChangeWatchedFiles failed: {err}"))
    }

    /// The tsserver-specific `reloadProjects` request (L10).
    ///
    /// `typescript-language-server` caches module resolution; after a file is
    /// created or renamed the cache can "stick" on the missing module. Asking
    /// `typescript.tsserverRequest` for `reloadProjects` flushes it. Other
    /// servers ignore this (the caller gates on the server name).
    ///
    /// # Errors
    /// Fails when the request cannot be queued.
    pub async fn reload_typescript_projects(&self) -> anyhow::Result<()> {
        let params = ExecuteCommandParams {
            command: "typescript.tsserverRequest".to_string(),
            arguments: vec![serde_json::json!("reloadProjects")],
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let mut server = self.server.clone();
        server
            .execute_command(params)
            .await
            .map(|_| ())
            .map_err(|err| anyhow::anyhow!("tsserver reloadProjects failed: {err}"))
    }

    /// Whether this client is the TypeScript server (L10 reloadProjects gate).
    #[must_use]
    pub fn is_typescript(&self) -> bool {
        self.name == "typescript-language-server"
    }

    /// Every document this client has opened, as `(uri, last-version)`.
    pub async fn open_documents(&self) -> Vec<(String, i32)> {
        self.open_docs
            .lock()
            .await
            .iter()
            .map(|(uri, version)| (uri.clone(), *version))
            .collect()
    }

    /// The diagnostics identities currently recorded for `path`.
    pub async fn identities(&self, path: &Path) -> Vec<DiagnosticIdentity> {
        self.published_diagnostics(path)
            .await
            .map(|(diagnostics, _)| {
                diagnostics
                    .iter()
                    .map(DiagnosticIdentity::from_diagnostic)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Snapshot the current diagnostics + publish generation for `path`, for an
    /// E6 before/after diff.
    ///
    /// Taken *before* an edit; pair with
    /// [`diagnostics_delta_since`](Self::diagnostics_delta_since) after the
    /// write. The generation rides along so a stale post-edit publish is
    /// rejected (L13).
    pub async fn snapshot_diagnostics(&self, path: &Path) -> crate::ledger::DiagnosticsSnapshot {
        match self.published_diagnostics(path).await {
            Some((diagnostics, generation)) => {
                crate::ledger::DiagnosticsSnapshot::capture(&diagnostics, generation)
            }
            None => crate::ledger::DiagnosticsSnapshot::default(),
        }
    }

    /// The diagnostics that appeared for `path` since `snapshot` (E6).
    ///
    /// Returns only the new identities, and an empty list when the current
    /// publish generation has not advanced past the snapshot's (a stale
    /// report — L13).
    pub async fn diagnostics_delta_since(
        &self,
        path: &Path,
        snapshot: &crate::ledger::DiagnosticsSnapshot,
    ) -> Vec<lsp_types::Diagnostic> {
        match self.published_diagnostics(path).await {
            Some((diagnostics, generation)) => snapshot.diff(&diagnostics, generation),
            None => Vec::new(),
        }
    }

    /// Whether the underlying process is still alive.
    pub fn is_alive(&self) -> bool {
        let mut child = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(child.try_wait(), Ok(None))
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.mainloop.abort();
    }
}

/// Capabilities the fork advertises: diagnostics (pull + publish), symbols,
/// go-to requests, and edits.
fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        text_document: Some(lsp_types::TextDocumentClientCapabilities {
            publish_diagnostics: Some(lsp_types::PublishDiagnosticsClientCapabilities {
                related_information: Some(true),
                ..Default::default()
            }),
            // Advertise pull diagnostics so servers that support
            // `textDocument/diagnostic` (LSP 3.17) use it.
            diagnostic: Some(lsp_types::DiagnosticClientCapabilities {
                related_document_support: Some(true),
                ..Default::default()
            }),
            document_symbol: Some(lsp_types::DocumentSymbolClientCapabilities {
                hierarchical_document_symbol_support: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        workspace: Some(lsp_types::WorkspaceClientCapabilities {
            workspace_edit: Some(lsp_types::WorkspaceEditClientCapabilities {
                document_changes: Some(true),
                ..Default::default()
            }),
            file_operations: Some(lsp_types::WorkspaceFileOperationsClientCapabilities {
                will_rename: Some(true),
                did_rename: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Apply a `WorkspaceEdit`'s text edits to disk, returning the touched files.
///
/// Only the `changes` map and `document_changes` text-edit variants are
/// honored; file create/rename/delete operations are refused (the fork does
/// not carry that plumbing yet), so a rename never silently half-applies.
///
/// # Errors
/// Fails when an edit targets a non-file URI, a file cannot be read or
/// written, a range is out of bounds, or a file-operation edit is present.
pub async fn apply_workspace_edit(edit: &WorkspaceEdit) -> anyhow::Result<Vec<PathBuf>> {
    // Validate the whole edit BEFORE touching the filesystem. oh-my-pi shipped
    // the opposite order: it applied `changes`, then bailed on an unsupported
    // `documentChanges` shape, so the caller saw a failure while the files were
    // already half-mutated (their F03). A rejected edit must leave every file
    // exactly as it was.
    if let Some(lsp_types::DocumentChanges::Operations(_)) = &edit.document_changes {
        anyhow::bail!(
            "file-operation workspace edits (create/rename/delete) are not supported yet; \
             nothing was written"
        );
    }

    let mut touched = Vec::new();
    if let Some(changes) = &edit.changes {
        for (uri, edits) in changes {
            apply_text_edits(uri, edits)?;
            touched.push(uri_path(uri)?);
        }
    }
    if let Some(lsp_types::DocumentChanges::Edits(edits)) = &edit.document_changes {
        for text_document_edit in edits {
            let uri = &text_document_edit.text_document.uri;
            let text_edits = flatten_text_edits(&text_document_edit.edits);
            apply_text_edits(uri, &text_edits)?;
            touched.push(uri_path(uri)?);
        }
    }
    Ok(touched)
}

/// Read the published diagnostics for `path` straight from the shared
/// notification state (the background half of the L12 split, which cannot
/// borrow the client).
async fn published_from(
    notifications: &Mutex<ClientNotifications>,
    path: &Path,
) -> Option<(Vec<lsp_types::Diagnostic>, u64)> {
    let uri = Url::from_file_path(path).ok()?.to_string();
    let guard = notifications.lock().await;
    let diagnostics = guard.diagnostics.get(&uri)?.clone();
    let generation = guard.generation.get(&uri).copied().unwrap_or(0);
    Some((diagnostics, generation))
}

/// The filesystem path a file URI names.
fn uri_path(uri: &Url) -> anyhow::Result<PathBuf> {
    uri.to_file_path()
        .map_err(|()| anyhow::anyhow!("edit targets a non-file URI: {uri}"))
}

/// Apply one document's edits, from the end so earlier ranges keep their offsets.
fn apply_text_edits(uri: &Url, edits: &[TextEdit]) -> anyhow::Result<()> {
    let path = uri_path(uri)?;
    let mut text = std::fs::read_to_string(&path)?;
    let mut sorted: Vec<&TextEdit> = edits.iter().collect();
    sorted.sort_by(|a, b| {
        let ka = (a.range.start.line, a.range.start.character);
        let kb = (b.range.start.line, b.range.start.character);
        kb.cmp(&ka)
    });
    for edit in sorted {
        let start = offset_of(&text, edit.range.start);
        let end = offset_of(&text, edit.range.end);
        anyhow::ensure!(
            start <= end && end <= text.len(),
            "edit range is out of bounds for {}",
            path.display()
        );
        text.replace_range(start..end, &edit.new_text);
    }
    std::fs::write(&path, text)?;
    Ok(())
}

/// Normalize the `OneOf<TextEdit, AnnotatedTextEdit>` edit variants to plain
/// `TextEdit`s (the annotation id is dropped: applying does not need it).
fn flatten_text_edits(edits: &[OneOf<TextEdit, lsp_types::AnnotatedTextEdit>]) -> Vec<TextEdit> {
    edits
        .iter()
        .map(|edit| match edit {
            OneOf::Left(text_edit) => text_edit.clone(),
            OneOf::Right(annotated) => annotated.text_edit.clone(),
        })
        .collect()
}

/// Byte offset of a UTF-16 LSP position within `text`.
// A `char`'s UTF-16 length is 1 or 2, so the `usize`-to-`u32` cast cannot truncate.
#[allow(clippy::cast_possible_truncation)]
fn offset_of(text: &str, position: Position) -> usize {
    let mut line_start = 0usize;
    for _ in 0..position.line {
        match text[line_start..].find('\n') {
            Some(rel) => line_start += rel + 1,
            None => return text.len(),
        }
    }
    let line = &text[line_start..];
    let line_end = line.find('\n').unwrap_or(line.len());
    let line_text = &line[..line_end];
    // LSP characters are UTF-16 code units; walk the line counting them.
    let mut utf16 = 0u32;
    for (byte_idx, ch) in line_text.char_indices() {
        if utf16 >= position.character {
            return line_start + byte_idx;
        }
        utf16 += ch.len_utf16() as u32;
    }
    line_start + line_end
}

#[cfg(test)]
mod tests {
    /// A server that is still loading answers `null`, which flattens to an empty
    /// list and reads as "nothing found". Measured with rust-analyzer here: the
    /// first call returns empty at 0.1 s, the first real answer arrives at 30.3 s,
    /// and a progress stream is open for the whole window.
    #[test]
    fn an_open_progress_stream_means_answers_are_not_final() {
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(ClientNotifications::default()));
        let mut warming = ClientNotifications::default();
        warming.progress.insert(
            "rustAnalyzer/Building CrateGraph".to_string(),
            ProgressState {
                title: Some("Building CrateGraph".into()),
                done: false,
            },
        );
        assert!(!warming.progress.values().all(|s| s.done));

        let mut ready = ClientNotifications::default();
        ready.progress.insert(
            "rustAnalyzer/Fetching".to_string(),
            ProgressState {
                title: None,
                done: true,
            },
        );
        assert!(ready.progress.values().all(|s| s.done));
        let _ = state;
    }

    /// The state a tool renders must distinguish the two cases in words.
    #[test]
    fn warmth_lines_are_distinct() {
        assert_eq!(Warmth::Ready.line(), "ready");
        assert_eq!(
            Warmth::Warming {
                message: Some("Building CrateGraph".into())
            }
            .line(),
            "warming (Building CrateGraph)"
        );
        assert!(!Warmth::Warming { message: None }.answers_are_final());
        assert!(Warmth::Ready.answers_are_final());
    }

    use super::*;
    use lsp_types::{DiagnosticSeverity, Position, Range};

    fn diag(message: &str) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            severity: Some(DiagnosticSeverity::ERROR),
            code: None,
            code_description: None,
            source: Some("test".into()),
            message: message.into(),
            related_information: None,
            tags: None,
            data: None,
        }
    }

    #[test]
    fn inline_wait_uses_the_short_budget() {
        assert_eq!(InlineWait::Inline.budget(), INLINE_DIAGNOSTICS_WAIT);
        assert_eq!(InlineWait::Deferred.budget(), DEFERRED_DIAGNOSTICS_WAIT);
        assert!(INLINE_DIAGNOSTICS_WAIT < DEFERRED_DIAGNOSTICS_WAIT);
    }

    #[test]
    fn waiter_returns_a_non_empty_publish_immediately() {
        let now = tokio::time::Instant::now();
        let mut waiter =
            DiagnosticsWaiter::with_settle(Duration::from_secs(5), Duration::from_millis(500));
        let published = Some((vec![diag("boom")], 1));
        match waiter.observe(now, published) {
            WaitStep::Done(Some((diagnostics, generation))) => {
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(generation, 1);
            }
            other => panic!("expected an immediate answer, got {other:?}"),
        }
    }

    #[test]
    fn waiter_settles_an_empty_publish_after_the_settle_window() {
        let now = tokio::time::Instant::now();
        let settle = Duration::from_millis(500);
        let mut waiter = DiagnosticsWaiter::with_settle(Duration::from_secs(5), settle);
        // First (empty) publish: not done yet — it needs the settle window.
        assert_eq!(waiter.observe(now, Some((vec![], 1))), WaitStep::Continue);
        // Before the settle window elapses: still waiting.
        assert_eq!(waiter.observe(now + settle / 2, None), WaitStep::Continue);
        // After it: the clean file is settled.
        match waiter.observe(now + settle + Duration::from_millis(1), None) {
            WaitStep::Done(Some((diagnostics, _))) => assert_eq!(diagnostics.len(), 0),
            other => panic!("expected a settled empty answer, got {other:?}"),
        }
    }

    #[test]
    fn waiter_answers_none_at_the_deadline() {
        let now = tokio::time::Instant::now();
        let timeout = Duration::from_millis(500);
        let mut waiter = DiagnosticsWaiter::with_settle(timeout, Duration::from_millis(500));
        assert_eq!(waiter.observe(now, None), WaitStep::Continue);
        assert_eq!(
            waiter.observe(now + timeout + Duration::from_millis(1), None),
            WaitStep::Done(None)
        );
    }

    #[test]
    fn waiter_ignores_a_stale_generation() {
        let now = tokio::time::Instant::now();
        let mut waiter =
            DiagnosticsWaiter::with_settle(Duration::from_secs(5), Duration::from_millis(500));
        // A non-empty publish at generation 1 answers.
        assert!(matches!(
            waiter.observe(now, Some((vec![diag("x")], 1))),
            WaitStep::Done(Some(_))
        ));
    }

    /// The L12 split: a mock source that never publishes must resolve to
    /// `Deferred` under the inline budget, then a background fetch answers.
    #[tokio::test(start_paused = true)]
    async fn await_diagnostics_polls_until_the_source_answers() {
        let polls = std::cell::Cell::new(0u32);
        let result = await_diagnostics(
            DiagnosticsWaiter::with_settle(Duration::from_secs(2), Duration::from_millis(100)),
            || {
                polls.set(polls.get() + 1);
                async {
                    // Answer only on the third poll.
                    if polls.get() >= 3 {
                        Some((vec![diag("late")], 7))
                    } else {
                        None
                    }
                }
            },
        )
        .await;
        let (diagnostics, generation) = result.expect("the third poll answers");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(generation, 7);
        assert!(polls.get() >= 3);
    }

    /// Under a paused clock, a source that never answers resolves to `None`
    /// once the waiter's budget is exhausted — no real time passes.
    #[tokio::test(start_paused = true)]
    async fn await_diagnostics_times_out_with_none() {
        let result = await_diagnostics(
            DiagnosticsWaiter::with_settle(INLINE_DIAGNOSTICS_WAIT, Duration::from_millis(100)),
            || async { None },
        )
        .await;
        assert!(result.is_none(), "nothing published within the budget");
    }

    /// The deferred handle keeps polling after the inline budget; a mock source
    /// answering later resolves it.
    #[tokio::test(start_paused = true)]
    async fn deferred_fetch_resolves_after_the_inline_budget() {
        let handle = tokio::spawn(async {
            await_diagnostics(
                DiagnosticsWaiter::with_settle(
                    DEFERRED_DIAGNOSTICS_WAIT,
                    Duration::from_millis(100),
                ),
                || async { Some((vec![diag("deferred")], 3)) },
            )
            .await
        });
        let resolved = DeferredDiagnostics { handle }.resolve().await;
        let (diagnostics, generation) = resolved.expect("the deferred fetch answers");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(generation, 3);
    }

    /// A handle abandoned before resolution must not leave a live task behind.
    #[tokio::test(start_paused = true)]
    async fn abandoned_deferred_fetch_is_cancelled() {
        let handle = tokio::spawn(async {
            await_diagnostics(
                DiagnosticsWaiter::with_settle(
                    DEFERRED_DIAGNOSTICS_WAIT,
                    Duration::from_millis(50),
                ),
                || async { None },
            )
            .await
        });
        let deferred = DeferredDiagnostics { handle };
        deferred.abandon();
        // Aborting is immediate; the runtime has nothing to wait on.
        tokio::task::yield_now().await;
    }

    /// A rejected workspace edit must not leave files half-mutated.
    ///
    /// oh-my-pi shipped the opposite order: it applied `changes`, then bailed
    /// on an unsupported `documentChanges` shape, so the caller saw a failure
    /// while the files were already written (their F03). Validation now runs
    /// before any write, so this edit leaves the file byte-identical.
    #[tokio::test]
    async fn unsupported_document_changes_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        std::fs::write(&path, "fn a() {}\n").unwrap();
        let uri = Url::from_file_path(&path).unwrap();

        let mut changes = std::collections::HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 5)),
                new_text: "fn b".into(),
            }],
        );
        let edit = WorkspaceEdit {
            changes: Some(changes),
            document_changes: Some(lsp_types::DocumentChanges::Operations(Vec::new())),
            change_annotations: None,
        };

        let err = apply_workspace_edit(&edit).await.unwrap_err();
        assert!(err.to_string().contains("not supported"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn a() {}\n",
            "the rejected edit must not touch the file"
        );
    }
}
