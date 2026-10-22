//! The native `graph` tool: a thin [`pa_agent::types::AgentTool`] over the
//! `pa-graph` code-graph store (index/search/trace/architecture/coverage/
//! changes/reindex). FORK-ONLY.
//!
//! The store is SQLite-backed and single-connection (`!Sync`), so it lives
//! behind a `Mutex<Option<GraphStore>>` and opens lazily on first use; the
//! whole dispatch runs on `spawn_blocking` (SQLite, the directory walk, and
//! the git subprocess are all blocking work).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// FORK-ONLY (jev Phase 2): the optional judge re-rank of `search` results.
use crate::jev::graph_rerank::{GraphReranker, RerankCandidate, VERIFIED_MARKER};

use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};
use pa_graph::{
    ArchitectureView, Direction, EdgeType, GraphStore, IndexOptions, IndexReport, SearchOptions,
    TraceOptions, TreeNode,
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

/// Render a count map as `key=value` pairs for report lines.
fn render_counts(counts: &std::collections::BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(key, count)| format!("{key}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Append one file-tree directory (and its children, recursively) to the
/// report lines.
fn render_file_tree(node: &TreeNode, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    lines.push(format!(
        "{indent}{}: {} file(s), {} symbol(s)",
        node.name,
        node.files.len(),
        node.symbol_count,
    ));
    for file in &node.files {
        lines.push(format!("{indent}  {file}"));
    }
    for child in &node.children {
        render_file_tree(child, depth + 1, lines);
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
    /// single-flight flag for the cold-start build (see `ensure_indexed`).
    in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
            in_flight: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

    /// Index the project when the graph has never seen it.
    ///
    /// `search`/`trace`/`architecture` on a cold graph answered "0 hits" and
    /// looked like a completed query, which is how the agent concluded the
    /// graph had nothing to say and fell back to reading files one by one. The
    /// first call is the only one that pays the build cost; later calls hit the
    /// on-disk store, and a `force` index from the model still overrides this.
    /// Returns the report when a build happened, so the reply can name it.
    fn ensure_indexed(&self, root: &Path, project: &str) -> anyhow::Result<Option<IndexReport>> {
        let indexed = self.with_store(|store| {
            Ok::<_, anyhow::Error>(
                store
                    .file_count(project)
                    .map_err(|err| anyhow::anyhow!("status failed: {err}"))?
                    > 0,
            )
        })?;
        if indexed {
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
        let opts = IndexOptions::new(project);
        let report = self.with_store(|store| {
            store
                .index_project(root, &opts)
                .map_err(|err| anyhow::anyhow!("index failed: {err}"))
        })?;
        Ok(Some(report))
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
        let project = project_name(&root);
        let line = self.with_store(|store| {
            Ok::<_, anyhow::Error>(pa_graph::status::report(store, &project, &root).ok())
        });
        line.ok().flatten().map(|report| report.line())
    }

    fn dispatch_action(
        &self,
        action: &str,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
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
            other => anyhow::bail!(
                "unknown graph action {other:?}: expected one of index, reindex, search, \
                 trace, architecture, coverage, changes, status, verify, adr"
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
        // A skeleton depths lets an agent get a usable graph in about a second
        // instead of two minutes, then deepen it in the background.
        if let Some(depth) = params.get("depth").and_then(serde_json::Value::as_str) {
            opts = opts.with_depth(pa_graph::indexer::Depth::from_str_or_default(depth));
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
        // A cold graph answers "0 hits" for everything; building it here turns
        // the first query into a real answer instead of a false negative.
        let indexed_now = self.ensure_indexed(&root, &project)?;
        let mut opts = SearchOptions::new(&project);
        if let Some(limit) = params.get("limit").and_then(serde_json::Value::as_u64) {
            opts = opts.with_limit(limit as usize);
        }
        let hits = self.with_store(|store| {
            store
                .search(query, &opts)
                .map_err(|err| anyhow::anyhow!("search failed: {err}"))
        })?;
        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(format!(
                "The graph was empty, so this query indexed the project first: {} nodes, {} edges,                  {} files parsed.",
                report.nodes,
                report.edges,
                report.parsed,
            ));
        }
        lines.push(format!("{} hit(s) for {query:?}:", hits.len()));
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
        let indexed_now = self.ensure_indexed(&root, &project)?;
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
        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(format!(
                "The graph was empty, so this trace indexed the project first: {} nodes, {} edges,                  {} files parsed.",
                report.nodes,
                report.edges,
                report.parsed,
            ));
        }
        lines.push(format!(
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
        ));
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
        let indexed_now = self.ensure_indexed(&root, &project)?;
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
        // Providers forward only `content` to the model; `details` stays for
        // tests and the UI. The report body therefore rides in `content`,
        // the same shape `search` and `trace` already use. Only the section
        // for the requested view is populated, so iterating every section
        // renders exactly that view.
        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(format!(
                "The graph was empty, so this query indexed the project first: {} nodes, {} edges, \
                 {} files parsed.",
                report.nodes,
                report.edges,
                report.parsed,
            ));
        }
        lines.push(format!(
            "Architecture view `{}` for project `{}`:",
            view.as_str(),
            arch.project,
        ));
        if let Some(overview) = &arch.overview {
            lines.push(format!(
                "overview: {} node(s), {} edge(s), {} file(s); labels [{}], edge types [{}], \
                 languages [{}]",
                overview.nodes,
                overview.edges,
                overview.files,
                render_counts(&overview.labels),
                render_counts(&overview.edge_types),
                render_counts(&overview.languages),
            ));
        }
        if !arch.structure.is_empty() {
            lines.push(format!("{} module edge(s):", arch.structure.len()));
            for edge in &arch.structure {
                lines.push(format!(
                    "- {} -> {} (weight={}, {})",
                    edge.from,
                    edge.to,
                    edge.weight,
                    edge.kinds.join("+"),
                ));
            }
        }
        if !arch.layers.is_empty() {
            lines.push(format!("{} layer(s):", arch.layers.len()));
            for layer in &arch.layers {
                lines.push(format!("layer {}: {}", layer.level, layer.files.join(", ")));
            }
        }
        if !arch.clusters.is_empty() {
            lines.push(format!("{} cluster(s):", arch.clusters.len()));
            for cluster in &arch.clusters {
                lines.push(format!(
                    "cluster {} ({} internal edge(s)): {}",
                    cluster.id,
                    cluster.internal_edges,
                    cluster.files.join(", "),
                ));
            }
        }
        if !arch.cycles.is_empty() {
            lines.push(format!("{} cycle(s):", arch.cycles.len()));
            for cycle in &arch.cycles {
                lines.push(format!("cycle: {}", cycle.files.join(" -> ")));
                for edge in &cycle.edges {
                    lines.push(format!(
                        "  {} -> {} (weight={})",
                        edge.from, edge.to, edge.weight,
                    ));
                }
            }
        }
        if !arch.hotspots.is_empty() {
            lines.push(format!("{} hotspot(s):", arch.hotspots.len()));
            for hotspot in &arch.hotspots {
                lines.push(format!(
                    "- {} [{}] fan_in={} fan_out={} degree={}",
                    hotspot.qualified_name,
                    hotspot.file_path,
                    hotspot.fan_in,
                    hotspot.fan_out,
                    hotspot.degree,
                ));
            }
        }
        if !arch.entry_points.is_empty() {
            lines.push(format!("{} entry point(s):", arch.entry_points.len()));
            for entry in &arch.entry_points {
                lines.push(format!(
                    "- {} [{}] out_degree={}",
                    entry.qualified_name, entry.file_path, entry.out_degree,
                ));
            }
        }
        if let Some(tree) = &arch.file_tree {
            render_file_tree(tree, 0, &mut lines);
        }
        // A `structure` view on a large repo is one line per module edge, so
        // the report gets the same byte ceiling as bash output and says so
        // when it is cut.
        let mut report = lines.join("\n");
        let (capped, was_truncated) = crate::resources::truncate_context_content(
            &report,
            crate::tools::truncate::DEFAULT_MAX_BYTES,
        );
        if was_truncated {
            report = format!(
                "{capped}\n[architecture report truncated at {} bytes; query a narrower view]",
                crate::tools::truncate::DEFAULT_MAX_BYTES,
            );
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(report)],
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

    /// `adr`: record or list Architecture Decision Records.
    ///
    /// ADRs live in the same SQLite file as the graph, so a decision and the
    /// code it describes cannot drift apart between files. This is the `adr`
    /// module's first consumer; it was written in 39c66260f and never wired.
    fn action_adr(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let record = params.get("title").and_then(serde_json::Value::as_str);
        let text = |key: &str| {
            params
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let rows = self.with_store(|store| {
            let adrs = pa_graph::adr::AdrStore::new(store.connection())
                .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
            if let Some(title) = record {
                let mut adr = pa_graph::adr::Adr::new(
                    title,
                    text("context"),
                    text("decision"),
                    text("consequences"),
                );
                if let Some(status) = params.get("status").and_then(serde_json::Value::as_str) {
                    adr = adr.with_status(pa_graph::adr::AdrStatus::from_str_or_default(status));
                }
                if let Some(supersedes) =
                    params.get("supersedes").and_then(serde_json::Value::as_i64)
                {
                    adr = adr.superseding(supersedes);
                }
                adrs.record_adr(&adr)
                    .map_err(|err| anyhow::anyhow!("record adr: {err}"))?;
            }
            adrs.query_adr(&pa_graph::adr::AdrQuery::new())
                .map_err(|err| anyhow::anyhow!("query adr: {err}"))
        })?;
        let mut lines = vec![format!(
            "ADRs for `{}`: {} record(s).",
            project_name(&root),
            rows.len()
        )];
        for adr in &rows {
            lines.push(format!(
                "- #{} [{}] {}",
                adr.id,
                adr.status.as_str(),
                adr.title
            ));
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::json!({ "count": rows.len() }),
            terminate: None,
            is_error: false,
        })
    }

    /// `verify`: check the run receipts, and the quality of the last run.
    ///
    /// The three modules this reports on — `witness` (receipts), `quality`
    /// (envelope) and `proof_gate` (decision) — were written together and had
    /// no consumer until now. Every input is a stored hash or a measured
    /// number, so the verdict is not a self-assessment.
    fn action_verify(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let verdict = self.with_store(|store| {
            store
                .verify_run(&project)
                .map_err(|err| anyhow::anyhow!("verify failed: {err}"))
        })?;
        let decision = if verdict.decision.is_accept() {
            "accept".to_string()
        } else {
            verdict.decision.reason().map_or_else(
                || "reject (unknown)".to_string(),
                |r| format!("reject ({})", r.as_str()),
            )
        };
        let flags: Vec<&str> = verdict.envelope.flags.iter().map(|f| f.as_str()).collect();
        let flags = if flags.is_empty() {
            "none".to_string()
        } else {
            flags.join(", ")
        };
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Verify `{project}`: {decision} over {} run receipt(s), chain {}; \
                 quality samples {} (min {:.2}, median {:.2}, max {:.2}), flags: {flags}.",
                verdict.receipts,
                if verdict.chain_ok { "intact" } else { "BROKEN" },
                verdict.envelope.count,
                verdict.envelope.min,
                verdict.envelope.median,
                verdict.envelope.max,
            ))],
            details: serde_json::to_value(&verdict).unwrap_or(serde_json::Value::Null),
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
         changes (git-diff → affected symbols), status (store counts), verify (run receipts + quality of the last index run)."
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
            let started = std::time::Instant::now();
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
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let Some(query) = rerank_query else {
                return Ok(result.with_duration_ms(elapsed_ms));
            };
            Ok(self
                .rerank_search(&query, result, &signal)
                .await
                .with_duration_ms(elapsed_ms))
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
                    "architecture", "coverage", "changes", "status",
                    "verify", "adr"
                ],
                "description": "The graph operation to run."
            },
            "path": {
                "type": "string",
                "description": "Project root to index/query, relative to the working directory (or absolute). Defaults to the working directory."
            },
            "title": {
                "type": "string",
                "description": "ADR title; when present the record is written, otherwise the list is returned."
            },
            "decision": {
                "type": "string",
                "description": "What was decided."
            },
            "context": {
                "type": "string",
                "description": "Why the decision was needed."
            },
            "consequences": {
                "type": "string",
                "description": "What follows from the decision."
            },
            "status": {
                "type": "string",
                "description": "Lifecycle status: proposed, accepted, rejected or superseded."
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
            "depth": {
                "type": "string",
                "enum": ["skeleton", "symbols", "full"],
                "description": "How deep to build on action=\"index\". skeleton records one node per file (~1 s on this repo) and resolves nothing; symbols adds definitions; full (the default) resolves calls and imports (~2 min)."
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

    /// A query on a cold graph builds the index first, so the first answer is
    /// real and says so. This is the behavioural fix for the session where
    /// `search`/`trace` on an unindexed project answered "0 hits" and the agent
    /// concluded the graph had nothing to say, then read files one by one.
    /// Run one graph action against a fresh sandbox project.
    async fn run_action(
        dir: &std::path::Path,
        action: &str,
        extra: serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let tool = std::sync::Arc::new(tool(dir));
        let mut params = serde_json::json!({ "action": action });
        if let (Some(obj), Some(extra_obj)) = (params.as_object_mut(), extra.as_object()) {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
        pa_agent::types::AgentTool::execute(
            tool,
            "call".to_string(),
            params,
            pa_agent::abort::AbortController::new().signal(),
            std::sync::Arc::new(|_| {}),
        )
        .await
    }

    #[tokio::test]
    async fn a_cold_graph_indexes_itself_on_the_first_query() {
        // A real mini-project: two files with a call edge the indexer can see.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("a.rs"),
            "pub fn entry() -> u32 { helper() }\npub fn helper() -> u32 { 1 }\n",
        )
        .unwrap();

        let result = run_action(dir.path(), "search", serde_json::json!({"query": "helper"}))
            .await
            .expect("search on a cold graph must build and answer");
        let text = text_of(&result);
        assert!(
            text.contains("indexed the project first"),
            "the auto-index is named: {text}"
        );
        assert!(
            text.contains("1 hit(s)") || text.contains("2 hit(s)"),
            "the query answers with real hits, not 0: {text}"
        );
        // The tool stamps its own wall time so the run summary and the TUI card
        // can show it (fork tools used to report none at all).
        assert!(
            result
                .details
                .get("durationMs")
                .and_then(serde_json::Value::as_u64)
                .is_some(),
            "the graph result carries durationMs: {}",
            result.details
        );
    }

    /// A warm graph must NOT claim it indexed anything: the note follows the
    /// build, not every query.
    #[tokio::test]
    async fn a_warm_graph_does_not_claim_an_index_build() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.rs"), "pub fn one() -> u32 { 1 }\n").unwrap();

        // Warm the graph through the public surface, then query it.
        run_action(dir.path(), "index", serde_json::json!({}))
            .await
            .expect("index");
        let result = run_action(dir.path(), "search", serde_json::json!({"query": "one"}))
            .await
            .expect("search");
        assert!(
            !text_of(&result).contains("indexed the project first"),
            "a warm graph must not claim a build: {}",
            text_of(&result)
        );
    }

    /// `architecture` used to put the whole report in `details` and leave the
    /// model-visible `content` at a one-line stub — but providers only forward
    /// `content`, so the model never saw the architecture data. The report
    /// body must live in the content text itself.
    /// `adr` is the `adr` module's first consumer: a decision and the code it
    /// describes live in the same store, so they cannot drift apart.
    #[tokio::test]
    async fn adr_records_and_lists_decisions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let saved = run_action(
            dir.path(),
            "adr",
            serde_json::json!({
                "title": "Use one SQLite file for graph and decisions",
                "decision": "Keep ADRs beside the graph",
                "status": "accepted"
            }),
        )
        .await
        .expect("record");
        let text = text_of(&saved).to_string();
        assert!(text.contains("1 record"), "expected one record: {text}");
        assert!(
            text.contains("Use one SQLite file for graph and decisions"),
            "the title must be listed: {text}"
        );
        assert!(text.contains("[accepted]"), "status must be shown: {text}");
        let listed = run_action(dir.path(), "adr", serde_json::json!({}))
            .await
            .expect("list");
        assert!(
            text_of(&listed).to_string().contains("1 record"),
            "the second call must not duplicate the record"
        );
    }

    #[tokio::test]
    async fn architecture_report_lives_in_content_not_details() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        // a.rs depends on b.rs (a cross-file call edge), so `layers` has two
        // real depth buckets and `structure` has one real module edge.
        std::fs::write(
            src.join("a.rs"),
            "pub fn entry() -> u32 { helper() }
",
        )
        .unwrap();
        std::fs::write(
            src.join("b.rs"),
            "pub fn helper() -> u32 { 41 }
",
        )
        .unwrap();

        let layers = run_action(
            dir.path(),
            "architecture",
            serde_json::json!({"view": "layers"}),
        )
        .await
        .expect("architecture answers");
        let text = text_of(&layers);
        assert!(text.contains("Architecture view `layers`"), "{text}");
        assert!(
            text.contains("layer 0") && text.contains("layer 1"),
            "the depth buckets are in the content, not a stub: {text}"
        );
        assert!(text.contains("b.rs"), "{text}");
        assert!(text.contains("a.rs"), "the dependent file is named: {text}");
        assert!(
            !text.contains("see details"),
            "the old stub pointed the model at invisible details: {text}"
        );
        // `details` keeps the structured rows for tests and the UI.
        assert!(
            layers.details["layers"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty()),
            "{}",
            layers.details
        );

        let structure = run_action(
            dir.path(),
            "architecture",
            serde_json::json!({"view": "structure"}),
        )
        .await
        .expect("architecture answers");
        let text = text_of(&structure);
        assert!(
            text.contains("module edge(s)") && text.contains("->"),
            "the structure edges are in the content: {text}"
        );
        assert!(text.contains("b.rs"), "{text}");

        let overview = run_action(dir.path(), "architecture", serde_json::json!({}))
            .await
            .expect("architecture answers");
        let text = text_of(&overview);
        assert!(
            text.contains("node(s)") && text.contains("labels ["),
            "the census is in the content: {text}"
        );
        assert!(
            !text.contains("see details"),
            "no stub for the default view either: {text}"
        );
    }
}
