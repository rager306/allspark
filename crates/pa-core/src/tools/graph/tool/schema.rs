//! The model-facing contract: `AgentTool`, the accepted-parameter table, the schema.

use crate::tools::graph::tool::GraphTool;
use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};
use std::sync::Arc;

/// The parameter names each action accepts (`action` itself aside).
///
/// The tool reads parameters with `.get(..)`, so without this table a
/// misspelt key silently falls back to the default and the call reports
/// success — the `GitNexus` bug #3261, where a caller sent `depth` instead of
/// `maxDepth`, got the default, and saw no error. The same shape was worse
/// here: `depth` named two different things (the index build depth, a string,
/// and the trace BFS depth, a number), so the wrong reading was silently
/// *plausible* instead of merely wrong. `index` still accepts plain `depth`
/// for compatibility; its canonical spelling is `index_depth`.
pub(crate) const ACCEPTED_PARAMS: &[(&str, &[&str])] = &[
    // `depth` here is the legacy spelling of `index_depth`.
    (
        "index",
        &[
            "path",
            "root",
            "force",
            "index_depth",
            "depth",
            "respect_gitignore",
            "lease_wait",
            "extensions",
            "max_file_bytes",
        ],
    ),
    // Discovery is shared with `index` (`reindex_incremental` calls the same
    // `discover`) and so is the writer lease, so the ignore mode and the three
    // scope knobs decide this run the same way.
    (
        "reindex",
        &[
            "path",
            "root",
            "respect_gitignore",
            "lease_wait",
            "extensions",
            "max_file_bytes",
        ],
    ),
    (
        "search",
        &[
            "path",
            "root",
            "query",
            "limit",
            "mmr",
            "label",
            "regex",
            "candidate_multiplier",
            "rerank",
        ],
    ),
    (
        "trace",
        &[
            "path",
            "root",
            "symbol",
            "direction",
            "depth",
            "limit",
            "edge_types",
            "confidence_floor",
            "work_budget",
            "timeout_ms",
        ],
    ),
    ("architecture", &["path", "root", "view"]),
    ("context", &["path", "root", "query", "limit"]),
    ("impact", &["path", "root", "symbol", "depth"]),
    ("explain_change", &["path", "root", "symbol", "depth"]),
    ("forget", &["path", "root"]),
    ("coverage", &["path", "root"]),
    // `refA`/`refB` are the camelCase spellings the schema never documented;
    // they stay accepted so an existing caller does not break.
    (
        "changes",
        &["path", "root", "ref_a", "refA", "ref_b", "refB"],
    ),
    ("status", &["path", "root"]),
    ("verify", &["path", "root"]),
    (
        "adr",
        &[
            "path",
            "root",
            "id",
            "scope",
            "title",
            "decision",
            "context",
            "consequences",
            "status",
            "supersedes",
        ],
    ),
];

/// The accepted parameter names for `action`, resolving the aliases
/// (`arch`, `detect_changes`, `reindex_incremental`) to their canonical entry.
pub(crate) fn accepted_params(action: &str) -> Option<&'static [&'static str]> {
    let canonical = match action {
        "arch" => "architecture",
        "detect_changes" => "changes",
        "reindex_incremental" => "reindex",
        "explain-change" | "explainChange" => "explain_change",
        other => other,
    };
    ACCEPTED_PARAMS
        .iter()
        .find(|(name, _)| *name == canonical)
        .map(|(_, accepted)| *accepted)
}

/// Reject any parameter `action` does not read.
///
/// The error names the offending key and lists the accepted ones: a caller
/// that gets a default back cannot tell a typo from an empty result, and the
/// next call repeats the mistake.
pub(crate) fn reject_unknown_params(
    action: &str,
    accepted: &[&str],
    params: &serde_json::Value,
) -> anyhow::Result<()> {
    let Some(object) = params.as_object() else {
        return Ok(());
    };
    for key in object.keys() {
        if key == "action" || accepted.contains(&key.as_str()) {
            continue;
        }
        anyhow::bail!(
            "unknown parameter {key:?} for action {action:?}: accepted are {}",
            accepted.join(", "),
        );
    }
    Ok(())
}

