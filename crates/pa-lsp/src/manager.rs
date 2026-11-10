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
use crate::servers::{project_root, ServerConfig};

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

/// What a file rename actually achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenameReport {
    /// Files whose imports/text the servers rewrote before the move.
    pub touched: Vec<PathBuf>,
    /// `Some(reason)` when no server could compute import updates, so the move
    /// happened but the callers of the renamed module were NOT repaired. The
    /// caller must report it rather than claiming a complete refactor.
    pub imports_unverified: Option<String>,
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

/// Which live clients were used when, for the idle sweep.
///
/// Split out from [`LspManager`] so the decision ("is this key stale?") is a pure
/// function of stamps and a clock. A manager test cannot seed a real client (a
/// client only exists by spawning a language server), so the arithmetic that
/// decides EVICTION lives here, where it is testable without one.
#[derive(Debug, Default)]
struct IdleRoster {
    last_used: RwLock<HashMap<(String, PathBuf), Instant>>,
}

impl IdleRoster {
    /// Record a use of `key` at `now`.
    async fn mark(&self, key: &(String, PathBuf), now: Instant) {
        self.last_used.write().await.insert(key.clone(), now);
    }

    /// Record one `now` for several keys (the roster-wide read path).
    async fn mark_all(&self, keys: &[(String, PathBuf)], now: Instant) {
        let mut guard = self.last_used.write().await;
        for key in keys {
            guard.insert(key.clone(), now);
        }
    }

    /// The keys not used for at least `limit`.
    async fn stale(&self, now: Instant, limit: Duration) -> Vec<(String, PathBuf)> {
        let guard = self.last_used.read().await;
        let mut stale: Vec<(String, PathBuf)> = guard
            .iter()
            .filter(|(_, at)| now.duration_since(**at) >= limit)
            .map(|(key, _)| key.clone())
            .collect();
        // Stable output: `HashMap` iteration order is not, and a test that
        // compares the evicted set should not depend on hashing.
        stale.sort();
        stale
    }

    /// Forget `key` (it is gone from the roster).
    async fn forget(&self, key: &(String, PathBuf)) {
        self.last_used.write().await.remove(key);
    }
}

/// Per-key single-flight: one acquisition at a time PER KEY, and keys never
/// wait on each other.
///
/// The manager needs this because acquiring a client is SLOW — it spawns a
/// language server and completes an LSP handshake. A single `RwLock` over the
/// whole roster serialized every key behind whichever spawn was in flight, so
/// opening a Python file blocked on a Rust server starting (measured shape: a
/// write guard held across an `await` that spawns a process and waits for
/// `initialize`).
///
/// The map is keyed by the same `(server, root)` tuple as the roster. An entry
/// is retired by its last holder, so a long-lived session does not accumulate
/// one mutex per file ever opened.
#[derive(Debug, Default)]
struct KeyedGate<K: Eq + std::hash::Hash + Clone> {
    gates: std::sync::Mutex<HashMap<K, Arc<tokio::sync::Mutex<()>>>>,
}

/// The held gate for one key. Dropping it releases the key and retires the map
/// entry when no other caller is waiting on it.
#[derive(Debug)]
struct KeyedGuard<'a, K: Eq + std::hash::Hash + Clone> {
    gate: &'a KeyedGate<K>,
    key: K,
    /// Taken first in `drop`, so the mutex releases before the retirement
    /// check reads the Arc's strong count.
    held: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl<K: Eq + std::hash::Hash + Clone> Drop for KeyedGuard<'_, K> {
    fn drop(&mut self) {
        self.held.take();
        self.gate.retire(&self.key);
    }
}

impl<K: Eq + std::hash::Hash + Clone> KeyedGate<K> {
    /// The gate for `key`, cloned out of the map so the release can happen
    /// without holding the map lock.
    fn gate_for(&self, key: &K) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self
            .gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            map.entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    /// Retire `key` when this caller is the last one holding it: the Arc's
    /// strong count is the coordination truth.
    fn retire(&self, key: &K) {
        let mut map = self
            .gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map
            .get(key)
            .is_some_and(|gate| Arc::strong_count(gate) == 1)
        {
            map.remove(key);
        }
    }

