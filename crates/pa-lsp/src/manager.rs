//! Per-session language-server client lifecycle.
//!
//! FORK-ONLY (allspark). Synthesizes oh-my-pi's shared client map: at most one
//! live client per `(server, root)` pair, spawned on first use and reused for
//! every later request. The manager is created with the session and dropped with
//! it, so a session's language servers die when the session does.
//!
//! L15 (`init_failure_backoff`): a server that fails to spawn or handshake is
//! remembered for [`INIT_FAILURE_BACKOFF`] (3 minutes) and *not* re-spawned on
//! every call. Without this, a broken server (bad binary, missing grammar,
//! crashing on `initialize`) would pay the spawn+handshake cost on every
//! request. A later success clears the entry; `restart` bypasses it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::client::LspClient;
use crate::servers::{project_root, server_for_path, ServerConfig};

/// How long a failed server spawn is remembered (oh-my-pi's
/// `INIT_FAILURE_BACKOFF_MS = 3min`).
pub const INIT_FAILURE_BACKOFF: Duration = Duration::from_secs(180);

/// A `(server, root)` client's reported state, for `lsp(action="status")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    /// The server table entry's name.
    pub server: String,
    /// The project root the client was started for.
    pub root: PathBuf,
    /// Whether the client's main loop (and thus its process) is still running.
    pub alive: bool,
}

/// A time source, so the backoff window can be tested without sleeping.
pub trait Clock: Send + Sync + 'static {
    /// The current instant.
    fn now(&self) -> Instant;
}

/// The real monotonic clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Negative cache of server init failures: `(server, root) -> (failed_at, msg)`.
pub struct InitFailureBackoff {
    ttl: Duration,
    clock: Arc<dyn Clock>,
    failures: RwLock<HashMap<(String, PathBuf), (Instant, String)>>,
}

impl Default for InitFailureBackoff {
    fn default() -> Self {
        Self::with_clock(INIT_FAILURE_BACKOFF, Arc::new(SystemClock))
    }
}

impl InitFailureBackoff {
    /// A backoff with the default 3-minute TTL over the system clock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A backoff with an explicit TTL and clock (used by the TTL tests).
    #[must_use]
    pub fn with_clock(ttl: Duration, clock: Arc<dyn Clock>) -> Self {
        Self {
            ttl,
            clock,
            failures: RwLock::new(HashMap::new()),
        }
    }

    /// The cached failure message for `key` if it is still within the window.
    async fn recent_failure(&self, key: &(String, PathBuf)) -> Option<String> {
        let guard = self.failures.read().await;
        let (failed_at, message) = guard.get(key)?;
        if self.clock.now().duration_since(*failed_at) < self.ttl {
            Some(message.clone())
        } else {
            None
        }
    }

    /// Record a failure for `key`.
    async fn record(&self, key: (String, PathBuf), message: String) {
        let mut guard = self.failures.write().await;
        guard.insert(key, (self.clock.now(), message));
    }

    /// Forget any failure for `key` (after a success, or an explicit restart).
    async fn clear(&self, key: &(String, PathBuf)) {
        self.failures.write().await.remove(key);
    }

    /// Drop expired entries; returns how many were evicted.
    pub async fn prune(&self) -> usize {
        let now = self.clock.now();
        let mut guard = self.failures.write().await;
        let before = guard.len();
        guard.retain(|_, (failed_at, _)| now.duration_since(*failed_at) < self.ttl);
        before - guard.len()
    }

    /// The number of recorded failures (including expired ones not yet pruned).
    pub async fn len(&self) -> usize {
        self.failures.read().await.len()
    }

    /// Whether the cache is empty.
    pub async fn is_empty(&self) -> bool {
        self.failures.read().await.is_empty()
    }
}

/// Owns the session's live clients, keyed by `(server name, root)`.
#[derive(Default)]
pub struct LspManager {
    clients: RwLock<HashMap<(String, PathBuf), Arc<LspClient>>>,
    backoff: InitFailureBackoff,
}

