//! FORK-ONLY: `graph.auto = "watch"` — live filesystem observation feeding the
//! graph's incremental reindex.
//!
//! The setting (`GraphSettings::auto`) always accepted `"watch"`, but nothing
//! read it: the value behaved exactly like `auto`, and an edit reached the graph
//! only when the next query happened to find the store stale. This module is the
//! consumer that value promised.
//!
//! Shape of the wiring, and why:
//!
//! * One watcher per session, started at `create_session` when
//!   `graph.auto == "watch"` and `graph.enabled` (the tool must exist for the
//!   graph to be worth keeping warm).
//! * The watcher is `pa_graph::NativeWatcher` — the OS facility — and its events
//!   are COALESCED: an editor writing a file emits several events, and a
//!   reindex per event would parse the same file a half-dozen times. The loop
//!   drains until quiet, then runs one `reindex_incremental` for the batch.
//! * The reindex takes the writer lease with a ZERO wait
//!   (`IndexOptions::with_lease_wait(ZERO)`). A watcher is a background
//!   convenience, and a session that blocks behind another process's build for
//!   seconds is the defect the read path already avoids.
//! * A busy lease is RETRIED, not dropped. The batch stays outstanding and the
//!   loop backs off (500 ms doubling to 30 s) until the lease frees. Dropping it
//!   was wrong for the one case that matters: a session that edits, loses the
//!   race, and then stops editing leaves the graph stale until some query
//!   happens to notice — with no further event to trigger a repair. Only a
//!   genuine failure (a parse error, a missing database) ends a batch early,
//!   because those do not fix themselves.
//! * Shutdown is a `CancelToken`: the session's teardown cancels it, the loop
//!   returns, and the `NativeWatcher` is dropped (which unwatches).
//!
//! What this deliberately does not do: no debounce timer thread per event, no
//! persistent watch registry across sessions, no watching outside the session's
//! own `cwd`. A session watches the tree it was started in.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pa_graph::lease::CancelToken;

/// The quiet window that ends an event batch: the loop keeps draining until no
/// event arrives for this long, so a multi-event write is one reindex.
const SETTLE: Duration = Duration::from_millis(250);

/// How often the loop checks the cancel token while no events arrive. The
/// watcher's channel is blocking, so the poll interval is what bounds shutdown
/// latency; it is also the idle cost of a watching session (a `try_recv`).
const TICK: Duration = Duration::from_millis(250);