    /// Wait for `key`'s gate and hand back the held guard.
    async fn acquire(&self, key: K) -> KeyedGuard<'_, K> {
        let lock = self.gate_for(&key);
        let held = lock.lock_owned().await;
        KeyedGuard {
            gate: self,
            key,
            held: Some(held),
        }
    }
}

/// Owns the session's live clients, keyed by `(server name, root)`.
pub struct LspManager {
    clients: RwLock<HashMap<(String, PathBuf), Arc<LspClient>>>,
    backoff: InitFailureBackoff,
    /// When each live client was last handed to a caller, for the idle sweep.
    /// `None` is the default manager: nothing evicts, which is what every
    /// caller got before `lsp.idleTimeoutMs` had a reader.
    idle_limit: Option<Duration>,
    clock: Arc<dyn Clock>,
    idle: IdleRoster,
    /// Per-key acquisition single-flight. See [`KeyedGate`]: acquiring a client
    /// spawns a server, so a global lock over the roster made every project wait
    /// on whichever language server happened to be starting.
    gate: KeyedGate<(String, PathBuf)>,
    /// The resolved server table: built-ins with the operator's `lsp.servers`
    /// overrides applied. Resolved once per manager, because a table that
    /// changed mid-session would strand the clients already spawned from it.
    servers: Vec<ServerConfig>,
}

impl Default for LspManager {
    fn default() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            backoff: InitFailureBackoff::default(),
            idle_limit: None,
            clock: Arc::new(SystemClock),
            idle: IdleRoster::default(),
            gate: KeyedGate::default(),
            servers: crate::servers::builtin_servers(),
        }
    }
}

impl LspManager {
    /// A manager with no clients; clients are spawned lazily by
    /// [`LspManager::ensure_client`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A manager whose table is the built-ins with `lsp.servers` applied.
    ///
    /// Resolved here rather than at each lookup: the setting decides which binary is
    /// spawned, so reading it per call would let a mid-session settings write strand
    /// the clients already running under the old command.
    #[must_use]
    pub fn with_servers(overrides: Option<&serde_json::Map<String, serde_json::Value>>) -> Self {
        Self {
            servers: crate::servers::resolve_servers(overrides),
            ..Self::default()
        }
    }

