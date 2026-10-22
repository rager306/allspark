//! Cross-process writer lease for one graph database.
//!
//! Every session defaults to the same database (`~/.prime/agent/graph.db`), and
//! each session's `GraphTool` singles-flights the *cold-start build* on an
//! `AtomicBool` it owns — per session. Two sessions therefore both start a full
//! walk of the same tree, and SQLite serialises only the writes: the outcome is
//! N redundant walks and lock waits, not corruption.
//!
//! The lease is the missing cross-process half of that guard: an exclusive
//! advisory lock on `<db>.index.lock`, taken by a writer before it registers or
//! writes anything, released by [`IndexLease`]'s destructor. The OS drops the
//! lock when the holding process dies, so a crashed indexer never leaves a stale
//! lease behind and there is no reclaim protocol to get wrong.
//!
//! Readers never take it. The store runs in WAL mode, where a read is consistent
//! while a writer appends, so requiring a shared lease on the read path would
//! only turn "an index is running" into "search is blocked".
//!
//! ```no_run
//! use pa_graph::lease::{CancelToken, IndexLease};
//! use std::time::Duration;
//!
//! let lease = IndexLease::acquire("graph.db", Duration::from_secs(300), &CancelToken::new())?;
//! // … walk, parse, write …
//! drop(lease);
//! # Ok::<(), pa_graph::lease::LeaseError>(())
//! ```

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Suffix appended to the database path to name its lease file.
pub const LEASE_SUFFIX: &str = ".index.lock";

/// How long an index run waits for another writer before giving up.
///
/// Long enough for a full symbol-depth pass over a large repository (measured at
/// ~135 s for this tree's 2,743 files), so the common case — a second session
/// arriving mid-build — waits and then finds the work already done, rather than
/// failing. Finite, so a wedged holder costs one bounded tool call instead of a
/// session that hangs forever.
pub const DEFAULT_LEASE_WAIT: Duration = Duration::from_secs(300);

/// How often a waiting writer retries the non-blocking lock.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A cooperative "stop waiting" flag, shared between a caller and an index run.
///
/// The store does not poll it on its own (an index run is synchronous); it exists
/// so a caller that is shutting down — a session teardown, a supervisor draining —
/// can abort a *wait for the lease* instead of sitting out the whole deadline.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// A token that nobody cancels.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask every [`IndexLease::acquire`] waiting on this token to stop.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Whether [`CancelToken::cancel`] has been called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Who the lease file says is holding the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// The pid the holder wrote into the lease file on acquire.
    Pid(u32),
    /// The lock is taken but the file carries no readable pid (a holder that has
    /// not written it yet, or one that is releasing right now).
    Unknown,
}

impl std::fmt::Display for Holder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pid(pid) => write!(f, "held by pid {pid}"),
            Self::Unknown => f.write_str("holder pid unrecorded"),
        }
    }
}

/// Why a writer did not get the lease.
///
/// The variants are distinct states, not one message: a caller decides whether to
/// report "another session is indexing" (timeout) or "we are shutting down"
/// (cancelled).
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// Another process held the lock for the whole deadline.
    #[error(
        "timed out after {waited:?} waiting for the index writer lease at {lock_path} ({holder})"
    )]
    TimedOut {
        /// The lease file that stayed locked.
        lock_path: PathBuf,
        /// How long this caller waited.
        waited: Duration,
        /// The holder recorded in the lease file.
        holder: Holder,
    },
    /// The cancel token fired before the lock was free.
    #[error("cancelled after {waited:?} waiting for the index writer lease at {lock_path}")]
    Cancelled {
        /// The lease file that stayed locked.
        lock_path: PathBuf,
        /// How long this caller waited before giving up.
        waited: Duration,
    },
    /// The lease file could not be created, opened, or locked.
    #[error("index writer lease at {lock_path} failed: {source}")]
    Io {
        /// The lease file involved.
        lock_path: PathBuf,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
}

impl LeaseError {
    /// The lease file this failure is about.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        match self {
            Self::TimedOut { lock_path, .. }
            | Self::Cancelled { lock_path, .. }
            | Self::Io { lock_path, .. } => lock_path,
        }
    }

    /// The holder pid recorded in the lease file, when one could be read.
    #[must_use]
    pub fn holder_pid(&self) -> Option<u32> {
        match self {
            Self::TimedOut {
                holder: Holder::Pid(pid),
                ..
            } => Some(*pid),
            Self::TimedOut { .. } | Self::Cancelled { .. } | Self::Io { .. } => None,
        }
    }
}