impl LspManager {
    /// A manager with no clients; clients are spawned lazily by
    /// [`LspManager::ensure_client`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A manager whose init-failure backoff uses `clock` (tests).
    #[must_use]
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            backoff: InitFailureBackoff::with_clock(INIT_FAILURE_BACKOFF, clock),
        }
    }

    /// The init-failure backoff, for status/introspection.
    #[must_use]
    pub fn backoff(&self) -> &InitFailureBackoff {
        &self.backoff
    }

    /// Return the client owning `path`, spawning one when none is live yet.
    ///
    /// The server is chosen from the file extension and the root from the
    /// nearest project marker, so two files in the same project share a client.
    ///
    /// # Errors
    /// Fails when the extension has no server entry, the root cannot be made
    /// absolute, the spawn/handshake fails, or a recent failure for this
    /// `(server, root)` is still within the backoff window.
    pub async fn ensure_client(&self, path: &Path) -> anyhow::Result<Arc<LspClient>> {
        let server: &'static ServerConfig = server_for_path(path).ok_or_else(|| {
            anyhow::anyhow!("no language server is configured for `{}`", path.display())
        })?;
        let root = canonical_root(path, server)?;
        let key = (server.name.to_string(), root.clone());

        if let Some(client) = self.clients.read().await.get(&key) {
            return Ok(Arc::clone(client));
        }
        // L15: a server that failed to start within the window is not retried.
        if let Some(message) = self.backoff.recent_failure(&key).await {
            anyhow::bail!(
                "`{}` failed to initialize recently and is backed off for {:?}: {}",
                server.name,
                INIT_FAILURE_BACKOFF,
                message
            );
        }

        // Hold the write guard across the spawn so two racing callers cannot
        // start duplicate servers for the same key.
        let mut guard = self.clients.write().await;
        if let Some(client) = guard.get(&key) {
            return Ok(Arc::clone(client));
        }
        match LspClient::spawn(server, &root).await {
            Ok(client) => {
                guard.insert(key.clone(), Arc::clone(&client));
                drop(guard);
                self.backoff.clear(&key).await;
                Ok(client)
            }
            Err(err) => {
                drop(guard);
                self.backoff.record(key, err.to_string()).await;
                Err(err)
            }
        }
    }

    /// Every live client, for a request that must reach all servers (a file
    /// rename that every server may want to react to).
    pub async fn clients(&self) -> Vec<Arc<LspClient>> {
        self.clients.read().await.values().cloned().collect()
    }

    /// Any live client, for requests that are not tied to a file (a
    /// workspace-symbol query with no `file` argument).
    pub async fn any_client(&self) -> Option<Arc<LspClient>> {
        self.clients.read().await.values().next().map(Arc::clone)
    }

    /// The current client roster, ordered by server then root for stable output.
    pub async fn status(&self) -> Vec<ServerStatus> {
        let guard = self.clients.read().await;
        let mut statuses: Vec<ServerStatus> = guard
            .iter()
            .map(|((server, root), client)| ServerStatus {
                server: server.clone(),
                root: root.clone(),
                alive: client.is_alive(),
            })
            .collect();
        statuses.sort_by(|a, b| a.server.cmp(&b.server).then_with(|| a.root.cmp(&b.root)));
        statuses
    }

    /// The names of the servers known to the table, for `status` when nothing
    /// is live yet.
    #[must_use]
    pub fn known_servers() -> Vec<&'static str> {
        crate::servers::SERVERS
            .iter()
            .map(|server| server.name)
            .collect()
    }

    /// Drop and re-spawn the client for `path`, returning the fresh client.
    ///
    /// A restart clears any init-failure backoff entry, so an explicit retry
    /// after fixing the environment always attempts a fresh spawn.
    ///
    /// # Errors
    /// Fails for the same reasons as [`LspManager::ensure_client`].
    pub async fn restart(&self, path: &Path) -> anyhow::Result<Arc<LspClient>> {
        if let Some(server) = server_for_path(path) {
            if let Ok(root) = canonical_root(path, server) {
                self.backoff.clear(&(server.name.to_string(), root)).await;
            }
        }
        self.shutdown_for(path).await;
        self.ensure_client(path).await
    }

    /// Run the post-write LSP pipeline (L10) for `path`: ensure a client, then
    /// re-sync, optionally format, and notify the save.
    ///
    /// A convenience over [`crate::writethrough::write_through`] for callers
    /// that hold a manager rather than a client. Returns `None` when no server
    /// owns `path` (an unknown extension), so a plain text edit still succeeds
    /// without LSP.
    ///
    /// # Errors
    /// Fails when the client spawn fails or a required server interaction
    /// (sync/save) errors.
    pub async fn write_through(
        &self,
        path: &Path,
        options: &crate::writethrough::WriteThroughOptions,
    ) -> anyhow::Result<Option<crate::writethrough::WriteThroughReport>> {
        if crate::servers::server_for_path(path).is_none() {
            return Ok(None);
        }
        let client = self.ensure_client(path).await?;
        Ok(Some(
            crate::writethrough::write_through(&client, path, options).await?,
        ))
    }

    /// Rename a file on disk and broadcast the file operation to every live
    /// server (L18).
    ///
    /// Every client is asked for its `workspace/willRenameFiles` edits *before*
    /// the move (which are applied to disk), then notified with
    /// `workspace/didRenameFiles` afterwards. A server that does not advertise
    /// the capability simply returns nothing; the move still happens. Returns
    /// the files touched by the applied pre-rename edits.
    ///
    /// # Errors
    /// Fails when either path is not absolute or the on-disk rename fails.
    pub async fn rename_file(&self, from: &Path, to: &Path) -> anyhow::Result<Vec<PathBuf>> {
        let from_url = lsp_types::Url::from_file_path(from)
            .map_err(|()| anyhow::anyhow!("not an absolute path: {}", from.display()))?;
        let to_url = lsp_types::Url::from_file_path(to)
            .map_err(|()| anyhow::anyhow!("not an absolute path: {}", to.display()))?;
        let renames = [(from_url.clone(), to_url.clone())];

        // Refuse to overwrite: `std::fs::rename` replaces the destination
        // silently on POSIX, so a typo in `dest` would destroy an unrelated
        // file and report success (oh-my-pi F05).
        if tokio::fs::try_exists(to).await.unwrap_or(false) {
            anyhow::bail!(
                "rename destination already exists: {} — remove it or pick another path",
                to.display()
            );
        }
        if !tokio::fs::try_exists(from).await.unwrap_or(false) {
            anyhow::bail!("rename source does not exist: {}", from.display());
        }

        // Pre-rename edits from every live server, applied before the move.
        // A server that cannot answer is NOT the same as "no import edits
        // needed": silently swallowing the error would move the file and leave
        // every import broken while reporting success (oh-my-pi F04). Refuse
        // and say which server failed, so the move can be retried.
        let mut touched = Vec::new();
        let clients = self.clients().await;
        for client in &clients {
            match client.rename_files(&renames).await {
                Ok(Some(edit)) => {
                    touched.extend(crate::client::apply_workspace_edit(&edit).await?);
                }
                Ok(None) => {}
                Err(err) => {
                    anyhow::bail!(
                        "the language server could not compute import edits for this rename, \
                         so the file was NOT moved ({err})"
                    );
                }
            }
        }

        tokio::fs::rename(from, to)
            .await
            .map_err(|err| anyhow::anyhow!("rename failed: {err}"))?;

        // Post-rename notification to every live server.
        for client in &clients {
            let _ = client.did_rename_files(&renames);
        }
        Ok(touched)
    }

    /// Remove the client owning `path` from the roster; dropping the last `Arc`
    /// kills its process.
    pub async fn shutdown_for(&self, path: &Path) {
        let Some(server) = server_for_path(path) else {
            return;
        };
        let Ok(root) = canonical_root(path, server) else {
            return;
        };
        let key = (server.name.to_string(), root);
        let mut guard = self.clients.write().await;
        guard.remove(&key);
    }

    /// Drop every client, killing their processes.
    pub async fn shutdown_all(&self) {
        let mut guard = self.clients.write().await;
        guard.clear();
    }
}

