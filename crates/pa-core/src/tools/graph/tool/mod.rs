//! The tool: construction, store access, and action dispatch.
//!
//! The 14 action bodies live in [`actions`]; the `AgentTool` contract in [`schema`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::jev::graph_rerank::GraphReranker;
use crate::tools::graph::args::{cold_build_note, root_is_stale};
use crate::tools::graph::build::{project_key_in, project_name};
use crate::tools::graph::tool::schema::{
    accepted_params, params_schema, reject_unknown_params, ACCEPTED_PARAMS,
};
use pa_agent::types::AgentToolResult;
use pa_graph::{GraphStore, IndexOptions};
pub mod actions;
pub mod schema;

/// The native `graph` tool. `db_path` is the SQLite store; `cwd` is the
/// default project root index/search/trace resolve against.
/// The graph tool's own output cap (`tools/truncate.rs`'s byte limit, inlined when the
/// shared module went away with the built-in bash tool upstream).
pub const GRAPH_OUTPUT_MAX_BYTES: usize = 50 * 1024;

pub struct GraphTool {
    db_path: PathBuf,
    cwd: PathBuf,
    /// The write connection: `index`/`reindex` and an `adr` that records.
    pub(crate) store: Mutex<Option<GraphStore>>,
    /// The read connection, used by every read action and by the status line
    /// that rides them. Separate from `store` so a read never queues behind a
    /// build; `Mutex<Option<_>>` for the same lazy-open reason.
    reader: Mutex<Option<GraphStore>>,
    /// single-flight flag for the cold-start build (see `ensure_indexed`).
    pub(crate) in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
    schema: serde_json::Value,
    /// FORK-ONLY (jev Phase 2): when set, `search` results are re-ranked by
    /// the judge (`[jev] graphRerank = auto` behind a native judge). `None`
    /// (the default) returns the store's own order, untouched.
    reranker: Option<Arc<GraphReranker>>,
    /// FORK-ONLY (jev Phase 2): the raw `[jev] graphRerank` spelling and
    /// whether the gate resolved ARMED, captured at session start. `status`
    /// reports it: the tools report's repeated trap was editing
    /// `settings.json` and expecting the change to be live, with no surface
    /// saying the gate was read once at construction.
    rerank_gate: Option<(String, bool)>,
}

impl GraphTool {
    /// Build the tool over `db_path`, defaulting every project to `cwd`.
    #[must_use]
    pub fn new(db_path: PathBuf, cwd: PathBuf) -> Self {
        Self {
            db_path,
            cwd,
            store: Mutex::new(None),
            reader: Mutex::new(None),
            in_flight: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            schema: params_schema(),
            reranker: None,
            rerank_gate: None,
        }
    }

    /// FORK-ONLY (jev Phase 2): attach the judge re-rank of `search`
    /// results.
    #[must_use]
    pub fn with_reranker(mut self, reranker: Option<Arc<GraphReranker>>) -> Self {
        self.reranker = reranker;
        self
    }

    /// FORK-ONLY (jev Phase 2): record the raw `[jev] graphRerank` spelling
    /// and whether the gate resolved armed, for `status` to report.
    #[must_use]
    pub fn with_rerank_gate(mut self, raw_spelling: Option<String>, armed: bool) -> Self {
        self.rerank_gate = Some((raw_spelling.unwrap_or_else(|| "(unset)".to_string()), armed));
        self
    }

    /// The project key for `root`, qualified when the plain basename already
    /// belongs to a DIFFERENT tree in this database (see [`project_key_in`]).
    ///
    /// A store read failure falls back to the plain basename: an index action
    /// must still run when the registration table cannot be read, and the
    /// write path's own refusal (on a genuine collision) remains the guard.
    pub(crate) fn project_key(&self, root: &std::path::Path) -> String {
        self.with_reader(|store| Ok::<_, anyhow::Error>(project_key_in(store, root)))
            .unwrap_or_else(|_| Ok(project_name(root)))
            .unwrap_or_else(|_| project_name(root))
    }