/// A started watch: the cancel token that stops it and the task handle.
///
/// Dropping the handle does NOT stop the task (a spawned task owns itself);
/// [`WatchHandle::stop`] is the shutdown path, and [`SessionEngine`] calls it.
///
/// [`SessionEngine`]: crate::session_engine::engine::SessionEngine
#[derive(Debug)]
pub struct WatchHandle {
    cancel: CancelToken,
    /// The loop's task, taken by [`WatchHandle::stop`]; a second `stop` finds
    /// `None` and does nothing.
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl WatchHandle {
    /// Ask the loop to stop and wait (bounded) for it to notice.
    ///
    /// The token is shared with any in-flight `IndexLease::acquire`, so a
    /// pending lease wait is cut short instead of sitting out its deadline.
    /// Takes `&self` because the engine tears down through `&self`; the task
    /// handle is behind a mutex so the stop is also idempotent.
    pub async fn stop(&self) {
        self.cancel.cancel();
        let task = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(task) = task else {
            return;
        };
        // The loop polls the token every `TICK`; a reindex in flight is bounded
        // by the index itself, not by this wait, so the timeout is a guard
        // against a wedged task and not the normal path.
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    }
}

/// Start watching `root` and keep the graph at `db_path` current.
///
/// Returns `None` when the OS watcher cannot be created (an unsupported
/// platform, a root that vanished, a watch limit): watching is an optimisation,
/// and a session that cannot watch must still run. The failure is logged, not
/// propagated — the query-time staleness check remains the fallback, exactly as
/// in `auto`.
#[must_use]
pub fn start(root: &Path, db_path: &Path, project: &str) -> Option<WatchHandle> {
    let watcher = match pa_graph::NativeWatcher::new(root) {
        Ok(watcher) => watcher,
        Err(err) => {
            tracing::warn!(
                "graph watch: cannot watch {} ({err}); falling back to query-time staleness",
                root.display()
            );
            return None;
        }
    };
    let cancel = CancelToken::new();
    let loop_cancel = cancel.clone();
    let root = root.to_path_buf();
    let db_path = db_path.to_path_buf();
    let project = project.to_string();
    let task = tokio::spawn(async move {
        run(watcher, &root, &db_path, &project, &loop_cancel).await;
    });
    Some(WatchHandle {
        cancel,
        task: std::sync::Mutex::new(Some(task)),
    })
}

/// Whether an event path is one the graph could index.
///
/// `pa-graph` owns the language table (`is_indexable`), so this asks it rather
/// than keeping a second extension list here. Directories and `.git` paths are
/// already filtered by `NativeWatcher::drain`; this only rejects non-source
/// files, which a recursive watch reports constantly (build output, editor swap
/// files).
fn is_source(path: &Path) -> bool {
    pa_graph::is_indexable(path)
}

/// The watch loop: coalesce a burst, reindex once, repeat until cancelled.
async fn run(
    watcher: pa_graph::NativeWatcher,
    root: &Path,
    db_path: &Path,
    project: &str,
    cancel: &CancelToken,
) {
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let events = watcher.drain();
        if events.is_empty() {
            tokio::time::sleep(TICK).await;
            continue;
        }
        let mut changed = events.iter().filter(|event| is_source(&event.path)).count();
        if changed == 0 {
            continue;
        }
        // Coalesce: keep draining until the burst goes quiet, so one save that
        // emits create+modify+attrib is one reindex rather than three.
        loop {
            tokio::time::sleep(SETTLE).await;
            if cancel.is_cancelled() {
                return;
            }
            let more = watcher.drain();
            if more.is_empty() {
                break;
            }
            changed += more.iter().filter(|event| is_source(&event.path)).count();
        }
        // The batch stays OUTSTANDING until it is applied. A busy lease used to
        // end the story here; instead the same batch is retried with backoff,
        // and only a real outcome (applied, or a failure a retry cannot fix)
        // clears it. A new event arriving during the wait is absorbed by the
        // next drain, which is correct: the reindex re-derives changes from file
        // hashes, so a superset batch subsumes the events it missed.
        let mut backoff = RETRY_MIN;
        loop {
            match reindex(root, db_path, project, changed, cancel).await {
                ReindexOutcome::Done => break,
                ReindexOutcome::LeaseBusy => {
                    tokio::time::sleep(backoff).await;
                    if cancel.is_cancelled() {
                        return;
                    }
                    backoff = (backoff * 2).min(RETRY_MAX);
                }
            }
        }
    }
}

/// What one reindex attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReindexOutcome {
    /// The batch was applied, or failed for a reason a retry cannot fix.
    Done,
    /// Another writer held the lease: the batch is STILL OUTSTANDING.
    LeaseBusy,
}

/// Whether an index error is the "someone else is writing" case.
///
/// Only a lease TIMEOUT counts. A parse error, a missing database or an I/O
/// failure must not be retried on a timer: those fail the same way every time
/// and would spin forever. A CANCELLED lease wait is not retried either — that
/// is this session's own token firing during shutdown, which the loop already
/// handles by returning.
fn is_lease_busy(err: &str) -> bool {
    err.contains("index writer lease") && err.contains("timed out")
}

