//! The native `graph` tool: a thin [`pa_agent::types::AgentTool`] over the
//! `pa-graph` code-graph store (index/search/trace/architecture/coverage/
//! changes/reindex). FORK-ONLY.
//!
//! The store is SQLite-backed and single-connection (`!Sync`), so it lives
//! behind a `Mutex<Option<GraphStore>>` and opens lazily on first use; the
//! whole dispatch runs on `spawn_blocking` (SQLite, the directory walk, and
//! the git subprocess are all blocking work).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// FORK-ONLY (jev Phase 2): the optional judge re-rank of `search` results.
use crate::jev::graph_rerank::{GraphReranker, RerankCandidate, VERIFIED_MARKER};

use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};
use pa_graph::{
    ArchitectureView, Direction, EdgeType, GraphStore, IndexOptions, SearchOptions, TraceOptions,
};

/// Project name a store rooted at `root` indexes under (its final path
/// component, or `"project"` when the root has none).
fn project_name(root: &std::path::Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "project".to_string(), str::to_string)
}

/// Parse a trace direction (`in`/`out`/`both`); anything else is outgoing.
fn parse_direction(raw: Option<&str>) -> Direction {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("in" | "incoming" | "callers") => Direction::In,
        Some("both" | "all") => Direction::Both,
        _ => Direction::Out,
    }
}

/// Parse the comma-separated edge-type filter (`calls,imports,...`).
fn parse_edge_types(raw: Option<&str>) -> Vec<EdgeType> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split(',')
        .filter_map(|name| match name.trim().to_ascii_lowercase().as_str() {
            "calls" | "call" => Some(EdgeType::Calls),
            "imports" | "import" => Some(EdgeType::Imports),
            "inherits" | "inherit" => Some(EdgeType::Inherits),
            "uses" | "use" => Some(EdgeType::Uses),
            "contains" => Some(EdgeType::Contains),
            _ => None,
        })
        .collect()
}

/// The native `graph` tool. `db_path` is the SQLite store; `cwd` is the
/// default project root index/search/trace resolve against.
pub struct GraphTool {
    db_path: PathBuf,
    cwd: PathBuf,
    store: Mutex<Option<GraphStore>>,
    schema: serde_json::Value,
    /// FORK-ONLY (jev Phase 2): when set, `search` results are re-ranked by
    /// the judge (`[jev] graphRerank = auto` behind a native judge). `None`
    /// (the default) returns the store's own order, untouched.
    reranker: Option<Arc<GraphReranker>>,
}

impl GraphTool {
    /// Build the tool over `db_path`, defaulting every project to `cwd`.
    #[must_use]
    pub fn new(db_path: PathBuf, cwd: PathBuf) -> Self {
        Self {
            db_path,
            cwd,
            store: Mutex::new(None),
            schema: params_schema(),
            reranker: None,
        }
    }

    /// FORK-ONLY (jev Phase 2): attach the judge re-rank of `search`
    /// results.
    #[must_use]
    pub fn with_reranker(mut self, reranker: Option<Arc<GraphReranker>>) -> Self {
        self.reranker = reranker;
        self
    }

    /// The project root an action resolves against: the `path`/`root`
    /// argument when given (resolved against `cwd`), else `cwd`.
    fn resolve_root(&self, params: &serde_json::Value) -> PathBuf {
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

    /// Open (once) and return a lock on the store, creating the DB on demand.
    fn with_store<T>(
        &self,
        f: impl FnOnce(&mut GraphStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("graph store lock poisoned"))?;
        if guard.is_none() {
            let store = GraphStore::open(&self.db_path)
                .map_err(|err| anyhow::anyhow!("failed to open graph store: {err}"))?;
            *guard = Some(store);
        }
        let store = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("graph store unavailable"))?;
        f(store)
    }

    fn dispatch(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let action = params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .map_or("", str::trim)
            .to_ascii_lowercase();
        match action.as_str() {
            "index" => self.action_index(params),
            "reindex" | "reindex_incremental" => self.action_reindex(params),
            "search" => self.action_search(params),
            "trace" => self.action_trace(params),
            "architecture" | "arch" => self.action_architecture(params),
            "coverage" => self.action_coverage(params),
            "changes" | "detect_changes" => self.action_changes(params),
            "status" => self.action_status(params),
            other => anyhow::bail!(
                "unknown graph action {other:?}: expected one of index, reindex, search, \
                 trace, architecture, coverage, changes, status"
            ),
        }
    }