/// Fold a lease failure into the `io` domain the store's error type carries.
///
/// A timeout becomes [`std::io::ErrorKind::TimedOut`] and a cancellation
/// [`std::io::ErrorKind::Interrupted`], so a caller that only sees
/// [`crate::GraphError::Io`] can still tell the two apart, and the message keeps
/// the lease path and the holder's pid.
impl From<LeaseError> for std::io::Error {
    fn from(err: LeaseError) -> Self {
        let kind = match &err {
            LeaseError::TimedOut { .. } => std::io::ErrorKind::TimedOut,
            LeaseError::Cancelled { .. } => std::io::ErrorKind::Interrupted,
            LeaseError::Io { .. } => std::io::ErrorKind::Other,
        };
        Self::new(kind, err)
    }
}

/// Exclusive, database-scoped right to index. Released on drop.
///
/// The guard holds the open file, not just its path: closing the file would drop
/// the lock, so the lease and the lock have the same lifetime by construction.
#[derive(Debug)]
pub struct IndexLease {
    held: Option<Held>,
}

#[derive(Debug)]
struct Held {
    file: File,
    path: PathBuf,
}

impl IndexLease {
    /// Take the writer lease for the database at `db_path`.
    ///
    /// Waits up to `wait` for another writer to finish. `Duration::ZERO` means
    /// "do not wait at all": one attempt, then [`LeaseError::TimedOut`].
    ///
    /// # Errors
    ///
    /// [`LeaseError::TimedOut`] when the lock stayed held for `wait`,
    /// [`LeaseError::Cancelled`] when `cancel` fired first, and [`LeaseError::Io`]
    /// when the lease file cannot be created or locked.
    pub fn acquire(
        db_path: impl AsRef<Path>,
        wait: Duration,
        cancel: &CancelToken,
    ) -> Result<Self, LeaseError> {
        let lock_path = lease_path(db_path.as_ref());
        if let Some(parent) = lock_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| LeaseError::Io {
                lock_path: lock_path.clone(),
                source,
            })?;
        }
        // One open, retried: `try_lock` on a handle that lost the race is a
        // fresh attempt, so a waiter needs no second descriptor. The handle is
        // dropped with this function on every failure path, which releases
        // nothing (it never locked) and closes the file.
        let file = open_lease_file(&lock_path).map_err(|source| LeaseError::Io {
            lock_path: lock_path.clone(),
            source,
        })?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => {
                    let held = Held {
                        file,
                        path: lock_path,
                    };
                    record_holder(&held);
                    return Ok(Self { held: Some(held) });
                }
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(source)) => {
                    return Err(LeaseError::Io { lock_path, source });
                }
            }
            let waited = started.elapsed();
            if cancel.is_cancelled() {
                return Err(LeaseError::Cancelled { lock_path, waited });
            }
            if waited >= wait {
                return Err(LeaseError::TimedOut {
                    holder: read_holder(&lock_path),
                    lock_path,
                    waited,
                });
            }
            std::thread::sleep(POLL_INTERVAL.min(wait.saturating_sub(waited)));
        }
    }

    /// A lease for a store that has no database file (an in-memory store).
    ///
    /// An in-memory store cannot be shared with another process, so there is no
    /// writer to serialise against and nothing to create on disk.
    #[must_use]
    pub fn unshared() -> Self {
        Self { held: None }
    }

    /// The lease file this guard holds, if it holds one.
    #[must_use]
    pub fn lock_path(&self) -> Option<&Path> {
        self.held.as_ref().map(|held| held.path.as_path())
    }
}

impl Drop for IndexLease {
    fn drop(&mut self) {
        let Some(held) = self.held.take() else {
            return;
        };
        // Clear the pid *before* releasing: a waiter that times out a moment
        // later must not blame a process that no longer holds anything.
        let _ = held.file.set_len(0);
        let _ = held.file.unlock();
    }
}

/// The lease file for `db_path`: `<db>.index.lock`, beside the database.
#[must_use]
pub fn lease_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push(LEASE_SUFFIX);
    PathBuf::from(name)
}

/// Open (creating if absent) the lease file, owner-only on creation.
fn open_lease_file(lock_path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(lock_path)
}

/// Write this process's pid into the held lease file, so a caller that loses the
/// race can name the winner.
fn record_holder(held: &Held) {
    let _ = held.file.set_len(0);
    let mut file = &held.file;
    let _ = file.write_all(format!("{}\n", std::process::id()).as_bytes());
    let _ = file.flush();
}