/// How long the watch loop waits before retrying a batch whose lease was busy.
///
/// The lease is held for the duration of one index pass, which is seconds on a
/// large tree, so this starts small and doubles: a short hold is retried almost
/// immediately, and a build that keeps the lease for a minute is not hammered.
const RETRY_MIN: Duration = Duration::from_millis(500);
/// Ceiling for the retry backoff. Past this the lease is held by something long
/// enough that polling faster buys nothing.
const RETRY_MAX: Duration = Duration::from_secs(30);

/// One incremental pass over the batch's tree.
///
/// The pass re-discovers the tree (the hashes decide what actually changed), so
/// a created or deleted file needs no special case here: `reindex_incremental`
/// classifies added/removed/changed from the store's own records.
async fn reindex(
    root: &Path,
    db_path: &Path,
    project: &str,
    events: usize,
    cancel: &CancelToken,
) -> ReindexOutcome {
    let root = root.to_path_buf();
    let db_path = db_path.to_path_buf();
    let project = project.to_string();
    let cancel = cancel.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut store = pa_graph::GraphStore::open(&db_path)
            .map_err(|err| format!("open {}: {err}", db_path.display()))?;
        store
            .reindex_incremental(
                &root,
                &pa_graph::IndexOptions::new(&project)
                    .with_respect_gitignore(true)
                    .with_lease_wait(Duration::ZERO)
                    .with_lease_cancel(cancel),
            )
            .map_err(|err| format!("reindex: {err}"))
    })
    .await;
    match result {
        Ok(Ok(report)) => {
            tracing::debug!(
                "graph watch: {events} event(s) -> {} file(s) re-parsed, {} dependent(s), {} removed",
                report.files_reindexed,
                report.closure_files,
                report.removed
            );
            ReindexOutcome::Done
        }
        // The lease is BUSY, not failed: another process is indexing right now.
        // The batch is still outstanding — this loop used to drop it and wait
        // for the next filesystem event, which for a session that then stops
        // editing means the graph stays stale until a query happens to notice.
        Ok(Err(err)) if is_lease_busy(&err) => {
            tracing::debug!("graph watch: writer lease busy, retrying the batch ({err})");
            ReindexOutcome::LeaseBusy
        }
        Ok(Err(err)) => {
            tracing::warn!("graph watch: batch dropped ({err})");
            ReindexOutcome::Done
        }
        Err(err) => {
            tracing::warn!("graph watch: reindex task failed ({err})");
            ReindexOutcome::Done
        }
    }
}

/// The project key a session's `cwd` maps to: the directory's basename, the same
/// rule `GraphTool` and the start-up auto-index use. Two checkouts with the same
/// basename collide by design (`register_project_at` refuses a different root).
#[must_use]
pub fn project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_string()
}