    fn action_index(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let mut opts = IndexOptions::new(&project);
        if params
            .get("force")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            opts = opts.with_force(true);
        }
        let report = self.with_store(|store| {
            store
                .index_project(&root, &opts)
                .map_err(|err| anyhow::anyhow!("index failed: {err}"))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Indexed project `{project}` at {}: {} nodes, {} edges, {} files parsed \
                 ({} added, {} modified, {} removed, {} unchanged, {} failed, {} partial, \
                 {} unusable, {} skipped).",
                root.display(),
                report.nodes,
                report.edges,
                report.parsed,
                report.added,
                report.modified,
                report.removed,
                report.unchanged,
                report.failed,
                report.partial,
                report.unusable,
                report.skipped,
            ))],
            details: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    fn action_reindex(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let opts = IndexOptions::new(&project);
        let report = self.with_store(|store| {
            store
                .reindex_incremental(&root, &opts)
                .map_err(|err| anyhow::anyhow!("reindex failed: {err}"))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Incrementally reindexed project `{project}`: {} files reindexed, {} closure \
                 dependents, {} removed, {} unchanged (reason: {:?}).",
                report.files_reindexed,
                report.closure_files,
                report.removed,
                report.unchanged,
                report.reason,
            ))],
            details: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    fn action_search(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let query = params
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .ok_or_else(|| anyhow::anyhow!("search requires a non-empty `query`"))?;
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let mut opts = SearchOptions::new(&project);
        if let Some(limit) = params.get("limit").and_then(serde_json::Value::as_u64) {
            opts = opts.with_limit(limit as usize);
        }
        let hits = self.with_store(|store| {
            store
                .search(query, &opts)
                .map_err(|err| anyhow::anyhow!("search failed: {err}"))
        })?;
        let mut lines = vec![format!("{} hit(s) for {query:?}:", hits.len())];
        for hit in &hits {
            lines.push(format!(
                "- {} ({}:{}) [{}] score={:.3}",
                hit.qualified_name,
                hit.file_path,
                hit.start_line,
                hit.label.as_str(),
                hit.score,
            ));
        }
        let results: Vec<serde_json::Value> = hits
            .iter()
            .map(|hit| {
                serde_json::json!({
                    "id": hit.id,
                    "label": hit.label.as_str(),
                    "name": hit.name,
                    "qualified_name": hit.qualified_name,
                    "file_path": hit.file_path,
                    "start_line": hit.start_line,
                    "end_line": hit.end_line,
                    "score": hit.score,
                    "in_degree": hit.in_degree,
                    "out_degree": hit.out_degree,
                })
            })
            .collect();
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::json!({ "hits": hits.len(), "results": results }),
            terminate: None,
            is_error: false,
        })
    }

    /// FORK-ONLY (jev Phase 2): re-rank a `search` result with the judge.
    ///
    /// Fail-open by construction: without a re-ranker, with no judge answer,
    /// or with a result this code cannot read, the store's own order and
    /// rendering pass through untouched. A demoted candidate is KEPT in the
    /// list, moved below the survivors and labelled with the probability that
    /// demoted it - the model sees the judge's opinion, not a silent
    /// deletion. A candidate beyond the judged top-K keeps its place and is
    /// marked unjudged, never "verified".
    async fn rerank_search(
        &self,
        query: &str,
        mut result: AgentToolResult,
        signal: &AbortSignal,
    ) -> AgentToolResult {
        let Some(reranker) = self.reranker.as_ref() else {
            return result;
        };
        let Some(rows) = result
            .details
            .get("results")
            .and_then(|value| value.as_array())
        else {
            return result;
        };
        let candidates: Vec<RerankCandidate> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| RerankCandidate {
                index,
                qualified_name: string_field(row, "qualified_name"),
                file_path: string_field(row, "file_path"),
                label: string_field(row, "label"),
            })
            .collect();
        // A cancelled call must not wait for the judgment: the abort wins
        // and the store's own result returns (fail-open, like any failure).
        let judged = tokio::select! {
            biased;
            () = signal.aborted() => None,
            outcome = reranker.rerank(query, &candidates) => outcome,
        };
        let Some(outcome) = judged else {
            return result;
        };
        // Rebuild the rendering in the re-ranked order: survivors first (in
        // their original relative order), then the demoted ones.
        let demoted: std::collections::BTreeMap<usize, f64> =
            outcome.demoted.iter().copied().collect();
        let mut order: Vec<usize> = outcome.kept.clone();
        order.extend(outcome.demoted.iter().map(|(index, _probability)| *index));
        let mut lines = vec![format!(
            "{} hit(s) for {query:?} (re-ranked by jev: {} of {} judged, {} demoted):",
            rows.len(),
            outcome.probabilities.len(),
            rows.len(),
            outcome.demoted.len()
        )];
        let mut reordered = Vec::with_capacity(rows.len());
        for index in order {
            let Some(row) = rows.get(index) else {
                continue;
            };
            let mut row = row.clone();
            let probability = outcome.probabilities.get(&index).copied();
            let demoted_at = demoted.get(&index).copied();
            if let Some(object) = row.as_object_mut() {
                object.insert(
                    "verified".to_string(),
                    serde_json::Value::Bool(probability.is_some() && demoted_at.is_none()),
                );
                object.insert(
                    "judged".to_string(),
                    serde_json::Value::Bool(probability.is_some()),
                );
                if let Some(probability) = probability {
                    object.insert("relevance".to_string(), serde_json::json!(probability));
                }
            }
            lines.push(render_hit_line(&row, demoted_at));
            reordered.push(row);
        }
        if let Some(details) = result.details.as_object_mut() {
            details.insert(
                VERIFIED_MARKER.to_string(),
                serde_json::json!({
                    "judged": outcome.probabilities.len(),
                    "demoted": outcome.demoted.len(),
                    "threshold": crate::jev::graph_rerank::DEMOTE_BELOW,
                }),
            );
            details.insert("results".to_string(), serde_json::Value::Array(reordered));
        }
        result.content = vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))];
        result
    }

    fn action_trace(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let symbol = params
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|symbol| !symbol.is_empty())
            .ok_or_else(|| anyhow::anyhow!("trace requires a non-empty `symbol`"))?;
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let mut opts = TraceOptions::new(&project, symbol);
        opts.direction =
            parse_direction(params.get("direction").and_then(serde_json::Value::as_str));
        if let Some(depth) = params.get("depth").and_then(serde_json::Value::as_u64) {
            opts.depth = depth as usize;
        }
        if let Some(limit) = params.get("limit").and_then(serde_json::Value::as_u64) {
            opts.limit = limit as usize;
        }
        let edge_types =
            parse_edge_types(params.get("edge_types").and_then(serde_json::Value::as_str));
        if !edge_types.is_empty() {
            opts.edge_types = edge_types;
        }
        let result = self.with_store(|store| {
            store
                .trace(&opts)
                .map_err(|err| anyhow::anyhow!("trace failed: {err}"))
        })?;
        let mut lines = vec![format!(
            "Trace from {} ({} step(s)){}:",
            result.origin,
            result.steps.len(),
            if result.truncated {
                format!(
                    " — TRUNCATED at the {} node limit; more reachable nodes were not listed, \
                     raise `limit` for the full set",
                    result.steps.len()
                )
            } else {
                String::new()
            }
        )];
        for step in &result.steps {
            lines.push(format!(
                "- [depth {}] {} ({}:{}) via {} confidence={:.2}",
                step.depth,
                step.qualified_name,
                step.file_path,
                step.start_line,
                step.via.as_str(),
                step.confidence,
            ));
        }
        let steps: Vec<serde_json::Value> = result
            .steps
            .iter()
            .map(|step| {
                serde_json::json!({
                    "id": step.id,
                    "name": step.name,
                    "qualified_name": step.qualified_name,
                    "file_path": step.file_path,
                    "start_line": step.start_line,
                    "depth": step.depth,
                    "via": step.via.as_str(),
                    "confidence": step.confidence,
                    "strategy": step.strategy,
                    "risk_labels": step.risk_labels,
                })
            })
            .collect();
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::json!({
                "origin_id": result.origin_id,
                "origin": result.origin,
                "steps": steps,
                "truncated": result.truncated,
            }),
            terminate: None,
            is_error: false,
        })
    }

    fn action_architecture(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let view = ArchitectureView::from_str_or_default(
            params
                .get("view")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("overview"),
        );
        let arch = self.with_store(|store| {
            store
                .architecture(&project, view)
                .map_err(|err| anyhow::anyhow!("architecture failed: {err}"))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Architecture view `{}` for project `{project}` (see details for the full report).",
                view.as_str(),
            ))],
            details: serde_json::to_value(&arch).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    fn action_coverage(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let report = self.with_store(|store| {
            store
                .coverage(&project)
                .map_err(|err| anyhow::anyhow!("coverage failed: {err}"))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Coverage for `{project}`: {}/{} files parsed cleanly ({:.0}%), {} partial, \
                 {} unusable, {} skipped; complete={}.",
                report.parsed,
                report.total,
                report.clean_ratio() * 100.0,
                report.partial,
                report.unusable,
                report.skipped,
                report.complete,
            ))],
            details: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    fn action_changes(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let ref_a = params
            .get("ref_a")
            .or_else(|| params.get("refA"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD~1");
        let ref_b = params
            .get("ref_b")
            .or_else(|| params.get("refB"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD");
        let report = self.with_store(|store| {
            store
                .detect_changes(&project, ref_a, ref_b)
                .map_err(|err| anyhow::anyhow!("changes failed: {err}"))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Changes in `{project}` ({ref_a}..{ref_b}): {} file(s), {} affected symbol(s), \
                 {} co-change pair(s); git={}.",
                report.file_count(),
                report.symbol_count(),
                report.co_changes.len(),
                report.is_git,
            ))],
            details: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    fn action_status(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let (nodes, edges, files) = self.with_store(|store| {
            let nodes = store
                .all_nodes(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?
                .len();
            let edges = store
                .all_edges(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?
                .len();
            let files = store
                .file_count(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?;
            Ok((nodes, edges, files))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Graph store {} — project `{project}`: {nodes} node(s), {edges} edge(s), \
                 {files} file(s).",
                self.db_path.display(),
            ))],
            details: serde_json::json!({
                "db_path": self.db_path.display().to_string(),
                "project": project,
                "nodes": nodes,
                "edges": edges,
                "files": files,
            }),
            terminate: None,
            is_error: false,
        })
    }
}

impl AgentTool for GraphTool {
    fn name(&self) -> &'static str {
        "graph"
    }

    fn description(&self) -> &'static str {
        "Query and build the on-disk code graph for the working directory. \
         Actions: index (full or incremental build of the symbol/call/import \
         graph), reindex (incremental closure repair), search (hybrid \
         BM25/regex/degree symbol search), trace (callers/callees from a \
         symbol), architecture (overview/structure/layers/clusters/cycles/\
         hotspots/entry_points/file_tree), coverage (index honesty report), \
         changes (git-diff → affected symbols), status (store counts)."
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.schema
    }

    fn label(&self) -> &'static str {
        "Graph"
    }

    fn execute(
        self: Arc<Self>,
        _tool_call_id: String,
        params: serde_json::Value,
        signal: AbortSignal,
        _on_update: AgentToolUpdateCallback,
    ) -> pa_agent::BoxFut<'static, anyhow::Result<AgentToolResult>> {
        Box::pin(async move {
            let tool = Arc::clone(&self);
            // The re-rank's query is read before the store work (the params
            // move into the blocking task).
            let rerank_query = (self.reranker.is_some()
                && params
                    .get("action")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|action| action.trim().eq_ignore_ascii_case("search")))
            .then(|| {
                params
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|query| !query.is_empty())
                    .map(str::to_string)
            })
            .flatten();
            let store_params = params.clone();
            // SQLite, the directory walk, and the git subprocess are all
            // blocking; keep them off the async executor.
            let result = tokio::task::spawn_blocking(move || tool.dispatch(&store_params))
                .await
                .map_err(|join| anyhow::anyhow!("graph task join failed: {join}"))??;
            // FORK-ONLY (jev Phase 2): the judge re-rank runs on the async
            // path (it awaits the judgment); a failure inside it is
            // fail-open and returns the store's own result.
            let Some(query) = rerank_query else {
                return Ok(result);
            };
            Ok(self.rerank_search(&query, result, &signal).await)
        })
    }
}