/// Canonicalize the file and resolve its project root, falling back to the
/// file's parent when the path cannot be canonicalized (e.g. it does not exist).
fn canonical_root(path: &Path, server: &ServerConfig) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let resolved = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let root = project_root(&resolved, server);
    // `Url::from_directory_path` needs an absolute path; canonicalize when the
    // root exists, else fall back to the (already absolute) ancestor.
    Ok(std::fs::canonicalize(&root).unwrap_or(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A clock whose `now` only moves when the test advances it.
    struct MockClock {
        base: Instant,
        offset: Mutex<Duration>,
    }

    impl MockClock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                base: Instant::now(),
                offset: Mutex::new(Duration::ZERO),
            })
        }

        fn advance(&self, by: Duration) {
            *self.offset.lock().unwrap() += by;
        }
    }

    impl Clock for MockClock {
        fn now(&self) -> Instant {
            self.base + *self.offset.lock().unwrap()
        }
    }

    #[tokio::test]
    async fn unknown_extension_is_an_error() {
        let manager = LspManager::new();
        let error = manager
            .ensure_client(Path::new("notes.md"))
            .await
            .err()
            .expect("an unknown extension must error");
        assert!(error.to_string().contains("no language server"));
    }

    #[tokio::test]
    async fn status_is_empty_before_any_client() {
        let manager = LspManager::new();
        assert_eq!(manager.status().await, Vec::new());
        assert!(LspManager::known_servers().contains(&"rust-analyzer"));
    }

    #[tokio::test]
    async fn backoff_hides_a_failure_until_the_ttl_expires() {
        let clock = MockClock::new();
        let backoff = InitFailureBackoff::with_clock(Duration::from_secs(180), clock.clone());
        let key = ("rust-analyzer".to_string(), PathBuf::from("/proj"));

        assert!(backoff.recent_failure(&key).await.is_none());
        backoff.record(key.clone(), "boom".to_string()).await;
        // Within the window the failure is visible.
        assert_eq!(backoff.recent_failure(&key).await.as_deref(), Some("boom"));

        // Just before the TTL it is still cached.
        clock.advance(Duration::from_secs(179));
        assert!(backoff.recent_failure(&key).await.is_some());

        // After the TTL the entry expires.
        clock.advance(Duration::from_secs(2));
        assert!(backoff.recent_failure(&key).await.is_none());
        assert_eq!(backoff.prune().await, 1);
        assert!(backoff.is_empty().await);
    }

    #[tokio::test]
    async fn success_clears_a_recorded_failure() {
        let clock = MockClock::new();
        let backoff = InitFailureBackoff::with_clock(Duration::from_secs(180), clock);
        let key = ("gopls".to_string(), PathBuf::from("/proj"));
        backoff.record(key.clone(), "boom".to_string()).await;
        assert!(backoff.recent_failure(&key).await.is_some());
        backoff.clear(&key).await;
        assert!(backoff.recent_failure(&key).await.is_none());
    }

    #[tokio::test]
    async fn backoff_prune_keeps_fresh_entries() {
        let clock = MockClock::new();
        let backoff = InitFailureBackoff::with_clock(Duration::from_secs(180), clock.clone());
        let fresh = ("a".to_string(), PathBuf::from("/a"));
        let stale = ("b".to_string(), PathBuf::from("/b"));
        backoff.record(fresh.clone(), "a".to_string()).await;
        backoff.record(stale.clone(), "b".to_string()).await;
        clock.advance(Duration::from_secs(200));
        // `fresh` is still expired by wall time; re-record it to keep it fresh.
        backoff.record(fresh.clone(), "a".to_string()).await;
        assert_eq!(backoff.prune().await, 1);
        assert!(backoff.recent_failure(&fresh).await.is_some());
        assert!(backoff.recent_failure(&stale).await.is_none());
    }

    /// Renaming onto an existing file must refuse, not overwrite.
    ///
    /// `std::fs::rename` replaces the destination silently on POSIX, so a typo
    /// in `dest` destroyed an unrelated file and reported success (oh-my-pi
    /// shipped exactly this). The manager must reject the move before touching
    /// either path.
    #[tokio::test]
    async fn rename_refuses_to_overwrite_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.rs");
        let to = dir.path().join("to.rs");
        std::fs::write(&from, "fn a() {}\n").unwrap();
        std::fs::write(&to, "fn keep_me() {}\n").unwrap();

        let manager = LspManager::new();
        let error = manager
            .rename_file(&from, &to)
            .await
            .expect_err("the move must be refused");
        assert!(
            error.to_string().contains("already exists"),
            "the error must name the cause: {error}"
        );
        // Neither file moved or changed.
        assert_eq!(std::fs::read_to_string(&from).unwrap(), "fn a() {}\n");
        assert_eq!(
            std::fs::read_to_string(&to).unwrap(),
            "fn keep_me() {}\n",
            "the destination must survive untouched"
        );
    }

    /// A missing source must fail loudly rather than report a successful move.
    #[tokio::test]
    async fn rename_refuses_a_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("absent.rs");
        let to = dir.path().join("to.rs");
        let manager = LspManager::new();
        let error = manager
            .rename_file(&from, &to)
            .await
            .expect_err("the move must be refused");
        assert!(
            error.to_string().contains("does not exist"),
            "the error must name the cause: {error}"
        );
        assert!(!to.exists(), "nothing may be created");
    }
}