    /// Replace the resolved table with the built-ins plus `lsp.servers`.
    ///
    /// A builder rather than another constructor: the idle-timeout constructor
    /// already exists, and a caller chains whichever it needs.
    #[must_use]
    pub fn with_server_overrides(
        mut self,
        overrides: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Self {
        self.servers = crate::servers::resolve_servers(overrides);
        self
    }

    /// The server that owns `path`, from this manager's resolved table.
    #[must_use]
    pub fn server_for(&self, path: &Path) -> Option<ServerConfig> {
        crate::servers::server_in(&self.servers, path).cloned()
    }

    /// The resolved table, for `lsp(action="status")`.
    #[must_use]
    pub fn servers(&self) -> &[ServerConfig] {
        &self.servers
    }

    /// A manager whose init-failure backoff uses `clock` (tests).
    #[must_use]
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            backoff: InitFailureBackoff::with_clock(INIT_FAILURE_BACKOFF, Arc::clone(&clock)),
            clock,
            ..Self::default()
        }
    }

    /// A manager that evicts a client it has not handed out for `idle` — the
    /// `lsp.idleTimeoutMs` setting.
    ///
    /// The setting was declared and documented as "idle timeout for a live
    /// language-server client" while nothing read it: a session's servers lived
    /// until the session ended, however long the agent spent on something else.
    /// The sweep runs on the same clock seam the backoff uses, so the bound is
    /// testable without sleeping.
    #[must_use]
    pub fn with_idle_timeout(idle: Duration, clock: Arc<dyn Clock>) -> Self {
        Self {
            idle_limit: Some(idle),
            clock: Arc::clone(&clock),
            backoff: InitFailureBackoff::with_clock(INIT_FAILURE_BACKOFF, clock),
            ..Self::default()
        }
    }

    /// The idle bound, if one was configured.
    #[must_use]
    pub fn idle_timeout(&self) -> Option<Duration> {
        self.idle_limit
    }

    /// How many clients are live right now.
    pub async fn live_count(&self) -> usize {
        self.clients.read().await.len()
    }

    /// Evict every client idle past the configured bound; returns how many went.
    ///
    /// Dropping the roster's last `Arc` for a key kills that client's process,
    /// which is the whole point: an idle language server is a real child process
    /// holding real memory. `None` (the default) evicts nothing.
    pub async fn evict_idle(&self) -> usize {
        let Some(limit) = self.idle_limit else {
            return 0;
        };
        let stale = self.idle.stale(self.clock.now(), limit).await;
        if stale.is_empty() {
            return 0;
        }
        let mut clients = self.clients.write().await;
        let mut evicted = 0;
        for key in stale {
            if clients.remove(&key).is_some() {
                evicted += 1;
                // The failure backoff is about STARTUP, not idleness: a server
                // that worked for an hour and then idled out must not be
                // reported as "failed to initialize recently" on the next use.
                self.backoff.clear(&key).await;
            }
            self.idle.forget(&key).await;
        }
        evicted
    }

    /// Record that `key` was just handed to a caller. Cheap and unconditional:
    /// with no idle bound the roster is never read.
    async fn mark_used(&self, key: &(String, PathBuf)) {
        if self.idle_limit.is_some() {
            self.idle.mark(key, self.clock.now()).await;
        }
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
        let server = self.server_for(path).ok_or_else(|| {
            anyhow::anyhow!("no language server is configured for `{}`", path.display())
        })?;
        let root = canonical_root(path, &server)?;
        let key = (server.name.clone(), root.clone());

        if let Some(client) = self.clients.read().await.get(&key).cloned() {
            self.mark_used(&key).await;
            return Ok(client);
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

        // Single-flight PER KEY, not over the roster. Acquiring a client spawns
        // a language server and completes an LSP handshake — seconds of work —
        // and the previous shape held `clients.write()` across it, so opening a
        // Python file waited on whichever Rust server was starting. The key is
        // the one this call needs; other keys never wait on it.
        let _flight = self.gate.acquire(key.clone()).await;

        // Re-check under the gate: a sibling that held it may have spawned the
        // client, which is exactly what the single-flight is for.
        if let Some(client) = self.clients.read().await.get(&key).cloned() {
            self.mark_used(&key).await;
            return Ok(client);
        }
        // The backoff is re-checked here for the same reason: a sibling may have
        // recorded a failure while this call waited.
        if let Some(message) = self.backoff.recent_failure(&key).await {
            anyhow::bail!(
                "`{}` failed to initialize recently and is backed off for {:?}: {}",
                server.name,
                INIT_FAILURE_BACKOFF,
                message
            );
        }
        match LspClient::spawn(&server, &root).await {
            Ok(client) => {
                // The roster write is brief: insert and release, never across
                // the spawn.
                self.clients
                    .write()
                    .await
                    .insert(key.clone(), Arc::clone(&client));
                self.mark_used(&key).await;
                self.backoff.clear(&key).await;
                Ok(client)
            }
            Err(err) => {
                self.backoff.record(key, err.to_string()).await;
                Err(err)
            }
        }
    }

    /// Every live client, for a request that must reach all servers (a file
    /// rename that every server may want to react to).
    pub async fn clients(&self) -> Vec<Arc<LspClient>> {
        let guard = self.clients.read().await;
        let keys: Vec<(String, PathBuf)> = guard.keys().cloned().collect();
        let rows: Vec<Arc<LspClient>> = guard.values().cloned().collect();
        drop(guard);
        if self.idle_limit.is_some() {
            self.idle.mark_all(&keys, self.clock.now()).await;
        }
        rows
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
    pub fn known_servers() -> Vec<String> {
        crate::servers::builtin_servers()
            .into_iter()
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
        if let Some(server) = self.server_for(path) {
            if let Ok(root) = canonical_root(path, &server) {
                self.backoff.clear(&(server.name.clone(), root)).await;
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
    /// The file's OWNING server is started first when none is live: this
    /// function used to ask only the clients that happened to exist, so a rename
    /// as the session's first LSP operation moved the file and left every import
    /// pointing at the old path — while reporting success (review #14).
    ///
    /// Every client is then asked for its `workspace/willRenameFiles` edits
    /// *before* the move (which are applied to disk), and notified with
    /// `workspace/didRenameFiles` afterwards. A server that does not advertise
    /// the capability simply returns nothing; the move still happens.
    ///
    /// # Errors
    /// Fails when either path is not absolute or the on-disk rename fails.
    pub async fn rename_file(&self, from: &Path, to: &Path) -> anyhow::Result<RenameReport> {
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
        // Start the server that owns the file before asking anyone: an import
        // rewrite is exactly what a server computes, and none of them can answer
        // for a file whose server never started.
        let owning = self.server_for(from);
        let mut imports_unverified = match &owning {
            None => Some(format!(
                "no language server is configured for `{}`, so imports were not updated",
                from.display()
            )),
            Some(server) => match self.ensure_client(from).await {
                Ok(_) => None,
                Err(err) => Some(format!(
                    "`{}` could not be started ({err}), so imports were not updated",
                    server.name
                )),
            },
        };
        // The owning client's roster key, so the answer can say whether THAT
        // server — not merely some live one — was asked.
        let owning_key = owning.as_ref().and_then(|server| {
            canonical_root(from, server)
                .ok()
                .map(|root| (server.name.clone(), root))
        });
        let clients = self.clients().await;
        let mut asked_owning = false;
        for client in &clients {
            // Only a server that ADVERTISED `workspace/willRenameFiles` can
            // compute import edits; a request it never registered is either
            // refused or answered with a meaningless `null`, and reading that
            // `null` as "no imports needed" is the same silent success this
            // function exists to prevent (review #14). The advertisement is the
            // distinction between "the server looked and found nothing" (a real
            // success) and "the server was never going to look".
            if !client_advertises_will_rename(client) {
                continue;
            }
            let key = (client.name.clone(), client.root.clone());
            asked_owning |= owning_key.as_ref() == Some(&key);
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
        // The answer must not claim more than was verified: when the file's OWN
        // server never answered `workspace/willRenameFiles` — because none was
        // live, or because the one that started does not implement the request —
        // say so instead of letting the caller report a complete refactor (the
        // prompt used to promise exactly that, review #14).
        if !asked_owning && imports_unverified.is_none() {
            let name = owning
                .as_ref()
                .map_or("no language server", |server| server.name.as_str());
            imports_unverified = Some(format!(
                "`{name}` does not implement `workspace/willRenameFiles`, so imports were not \
                 updated"
            ));
        }
        Ok(RenameReport {
            touched,
            imports_unverified,
        })
    }

    /// Remove the client owning `path` from the roster, shutting its server down first.
    ///
    /// Dropping the last `Arc` only aborts the client's mainloop (`impl Drop for
    /// LspClient`); the server process is left to die with its stdin, which means no
    /// `shutdown`/`exit` handshake and no `didClose` for the documents the client opened.
    /// A server that keeps a disk cache or writes a log at exit therefore sees a crash,
    /// not a close (review #46).
    pub async fn shutdown_for(&self, path: &Path) -> Option<Option<std::process::ExitStatus>> {
        let server = self.server_for(path)?;
        let root = canonical_root(path, &server).ok()?;
        let key = (server.name.clone(), root);
        // Take the client out under the lock, release it, then shut down: the handshake
        // awaits, and holding the roster write lock across it would stall every other
        // server's lookup.
        let client = {
            let mut guard = self.clients.write().await;
            guard.remove(&key)
        }?;
        let status = self.shut_down(&client).await;
        Some(status)
    }

    /// Run the handshake on `client` and report how its server exited.
    ///
    /// Shared by both eviction paths so the "a server that will not exit is a defect"
    /// rule lives in one place. Returns `None` when the server did not exit within the
    /// grace period.
    async fn shut_down(&self, client: &LspClient) -> Option<std::process::ExitStatus> {
        client.shutdown().await;
        let status = client.wait_exit().await;
        match status {
            Some(status) if status.success() => {}
            Some(status) => tracing::warn!(
                target: "pa_lsp",
                "language server exited with {status} after shutdown; it may not have flushed state"
            ),
            None => tracing::warn!(
                target: "pa_lsp",
                "language server did not exit within the grace period after shutdown"
            ),
        }
        status
    }

    /// Shut every client down cleanly, then empty the roster.
    pub async fn shutdown_all(&self) {
        // Same reasoning as `shutdown_for`, for every entry: drain under the lock, handshake
        // outside it.
        let clients: Vec<_> = {
            let mut guard = self.clients.write().await;
            guard.drain().map(|(_, client)| client).collect()
        };
        for client in clients {
            self.shut_down(&client).await;
        }
    }
}

/// Whether `client` asked for `workspace/willRenameFiles` in its `initialize` answer.
///
/// Only that advertisement makes the request meaningful: a server that never
/// registered it answers `MethodNotFound`, and the permissive ones answer `null`,
/// which is indistinguishable from "looked, found no imports". Sending the
/// request anyway and reading `null` as success is what let a rename report a
/// complete refactor it never computed (review #14).
fn client_advertises_will_rename(client: &LspClient) -> bool {
    client
        .capabilities()
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.file_operations.as_ref())
        .is_some_and(|operations| operations.will_rename.is_some())
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
        assert!(LspManager::known_servers()
            .iter()
            .any(|name| name == "rust-analyzer"));
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

    /// Review #14: a rename must NAME its import gap instead of implying a complete
    /// refactor. The old path asked only the already-live clients, so a first-operation
    /// rename moved the file, touched no import, and returned success.
    ///
    /// Two deterministic shapes: an extension with no configured server at all, and a
    /// server whose binary cannot be spawned.
    #[tokio::test]
    async fn a_rename_that_cannot_rewrite_imports_says_so() {
        // (a) No server is configured for the extension.
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("module.zzz");
        let to = dir.path().join("renamed.zzz");
        std::fs::write(&from, "content\n").unwrap();
        let report = LspManager::new()
            .rename_file(&from, &to)
            .await
            .expect("the move itself");
        assert!(to.exists() && !from.exists(), "the file moved");
        let reason = report
            .imports_unverified
            .expect("an unowned extension cannot rewrite imports");
        assert!(
            reason.contains("no language server is configured"),
            "the reason must name the gap: {reason}"
        );

        // (b) A configured server that cannot be spawned.
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("module.rs");
        let to = dir.path().join("renamed.rs");
        std::fs::write(&from, "pub fn kept() {}\n").unwrap();
        let overrides = serde_json::json!({
            "rust-analyzer": { "command": "definitely-not-a-language-server" }
        });
        let manager = LspManager::new().with_server_overrides(overrides.as_object());
        let report = manager
            .rename_file(&from, &to)
            .await
            .expect("the move itself");
        assert!(to.exists(), "the file moved");
        let reason = report
            .imports_unverified
            .expect("a server that cannot start cannot rewrite imports");
        assert!(
            reason.contains("could not be started"),
            "the reason must name the failed server: {reason}"
        );
    }

    /// The three answers a live server can give `workspace/willRenameFiles`, as a
    /// stdio LSP script. `advertise` asked for the request and answered "no edits";
    /// `silent` never asked for it and still answers `null`; `refuse` never asked
    /// and answers `MethodNotFound` (a real server without the capability).
    const FAKE_LSP_SERVER: &str = r#"#!/usr/bin/env python3
import json
import sys

MODE = sys.argv[1] if len(sys.argv) > 1 else "silent"


def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        key, _, value = line.partition(b":")
        headers[key.strip().lower()] = value.strip()
    return json.loads(sys.stdin.buffer.read(int(headers.get(b"content-length", b"0"))))


def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()


while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        capabilities = {}
        if MODE == "advertise":
            capabilities["workspace"] = {
                "fileOperations": {
                    "willRename": {"filters": [{"pattern": {"glob": "**"}}]}
                }
            }
        send({"jsonrpc": "2.0", "id": request_id,
              "result": {"capabilities": capabilities,
                         "serverInfo": {"name": "fake-lsp"}}})
    elif method == "workspace/willRenameFiles":
        if MODE == "refuse":
            send({"jsonrpc": "2.0", "id": request_id,
                  "error": {"code": -32601, "message": "method not found"}})
        else:
            send({"jsonrpc": "2.0", "id": request_id, "result": None})
    elif method is not None and request_id is not None:
        send({"jsonrpc": "2.0", "id": request_id, "result": None})
"#;

    /// A manager whose `.fake` files are served by [`FAKE_LSP_SERVER`] in `mode`.
    ///
    /// `python3` is the interpreter the repo's other stdio fixtures use; its
    /// absence would be an environment defect, so the test fails rather than skips.
    fn fake_server_manager(dir: &std::path::Path, mode: &str) -> LspManager {
        let script = dir.join("fake-lsp.py");
        std::fs::write(&script, FAKE_LSP_SERVER).expect("write the fake server");
        let python = [
            "/usr/bin/python3",
            "/usr/local/bin/python3",
            "/opt/homebrew/bin/python3",
        ]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .unwrap_or("python3");
        let overrides = serde_json::json!({
            "fake-lsp": {
                "command": python,
                "args": [script.to_string_lossy(), mode],
                "extensions": ["fake"],
                "languageId": "plaintext",
            }
        });
        LspManager::new().with_server_overrides(overrides.as_object())
    }

    /// A CONFIGURED server that starts and answers is not enough: a rename must
    /// know whether that server actually implements `workspace/willRenameFiles`
    /// (review #14, the remaining half).
    ///
    /// The previous code started the owning server and then treated ANY answer —
    /// including the `null` of a server that never registered the request — as
    /// "imports are fine", so a project whose only server lacks the capability got
    /// a reported complete refactor with no import touched. The advertisement is
    /// the difference between "the server looked" and "the server was never asked".
    ///
    /// Red on the pre-fix code: both cases returned `imports_unverified == None`.
    #[tokio::test]
    async fn a_server_that_does_not_implement_the_rename_request_is_not_trusted() {
        for mode in ["silent", "refuse"] {
            let dir = tempfile::tempdir().unwrap();
            let manager = fake_server_manager(dir.path(), mode);
            let from = dir.path().join("module.fake");
            let to = dir.path().join("renamed.fake");
            std::fs::write(&from, "content\n").unwrap();

            let report = manager
                .rename_file(&from, &to)
                .await
                .unwrap_or_else(|error| panic!("[{mode}] the move itself must succeed: {error}"));
            assert!(to.exists() && !from.exists(), "[{mode}] the file moved");
            let reason = report.imports_unverified.unwrap_or_else(|| {
                panic!("[{mode}] a server without willRenameFiles cannot have verified imports")
            });
            assert!(
                reason.contains("does not implement") && reason.contains("fake-lsp"),
                "[{mode}] the reason must name the gap and the server: {reason}"
            );
            manager.shutdown_all().await;
        }
    }

    /// The other half of the same contract: a server that DID advertise the
    /// request and answered "no edits" is a real success, so the report stays
    /// quiet. Without this the previous test could be satisfied by never
    /// trusting any server, which is a different lie.
    #[tokio::test]
    async fn a_server_that_answered_the_rename_request_is_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let manager = fake_server_manager(dir.path(), "advertise");
        let from = dir.path().join("module.fake");
        let to = dir.path().join("renamed.fake");
        std::fs::write(&from, "content\n").unwrap();

        let report = manager.rename_file(&from, &to).await.expect("the move");
        assert!(to.exists() && !from.exists(), "the file moved");
        assert_eq!(
            report.imports_unverified, None,
            "an advertised request that answered is a verified rewrite"
        );
        manager.shutdown_all().await;
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
    /// The property the roster gate exists for, and the one the previous shape
    /// violated: two DIFFERENT keys never wait on each other, while the SAME key
    /// does.
    ///
    /// This is the whole reason `ensure_client` no longer holds `clients.write()`
    /// across `LspClient::spawn`: that lock serialized every project behind
    /// whichever language server was starting.
    #[tokio::test]
    async fn different_keys_never_wait_on_each_others_acquisition() {
        let gate = KeyedGate::<(String, PathBuf)>::default();
        let key_a = ("rust-analyzer".to_string(), PathBuf::from("/a"));
        let key_b = ("pyright".to_string(), PathBuf::from("/b"));

        // A slow holder on A (a spawn in flight).
        let held_a = gate.acquire(key_a.clone()).await;

        // B must not wait: 50 ms is far below A's hold, so a global lock would
        // time out here.
        let b = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            gate.acquire(key_b.clone()),
        )
        .await
        .expect("a different key must not wait on the first key's acquisition");
        drop(b);

        // The SAME key waits until the holder releases.
        let same = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            gate.acquire(key_a.clone()),
        )
        .await;
        assert!(
            same.is_err(),
            "a second acquisition of the same key must wait: that is the single-flight"
        );

        drop(held_a);

        // Released, the wait completes and the map entry retires.
        let after = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            gate.acquire(key_a.clone()),
        )
        .await
        .expect("the released gate admits the next acquisition");
        drop(after);
    }

    /// The gate retires its entry on the last release: a daemon that has opened
    /// a thousand session files must not carry a thousand mutexes forever.
    #[tokio::test]
    async fn the_gate_retires_its_entry_when_the_last_holder_leaves() {
        let gate = KeyedGate::<(String, PathBuf)>::default();
        let key = ("rust-analyzer".to_string(), PathBuf::from("/a"));
        {
            let _held = gate.acquire(key.clone()).await;
            assert_eq!(
                gate.gates
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len(),
                1
            );
        }
        assert_eq!(
            gate.gates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            0,
            "the last release must retire the entry"
        );
    }

    /// `lsp.idleTimeoutMs` had no reader: a live client lived until the session
    /// ended, however long the agent worked elsewhere. The decision is a pure
    /// function of the stamps, so it is testable without spawning a server.
    #[tokio::test]
    async fn the_idle_roster_separates_a_stale_key_from_a_used_one() {
        let clock = MockClock::new();
        let roster = IdleRoster::default();
        let stale = ("stale".to_string(), PathBuf::from("/stale"));
        let busy = ("busy".to_string(), PathBuf::from("/busy"));
        roster.mark(&stale, clock.now()).await;
        roster.mark(&busy, clock.now()).await;

        // Inside the bound: nothing is stale.
        clock.advance(Duration::from_mins(4));
        assert_eq!(
            roster.stale(clock.now(), Duration::from_mins(5)).await,
            Vec::new(),
            "inside the bound"
        );

        // The busy key is used again; the stale one is not.
        roster.mark(&busy, clock.now()).await;
        clock.advance(Duration::from_mins(2));
        let expired = roster.stale(clock.now(), Duration::from_mins(5)).await;
        assert_eq!(expired, vec![stale], "exactly the idle key expires");
    }

    /// The manager-level wiring: no bound means the sweep is inert (the
    /// pre-change lifetime, byte for byte), and a bound is what the setting
    /// produces.
    #[tokio::test]
    async fn no_bound_means_the_sweep_is_inert() {
        let manager = LspManager::new();
        assert_eq!(manager.idle_timeout(), None);
        assert_eq!(manager.evict_idle().await, 0);
        assert_eq!(manager.live_count().await, 0);

        let clock = MockClock::new();
        let bounded = LspManager::with_idle_timeout(Duration::from_mins(5), clock);
        assert_eq!(bounded.idle_timeout(), Some(Duration::from_mins(5)));
        assert_eq!(
            bounded.evict_idle().await,
            0,
            "an empty roster evicts nothing"
        );
    }
}