/// One string field of a result row (`""` when absent or not a string).
fn string_field(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Render one re-ranked hit line: a demoted candidate carries the
/// probability that demoted it, a judged survivor is marked verified, and a
/// candidate the judge never looked at renders exactly like the store's own
/// line.
fn render_hit_line(row: &serde_json::Value, demoted_at: Option<f64>) -> String {
    let name = string_field(row, "qualified_name");
    let file = string_field(row, "file_path");
    let line = row
        .get("start_line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let label = string_field(row, "label");
    let score = row
        .get("score")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let judged = row
        .get("judged")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let base = format!("- {name} ({file}:{line}) [{label}] score={score:.3}");
    match (demoted_at, judged) {
        (Some(probability), _) => format!("{base} demoted-by-jev (p={probability:.2})"),
        (None, true) => format!("{base} verified-by-jev"),
        (None, false) => base,
    }
}

/// The tool's JSON-Schema parameters.
#[must_use]
pub fn params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": [
                    "index", "reindex", "search", "trace",
                    "architecture", "coverage", "changes", "status"
                ],
                "description": "The graph operation to run."
            },
            "path": {
                "type": "string",
                "description": "Project root to index/query, relative to the working directory (or absolute). Defaults to the working directory."
            },
            "query": {
                "type": "string",
                "description": "Symbol query for action=\"search\"."
            },
            "symbol": {
                "type": "string",
                "description": "Start symbol (qualified or unique short name) for action=\"trace\"."
            },
            "direction": {
                "type": "string",
                "enum": ["out", "in", "both"],
                "description": "Trace direction (default \"out\": what the symbol calls)."
            },
            "depth": {
                "type": "integer",
                "description": "Maximum trace BFS depth in hops (default 2)."
            },
            "edge_types": {
                "type": "string",
                "description": "Comma-separated edge filter for trace, e.g. \"calls,imports\" (default calls)."
            },
            "view": {
                "type": "string",
                "enum": [
                    "overview", "structure", "layers", "clusters",
                    "cycles", "hotspots", "entry_points", "file_tree"
                ],
                "description": "Architecture view for action=\"architecture\" (default overview)."
            },
            "limit": {
                "type": "integer",
                "description": "Result cap for search/trace."
            },
            "force": {
                "type": "boolean",
                "description": "Re-parse every file on action=\"index\", ignoring stored hashes."
            },
            "ref_a": {
                "type": "string",
                "description": "Base git ref for action=\"changes\" (default HEAD~1)."
            },
            "ref_b": {
                "type": "string",
                "description": "Head git ref for action=\"changes\" (default HEAD)."
            }
        },
        "required": ["action"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(dir: &std::path::Path) -> GraphTool {
        GraphTool::new(dir.join("graph.db"), dir.to_path_buf())
    }

    /// The first text block of a result, for assertions.
    fn text_of(result: &AgentToolResult) -> &str {
        match &result.content[0] {
            pa_agent::types::ToolResultContent::Text(content) => &content.text,
            other @ pa_agent::types::ToolResultContent::Image(_) => {
                panic!("expected a text block, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn index_then_search_finds_the_symbol() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn parse_config(path: &str) -> u32 { load(path) }\n\
             pub fn load(path: &str) -> u32 { path.len() as u32 }\n",
        )
        .unwrap();
        let tool = tool(dir.path());
        let result = tool
            .dispatch(&serde_json::json!({ "action": "index" }))
            .unwrap();
        assert!(!result.is_error);
        let text = text_of(&result);
        assert!(text.contains("Indexed project"), "{text}");

        let search = tool
            .dispatch(&serde_json::json!({ "action": "search", "query": "load" }))
            .unwrap();
        let text = text_of(&search);
        assert!(text.contains("load"), "{text}");
    }

    #[tokio::test]
    async fn trace_and_status_work_over_the_indexed_store() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn parse_config(path: &str) -> u32 { load(path) }\n\
             pub fn load(path: &str) -> u32 { path.len() as u32 }\n",
        )
        .unwrap();
        let tool = tool(dir.path());
        tool.dispatch(&serde_json::json!({ "action": "index" }))
            .unwrap();
        let trace = tool
            .dispatch(&serde_json::json!({ "action": "trace", "symbol": "parse_config" }))
            .unwrap();
        assert!(
            text_of(&trace).contains("parse_config"),
            "{}",
            text_of(&trace)
        );
        let status = tool
            .dispatch(&serde_json::json!({ "action": "status" }))
            .unwrap();
        assert!(text_of(&status).contains("node(s)"));
    }

    #[tokio::test]
    async fn unknown_action_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool(dir.path());
        let error = tool
            .dispatch(&serde_json::json!({ "action": "nope" }))
            .unwrap_err();
        assert!(
            error.to_string().contains("unknown graph action"),
            "{error}"
        );
    }

    /// A judge that answers the re-rank's `noul` questions from a table,
    /// keyed by the symbol it sees in the question text.
    struct SymbolJudge {
        relevant: Vec<&'static str>,
    }

    #[async_trait::async_trait]
    impl crate::jev::Judge for SymbolJudge {
        fn label(&self) -> &'static str {
            "scripted/symbols"
        }

        async fn judge(
            &self,
            request: &pa_jev::JudgmentRequest,
        ) -> Result<pa_jev::JudgmentResult, crate::jev::JudgeError> {
            let mut answers = std::collections::BTreeMap::new();
            for (id, question) in &request.questions {
                let pa_jev::Question::Noul { instructions, .. } = question else {
                    panic!("the re-rank asks noul questions");
                };
                // The question renders the symbol in quotes, so an exact
                // symbol match cannot hit `lib::load_balancer`.
                let relevant = self
                    .relevant
                    .iter()
                    .any(|symbol| instructions.contains(&format!("{symbol:?}")));
                answers.insert(
                    id.clone(),
                    pa_jev::Answer::Noul {
                        noul: if relevant { 0.95 } else { 0.02 },
                    },
                );
            }
            Ok(pa_jev::JudgmentResult {
                api: "scripted".to_string(),
                provider: "scripted".to_string(),
                model: "scripted/symbols".to_string(),
                answers,
                usage: pa_jev::Usage::default(),
            })
        }
    }

    /// A store with two symbols whose names both match `load`.
    fn indexed_store(dir: &std::path::Path) -> GraphTool {
        std::fs::write(
            dir.join("lib.rs"),
            "pub fn load_balancer(path: &str) -> u32 { 0 }\n\
             pub fn load(path: &str) -> u32 { path.len() as u32 }\n",
        )
        .unwrap();
        let tool = tool(dir);
        tool.dispatch(&serde_json::json!({ "action": "index" }))
            .unwrap();
        tool
    }

    #[tokio::test]
    async fn without_a_reranker_the_search_result_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let tool = indexed_store(dir.path());
        let result = tool
            .dispatch(&serde_json::json!({ "action": "search", "query": "load" }))
            .unwrap();
        assert!(
            result
                .details
                .get(crate::jev::graph_rerank::VERIFIED_MARKER)
                .is_none(),
            "an unconfigured tool carries no marker"
        );
        assert!(!text_of(&result).contains("verified-by-jev"));
    }

    #[tokio::test]
    async fn the_reranker_marks_survivors_and_demotes_confident_misses() {
        let dir = tempfile::tempdir().unwrap();
        let judge: std::sync::Arc<dyn crate::jev::Judge> = std::sync::Arc::new(SymbolJudge {
            relevant: vec!["lib::load"],
        });
        let tool = indexed_store(dir.path()).with_reranker(Some(std::sync::Arc::new(
            crate::jev::GraphReranker::new(judge),
        )));
        let query = serde_json::json!({ "action": "search", "query": "load" });
        // The re-rank runs on the async execute path (it awaits the judge).
        let result = pa_agent::types::AgentTool::execute(
            std::sync::Arc::new(tool),
            "call".to_string(),
            query,
            pa_agent::abort::AbortController::new().signal(),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .expect("the tool answers");
        let marker = result
            .details
            .get(crate::jev::graph_rerank::VERIFIED_MARKER)
            .expect("a re-ranked result is marked");
        assert_eq!(marker["judged"].as_u64(), Some(2), "{marker}");
        let text = text_of(&result);
        assert!(text.contains("re-ranked by jev"), "{text}");
        assert!(text.contains("verified-by-jev"), "{text}");
        assert!(text.contains("demoted-by-jev"), "{text}");
        // A demoted candidate is kept and flagged, never silently deleted.
        let rows = result.details["results"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        let verified: Vec<bool> = rows
            .iter()
            .map(|row| row["verified"].as_bool().unwrap_or_default())
            .collect();
        assert_eq!(verified, vec![true, false], "the survivor comes first");
        // The demoted row keeps its probability as evidence.
        assert!(
            rows[1]["relevance"].as_f64().is_some_and(|p| p < 0.1),
            "{:?}",
            rows[1]
        );
    }

    #[tokio::test]
    async fn an_aborted_search_never_waits_for_the_judgment() {
        struct HangingJudge;
        #[async_trait::async_trait]
        impl crate::jev::Judge for HangingJudge {
            fn label(&self) -> &'static str {
                "hanging"
            }
            async fn judge(
                &self,
                _request: &pa_jev::JudgmentRequest,
            ) -> Result<pa_jev::JudgmentResult, crate::jev::JudgeError> {
                std::future::pending().await
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let judge: std::sync::Arc<dyn crate::jev::Judge> = std::sync::Arc::new(HangingJudge);
        let tool = indexed_store(dir.path()).with_reranker(Some(std::sync::Arc::new(
            crate::jev::GraphReranker::new(judge),
        )));
        let controller = pa_agent::abort::AbortController::new();
        controller.abort();
        // The aborted signal short-circuits the judgment: the store's result
        // returns instead of waiting on the pending judge.
        let result = pa_agent::types::AgentTool::execute(
            std::sync::Arc::new(tool),
            "call".to_string(),
            serde_json::json!({ "action": "search", "query": "load" }),
            controller.signal(),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .expect("the search still answers");
        assert!(
            result
                .details
                .get(crate::jev::graph_rerank::VERIFIED_MARKER)
                .is_none(),
            "an aborted re-rank leaves no marker"
        );
        assert!(
            text_of(&result).contains("hit(s) for"),
            "{}",
            text_of(&result)
        );
    }

    #[tokio::test]
    async fn a_failing_rerank_judge_leaves_the_search_result_untouched() {
        struct FailingJudge;
        #[async_trait::async_trait]
        impl crate::jev::Judge for FailingJudge {
            fn label(&self) -> &'static str {
                "failing"
            }
            async fn judge(
                &self,
                _request: &pa_jev::JudgmentRequest,
            ) -> Result<pa_jev::JudgmentResult, crate::jev::JudgeError> {
                Err(crate::jev::JudgeError::Candidate {
                    identity: "failing".to_string(),
                    message: "scripted failure".to_string(),
                    status: None,
                })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let judge: std::sync::Arc<dyn crate::jev::Judge> = std::sync::Arc::new(FailingJudge);
        let tool = indexed_store(dir.path()).with_reranker(Some(std::sync::Arc::new(
            crate::jev::GraphReranker::new(judge),
        )));
        let result = pa_agent::types::AgentTool::execute(
            std::sync::Arc::new(tool),
            "call".to_string(),
            serde_json::json!({ "action": "search", "query": "load" }),
            pa_agent::abort::AbortController::new().signal(),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .expect("fail-open: the store's own result still answers");
        assert!(
            result
                .details
                .get(crate::jev::graph_rerank::VERIFIED_MARKER)
                .is_none(),
            "a failed re-rank leaves no marker"
        );
        assert!(!text_of(&result).contains("verified-by-jev"));
    }

    #[test]
    fn the_tool_exposes_its_name_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool(dir.path());
        assert_eq!(tool.name(), "graph");
        assert_eq!(tool.label(), "Graph");
        assert_eq!(tool.parameters()["required"][0], "action");
    }
}