impl AgentTool for GraphTool {
    fn name(&self) -> &'static str {
        "graph"
    }

    fn description(&self) -> &'static str {
        "Query and build the on-disk code graph for the working directory. \
         Actions: index (full or incremental build of the symbol/call/import \
         graph; index_depth = skeleton|symbols|full, default full; \
         respect_gitignore = default true, false indexes what is on disk), reindex \
         (incremental closure repair), search (hybrid BM25/regex/degree symbol \
         search; mmr, label, regex and candidate_multiplier tune it), trace \
         (callers/callees from a symbol; depth = BFS hops, \
         default 2), architecture (overview/structure/layers/clusters/cycles/\
         hotspots/entry_points/file_tree), coverage (index honesty report), \
         changes (git-diff → affected symbols), status (store counts), verify \
         (run receipts + quality of the last index run), adr (record or list \
         architecture decisions; an ADR's prose file references are checked against \
         the index and drift is named), context (evidence pack under a budget), \
         impact (what a change reaches, with unresolved and guessed mentions counted \
         as candidates), explain_change (impact + decision + test files in one answer), \
         forget (drop THIS project's rows from the store; the key is the resolved \
         project name, so a same-named tree elsewhere is untouched). \
         Each action reads only its own parameters: \
         an unknown or misspelt one is an error naming the accepted set, never \
         a silent default."
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
            // `rerank: false` is the per-call opt-out the tools report asked
            // for: the judge's order is one input, and a caller who wants the
            // store's own RRF order — or does not want to spend a judgment on a
            // scoping query — says so for this one call. Only an explicit false
            // opts out; any other spelling falls through to `action_search`'s
            // validation, which names the type.
            let rerank_opt_out = params
                .get("rerank")
                .is_some_and(|raw| raw == &serde_json::json!(false));
            let rerank_query = (self.reranker.is_some()
                && !rerank_opt_out
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

/// The tool's JSON-Schema parameters.
#[must_use]
pub fn params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": [
                    "index",
                    "reindex",
                    "search",
                    "trace",
                    "impact",
                    "explain_change",
                    "architecture",
                    "coverage",
                    "changes",
                    "status",
                    "verify",
                    "adr",
                    "context",
                    "forget"
                ],
                "description": "The graph operation to run. `context` assembles a bounded evidence pack for a question: ranked symbols with their CURRENT line placement (flagged when the index disagrees with the file), the call links between symbols that are both in the pack, and the graph's own coverage. Prefer it over search+trace+open when you need several related facts at once."
            },
            "path": {
                "type": "string",
                "description": "Project root to index/query, relative to the working directory (or absolute). Defaults to the working directory."
            },
            "title": {
                "type": "string",
                "description": "ADR title; when present the record is written, otherwise the list is returned."
            },
            "id": {
                "type": "integer",
                "description": "ADR id for action=\"adr\": returns that decision's full body (context, decision, consequences)."
            },
            "scope": {
                "type": "string",
                "enum": ["project", "global", "all"],
                "description": "ADR scope: this project's decisions (default), install-wide decisions, or every record."
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
                "description": "Maximum trace BFS depth in hops for action=\"trace\" (default 2). Not the index build depth - that is index_depth."
            },
            "index_depth": {
                "type": "string",
                "enum": ["skeleton", "symbols", "full"],
                "description": "How deep to build on action=\"index\". skeleton records one node per file (~1 s on this repo) and resolves nothing; symbols adds definitions; full (the default) resolves calls and imports (~2 min). Plain \"depth\" is accepted as a legacy alias for index only."
            },
            "edge_types": {
                "type": "string",
                "description": "Comma-separated edge filter for trace, e.g. \"calls,imports\" (default calls)."
            },
            "confidence_floor": {
                "type": "number",
                "minimum": 0.0,
                "maximum": 1.0,
                "description": "action=\"trace\": keep only steps whose confidence is at least this, in [0, 1]. A step's confidence is the product of the edge confidences along its path from the start symbol, so it falls with depth and one guessed edge discounts everything reached through it. Each edge's baseline is set by how it was resolved: an imported callee 0.95, one in the same module 0.90, a known external 0.80, a project-wide unique name 0.75, a suffix guess 0.55. A floor of 0.6 therefore keeps direct calls and drops anything reached through a guess; because the values multiply, a deep path through only-resolved edges falls fastest, so lower the floor or check the printed per-step confidence before concluding a deep path is absent. The trace header states how many steps the floor removed. Omit to keep every step and only label the weak ones. An edge that recorded no confidence counts as fully confident and is never dropped: the indexer writes a confidence on every edge it resolves, so an absent one belongs to an edge that never claimed it."
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
                "description": "Result cap for search/trace: how many hits or steps may be RETURNED."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "action=\"trace\": wall-clock ceiling for the walk in milliseconds. `work_budget` bounds a walk that finishes; this bounds one that should NOT finish — a very high-fanout symbol whose answer is not worth the cost. On expiry the walk stops where it is and the header says `STOPPED by cancellation ... not the reachable set`, so a short answer is not read as the whole neighbourhood. 0 stops at the first edge, which is how a caller asks for the shape without the cost."
            },
            "work_budget": {
                "type": "integer",
                "minimum": 0,
                "description": "action=\"trace\": how many EDGES the walk may examine (default 1000). This bounds cost, not the answer: `limit` caps the steps returned, while `work_budget` caps the work spent finding them. They differ when a node has many edges below the confidence floor - those are counted as filtered and still expanded, so a high-fanout call can exhaust the budget and return few steps. The header then says the walk stopped on COST, not on the graph's size, which calls for a narrower query rather than a bigger `limit`. 0 lifts the ceiling for an exhaustive walk."
            },
            "rerank": {
                "type": "boolean",
                "description": "action=\"search\": set false to skip the judge re-rank for THIS call and get the store's own RRF order, every hit included. Default: the session's [jev] graphRerank setting (read at session start). A demoted hit is never deleted — it is moved below the survivors and labelled `demoted-by-jev (p=…)` — so the opt-out is for callers who want the un-judged ordering, not for seeing hidden hits."
            },
            "mmr": {
                "type": "number",
                "minimum": 0.0,
                "maximum": 1.0,
                "description": "action=\"search\": diversity reranking weight in [0, 1]. 0 keeps pure relevance and 1 keeps the most spread-out set; 0.5-0.7 is the useful band when the top hits are near-duplicate names (overloaded methods, generated wrappers) and you want one representative of each instead of five spellings of the same idea. Omit to keep the store's own order."
            },
            "label": {
                "type": "string",
                "description": "action=\"search\": comma-separated node labels to restrict the result to, e.g. \"function,method\" or \"struct\". Accepted: function, method, class, struct, enum, interface, module, file, folder, branch. Use it when the answer must be a definition of one kind (\"which structs do this\") instead of any symbol whose name matches. An unknown label is an error naming the accepted ones."
            },
            "regex": {
                "type": "string",
                "description": "action=\"search\": Rust regex applied as a filter over the hit's name, qualified name and file path, e.g. \"^render_\" or \"/crates/pa-graph/\". Use it to narrow a broad query when you know the spelling or the directory; it filters the ranked candidates rather than replacing the text query. An invalid pattern is an error carrying the compiler's message and position."
            },
            "candidate_multiplier": {
                "type": "integer",
                "minimum": 1,
                "description": "action=\"search\": how many candidates each retrieval channel fetches per requested hit before fusion (default 4, i.e. limit x 4). Raise it when a filter, regex or mmr drops candidates after ranking and the answer comes back short; it costs recall work, not accuracy."
            },
            "force": {
                "type": "boolean",
                "description": "Re-parse every file on action=\"index\", ignoring stored hashes."
            },
            "lease_wait": {
                "type": "integer",
                "minimum": 0,
                "description": "action=\"index\"/\"reindex\": how many milliseconds to wait for another session's index run to release the database's single writer before failing, instead of failing at once. A full pass over a large tree is measured at over two minutes, so a second session arriving mid-build usually wants to wait and then find the work already done. 0 means one attempt, then give up (the default). Raise it when two sessions may index the same project; the call then comes back with the finished graph instead of a timeout."
            },
            "extensions": {
                "type": "string",
                "description": "action=\"index\"/\"reindex\": comma-separated extension allowlist, e.g. \"rs,ts\". Case and a leading dot are ignored (.RS is rs). Use it to scope a build to one language in a mixed tree, so a query is not diluted by generated or vendored files in another language. An extension that matches nothing is NOT an error - the answer says which ones matched no file, because an empty graph from a misspelled extension would otherwise read as an empty codebase."
            },
            "max_file_bytes": {
                "type": "integer",
                "minimum": 1,
                "description": "action=\"index\"/\"reindex\": largest file to index, in bytes (default 2 MiB). Use it to keep a huge generated or minified file out of the graph. On `index` the file is read just far enough to measure it, then skipped: `skipped` counts it and the coverage notes name it as `oversized: N bytes`, so a symbol that is missing from the graph has a stated reason. `reindex` applies the same cap to its repair set and leaves such a file alone rather than re-parsing it, but reports no per-file skip."
            },
            "respect_gitignore": {
                "type": "boolean",
                "description": "action=\"index\"/\"reindex\": whether to hide files the project's gitignore rules exclude (.gitignore, .git/info/exclude, the global gitignore, and those of parent directories). Default true (the lean view, matching the implicit cold-start build): where the ignored set is build output, vendored copies or state directories, it keeps the duplicate hits and most of the walk out of the graph. false indexes what is on disk, so a deliberately ignored but authored file becomes visible and searchable — one parameter away when wanted. The mode decides the discovered set, so a run in the other mode adds or removes the files only it can see (`removed` counts them). .git is always skipped; `.ignore` files are honoured in both modes. On a large tree, measure both before choosing: the difference can be an order of magnitude of files."
            },
            "supersedes": {
                "type": "integer",
                "description": "action=\"adr\": id of an earlier decision this one replaces."
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
