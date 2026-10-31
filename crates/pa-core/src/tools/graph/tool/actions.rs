//! The action bodies, one method per action (`index` .. `status`).

use crate::jev::graph_rerank::{RerankCandidate, VERIFIED_MARKER};
use crate::tools::graph::args::{
    parse_candidate_multiplier, parse_confidence_floor, parse_direction, parse_edge_types,
    parse_labels, parse_mmr, parse_respect_gitignore, render_counts, render_file_tree,
    render_hit_line, string_field, unmatched_note, with_effective_ignore, IndexScope,
    PATTERN_DEFAULT_LIMIT, SEARCH_MAX_LIMIT,
};
use crate::tools::graph::build::{build_evidence_pack, build_impact, project_name};
use crate::tools::graph::reports::{
    is_test_path, ChangeExplanation, RelevantDecision, PACK_DEFAULT_SYMBOLS, PACK_MAX_SYMBOLS,
};
use crate::tools::graph::tool::{GraphTool, GRAPH_OUTPUT_MAX_BYTES};
use pa_agent::abort::AbortSignal;
use pa_agent::types::AgentToolResult;
use pa_graph::{ArchitectureView, IndexOptions, SearchOptions, TraceOptions, Truncation};
use std::fmt::Write as _;

impl GraphTool {
    pub(crate) fn action_index(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let mut opts = IndexOptions::new(&project);
        if params
            .get("force")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            opts = opts.with_force(true);
        }
        // A skeleton depth lets an agent get a usable graph in about a second
        // instead of two minutes, then deepen it in the background.
        //
        // `index_depth` is the canonical spelling because plain `depth` means
        // the trace BFS depth on `trace`; keeping one name for both was the
        // ambiguity this tool now refuses to guess at. `depth` is still read
        // here, for `index` only, so an existing caller keeps working.
        let depth = params
            .get("index_depth")
            .or_else(|| params.get("depth"))
            .and_then(serde_json::Value::as_str);
        if let Some(depth) = depth {
            opts = opts.with_depth(pa_graph::indexer::Depth::from_str_or_default(depth));
        }
        // `false` (the default) indexes what is on disk, ignored files included;
        // `true` is the lean view. The mode decides the whole discovered set, so
        // switching it also widens or prunes what the run reports: a lean pass over
        // a store built with `false` drops the ignored files it can no longer see.
        let respect_gitignore = parse_respect_gitignore(params)?;
        opts = opts.with_respect_gitignore(respect_gitignore);
        // The store could already narrow the walk to an extension allowlist, cap a
        // file's size, and wait for another session's writer lease. None of the
        // three was reachable from a tool call, so the model could not scope a build
        // to one language, keep a vendored multi-megabyte file out, or ask a
        // contended build to wait instead of fail.
        let scope = IndexScope::read(params)?;
        opts = scope.apply(opts);
        self.warm_reader()?;
        let (report, unmatched) = self.with_store(|store| {
            let report = store
                .index_project(&root, &opts)
                .map_err(|err| anyhow::anyhow!("index failed: {err}"))?;
            // Read the misses back from the store: the honest answer to "which
            // extension matched nothing" is about the files this run left indexed.
            let unmatched = scope.unmatched(store, &project)?;
            Ok((report, unmatched))
        })?;
        let mut text = format!(
            "Indexed project `{project}` at {}: {} nodes in the store, {} edges, \
             {} files parsed ({} added, {} modified, {} removed, {} unchanged, {} failed, \
             {} partial, {} unusable, {} skipped; {} node(s) written this run).",
            root.display(),
            report.store_nodes,
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
            report.nodes,
        );
        if let Some(note) = unmatched_note(&unmatched) {
            text.push('\n');
            text.push_str(&note);
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details: with_effective_ignore(&report, respect_gitignore, scope.details(&unmatched)),
            terminate: None,
            is_error: false,
        })
    }

    pub(crate) fn action_reindex(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let respect_gitignore = parse_respect_gitignore(params)?;
        // The incremental path shares discovery with `index` and takes the same
        // writer lease, so it reads the same three scope knobs.
        let scope = IndexScope::read(params)?;
        let opts =
            scope.apply(IndexOptions::new(&project).with_respect_gitignore(respect_gitignore));
        self.warm_reader()?;
        let (report, unmatched) = self.with_store(|store| {
            let report = store
                .reindex_incremental(&root, &opts)
                .map_err(|err| anyhow::anyhow!("reindex failed: {err}"))?;
            let unmatched = scope.unmatched(store, &project)?;
            Ok((report, unmatched))
        })?;
        let mut text = format!(
            "Incrementally reindexed project `{project}`: {} files reindexed, {} closure \
             dependents, {} removed, {} unchanged (reason: {:?}).",
            report.files_reindexed,
            report.closure_files,
            report.removed,
            report.unchanged,
            report.reason,
        );
        if let Some(note) = unmatched_note(&unmatched) {
            text.push('\n');
            text.push_str(&note);
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details: with_effective_ignore(&report, respect_gitignore, scope.details(&unmatched)),
            terminate: None,
            is_error: false,
        })
    }

    pub(crate) fn action_search(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let query = params
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .ok_or_else(|| anyhow::anyhow!("search requires a non-empty `query`"))?;
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        // A cold graph answers "0 hits" for everything; building it here turns
        // the first query into a real answer instead of a false negative.
        let indexed_now = self.ensure_indexed(&root, &project)?;
        let mut opts = SearchOptions::new(&project);
        // The store can already diversify (MMR), restrict by label and filter by
        // regex, but nothing could ask it to: every knob was reachable only from
        // Rust. These four read the model's request into the same builder, so a
        // tool call can use what `SearchOptions` carries.
        //
        // What was asked is echoed in `details.search`, and only the keys the
        // caller actually set are recorded: a call that sets no knob produces no
        // `search` object at all, so its answer stays exactly what it was before
        // these knobs existed.
        let mut applied = serde_json::Map::new();
        if let Some(limit) = params.get("limit").and_then(serde_json::Value::as_u64) {
            // Clamped, not trusted (review #8): the limit sizes the pool, which sizes two FTS
            // `LIMIT`s, and `as i64` on a saturated pool is negative — which SQLite reads as
            // unlimited. A model-facing knob needs a ceiling the model cannot raise.
            let asked = usize::try_from(limit).unwrap_or(usize::MAX);
            let applied_limit = asked.min(SEARCH_MAX_LIMIT);
            // `limit` alone is not a tunable knob, so a search that asked for it and got it
            // must produce NO `details.search` — an existing test pins that. Only a real cut
            // is news, and it is the one thing the caller cannot otherwise notice.
            if applied_limit != asked {
                applied.insert(
                    "limitClampedTo".to_string(),
                    serde_json::json!(applied_limit),
                );
            }
            opts = opts.with_limit(applied_limit);
        }
        if let Some(raw) = params.get("rerank") {
            raw.as_bool()
                .ok_or_else(|| anyhow::anyhow!("`rerank` must be a boolean, got {raw}"))?;
        }
        if let Some(raw) = params.get("mmr") {
            let diversity = parse_mmr(raw)?;
            applied.insert("mmr".to_string(), serde_json::json!(diversity));
            // The parameter is a *diversity* weight (0 = none), while the store's
            // lambda is a *relevance* weight (1.0 = pure relevance); the two are
            // complements. `0` also leaves the store on its plain path instead of
            // reranking at lambda 1.0: "no diversity reranking" must mean the same
            // order as asking for nothing, not a re-sort that happens to ignore
            // diversity.
            if diversity > 0.0 {
                // In `[0, 1]` by `parse_mmr`, so the cosine weighting is exact
                // enough and the narrowing cast is safe.
                #[allow(clippy::cast_possible_truncation)]
                let lambda = (1.0 - diversity) as f32;
                opts = opts.with_mmr(lambda);
            }
        }
        if let Some(raw) = params.get("label") {
            let raw = raw.as_str().ok_or_else(|| {
                anyhow::anyhow!("`label` must be a comma-separated string, got {raw}")
            })?;
            let (labels, names) = parse_labels(raw)?;
            if !labels.is_empty() {
                applied.insert("labels".to_string(), serde_json::json!(names));
                opts = opts.with_labels(labels);
            }
        }
        if let Some(raw) = params.get("regex") {
            let pattern = raw
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("`regex` must be a string pattern, got {raw}"))?
                .trim();
            if !pattern.is_empty() {
                applied.insert("regex".to_string(), serde_json::json!(pattern));
                opts = opts.with_regex(pattern);
            }
        }
        if let Some(raw) = params.get("candidate_multiplier") {
            let (multiplier, clamped) = parse_candidate_multiplier(raw, opts.limit)?;
            applied.insert(
                "candidate_multiplier".to_string(),
                serde_json::json!(multiplier),
            );
            if clamped {
                applied.insert(
                    "candidate_multiplierClampedTo".to_string(),
                    serde_json::json!(multiplier),
                );
            }
            opts.candidate_multiplier = multiplier;
        }
        let (hits, cascade) = self.with_read(|store| {
            store
                .search_report(query, &opts)
                .map_err(|err| anyhow::anyhow!("search failed: {err}"))
        })?;
        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(report.clone());
        }
        // The cascade decides which channels may run; say so when it pruned one, so a
        // narrow result is attributable to the query's shape instead of looking like a
        // miss. A plan that runs everything is the default and stays unmentioned.
        if !cascade.skipped.is_empty() {
            let skipped: Vec<String> = cascade
                .skipped
                .iter()
                .map(|(tier, reason)| format!("{}({})", tier.as_str(), reason.as_str()))
                .collect();
            lines.push(format!(
                "[cascade: {} query, ran {}; skipped {}]",
                cascade.shape.as_str(),
                cascade
                    .running
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join("+"),
                skipped.join(","),
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
        // `details.search` sits between the hits and the cascade: the store's
        // answer, then what was asked of it, then how the plan was chosen.
        let mut details = serde_json::Map::new();
        details.insert("hits".to_string(), serde_json::json!(hits.len()));
        details.insert("results".to_string(), serde_json::Value::Array(results));
        if !applied.is_empty() {
            // A knob was set, so the echo is useful: it leads with the effective
            // limit, which bounds every other knob, then the knobs themselves. The
            // `limit` entry is set by the caller's own request, so it is merged rather
            // than overwritten — a clamped limit must report the CLAMP, not the ask.
            let mut echo = serde_json::Map::new();
            echo.insert("limit".to_string(), serde_json::json!(opts.limit));
            echo.append(&mut applied);
            // `append` lets the applied map win, which is what a reported clamp needs.
            details.insert("search".to_string(), serde_json::Value::Object(echo));
        }
        details.insert(
            "cascade".to_string(),
            serde_json::json!({
                "shape": cascade.shape.as_str(),
                "running": cascade.running.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                "skipped": cascade
                    .skipped
                    .iter()
                    .map(|(tier, reason)| serde_json::json!({
                        "tier": tier.as_str(),
                        "reason": reason.as_str(),
                    }))
                    .collect::<Vec<_>>(),
                "cost": cascade.cost,
            }),
        );
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::Value::Object(details),
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
    /// marked unjudged, never `kept`.
    pub(crate) async fn rerank_search(
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
                // `kept`, not `verified`: the hit cleared a 0.1 threshold, which
                // means the judge did not confidently reject it - not that it
                // confirmed it (audit F18). `judged` stays beside it so "the
                // judge never looked" is distinguishable from "the judge
                // rejected it".
                object.insert(
                    "kept".to_string(),
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

    pub(crate) fn action_trace(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let symbol = params
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|symbol| !symbol.is_empty())
            .ok_or_else(|| anyhow::anyhow!("trace requires a non-empty `symbol`"))?;
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
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
        // The work budget is the other half of a cap: `limit` bounds the answer,
        // this bounds the walk. `work_budget: 0` lifts the ceiling for a caller
        // that wants an exhaustive walk and accepts the cost.
        if let Some(budget) = params
            .get("work_budget")
            .and_then(serde_json::Value::as_u64)
        {
            opts = opts.with_work_budget((budget > 0).then_some(budget as usize));
        }
        // A wall-clock ceiling on the WALK: `work_budget` bounds a walk that finishes,
        // this bounds one that should not finish at all. `0` means "stop at the first
        // edge", which is how a caller asks for the shape without the cost.
        if let Some(ms) = params.get("timeout_ms").and_then(serde_json::Value::as_u64) {
            opts = opts
                .with_deadline(std::time::Instant::now() + std::time::Duration::from_millis(ms));
        }
        let edge_types =
            parse_edge_types(params.get("edge_types").and_then(serde_json::Value::as_str));
        if !edge_types.is_empty() {
            opts.edge_types = edge_types;
        }
        // The store already computed a confidence per step and already labelled
        // the weak ones; nothing could ask it to hide them. A floor is a filter
        // over that existing number, not a new signal, so it costs one comparison
        // per step and lets a caller read only the edges worth trusting.
        let confidence_floor = parse_confidence_floor(params)?;
        if let Some(floor) = confidence_floor {
            // In `[0, 1]` by `parse_confidence_floor`, so the narrowing cast keeps
            // every value the caller can write.
            #[allow(clippy::cast_possible_truncation)]
            let floor = floor as f32;
            opts = opts.with_confidence_floor(Some(floor));
        }
        let result = self.with_read(|store| {
            store
                .trace(&opts)
                .map_err(|err| anyhow::anyhow!("trace failed: {err}"))
        })?;
        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(report.clone());
        }
        lines.push(format!(
            "Trace from {} ({} step(s)){}{}:",
            result.origin,
            result.steps.len(),
            match result.stopped_by {
                Some(Truncation::Limit) => format!(
                    " — TRUNCATED at the {} node limit after examining {} edge(s); more \
                     reachable nodes were not listed, raise `limit` for the full set",
                    result.steps.len(),
                    result.edges_examined
                ),
                Some(Truncation::Cancelled) => format!(
                    " — STOPPED by cancellation after examining {} edge(s); the {} step(s) \
                     below are what the walk reached, not the reachable set",
                    result.edges_examined,
                    result.steps.len()
                ),
                Some(Truncation::WorkBudget) => format!(
                    " — TRUNCATED at the work budget after examining {} edge(s) and returning \
                     {} step(s); the walk stopped on COST, not on the graph's size, so a \
                     narrower query (lower `depth`, fewer `edge_types`, a more specific \
                     symbol) is what reaches the rest",
                    result.edges_examined,
                    result.steps.len()
                ),
                Some(Truncation::WitnessBytes) => format!(
                    " — TRUNCATED at the witness-byte ceiling after examining {} edge(s) and \
                     returning {} step(s); the paths kept for queued nodes grew past the \
                     budget, so deeper nodes were not explored. Lower `depth`",
                    result.edges_examined,
                    result.steps.len()
                ),
                Some(Truncation::Frontier) => format!(
                    " — TRUNCATED at the frontier ceiling after examining {} edge(s) and \
                     returning {} step(s); one node expanded into more than the walk will \
                     hold at once, so nodes BEYOND that fan-out were not explored. Lower \
                     `depth` or name a more specific symbol",
                    result.edges_examined,
                    result.steps.len()
                ),
                None => String::new(),
            },
            // A floor makes a short listing ambiguous unless the drops are named:
            // "2 steps" from a symbol that calls twenty reads as a small symbol.
            if result.filtered > 0 {
                format!(
                    " — {} step(s) below the confidence floor were filtered out;                      lower `confidence_floor` to see them",
                    result.filtered
                )
            } else {
                String::new()
            },
        ));
        // A zero with nothing filtered is UNKNOWN, not "none": method-values,
        // interface dispatch and cross-package calls do not resolve into call
        // edges, so "0" has read as "no callers" and sent an agent to delete
        // live code. The tools review's headline finding; the rule is the
        // fork's own "empty trace = UNKNOWN".
        let zero_is_unknown = result.steps.is_empty() && result.filtered == 0;
        if zero_is_unknown {
            lines.push(
                "0 resolved edges is UNKNOWN, not \"no callers\": method-value calls, interface                  dispatch and cross-package calls may not resolve into edges. Cross-check with \
                 `lsp find_referencing_symbols` or a text search before treating this as dead                  code."
                    .to_string(),
            );
        }
        for step in &result.steps {
            lines.push(format!(
                "- [depth {}] {} ({}:{}) via {} confidence={:.2}{}",
                step.depth,
                step.qualified_name,
                step.file_path,
                step.start_line,
                step.via.as_str(),
                step.confidence,
                // An ambiguous edge is a guess among same-named definitions. Say so:
                // measured worst case here is `new` with 400 candidates. GitNexus
                // records this exact shape as its "wrong edge at full confidence" bug.
                if step.candidates > 1 {
                    format!(" (ambiguous: {} candidates)", step.candidates)
                } else {
                    String::new()
                },
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
                    "candidates": step.candidates,
                    "risk_labels": step.risk_labels,
                })
            })
            .collect();
        let mut details = serde_json::Map::new();
        details.insert("origin_id".to_string(), serde_json::json!(result.origin_id));
        details.insert("origin".to_string(), serde_json::json!(result.origin));
        details.insert("steps".to_string(), serde_json::Value::Array(steps));
        details.insert("truncated".to_string(), serde_json::json!(result.truncated));
        if zero_is_unknown {
            details.insert("unknown".to_string(), serde_json::json!(true));
        }
        // Only a call that set a floor carries the echo, so a plain trace keeps
        // exactly the answer it had before the parameter existed.
        if let Some(floor) = confidence_floor {
            details.insert("confidence_floor".to_string(), serde_json::json!(floor));
            details.insert("filtered".to_string(), serde_json::json!(result.filtered));
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::Value::Object(details),
            terminate: None,
            is_error: false,
        })
    }

    pub(crate) fn action_architecture(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let indexed_now = self.ensure_indexed(&root, &project)?;
        let view = ArchitectureView::from_str_or_default(
            params
                .get("view")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("overview"),
        );
        let arch = self.with_read(|store| {
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
            lines.push(report.clone());
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
        if let Some(report) = &arch.cluster_report {
            // The honest header: "1 cluster" without this reads as a bug, and with it
            // reads as the shape of the code — which is what it is.
            if report.separated {
                lines.push(format!("{} component(s):", report.clusters.len()));
            } else {
                lines.push(
                    "Not separated into communities: one component holds most of the tree, \
                     so any grouping here would be invented. The hubs below are why."
                        .to_string(),
                );
            }
            for cluster in &report.clusters {
                lines.push(format!(
                    "component {} ({} file(s), {} internal edge(s)): {}",
                    cluster.id,
                    cluster.files.len(),
                    cluster.internal_edges,
                    cluster.files.join(", "),
                ));
            }
            if !report.hubs.is_empty() {
                lines.push(format!(
                    "{} hub file(s), in the top degree percentile: {}",
                    report.hubs.len(),
                    report.hubs.join(", ")
                ));
            }
        }
        if !arch.cycles.is_empty() {
            lines.push(format!("{} cycle(s):", arch.cycles.len()));
            for cycle in &arch.cycles {
                lines.push(format!("cycle: {}", cycle.files.join(" -> ")));
                for edge in &cycle.edges {
                    // The file pair says a dependency exists; the witness says WHICH symbols
                    // make it, and a cycle is always broken at a symbol (review §2:
                    // "объяснение исходными связями").
                    match &edge.witness {
                        Some(witness) => lines.push(format!(
                            "  {} -> {} (weight={}) via {witness}",
                            edge.from, edge.to, edge.weight,
                        )),
                        None => lines.push(format!(
                            "  {} -> {} (weight={})",
                            edge.from, edge.to, edge.weight,
                        )),
                    }
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
        let (capped, was_truncated) =
            crate::resources::truncate_context_content(&report, GRAPH_OUTPUT_MAX_BYTES);
        if was_truncated {
            report = format!(
                "{capped}\n[architecture report truncated at {GRAPH_OUTPUT_MAX_BYTES} bytes; \
                 query a narrower view]",
            );
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(report)],
            details: serde_json::to_value(&arch).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    pub(crate) fn action_coverage(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let report = self.with_read(|store| {
            store
                .coverage(&project)
                .map_err(|err| anyhow::anyhow!("coverage failed: {err}"))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Coverage for `{project}`: {}/{} files parsed cleanly ({:.0}%), {} partial, \
                 {} unusable, {} skipped, {} with no symbols; complete={}.\n\
                 Call sites: {} recorded, {} resolved to a definition ({:.0}%), \
                 {} in a workspace dependency, {} unresolved.",
                report.parsed,
                report.total,
                report.clean_ratio() * 100.0,
                report.partial,
                report.unusable,
                report.skipped,
                report.no_symbols,
                report.complete,
                report.call_sites,
                report.call_sites_resolved,
                report.resolution_ratio() * 100.0,
                report.call_sites_dependency,
                report.call_sites_unresolved,
            ))],
            details: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    /// `explain_change`: what a change to this symbol touches, in one answer.
    ///
    /// The review's §9 lists it among four task-oriented actions and every part already
    /// existed separately. Composition, not a new traversal: an agent asking this question
    /// should not have to call `impact`, `adr` and `search` and merge them.
    /// Structural search with ast-grep: `pattern`.
    ///
    /// Regex cannot answer `$A.foo($$$)` — that is what a structural match is for — so
    /// this is the one search shape `search` cannot stand in for. The engine lives in
    /// `pa-graph` (which already depends on `pa-ast` for symbol extraction) and this
    /// action is the caller that layer never had.
    pub(crate) fn action_pattern(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let pattern = params
            .get("pattern")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .ok_or_else(|| anyhow::anyhow!("pattern requires a non-empty `pattern`"))?
            .to_string();
        let root = self.resolve_root(params);
        // `path` narrows to one file or subtree; `glob` (default `**/*`) shapes the walk.
        let glob = params
            .get("glob")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|glob| !glob.is_empty())
            .map(str::to_string);
        let lang = params
            .get("lang")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let limit = params
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(PATTERN_DEFAULT_LIMIT, |limit| {
                usize::try_from(limit).unwrap_or(PATTERN_DEFAULT_LIMIT)
            });
        let search = pa_graph::PatternSearch {
            pattern: pattern.clone(),
            glob,
            lang,
            limit,
        };
        let report =
            pa_graph::search_pattern(&root, &search).map_err(|err| anyhow::anyhow!("{err}"))?;
        let mut text = format!(
            "{} structural match(es) for `{pattern}` in {} file(s) \
             ({} skipped: unsupported language or unreadable).",
            report.hits.len(),
            report.scanned_files,
            report.skipped_files,
        );
        if report.truncated {
            use std::fmt::Write as _;
            let _ = write!(text, " Stopped at the limit of {limit}.");
        }
        // Every answer carries the store's state on its last line; a structural scan
        // reads the tree, not the store, so it does not need to.
        if !report.hits.is_empty() {
            text.push('\n');
            text.push_str(
                &report
                    .hits
                    .iter()
                    .map(|hit| {
                        format!(
                            "{}:{}:{}: {}",
                            hit.path,
                            hit.line,
                            hit.column,
                            hit.text.lines().next().unwrap_or_default().trim()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details: serde_json::json!({
                "pattern": pattern,
                "matches": report.hits,
                "scanned_files": report.scanned_files,
                "skipped_files": report.skipped_files,
                "truncated": report.truncated,
            }),
            terminate: None,
            is_error: false,
        })
    }

    /// Drop one project from the store: `forget`.
    ///
    /// The store grew a project row per tree and had no way to drop one, so a database
    /// accumulated every checkout ever indexed. Four store operations existed for exactly
    /// this and had no caller (`delete_project`, `clear_project_graph`, `clear_coverage`,
    /// `coverage_count`) — the capability was written, the action never was.
    ///
    /// Safety rests on the key: `delete_project` takes the project NAME, and
    /// `project_key_in` forks a colliding basename into `name#digest`, so a delete by the
    /// key this tool computes cannot reach a same-named tree elsewhere. Deleting by bare
    /// basename would not have been safe to offer.
    pub(crate) fn action_forget(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        // Counted BEFORE the delete: afterwards there is nothing left to report, and the
        // caller needs to know what it removed.
        let before = self.with_read(|store| {
            Ok::<_, anyhow::Error>((
                store.node_count(&project).unwrap_or(0),
                store.edge_count(&project).unwrap_or(0),
                store.coverage_count(&project).unwrap_or(0),
            ))
        })?;
        if before.0 == 0 && before.1 == 0 && before.2 == 0 {
            return Ok(AgentToolResult::text(format!(
                "Nothing to forget: project `{project}` has no rows in {}.",
                self.db_path.display()
            )));
        }
        let removed = self.with_store(|store| {
            // The project row cascades to nodes, edges, hashes, call sites and receipts.
            // Coverage notes have no foreign key, so they are dropped explicitly — that is
            // what makes this one coherent operation instead of a half-cleaned database.
            let projects = store
                .delete_project(&project)
                .map_err(|err| anyhow::anyhow!("forget failed: {err}"))?;
            let coverage = store
                .clear_coverage(&project)
                .map_err(|err| anyhow::anyhow!("forget failed: {err}"))?;
            Ok::<_, anyhow::Error>((projects, coverage))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Forgot project `{project}` from {}: {} node(s), {} edge(s), \
                 {rows} project row(s), {notes} coverage note(s) removed. \
                 The next `index` rebuilds it from the tree.",
                self.db_path.display(),
                before.0,
                before.1,
                rows = removed.0,
                notes = removed.1,
            ))],
            details: serde_json::json!({
                "db_path": self.db_path.display().to_string(),
                "project": project,
                "nodes": before.0,
                "edges": before.1,
                "coverage_notes": removed.1,
            }),
            terminate: None,
            is_error: false,
        })
    }

    pub(crate) fn action_explain_change(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let symbol = params
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("`explain_change` needs a `symbol`"))?
            .to_string();
        let depth = params
            .get("depth")
            .and_then(serde_json::Value::as_u64)
            .map_or(3, |d| usize::try_from(d.clamp(1, 8)).unwrap_or(3));

        let impact = self.with_read(|store| build_impact(store, &project, &symbol, depth))?;

        // Which file does the symbol live in? The ADRs are searched by that file's name,
        // because that is how an ADR names the code it governs.
        let changed_file = impact
            .callers
            .first()
            .map(|caller| caller.file_path.clone())
            .or_else(|| {
                self.with_read(|store| {
                    store
                        .node_by_qn(&project, &symbol)
                        .map_err(|err| anyhow::anyhow!("lookup: {err}"))
                })
                .ok()
                .flatten()
                .map(|node| node.file_path)
            });

        // Which decisions really name the changed file: ONE query, ONE store read, and
        // the scope filter decides in memory (review #65).
        //
        // The candidate query already returns the full row, so the verification stage that
        // re-read each candidate through `get_adr_in_project` was an N+1 of SELECTs over
        // rows it held: measured with 1/5/20 matching candidates it issued 2/6/21 SELECTs
        // where 1 answers the question.
        //
        // It also read the scope rule differently from the list stage beside it: this
        // project's decisions OR the shared global bucket (an empty `project`, which
        // `action_adr`'s `id` read accepts for the same reason). Requiring an exact project
        // match dropped every global decision that named the file.
        let decisions: Vec<RelevantDecision> = match &changed_file {
            Some(file) => self.with_read(|store| {
                let adrs = pa_graph::adr::AdrStore::new(store.connection())
                    .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
                let file_name = file.rsplit('/').next().unwrap_or(file);
                let candidates = adrs
                    .query_adr(&pa_graph::adr::AdrQuery::new().with_text(file_name))
                    .map_err(|err| anyhow::anyhow!("adr query: {err}"))?;
                // `query_adr` returns newest first; the order is kept, so this reads like
                // the `adr` list action.
                let found: Vec<RelevantDecision> = candidates
                    .into_iter()
                    .filter(|adr| adr.project == project || adr.project.is_empty())
                    // Keep only decisions that really NAME the file: the query matched on
                    // the bare file name, and prose that mentions it in passing is not a
                    // decision about it.
                    .filter(|adr| {
                        let prose = format!(
                            "{}\n{}\n{}\n{}",
                            adr.title, adr.context, adr.decision, adr.consequences
                        );
                        pa_graph::adr::code_refs_in(&prose)
                            .iter()
                            .any(|reference| file.ends_with(reference) || reference.ends_with(file))
                    })
                    .map(|adr| RelevantDecision {
                        id: adr.id,
                        title: adr.title,
                        status: adr.status.as_str().to_string(),
                    })
                    .collect();
                Ok(found)
            })?,
            None => Vec::new(),
        };

        let tests: Vec<String> = impact
            .callers
            .iter()
            .filter(|caller| is_test_path(&caller.file_path))
            .map(|caller| format!("{} ({})", caller.qualified_name, caller.file_path))
            .collect();

        let mut not_covered = Vec::new();
        if impact.envelope.unresolved_mentions > 0 {
            not_covered.push(format!(
                "{} site(s) reference this symbol without a resolvable edge — see the \
                 candidates; they may be callers this answer cannot name",
                impact.envelope.unresolved_mentions
            ));
        }
        if !impact.envelope.empty_is_conclusive() {
            not_covered.push(
                "the caller set is at least what is listed and at most the index's \
                 completeness"
                    .to_string(),
            );
        }
        not_covered.push(
            "test coverage is by PATH CONVENTION: the graph has no test label, so a listed \
             file may not exercise this symbol and an unlisted one may"
                .to_string(),
        );

        let explanation = ChangeExplanation {
            symbol: symbol.clone(),
            impact: impact.clone(),
            decisions,
            tests,
            not_covered,
        };

        let mut lines = vec![format!("Changing `{symbol}`:")];
        if impact.callers.is_empty() {
            lines.push("  no resolved caller".to_string());
        } else {
            for caller in &impact.callers {
                lines.push(format!(
                    "  [depth {}] {} ({}:{}) via {}",
                    caller.depth,
                    caller.qualified_name,
                    caller.file_path,
                    caller.start_line,
                    caller.witness.join(" -> ")
                ));
            }
        }
        if impact.envelope.unresolved_mentions > 0 {
            lines.push(format!(
                "  {} candidate(s) the walk cannot follow:",
                impact.envelope.unresolved_mentions
            ));
            for candidate in &impact.unresolved_candidates {
                lines.push(format!(
                    "    {} ({}:{}) — {}",
                    candidate.caller_qn, candidate.rel_path, candidate.line, candidate.reason
                ));
            }
        }
        if explanation.decisions.is_empty() {
            lines.push("Decisions: none name this file.".to_string());
        } else {
            lines.push("Decisions:".to_string());
            for decision in &explanation.decisions {
                lines.push(format!(
                    "  #{} [{}] {}",
                    decision.id, decision.status, decision.title
                ));
            }
        }
        if explanation.tests.is_empty() {
            lines.push("Test files that call this: none found.".to_string());
        } else {
            lines.push("Test files that call this:".to_string());
            for test in &explanation.tests {
                lines.push(format!("  {test}"));
            }
        }
        lines.push("Not covered:".to_string());
        for gap in &explanation.not_covered {
            lines.push(format!("  {gap}"));
        }

        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::to_value(&explanation).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    /// `impact`: what a change to this symbol can reach, with its omissions counted.
    ///
    /// The review's §4: "Impact с UNKNOWN и lower bound ... Нулевой caller set не означает
    /// безопасное удаление". `trace direction=in` cannot answer it: it walks resolved CALLS
    /// edges, and a call the resolver could not place has no edge — so its "0 callers" is a
    /// lower bound that reads as safety. The unresolved half comes from `call_sites`, where
    /// every site is recorded whether it resolved or not.
    pub(crate) fn action_impact(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let symbol = params
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|symbol| !symbol.is_empty())
            .ok_or_else(|| anyhow::anyhow!("impact requires a non-empty `symbol`"))?;
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let indexed_now = self.ensure_indexed(&root, &project)?;
        let depth = params
            .get("depth")
            .and_then(serde_json::Value::as_u64)
            .map_or(3, |depth| (depth as usize).clamp(1, 8));

        let report = self.with_read(|store| {
            build_impact(store, &project, symbol, depth)
                .map_err(|err| anyhow::anyhow!("impact failed: {err}"))
        })?;

        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(report.clone());
        }
        lines.push(format!(
            "Impact of `{}`: {} resolved caller(s), {} unresolved site(s) that name it, \
             {} edge(s) examined{}.",
            report.symbol,
            report.envelope.resolved_callers,
            report.envelope.unresolved_mentions,
            report.envelope.edges_examined,
            if report.envelope.truncated {
                " (TRUNCATED — the walk hit its cap)"
            } else {
                ""
            },
        ));
        for caller in report.callers.iter().take(40) {
            // The path IS the explanation; a reader asking "why is my module affected"
            // needs the chain, not the last edge's type.
            let chain = caller.witness.join(" -> ");
            lines.push(format!(
                "- [depth {}] {} ({}:{}) confidence={:.2} via {}\n  why: {chain}",
                caller.depth,
                caller.qualified_name,
                caller.file_path,
                caller.start_line,
                caller.confidence,
                caller.strategy,
            ));
        }
        if !report.unresolved_candidates.is_empty() {
            lines.push(
                "Candidates the walk CANNOT follow (method values, interface dispatch, name \
                 guesses) — each may call this symbol:"
                    .to_string(),
            );
            for candidate in report.unresolved_candidates.iter().take(20) {
                lines.push(format!(
                    "- {} ({}:{}) — {}",
                    candidate.caller_qn, candidate.rel_path, candidate.line, candidate.reason
                ));
            }
        }
        if report.envelope.empty_is_conclusive() {
            // The one case where zero may be stated as a fact — and even then the scope is
            // named, because an incomplete INDEX is not the same as an empty caller set.
            lines.push(format!(
                "No caller is known for this symbol, resolved OR unresolved, and the walk was \
                 not capped. That is still bounded by index completeness: {} call site(s) in \
                 this project did NOT resolve, so a reference the indexer never placed is \
                 invisible to this answer.",
                report.envelope.project_unresolved_sites,
            ));
        } else if report.callers.is_empty() {
            lines.push(
                "0 RESOLVED callers is a LOWER BOUND, not safety: the unresolved sites above \
                 may call this symbol. Check them before deleting it or changing its signature."
                    .to_string(),
            );
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    /// `context`: assemble an evidence pack for a question under a budget.
    ///
    /// The single answer to "give me what I need to know about X", replacing the
    /// sequence a reader performs by hand: search, pick a few hits, trace them, open
    /// the files, notice the index is behind. Each of those steps is here, and each
    /// element says where it came from.
    pub(crate) fn action_context(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let query = params
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .ok_or_else(|| anyhow::anyhow!("context requires a non-empty `query`"))?;
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let indexed_now = self.ensure_indexed(&root, &project)?;
        let budget = params
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(PACK_DEFAULT_SYMBOLS, |limit| {
                (limit as usize).clamp(1, PACK_MAX_SYMBOLS)
            });

        let pack = self.with_read(|store| {
            build_evidence_pack(store, &project, &root, query, budget)
                .map_err(|err| anyhow::anyhow!("context failed: {err}"))
        })?;

        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(report.clone());
        }
        lines.push(format!(
            "Evidence pack for {:?}: {} symbol(s), {} link(s), {} omitted. Coverage: \
             {}/{} call site(s) resolved.",
            pack.query,
            pack.symbols.len(),
            pack.links.len(),
            pack.omitted.symbols_below_cut,
            pack.coverage.resolved,
            pack.coverage.sites,
        ));
        for symbol in &pack.symbols {
            let freshness = match symbol.current_lines {
                None => " FILE UNREADABLE".to_string(),
                Some(_) if symbol.stale => " STALE: the index disagrees with the file".to_string(),
                Some(_) => String::new(),
            };
            lines.push(format!(
                "- {} ({}:{}-{}) — {}{freshness}",
                symbol.qualified_name,
                symbol.file_path,
                symbol.start_line,
                symbol.end_line,
                symbol.reason
            ));
        }
        if pack.symbols.is_empty() {
            lines.push(
                "0 symbols is UNKNOWN, not \"nothing here\": the query may name behaviour the \
                 index does not resolve, and `coverage` above says how much it resolves at all."
                    .to_string(),
            );
        }
        if !pack.links.is_empty() {
            lines.push("Links between these symbols:".to_string());
            for link in &pack.links {
                let confidence = link
                    .confidence
                    .map(|value| format!(" confidence={value:.2}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "- {} -> {} ({}){confidence}",
                    link.from, link.to, link.edge_type
                ));
            }
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::to_value(&pack).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    pub(crate) fn action_changes(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
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
        let report = self.with_read(|store| {
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
    pub(crate) fn action_adr(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let record = params.get("title").and_then(serde_json::Value::as_str);
        let text = |key: &str| {
            params
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        // The list is scoped: this project's decisions by default, the global
        // bucket with `scope: "global"`, everything with `scope: "all"`.
        let query = match params
            .get("scope")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("all") => pa_graph::adr::AdrQuery::new(),
            Some("global") => pa_graph::adr::AdrQuery::new().in_project(String::new()),
            _ => pa_graph::adr::AdrQuery::new().in_project(project.clone()),
        };
        let query = match params.get("status").and_then(serde_json::Value::as_str) {
            Some(status) => {
                query.with_status(pa_graph::adr::AdrStatus::from_str_or_default(status))
            }
            None => query,
        };
        // `id` reads one decision's full body; listing stays a summary.
        //
        // The read is SCOPED like the list: `scope: "all"` answers any
        // workspace's decision, everything else requires the decision to belong
        // to this project or the global bucket. An unscoped `get_adr` let an id
        // read in one checkout return another checkout's decision (review 4.4).
        if let Some(id) = params.get("id").and_then(serde_json::Value::as_i64) {
            let unscoped = matches!(
                params
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("all")
            );
            let found = self.with_reader(|store| {
                let adrs = pa_graph::adr::AdrStore::new(store.connection())
                    .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
                if unscoped {
                    adrs.get_adr(id)
                        .map_err(|err| anyhow::anyhow!("get adr: {err}"))
                } else {
                    adrs.get_adr_in_project(id, &project)
                        .map_err(|err| anyhow::anyhow!("get adr: {err}"))
                }
                .map(|found| {
                    // A row may live in the GLOBAL bucket (empty project): it is
                    // shared vocabulary, so a scoped read accepts it too.
                    found.filter(|adr| unscoped || adr.project == project || adr.project.is_empty())
                })
            })?;
            let Some(adr) = found else {
                anyhow::bail!("adr #{id} not found");
            };
            let mut body = format!(
                "#{} [{}] {}\n\nContext:\n{}\n\nDecision:\n{}\n\nConsequences:\n{}\n",
                adr.id,
                adr.status.as_str(),
                adr.title,
                adr.context.trim(),
                adr.decision.trim(),
                adr.consequences.trim()
            );
            if let Some(supersedes) = adr.supersedes {
                use std::fmt::Write as _;
                let _ = writeln!(body, "\nSupersedes: #{supersedes}");
            }
            // "Which decision governs this file" needs the ADR's prose references checked
            // against the tree. The review's §2 names this as ADR's next step; `indexed` is
            // what keeps an extraction honest — a path the ADR names but the tree does not
            // have is drift a reader can act on, not a dependency fact.
            {
                let prose = format!(
                    "{}\n{}\n{}\n{}",
                    adr.title, adr.context, adr.decision, adr.consequences
                );
                let refs: Vec<pa_graph::adr::AdrCodeRef> = self.with_read(|store| {
                    let found = pa_graph::adr::code_refs_in(&prose)
                        .into_iter()
                        .map(|path| {
                            let symbols = store
                                .symbols_in_file(&adr.project, &path)
                                .map_err(|err| anyhow::anyhow!("symbols_in_file: {err}"))?;
                            Ok(pa_graph::adr::AdrCodeRef {
                                path,
                                indexed: symbols.is_some(),
                                symbols: symbols.unwrap_or(0),
                            })
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    Ok(found)
                })?;
                if !refs.is_empty() {
                    let _ = writeln!(body, "\nCode this decision names:");
                    for cref in &refs {
                        let _ = if cref.indexed {
                            writeln!(body, "  {} ({} symbol(s))", cref.path, cref.symbols)
                        } else {
                            writeln!(
                                body,
                                "  {} — NOT IN THE TREE (renamed or never existed)",
                                cref.path
                            )
                        };
                    }
                }
            }
            // Who replaced THIS one, which the record above cannot answer. The review's
            // check #7 says a superseded ADR must not become a confirmed dependency fact;
            // the reader needs that verdict in the same breath as the decision, or it will
            // act on a decision that no longer holds. `superseded_by_in_project` already
            // existed for this and had no caller.
            let replaced_by = self.with_reader(|store| {
                let adrs = pa_graph::adr::AdrStore::new(store.connection())
                    .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
                let found = adrs
                    .superseded_by_in_project(adr.id, &adr.project)
                    .map_err(|err| anyhow::anyhow!("superseded_by: {err}"))?;
                Ok::<_, anyhow::Error>(found)
            })?;
            if let Some(newer) = replaced_by {
                let _ = writeln!(
                    body,
                    "\nSUPERSEDED BY #{newer}: this decision no longer holds. Read #{newer} \
                     before acting on it."
                );
            }
            if !adr.project.is_empty() {
                let _ = writeln!(body, "Project: {}", adr.project);
            }
            return Ok(AgentToolResult {
                content: vec![pa_agent::types::ToolResultContent::text(body)],
                details: serde_json::json!({
                    "id": adr.id,
                    "title": adr.title,
                    "status": adr.status.as_str(),
                    "context": adr.context,
                    "decision": adr.decision,
                    "consequences": adr.consequences,
                    "supersedes": adr.supersedes,
                    "project": adr.project,
                }),
                terminate: None,
                is_error: false,
            });
        }
        // Recording writes, so it holds the store's lock. Listing reads only the
        // `adr` table — no graph content — so it takes the reader unconditionally
        // and can be answered during an index run.
        let rows = match record {
            Some(title) => self.with_store(|store| {
                let adrs = pa_graph::adr::AdrStore::new(store.connection())
                    .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
                let mut adr = pa_graph::adr::Adr::new(
                    title,
                    text("context"),
                    text("decision"),
                    text("consequences"),
                )
                // A decision belongs to the project it was made in: the graph DB
                // holds several projects, and one project's history is not
                // another's memory. `scope: "global"` records an install-wide
                // decision (the empty legacy bucket is only for rows written
                // before the column existed).
                .in_project(
                    if params
                        .get("scope")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|scope| scope.eq_ignore_ascii_case("global"))
                    {
                        String::new()
                    } else {
                        project.clone()
                    },
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
                adrs.query_adr(&query)
                    .map_err(|err| anyhow::anyhow!("query adr: {err}"))
            })?,
            None => self.with_reader(|store| {
                let adrs = pa_graph::adr::AdrStore::new(store.connection())
                    .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
                adrs.query_adr(&query)
                    .map_err(|err| anyhow::anyhow!("query adr: {err}"))
            })?,
        };
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
    pub(crate) fn action_verify(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let verdict = self.with_read(|store| {
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
        // The two questions the review says `verify` must keep apart: is the RUN honestly
        // recorded, and can THIS INDEX support a complete answer. A graph with half its
        // calls unresolved can pass the first and fail the second.
        let answer_coverage = match verdict.coverage {
            Some(coverage) if coverage.is_clean() => format!(
                "answer coverage {}/{} call site(s) resolved",
                coverage.resolved, coverage.sites
            ),
            Some(coverage) => format!(
                "answer coverage {}/{} ({:.0}%) — ANSWERS ARE PARTIAL: more than a third of \
                     the call graph did not resolve, so a caller set from this index is a lower \
                     bound",
                coverage.resolved,
                coverage.sites,
                coverage.ratio() * 100.0
            ),
            None => "answer coverage UNKNOWN (no call sites recorded)".to_string(),
        };
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Verify `{project}`: {decision} over {} run receipt(s), chain {}; \
                 quality samples {} (min {:.2}, median {:.2}, max {:.2}), flags: {flags}; \
                 {answer_coverage}.",
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

    pub(crate) fn action_status(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = self.project_key(&root);
        let (nodes, edges, files) = self.with_read(|store| {
            // Counts, not loads: the previous shape materialized every node and
            // edge row to answer with three integers (review §3.3).
            let nodes = store
                .node_count(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?;
            let edges = store
                .edge_count(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?;
            let files = store
                .file_count(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?;
            Ok((nodes, edges, files))
        })?;
        // The jev gate's state, stated where an agent checks it (tools report
        // finding 5): the gate is read ONCE at session construction, `on`
        // resolves to `auto` (the judgment layer runs only behind a native
        // judge), and neither fact was visible anywhere — so a settings.json
        // edit looked live when it was not.
        let rerank_line = self.rerank_gate.as_ref().map(|(raw, armed)| {
            let resolved = if *armed { "armed" } else { "off" };
            let mut line = format!(
                "Judge re-rank: {resolved} ([jev] graphRerank = {raw}; read at session                  start — an edit to settings.json applies to the NEXT session)."
            );
            if raw.eq_ignore_ascii_case("on") {
                line.push_str(
                    " \"on\" resolves to auto: the judgment layer runs only behind a \
                     native judge, so the two spellings are the same gate.",
                );
            }
            if *armed {
                line.push_str(" Opt out for one call with `rerank: false`.");
            }
            line
        });
        let mut text = format!(
            "Graph store {} — project `{project}`: {nodes} node(s), {edges} edge(s), \
             {files} file(s).",
            self.db_path.display(),
        );
        if let Some(line) = &rerank_line {
            text.push('\n');
            text.push_str(line);
        }
        // Tool cost, stated where a caller already asks about the store's health. The
        // numbers come from the process registry filled at the tool-call boundary, so
        // this is measured, not estimated — and `status` itself is the cheapest action,
        // so reading it does not distort what it reports.
        let tool_lines = crate::tools::metrics::summary_lines();
        if !tool_lines.is_empty() {
            text.push('\n');
            text.push_str(&tool_lines.join("\n"));
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details: serde_json::json!({
                "db_path": self.db_path.display().to_string(),
                "project": project,
                "nodes": nodes,
                "edges": edges,
                "files": files,
                "rerank_gate": rerank_line.map(|line| serde_json::json!(line)),
            }),
            terminate: None,
            is_error: false,
        })
    }
}