    /// The project root an action resolves against: the `path`/`root`
    /// argument when given (resolved against `cwd`), else `cwd`.
    pub(crate) fn resolve_root(&self, params: &serde_json::Value) -> PathBuf {
        let raw = params
            .get("path")
            .or_else(|| params.get("root"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|raw| !raw.is_empty());
        match raw {
            Some(raw) => {
                let candidate = PathBuf::from(raw);
                if candidate.is_absolute() {
                    candidate
                } else {
                    self.cwd.join(candidate)
                }
            }
            None => self.cwd.clone(),
        }
    }

    /// Index the project when the graph has never seen it.
    ///
    /// `search`/`trace`/`architecture` on a cold graph answered "0 hits" and
    /// looked like a completed query, which is how the agent concluded the
    /// graph had nothing to say and fell back to reading files one by one. The
    /// first call is the only one that pays the build cost; later calls hit the
    /// on-disk store, and a `force` index from the model still overrides this.
    /// Returns the report when a build happened, so the reply can name it.
    pub(crate) fn ensure_indexed(
        &self,
        root: &Path,
        project: &str,
    ) -> anyhow::Result<Option<String>> {
        // The project key is the root's BASENAME, so two checkouts named the
        // same (`/a/service`, `/b/service`) share one row set in the shared DB.
        // The write path refuses to overwrite a project with a different root
        // (`register_project_at`), but the WARM path used to answer from
        // `file_count > 0` alone — so a query for B read A's rows and returned
        // another project's symbols as its own answer. The root is validated
        // before any read below, on every path.
        let (indexed, stored_root) = self.with_reader(|store| {
            let count = store
                .file_count(project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?;
            let stored = store
                .project_root(project)
                .map_err(|err| anyhow::anyhow!("project root read failed: {err}"))?;
            Ok::<_, anyhow::Error>((count > 0, stored))
        })?;
        if indexed {
            // READ-YOUR-EDIT: a warm store is not a fresh one. The previous
            // implementation answered from `file_count > 0` and never looked at
            // the files again, so an edit made after the index was invisible
            // until something forced a full rebuild (audit §4.3). A cheap
            // mtime/size pass decides staleness; only a changed set is re-parsed.
            let stale = self.with_reader(|store| {
                let stored = store
                    .all_file_hashes(project)
                    .map_err(|err| anyhow::anyhow!("hash read failed: {err}"))?;
                Ok::<_, anyhow::Error>(root_is_stale(root, &stored))
            })?;
            let same_root = stored_root.as_deref().is_some_and(|stored| {
                stored == root.to_string_lossy()
                    || match (std::fs::canonicalize(stored), std::fs::canonicalize(root)) {
                        (Ok(stored), Ok(requested)) => stored == requested,
                        _ => false,
                    }
            });
            if !same_root {
                anyhow::bail!(
                    "graph project {project:?} is indexed from a different root ({}), not {}:                      two checkouts with the same directory name share one project key, and an                      answer from the other tree would be wrong. Set `graph.dbPath` to a                      per-workspace database, or run the `index` action for this root.",
                    stored_root.as_deref().unwrap_or("<none>"),
                    root.display()
                );
            }
            if stale {
                // A refresh must never make a reader wait for a writer: another
                // session's build can hold the lease for seconds, and a search
                // that queues behind it is the defect this test pins
                // (`a_search_answers_while_an_index_holds_the_writer`). One
                // non-blocking attempt; when the writer is busy the answer comes
                // from the current index and SAYS it is stale.
                let refreshed = self.try_with_store(|store| {
                    store
                        .reindex_incremental(
                            root,
                            &IndexOptions::new(project)
                                .with_respect_gitignore(true)
                                .with_lease_wait(std::time::Duration::ZERO),
                        )
                        .map_err(|err| anyhow::anyhow!("incremental reindex failed: {err}"))
                });
                return match refreshed {
                    Ok(Some(report)) => Ok(Some(format!(
                        "The graph was stale, so this call refreshed it first: {} file(s) \
                         re-parsed, {} dependent file(s), {} removed. The answer below reflects \
                         the working tree as of this call.",
                        report.files_reindexed, report.closure_files, report.removed
                    ))),
                    // The writer is busy here or elsewhere: answer from the index
                    // that exists and flag it, rather than block the turn.
                    Ok(None) | Err(_) => Ok(Some(format!(
                        "The graph is STALE: files changed since the last index and another \
                         writer holds the build lease, so this answer reflects the last indexed \
                         revision of `{project}`. Re-run once the build settles, or run the \
                         `index` action to force a rebuild."
                    ))),
                };
            }
            return Ok(None);
        }
        // single-flight: one cold-start build at a time. A caller that arrives
        // while another build is running proceeds against the current store —
        // `compare_exchange` returning false means a build is already under
        // way — rather than starting a second walk over the same files.
        if self
            .in_flight
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            return Ok(None);
        }
        let _guard = crate::tools::graph_in_flight::InFlightGuard(self.in_flight.clone());
        // Implicit builds are lean. This one runs because a query arrived, not
        // because anyone asked for an index: the answer must not carry the same
        // symbol twice, and the walk must not drag in `target/` or a scratch copy of
        // the tree. A caller who wants the ignored files in the graph runs the
        // `index` action with `respect_gitignore: false` (the explicit default is
        // the same lean view, so the two entry points cannot disagree).
        let opts = IndexOptions::new(project).with_respect_gitignore(true);
        let report = self.with_store(|store| {
            store
                .index_project(root, &opts)
                .map_err(|err| anyhow::anyhow!("index failed: {err}"))
        })?;
        Ok(Some(cold_build_note(&report)))
    }

    /// Open (once) and return a lock on the write store, creating the DB on
    /// demand.
    fn with_store<T>(
        &self,
        f: impl FnOnce(&mut GraphStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("graph store lock poisoned"))?;
        let store = Self::open_into(&mut guard, &self.db_path)?;
        f(store)
    }

    /// `with_store`, but never waiting: a writer busy elsewhere means the caller
    /// answers from the current index instead of queueing behind a build.
    fn try_with_store<T>(
        &self,
        f: impl FnOnce(&mut GraphStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        let Ok(mut guard) = self.store.try_lock() else {
            return Ok(None);
        };
        let store = Self::open_into(&mut guard, &self.db_path)?;
        f(store).map(Some)
    }

    /// Run `f` on the reader connection: one no writer holds, so it runs while
    /// a build is in flight.
    pub(crate) fn with_reader<T>(
        &self,
        f: impl FnOnce(&GraphStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut guard = self
            .reader
            .lock()
            .map_err(|_| anyhow::anyhow!("graph reader lock poisoned"))?;
        let store = Self::open_into(&mut guard, &self.db_path)?;
        f(store)
    }

    /// Run a read of the graph.
    ///
    /// Normally on the reader, so it never waits for a build: an `index` over an
    /// existing store is a measured 133 s pass and a search is 2 ms of SQLite,
    /// and nothing about a read gets better by queueing behind that.
    ///
    /// The exception is the cold-start build (`ensure_indexed`), which is
    /// running precisely because the store has nothing to read yet. There a read
    /// waits for it on the writer, as it always did: answering "0 hits" from a
    /// store the first build is still filling is the false negative that once
    /// made the agent conclude the graph had nothing to say and read files one
    /// by one.
    pub(crate) fn with_read<T>(
        &self,
        f: impl FnOnce(&GraphStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if self.in_flight.load(std::sync::atomic::Ordering::Acquire) {
            return self.with_store(|store| f(store));
        }
        self.with_reader(f)
    }

    /// Open the reader before a build starts.
    ///
    /// `GraphStore::open` re-applies the schema and stamps
    /// `store_meta.schema_version`, so it is a small *write*: opening the reader
    /// for the first time while a build is running would wait for that build's
    /// write lock (measured 850 ms worst case). Opening it before the build
    /// takes the lock keeps the first read during the build as cheap as the rest.
    fn warm_reader(&self) -> anyhow::Result<()> {
        self.with_reader(|_| Ok(()))
    }

    fn open_into<'a>(
        slot: &'a mut Option<GraphStore>,
        db_path: &Path,
    ) -> anyhow::Result<&'a mut GraphStore> {
        if slot.is_none() {
            let store = GraphStore::open(db_path)
                .map_err(|err| anyhow::anyhow!("failed to open graph store: {err}"))?;
            *slot = Some(store);
        }
        slot.as_mut()
            .ok_or_else(|| anyhow::anyhow!("graph store unavailable"))
    }

    pub(crate) fn dispatch(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let action = params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .map_or("", str::trim)
            .to_ascii_lowercase();
        let mut result = self.dispatch_action(&action, params)?;
        // Every answer carries the store's own state on its last line, so a
        // stale or partial graph cannot answer as confidently as a fresh one.
        // `index`/`reindex` skip it: the run they just performed defines the
        // state they would report.
        if !matches!(action.as_str(), "index" | "reindex" | "reindex_incremental") {
            if let Some(line) = self.status_line(params) {
                if let Some(pa_agent::types::ToolResultContent::Text(content)) =
                    result.content.first_mut()
                {
                    content.text.push('\n');
                    content.text.push_str(&line);
                }
            }
        }
        Ok(result)
    }

    /// The `[graph: ...]` state line for the project a call targets.
    ///
    /// Returns `None` when the store cannot be read — a status failure must
    /// never turn an answer into an error.
    fn status_line(&self, params: &serde_json::Value) -> Option<String> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        // Memoised: a status line rides every answer, and the full probe costs
        // more than the query it annotates (measured 255 ms against a 2 ms trace).
        let line = self.with_read(|store| {
            Ok::<_, anyhow::Error>(pa_graph::status::cached_report(store, &project, &root).ok())
        });
        line.ok().flatten().map(|report| report.line())
    }

    fn dispatch_action(
        &self,
        action: &str,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        // An unknown *action* is the more fundamental error, so it keeps its own
        // message: only a known action has a parameter set to check against.
        if let Some(accepted) = accepted_params(action) {
            reject_unknown_params(action, accepted, params)?;
        }
        match action {
            "index" => self.action_index(params),
            "reindex" | "reindex_incremental" => self.action_reindex(params),
            "search" => self.action_search(params),
            "trace" => self.action_trace(params),
            "architecture" | "arch" => self.action_architecture(params),
            "coverage" => self.action_coverage(params),
            "changes" | "detect_changes" => self.action_changes(params),
            "status" => self.action_status(params),
            "verify" => self.action_verify(params),
            "adr" => self.action_adr(params),
            "context" => self.action_context(params),
            "impact" => self.action_impact(params),
            "explain_change" | "explain-change" | "explainChange" => {
                self.action_explain_change(params)
            }
            "forget" => self.action_forget(params),
            "pattern" => self.action_pattern(params),
            other => {
                // Derived, never hand-listed: this text had drifted twice — it named no
                // `context`, `impact` or `explain_change` while all three worked, because a
                // second copy of the action list has no way to stay true.
                let accepted: Vec<&str> = ACCEPTED_PARAMS.iter().map(|(name, _)| *name).collect();
                anyhow::bail!(
                    "unknown graph action {other:?}: expected one of {}",
                    accepted.join(", ")
                )
            }
        }
    }
}
