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
//!   convenience, and a session that queues behind another process's build for
//!   seconds is the defect the read path already avoids; when the lease is held
//!   the batch is DROPPED, not queued — the next event, or any query, re-derives
//!   staleness from the file hashes and repairs it then.
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
        reindex(root, db_path, project, changed, cancel).await;
    }
}

/// One incremental pass over the batch's tree.
///
/// The pass re-discovers the tree (the hashes decide what actually changed), so
/// a created or deleted file needs no special case here: `reindex_incremental`
/// classifies added/removed/changed from the store's own records.
async fn reindex(root: &Path, db_path: &Path, project: &str, events: usize, cancel: &CancelToken) {
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
        Ok(Ok(report)) => tracing::debug!(
            "graph watch: {events} event(s) -> {} file(s) re-parsed, {} dependent(s), {} removed",
            report.files_reindexed,
            report.closure_files,
            report.removed
        ),
        // A lease held elsewhere (or by an in-flight query refresh) is the
        // normal busy case: drop the batch, the next event tries again.
        Ok(Err(err)) => tracing::debug!("graph watch: batch dropped ({err})"),
        Err(err) => tracing::warn!("graph watch: reindex task failed ({err})"),
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