/// Read the holder pid out of the lease file; unreadable or empty means unknown.
fn read_holder(lock_path: &Path) -> Holder {
    std::fs::read_to_string(lock_path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .map_or(Holder::Unknown, Holder::Pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GraphError;
    use crate::indexer::IndexOptions;
    use crate::store::GraphStore;
    use std::process::{Child, Command, Stdio};

    /// Env var that turns this test binary into a lease holder. Set by
    /// [`spawn_holder`] only, so the fixture contributes nothing to a plain run.
    const HOLDER_CHILD: &str = "PA_GRAPH_LEASE_HOLDER_CHILD";

    /// Child side of [`a_second_writer_waits_for_the_first`]: take the lease on
    /// `<dir>/graph.db`, publish the pid, and hold the lock until the parent
    /// creates `<dir>/release`. A real second process is the point — the defect
    /// is cross-process, and `in_flight` is per session.
    #[test]
    fn holder_child_process() {
        let Some(dir) = std::env::var_os(HOLDER_CHILD).map(PathBuf::from) else {
            return;
        };
        let lease = IndexLease::acquire(
            dir.join("graph.db"),
            Duration::from_secs(10),
            &CancelToken::new(),
        )
        .expect("child takes the lease");
        std::fs::write(dir.join("ready"), format!("{}\n", std::process::id())).expect("ready");
        let deadline = Instant::now() + Duration::from_secs(60);
        while !dir.join("release").exists() {
            assert!(
                Instant::now() < deadline,
                "parent never asked the child to stop"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(lease);
    }

    /// Start the holder child and wait until it holds the lease. Returns the
    /// child and the pid it published.
    fn spawn_holder(dir: &Path) -> (Child, u32) {
        let child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "lease::tests::holder_child_process",
                "--test-threads=1",
            ])
            .env(HOLDER_CHILD, dir)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn the holder child");
        let ready = dir.join("ready");
        let deadline = Instant::now() + Duration::from_secs(60);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "child {} never took the lease",
                child.id()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let pid = std::fs::read_to_string(&ready)
            .expect("ready")
            .trim()
            .parse()
            .expect("pid");
        (child, pid)
    }

    /// Create the release file and require the child to exit cleanly.
    fn release_holder(dir: &Path, mut child: Child) {
        std::fs::write(dir.join("release"), "").expect("release");
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                assert!(status.success(), "holder child failed: {status}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "holder child {} did not exit",
                child.id()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn error_kind(err: &GraphError) -> std::io::ErrorKind {
        match err {
            GraphError::Io(io) => io.kind(),
            other => panic!("expected an io error from the lease, got {other}"),
        }
    }

    /// The lease failure the store wrapped in [`GraphError::Io`], so a test can
    /// assert on its variants rather than on the message.
    fn lease_error(err: &GraphError) -> &LeaseError {
        let GraphError::Io(io) = err else {
            panic!("expected an io error from the lease, got {err}");
        };
        io.get_ref()
            .and_then(|source| source.downcast_ref::<LeaseError>())
            .expect("the io error carries the lease failure")
    }

    #[test]
    fn a_second_writer_waits_for_the_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("a.rs"),
            "pub fn a() { b() }\npub fn b() {}\n",
        )
        .expect("a.rs");
        let (child, child_pid) = spawn_holder(dir.path());
        let db = dir.path().join("graph.db");

        // 1. The lease itself: a second holder times out, names the lock file
        //    and the pid the holder wrote into it.
        let started = Instant::now();
        let err = IndexLease::acquire(&db, Duration::from_millis(300), &CancelToken::new())
            .expect_err("a held lease must not be granted");
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "acquire returned after {:?}, before the deadline",
            started.elapsed()
        );
        match err {
            LeaseError::TimedOut {
                lock_path, holder, ..
            } => {
                assert_eq!(lock_path, lease_path(&db), "the error names the lease file");
                assert_eq!(holder, Holder::Pid(child_pid), "the error names the holder");
            }
            other => panic!("expected a timeout, got {other}"),
        }

        // 2. And a writer does not proceed: the run fails instead of walking
        //    the tree a second time, and nothing was registered or written.
        let opts = IndexOptions::new("contended").with_lease_wait(Duration::from_millis(200));
        let mut store = GraphStore::open(&db).expect("open the store while the lease is held");
        let err = store
            .index_project(dir.path(), &opts)
            .expect_err("a contended index run must not proceed");
        assert_eq!(error_kind(&err), std::io::ErrorKind::TimedOut, "{err}");
        let lease_err = lease_error(&err);
        assert_eq!(lease_err.holder_pid(), Some(child_pid));
        assert_eq!(lease_err.lock_path(), lease_path(&db));
        let message = err.to_string();
        assert!(
            message.contains(child_pid.to_string().as_str()),
            "the message names the holder pid: {message}"
        );
        assert_eq!(
            store.list_projects().expect("projects"),
            Vec::<String>::new(),
            "the run registered a project before it owned the lease"
        );

        release_holder(dir.path(), child);

        // 3. Once the first writer is gone the second one proceeds, and a
        //    following run finds the tree already walked.
        let report = store
            .index_project(dir.path(), &opts)
            .expect("index after release");
        assert_eq!(report.project, "contended");
        assert_eq!((report.added, report.parsed), (1, 1), "{report:?}");
        let again = store.index_project(dir.path(), &opts).expect("second run");
        assert_eq!(
            (again.added, again.parsed, again.unchanged),
            (0, 0, 1),
            "{again:?}"
        );
    }

    #[test]
    fn the_incremental_path_takes_the_same_lease() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        let (child, child_pid) = spawn_holder(dir.path());

        let opts = IndexOptions::new("incremental").with_lease_wait(Duration::from_millis(200));
        let mut store = GraphStore::open(&db).expect("open the store");
        let err = store
            .reindex_incremental(dir.path(), &opts)
            .expect_err("a contended reindex must not proceed");
        assert_eq!(error_kind(&err), std::io::ErrorKind::TimedOut, "{err}");
        assert_eq!(lease_error(&err).holder_pid(), Some(child_pid));
        assert!(
            err.to_string().contains(child_pid.to_string().as_str()),
            "the error names the holder pid: {err}"
        );
        assert_eq!(
            store.list_projects().expect("projects"),
            Vec::<String>::new(),
            "the reindex registered a project before it owned the lease"
        );

        release_holder(dir.path(), child);
        store
            .reindex_incremental(dir.path(), &opts)
            .expect("reindex after release");
    }

    #[test]
    fn the_lease_is_released_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        let lock_path = lease_path(&db);

        let first = IndexLease::acquire(&db, Duration::ZERO, &CancelToken::new()).expect("acquire");
        assert_eq!(first.lock_path(), Some(lock_path.as_path()));
        // The holder writes its pid, which is what a loser of the race reports.
        assert_eq!(
            std::fs::read_to_string(&lock_path).expect("lease file"),
            format!("{}\n", std::process::id())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&lock_path)
                .expect("lease file")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the lease file is owner-only");
        }

        drop(first);
        IndexLease::acquire(&db, Duration::ZERO, &CancelToken::new())
            .expect("the drop released the lease");
        // An in-memory store has no other writer, so it holds no lock file.
        assert_eq!(IndexLease::unshared().lock_path(), None);
    }

    #[test]
    fn the_lease_is_released_on_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        let mut store = GraphStore::open(&db).expect("open");

        // A missing root does not fail (the walk is simply empty), so force a
        // genuine failure *inside* the run: a read-only connection fails at
        // `register_project`, which runs after the lease is taken.
        store
            .connection()
            .pragma_update(None, "query_only", true)
            .expect("query_only");
        let err = store
            .index_project(dir.path(), &IndexOptions::new("readonly"))
            .expect_err("a read-only connection cannot index");
        assert!(matches!(err, GraphError::Sqlite(_)), "{err}");
        store
            .connection()
            .pragma_update(None, "query_only", false)
            .expect("query_only off");

        IndexLease::acquire(&db, Duration::ZERO, &CancelToken::new())
            .expect("the error path released the lease");
        let report = store
            .index_project(dir.path(), &IndexOptions::new("readonly"))
            .expect("the store still indexes after the failure");
        assert_eq!(report.project, "readonly");
    }

    #[test]
    fn a_cancelled_writer_reports_cancellation_not_a_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        let _held = IndexLease::acquire(&db, Duration::ZERO, &CancelToken::new()).expect("acquire");

        let cancel = CancelToken::new();
        let waiter = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            waiter.cancel();
        });
        let started = Instant::now();
        let err = IndexLease::acquire(&db, Duration::from_secs(30), &cancel)
            .expect_err("a cancelled waiter must not get the lease");
        assert!(
            matches!(err, LeaseError::Cancelled { .. }),
            "cancellation is its own state, not a timeout: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancellation returned after {:?}",
            started.elapsed()
        );
        assert_eq!(err.holder_pid(), None);
        assert_eq!(err.lock_path(), lease_path(&db));
    }
}