/// The graph store path for a session: `graph.dbPath` when set, else
/// `<agentDir>/graph.db`.
#[must_use]
pub fn store_path(configured: Option<PathBuf>, agent_dir: &Path) -> PathBuf {
    configured.unwrap_or_else(|| agent_dir.join("graph.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lightweight-graph review §3.2: the watcher's key must resolve by the
    /// SAME rule the manual tool uses, so a basename collision cannot fork the
    /// graph into a watched copy and a queried copy.
    #[test]
    fn the_watch_key_falls_back_to_the_basename() {
        // The fallback is the plain basename (the engine resolves the store-
        // backed key before starting the watcher; this helper is its fallback).
        assert_eq!(project_name(Path::new("/a/service")), "service");
    }

    /// The review's acceptance check #3, verbatim: "Новый файл появляется во время
    /// занятого writer; после этого нет событий. Он всё равно должен попасть в индекс."
    ///
    /// The existing test proves the batch is RETRIED. It does not prove the case the review
    /// names: a NEW file nobody has seen yet, appearing while another writer holds the
    /// lease, with no further filesystem event to trigger a second look. That is the
    /// failure mode where a dropped batch means the file is invisible until something
    /// unrelated happens to notice.
    #[tokio::test]
    async fn a_file_created_during_a_held_lease_is_indexed_without_a_second_event() {
        use pa_graph::lease::{CancelToken, IndexLease};
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("a.rs"), "pub fn one() {}\n").expect("a.rs");
        let db_path = root.join("graph.db");
        let cancel = CancelToken::new();
        // Establish the store so the check below reads a real project, not an empty file.
        assert_eq!(
            reindex(&root, &db_path, "watchtest", 1, &cancel).await,
            ReindexOutcome::Done
        );

        // A writer we do not control takes the lease, and a NEW file lands while it is held.
        let held = IndexLease::acquire(&db_path, Duration::ZERO, &cancel).expect("lease");
        std::fs::write(root.join("b_new.rs"), "pub fn brand_new() {}\n").expect("new file");
        assert_eq!(
            reindex(&root, &db_path, "watchtest", 1, &cancel).await,
            ReindexOutcome::LeaseBusy,
            "the batch is outstanding, not lost"
        );
        drop(held);

        // The retry the loop performs — and NO new event: the file is still there, and the
        // pass re-derives changes from hashes rather than from the event stream.
        assert_eq!(
            reindex(&root, &db_path, "watchtest", 1, &cancel).await,
            ReindexOutcome::Done
        );
        let store = pa_graph::GraphStore::open(&db_path).expect("open");
        let names: Vec<String> = store
            .search("brand_new", &pa_graph::SearchOptions::new("watchtest"))
            .expect("search")
            .into_iter()
            .map(|hit| hit.qualified_name)
            .collect();
        assert!(
            names.iter().any(|name| name.contains("brand_new")),
            "the file created during the held lease must be in the graph: {names:?}"
        );
    }

    /// The end-to-end shape of the retry: with the writer lease held by someone
    /// else, the watcher's pass reports `LeaseBusy` (so the loop retries) and
    /// NOT success, and once the lease is released the same pass applies.
    ///
    /// This is the assertion the old code could not make: it mapped every error
    /// to "batch dropped", so the state "still outstanding" did not exist.
    #[tokio::test]
    async fn a_held_lease_reports_the_batch_as_still_outstanding() {
        use pa_graph::lease::{CancelToken, IndexLease};
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("a.rs"), "pub fn one() {}\n").expect("a.rs");
        let db_path = root.join("graph.db");
        let cancel = CancelToken::new();

        // Hold the lease as another writer would.
        let held = IndexLease::acquire(&db_path, Duration::ZERO, &cancel).expect("first lease");
        let busy = reindex(&root, &db_path, "watchtest", 1, &cancel).await;
        assert_eq!(
            busy,
            ReindexOutcome::LeaseBusy,
            "a held lease must leave the batch outstanding, not drop it"
        );

        // Release it: the SAME pass now applies, which is what the retry loop
        // depends on.
        drop(held);
        let applied = reindex(&root, &db_path, "watchtest", 1, &cancel).await;
        assert_eq!(
            applied,
            ReindexOutcome::Done,
            "after the lease frees, the batch must apply"
        );
        let store = pa_graph::GraphStore::open(&db_path).expect("open");
        assert!(
            store.node_count("watchtest").expect("count") > 0,
            "the retried pass must have indexed the file"
        );
    }

    /// A busy lease must not end a batch. The old loop dropped it and waited for
    /// another filesystem event; a session that edits and then stops leaves the
    /// graph stale with no event coming.
    ///
    /// `is_lease_busy` is the decision point, so it is what the test pins: a
    /// lease timeout retries, and every other failure does not (retrying a parse
    /// error on a timer would spin forever).
    #[test]
    fn only_a_busy_lease_is_retried() {
        // The lease error renders as `timed out after {waited:?} waiting for the
        // index writer lease at {lock_path} ({holder})` (pa-graph's LeaseError).
        assert!(is_lease_busy(
            "reindex: timed out after 0ns waiting for the index writer lease \
             at /tmp/g.db.index.lock (pid 42 on host A)"
        ));
        // Cancelled means THIS session's token fired: it is shutting down, and
        // the retry loop already returns on `cancel.is_cancelled()`.
        assert!(!is_lease_busy(
            "cancelled after 1s waiting for the index writer lease"
        ));
        // Failures a retry cannot fix must not spin.
        assert!(!is_lease_busy(
            "reindex: failed to read src/a.rs: no such file"
        ));
        assert!(!is_lease_busy("reindex: malformed store"));
        assert!(!is_lease_busy(
            "open /tmp/g.db: unable to open database file"
        ));
        // A generic I/O error that merely mentions a lease is not a hold.
        assert!(!is_lease_busy(
            "lock file for the index writer lease is unreadable"
        ));
    }

    #[test]
    fn the_project_key_is_the_directory_basename() {
        assert_eq!(project_name(Path::new("/a/service")), "service");
        assert_eq!(project_name(Path::new("/")), "project");
    }

    #[test]
    fn the_store_falls_back_under_the_agent_dir() {
        assert_eq!(
            store_path(None, Path::new("/agent")),
            PathBuf::from("/agent/graph.db")
        );
        assert_eq!(
            store_path(Some(PathBuf::from("/custom/g.db")), Path::new("/agent")),
            PathBuf::from("/custom/g.db")
        );
    }

    /// The end-to-end property the setting promised: an edit reaches the graph
    /// while the session is idle, with no query to trigger the staleness check.
    ///
    /// The watcher is the real one (`NativeWatcher`), the store is real, and the
    /// assertion reads a fresh connection — so this fails when the watch loop is
    /// absent, and passes only when the loop actually reindexed.
    #[tokio::test]
    async fn a_watched_edit_reaches_the_graph_without_a_query() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let root = dir.path().join("watched");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("a.rs"), "pub fn before_the_edit() {}\n").expect("seed");
        let db = dir.path().join("graph.db");
        {
            let mut store = pa_graph::GraphStore::open(&db).expect("open");
            store
                .index_project(&root, &pa_graph::IndexOptions::new("watched"))
                .expect("seed index");
        }
        let handle = super::start(&root, &db, "watched").expect("the OS watcher starts");
        // The watch is live but the baseline is empty: write a file the seed
        // build never saw. `stop` cancels only when asked, so the loop gets the
        // whole deadline to observe it.
        std::fs::write(root.join("b.rs"), "pub fn only_after_the_watch() {}\n").expect("write");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let seen = loop {
            // A reader connection: the watcher holds the writer.
            if let Ok(store) = pa_graph::GraphStore::open(&db) {
                if let Ok(hits) = store.search(
                    "only_after_the_watch",
                    &pa_graph::SearchOptions::new("watched"),
                ) {
                    // The search is a cascade (BM25 + degree + …): it answers
                    // SOMETHING even for a miss, so the oracle names the symbol
                    // instead of asserting non-emptiness. Measured: an unfiltered
                    // `!hits.is_empty()` was green with the reindex removed.
                    if hits.iter().any(|hit| hit.name == "only_after_the_watch") {
                        break true;
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        };
        handle.stop().await;
        assert!(
            seen,
            "the watcher never brought `only_after_the_watch` into the graph: \
             `graph.auto = \"watch\"` must observe the tree, not wait for a query"
        );
    }

    #[test]
    fn only_source_files_are_events_of_interest() {
        assert!(is_source(Path::new("/p/src/main.rs")));
        assert!(is_source(Path::new("/p/app.py")));
        // Markdown is a supported grammar, so a README is indexed too; the
        // filter is the grammar table, not good taste.
        assert!(is_source(Path::new("/p/README.md")));
        // What a recursive watch reports constantly and the graph never reads.
        assert!(!is_source(Path::new("/p/.DS_Store")));
        assert!(!is_source(Path::new("/p/target/debug/prime-agent")));
    }
}
