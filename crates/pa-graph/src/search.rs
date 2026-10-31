//! Symbol search: BM25 (FTS5) + regex + graph-degree, fused with RRF.
//!
//! Three independent ranked channels feed a single RRF merge:
//!
//! 1. **name** — FTS5 BM25 over the split-token column (identifier-aware);
//! 2. **body** — FTS5 BM25 over docstrings and comments;
//! 3. **graph** — in/out degree (structurally central symbols float up).
//!
//! A regex channel and a payload [`Filter`] narrow the candidate set before
//! ranking; an optional [`Embedder`] seam is reserved for a future dense
//! channel, and [`SearchOptions::mmr_lambda`] applies MMR diversity reranking.
//!
//! Which of those channels do any work is decided by [`cascade::plan`] over the
//! query's shape (see [`search_report`]), and the plan travels out with the
//! results so a thin answer can be explained instead of guessed at.

use std::collections::{HashMap, HashSet};

use regex::Regex;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;

use crate::bm25::min_max_normalize;
use crate::cascade::{self, Plan, QueryShape, SkipReason, Tier};
use crate::error::{GraphError, Result};
use crate::filter::{renumber_placeholders, Filter};
use crate::hybrid::{rrf_fuse_weighted, MmrCandidate};
use crate::tokenize::{fts_match_expr, split_for_fts};
use crate::types::Label;

/// A reserved seam for a future dense/vector retrieval channel.
///
/// The MVP ranks with BM25 + regex + graph degree only; when embeddings land,
/// an implementation of this trait becomes a fourth RRF channel without
/// changing the public search signature.
pub trait Embedder: Send + Sync {
    /// Embed `text` into a dense vector.
    fn embed(&self, text: &str) -> Vec<f32>;

    /// Rank `candidate_ids` by similarity to `query`, best first.
    fn rank(&self, query: &str, candidate_ids: &[i64]) -> Vec<i64>;
}

/// A no-op embedder used when no vector channel is configured.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEmbedder;

impl Embedder for NoopEmbedder {
    fn embed(&self, _text: &str) -> Vec<f32> {
        Vec::new()
    }

    fn rank(&self, _query: &str, _candidate_ids: &[i64]) -> Vec<i64> {
        Vec::new()
    }
}

/// Largest candidate pool one search may fuse.
///
/// A `usize` pool above `i64::MAX` casts to a NEGATIVE `i64`, which SQLite reads as
/// `LIMIT -1` — the opposite of a bound. This is the ceiling that keeps the cast honest, and
/// it sits in the library so a caller outside the tool cannot bypass it.
pub const MAX_POOL: usize = 4096;

/// Options controlling a [`crate::GraphStore::search`] call.
#[derive(Clone)]
pub struct SearchOptions {
    /// Project to search within.
    pub project: String,
    /// Maximum number of hits to return.
    pub limit: usize,
    /// Candidate pool fetched per channel before fusion (`limit * this`).
    pub candidate_multiplier: usize,
    /// Restrict to these labels.
    pub labels: Vec<Label>,
    /// Optional Rust regex over `name` / `qualified_name` / `file_path`.
    pub regex: Option<String>,
    /// Optional payload filter compiled to SQL.
    pub filter: Option<Filter>,
    /// Optional MMR diversity parameter in `[0, 1]`; `None` disables reranking.
    pub mmr_lambda: Option<f32>,
    /// Optional dense channel (reserved).
    pub embedder: Option<std::sync::Arc<dyn Embedder>>,
    /// Per-channel weights for the RRF merge (name, body, graph).
    pub weights: [f32; 3],
}

impl std::fmt::Debug for SearchOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchOptions")
            .field("project", &self.project)
            .field("limit", &self.limit)
            .field("candidate_multiplier", &self.candidate_multiplier)
            .field("labels", &self.labels)
            .field("regex", &self.regex)
            .field("filter", &self.filter)
            .field("mmr_lambda", &self.mmr_lambda)
            .field("embedder", &self.embedder.as_ref().map(|_| "<embedder>"))
            .field("weights", &self.weights)
            .finish()
    }
}

impl SearchOptions {
    /// Default options for `project` (`limit = 20`).
    #[must_use]
    pub fn new(project: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            limit: 20,
            candidate_multiplier: 4,
            labels: Vec::new(),
            regex: None,
            filter: None,
            mmr_lambda: None,
            embedder: None,
            weights: [1.0, 0.5, 0.25],
        }
    }

    /// Set the result limit.
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Restrict to specific labels.
    #[must_use]
    pub fn with_labels(mut self, labels: Vec<Label>) -> Self {
        self.labels = labels;
        self
    }

    /// Apply a regex pre-filter.
    #[must_use]
    pub fn with_regex(mut self, pattern: impl Into<String>) -> Self {
        self.regex = Some(pattern.into());
        self
    }

    /// Apply a payload filter.
    #[must_use]
    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Enable MMR diversity reranking with the given lambda.
    #[must_use]
    pub fn with_mmr(mut self, lambda: f32) -> Self {
        self.mmr_lambda = Some(lambda);
        self
    }
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Node id.
    pub id: i64,
    /// Node label.
    pub label: Label,
    /// Short name.
    pub name: String,
    /// Qualified name.
    pub qualified_name: String,
    /// Repo-relative file path.
    pub file_path: String,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line.
    pub end_line: u32,
    /// Fused RRF score.
    pub score: f32,
    /// Incoming edge count.
    pub in_degree: usize,
    /// Outgoing edge count.
    pub out_degree: usize,
    /// Rank in each contributing channel (0-based), absent when not ranked.
    pub channels: ChannelRanks,
}

/// Which channels contributed a hit, and at what rank.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChannelRanks {
    /// Rank in the FTS name channel.
    pub name: Option<usize>,
    /// Rank in the FTS body channel.
    pub body: Option<usize>,
    /// Rank in the graph-degree channel.
    pub graph: Option<usize>,
}

/// Row projection shared by the SQL queries below.
struct Row {
    id: i64,
    label: String,
    name: String,
    qualified_name: String,
    file_path: String,
    start_line: i64,
    end_line: i64,
}

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: r.get(0)?,
        label: r.get(1)?,
        name: r.get(2)?,
        qualified_name: r.get(3)?,
        file_path: r.get(4)?,
        start_line: r.get(5)?,
        end_line: r.get(6)?,
    })
}

const ROW_COLS: &str = "id,label,name,qualified_name,file_path,start_line,end_line";

/// What the cascade decided for one query: the tiers that ran, the tiers that
/// did not, and why.
///
/// Travels out of the store unchanged on every call, including an error-free
/// call with no hits, so a caller can tell "nothing matched" from "the channel
/// that would have matched was not run".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeReport {
    /// The query shape the plan was made for.
    pub shape: QueryShape,
    /// The tiers that ran, in order.
    pub running: Vec<Tier>,
    /// Tier -> why it was skipped, for every tier that did not run.
    pub skipped: Vec<(Tier, SkipReason)>,
    /// Sum of the running tiers' `cost_weight`.
    pub cost: u32,
}

impl CascadeReport {
    fn of(plan: &Plan) -> Self {
        Self {
            shape: plan.shape,
            running: plan.running(),
            skipped: plan
                .steps
                .iter()
                .filter_map(|step| step.reason.map(|reason| (step.tier, reason)))
                .collect(),
            cost: plan.cost,
        }
    }

    /// Why `tier` did not run, or `None` when it did.
    #[must_use]
    pub fn skip_reason(&self, tier: Tier) -> Option<SkipReason> {
        self.skipped
            .iter()
            .find(|(t, _)| *t == tier)
            .map(|(_, reason)| *reason)
    }

    /// The report as JSON, the shape `details.cascade` carries.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let skipped: serde_json::Map<String, serde_json::Value> = self
            .skipped
            .iter()
            .map(|(tier, reason)| {
                (
                    tier.as_str().to_string(),
                    serde_json::Value::from(reason.as_str()),
                )
            })
            .collect();
        serde_json::json!({
            "shape": self.shape.as_str(),
            "running": self.running.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
            "skipped": skipped,
            "cost": self.cost,
        })
    }
}

/// Whether the cascade stopped at the exact-name answer.
///
/// This is the whole tier-to-channel mapping. Only one tier maps onto a channel
/// honestly: `Tier::Name` gates the name channel, and every shape runs it, so
/// the name channel always runs. `Tier::Signals` would be the natural home of
/// the body channel, but `search` has no similarity channel today and this
/// change does not invent one; `Tier::Structure` (the call-graph neighbourhood)
/// belongs to `trace` and `Tier::Community` (the architecture view) to
/// `architecture`, so both gate nothing here. They are left out of the mapping
/// rather than pretended onto a channel.
///
/// What the plan does decide for `search` is whether anything beyond exact name
/// lookup is worth doing: an identifier stops at `Name`, and a second pass over
/// docstrings and comments cannot improve on an exact name hit.
///
/// The graph-degree channel is deliberately ungated: it is one extra ordering
/// over the same FTS match, no tier describes it, and dropping it would change
/// the RRF fusion for identifier queries.
fn stops_at_exact_name(plan: &Plan) -> bool {
    plan.running() == [Tier::Name]
}

/// Execute a search against `conn` for `query`.
///
/// # Errors
/// Returns [`GraphError::InvalidQuery`] for an unparseable regex or empty
/// query, and propagates SQLite errors.
pub fn search(conn: &Connection, query: &str, opts: &SearchOptions) -> Result<Vec<SearchHit>> {
    search_report(conn, query, opts).map(|(hits, _)| hits)
}

/// Run a search and also return what the cascade decided for it.
///
/// Same query, same ranking, same order as [`search`]; the second element is the
/// plan's outcome, for a caller that wants to show the decision (the `graph`
/// tool's `details.cascade`) rather than just the hits.
///
/// # Errors
/// As [`search`].
pub fn search_report(
    conn: &Connection,
    query: &str,
    opts: &SearchOptions,
) -> Result<(Vec<SearchHit>, CascadeReport)> {
    let query = query.trim();
    if query.is_empty() {
        return Err(GraphError::InvalidQuery("empty query".into()));
    }
    let regex = match &opts.regex {
        Some(pattern) => Some(
            Regex::new(pattern)
                .map_err(|e| GraphError::InvalidQuery(format!("invalid regex: {e}")))?,
        ),
        None => None,
    };

    // The cascade decides the work before any channel runs, so the plan is a
    // property of the query alone — not of what the channels happened to find.
    let plan = cascade::plan(QueryShape::classify(query), cascade::DEFAULT_BUDGET, &[]);
    let report = CascadeReport::of(&plan);

    // `saturating_mul` can reach `usize::MAX`, and `usize::MAX as i64` is NEGATIVE, which
    // SQLite reads as `LIMIT -1` — no limit at all. The cast below is the last place this is
    // safe, so the ceiling lives here as well as at the tool boundary: a library caller that
    // never sees the tool must not be able to turn one query into a full-table scan.
    let pool = opts
        .limit
        .max(1)
        .saturating_mul(opts.candidate_multiplier.max(1))
        .min(crate::search::MAX_POOL);

    let name_channel = fts_channel(conn, query, "tokens", opts, pool)?;
    // Skipped when the plan stops at the exact-name answer: a docstring pass
    // cannot say anything an exact name hit does not.
    let body_channel = if stops_at_exact_name(&plan) {
        Vec::new()
    } else {
        fts_channel(conn, query, "body", opts, pool)?
    };
    // The graph channel ranks by degree inside its own query, and that degree ordering is
    // the channel's whole contribution to the fusion: it stays.
    let graph_channel = graph_channel(conn, query, opts, pool)?;

    let mut channels: Vec<(Vec<i64>, f32)> = vec![
        (name_channel.clone(), opts.weights[0]),
        (body_channel.clone(), opts.weights[1]),
        (graph_channel.clone(), opts.weights[2]),
    ];
    // A dense channel is the only producer of ids that need not name a row: the three
    // FTS channels join `nodes`, so their ids always do.
    let mut dense_channel = false;
    if let Some(embedder) = &opts.embedder {
        let mut ids: Vec<i64> = name_channel
            .iter()
            .chain(body_channel.iter())
            .chain(graph_channel.iter())
            .copied()
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let dense = embedder.rank(query, &ids);
        if !dense.is_empty() {
            channels.push((dense, 1.0));
            dense_channel = true;
        }
    }

    let page = ranked_page(conn, opts, &channels, regex.as_ref(), dense_channel)?;

    let rank_maps = RankMaps {
        name: index_ranks(&name_channel),
        body: index_ranks(&body_channel),
        graph: index_ranks(&graph_channel),
    };
    let mut hits: Vec<SearchHit> = page
        .entries
        .into_iter()
        .map(|(row, score)| SearchHit {
            id: row.id,
            label: Label::from_str_or_default(&row.label),
            name: row.name,
            qualified_name: row.qualified_name,
            file_path: row.file_path,
            start_line: row.start_line as u32,
            end_line: row.end_line as u32,
            score,
            // Degrees are display-only: they never order a hit, so they are counted
            // after the last cut, for the page that survives it (review #26).
            in_degree: 0,
            out_degree: 0,
            channels: ChannelRanks {
                name: rank_maps.name.get(&row.id).copied(),
                body: rank_maps.body.get(&row.id).copied(),
                graph: rank_maps.graph.get(&row.id).copied(),
            },
        })
        .collect();

    if let Some(lambda) = opts.mmr_lambda {
        hits = apply_mmr(hits, opts.limit, lambda);
    }

    // Two queries for the whole page instead of two per row, and only for the rows that
    // survive the cut.
    let page_ids: Vec<i64> = hits.iter().map(|hit| hit.id).collect();
    let degrees_by_id = degrees_batch(conn, &opts.project, &page_ids)?;
    for hit in &mut hits {
        let (in_degree, out_degree) = degrees_by_id.get(&hit.id).copied().unwrap_or((0, 0));
        hit.in_degree = in_degree;
        hit.out_degree = out_degree;
    }

    Ok((hits, report))
}

/// The rows a search will report, in page order, each with its fused score.
struct Page {
    /// `(row, fused score)`, best first.
    entries: Vec<(Row, f32)>,
}

/// Fuse the channels, filter, and cut to the page — the half of `search_report` that
/// decides WHICH rows are reported, before any of them is decorated.
///
/// The order of the stages is the point (review #26/#59). A row is loaded once, and only
/// for what survives: the candidate set is materialised only when something before the
/// cut needs a row (a regex, or a dense channel whose ids may name none), and otherwise
/// the page is cut first and loaded once.
fn ranked_page(
    conn: &Connection,
    opts: &SearchOptions,
    channels: &[(Vec<i64>, f32)],
    regex: Option<&Regex>,
    dense_channel: bool,
) -> Result<Page> {
    let fused = rrf_fuse_weighted(channels);

    // Candidate rows are materialised ONCE (review #59): a regex needs `name`,
    // `qualified_name` and `file_path`, and the page needs the same columns, so the
    // filter's map is the page's map instead of a second read of the same ids.
    let candidate_rows = if regex.is_some() || dense_channel {
        Some(load_rows(conn, &ids_of(&fused))?)
    } else {
        None
    };

    // A dense id with no row cannot be a hit, and must not take a page slot a real hit
    // would have had. Only the dense path can produce one, so only it pays for the check.
    let mut fused = fused;
    if dense_channel {
        if let Some(rows) = &candidate_rows {
            fused.retain(|(id, _)| rows.contains_key(id));
        }
    }

    // Regex is a *filter*, not a ranker: drop non-matching candidates.
    let fused: Vec<(i64, f32)> = match (regex, &candidate_rows) {
        (Some(re), Some(rows)) => fused
            .into_iter()
            .filter(|(id, _)| {
                rows.get(id).is_some_and(|r| {
                    re.is_match(&r.name)
                        || re.is_match(&r.qualified_name)
                        || re.is_match(&r.file_path)
                })
            })
            .collect(),
        _ => fused,
    };

    // The page is cut before any per-row work: fusion order and scores decide who is on
    // it, and neither needs a row. MMR is the one exception — it reranks the candidate
    // set on name features — so it takes the rows of every candidate.
    let hits: Vec<(i64, f32)> = if opts.mmr_lambda.is_some() {
        fused
    } else {
        fused.into_iter().take(opts.limit).collect()
    };

    let mut rows = match candidate_rows {
        Some(rows) => rows,
        None => load_rows(conn, &ids_of(&hits))?,
    };
    // An id whose row is gone is not a hit: `search` has always dropped those, and the
    // page inherits that tolerance rather than asserting rows every id.
    let entries = hits
        .into_iter()
        .filter_map(|(id, score)| rows.remove(&id).map(|row| (row, score)))
        .collect();
    Ok(Page { entries })
}

/// The ids of a ranked list, in rank order.
fn ids_of(ranked: &[(i64, f32)]) -> Vec<i64> {
    ranked.iter().map(|(id, _)| *id).collect()
}

struct RankMaps {
    name: HashMap<i64, usize>,
    body: HashMap<i64, usize>,
    graph: HashMap<i64, usize>,
}

fn index_ranks(channel: &[i64]) -> HashMap<i64, usize> {
    channel.iter().enumerate().map(|(i, id)| (*id, i)).collect()
}

/// Run an FTS5 BM25 query restricted to the `restrict_col` column.
///
/// `restrict_col` is a compile-time constant (`tokens` for the name channel,
/// `body` for the docstring channel), never user input.
fn fts_channel(
    conn: &Connection,
    query: &str,
    restrict_col: &str,
    opts: &SearchOptions,
    pool: usize,
) -> Result<Vec<i64>> {
    let Some(match_expr) = fts_match_expr(query) else {
        return Ok(Vec::new());
    };
    // Restrict the MATCH to one column: `{col} : (expr)`.
    let column_match = format!("{restrict_col} : ({match_expr})");

    // Parameters, in placeholder order: ?1 = MATCH, ?2 = project, then the
    // label and filter parameters, then the LIMIT.
    let mut params: Vec<Value> = vec![Value::from(column_match), Value::from(opts.project.clone())];
    let (label_sql, label_params) = label_clause(&opts.labels, params.len() + 1);
    params.extend(label_params);
    let (filter_sql, filter_params) = compile_filter(opts.filter.as_ref(), params.len() + 1)?;
    params.extend(filter_params);
    let limit_idx = params.len() + 1;
    params.push(Value::from(i64::try_from(pool).unwrap_or(i64::MAX)));

    // Only the id is read: the row projection happens once, for the page, after the
    // fusion and the cut (`load_rows`). Fetching six string columns per candidate here
    // copied them for every row the cut discards (review #25's shape, in this query).
    let sql = format!(
        "SELECT n.id
         FROM symbols_fts f
         JOIN nodes n ON n.id = f.rowid
         WHERE f.symbols_fts MATCH ?1 AND n.project = ?2 {label_sql} {filter_sql}
         ORDER BY bm25(symbols_fts, 8.0, 4.0, 2.0, 1.0, 0.5) LIMIT ?{limit_idx}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(to_sql_params(&params)), |r| {
        r.get::<_, i64>(0)
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// In+out edge count for the joined node `n`.
///
/// `e.project = n.project` is load-bearing, not cosmetic: `idx_edges_target` is
/// `(project, target, edge_type)` and `idx_edges_source` is
/// `(project, source, edge_type)`. Without the leading column SQLite cannot seek
/// and falls back to a covering-index SCAN per candidate — measured 2,669 ms vs
/// 2.2 ms on a 36k-node store, for the same result set.
const DEGREE_EXPR: &str =
    "(SELECT count(*) FROM edges e WHERE e.project = n.project AND e.target = n.id) \
     + (SELECT count(*) FROM edges e WHERE e.project = n.project AND e.source = n.id)";

/// Graph-degree channel: the most-connected symbols matching `query` by name.
fn graph_channel(
    conn: &Connection,
    query: &str,
    opts: &SearchOptions,
    pool: usize,
) -> Result<Vec<i64>> {
    let Some(match_expr) = fts_match_expr(query) else {
        return Ok(Vec::new());
    };
    let column_match = format!("tokens : ({match_expr})");
    let mut params: Vec<Value> = vec![Value::from(column_match), Value::from(opts.project.clone())];
    let (label_sql, label_params) = label_clause(&opts.labels, params.len() + 1);
    params.extend(label_params);
    let (filter_sql, filter_params) = compile_filter(opts.filter.as_ref(), params.len() + 1)?;
    params.extend(filter_params);
    let limit_idx = params.len() + 1;
    params.push(Value::from(i64::try_from(pool).unwrap_or(i64::MAX)));

    let sql = format!(
        "SELECT n.id,
                {DEGREE_EXPR} AS deg
         FROM symbols_fts f
         JOIN nodes n ON n.id = f.rowid
         WHERE f.symbols_fts MATCH ?1 AND n.project = ?2 {label_sql} {filter_sql}
         ORDER BY deg DESC LIMIT ?{limit_idx}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(to_sql_params(&params)), |r| {
        r.get::<_, i64>(0)
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Build `AND n.label IN (...)` for a label set, starting at parameter `next`.
fn label_clause(labels: &[Label], next: usize) -> (String, Vec<Value>) {
    if labels.is_empty() {
        return (String::new(), Vec::new());
    }
    let holes: Vec<String> = (0..labels.len())
        .map(|i| format!("?{}", next + i))
        .collect();
    let params = labels.iter().map(|l| Value::from(l.as_str())).collect();
    (format!("AND n.label IN ({})", holes.join(", ")), params)
}

/// Compile an optional payload filter, renumbering its placeholders to follow
/// `next - 1` existing parameters.
fn compile_filter(filter: Option<&Filter>, next: usize) -> Result<(String, Vec<Value>)> {
    let Some(f) = filter else {
        return Ok((String::new(), Vec::new()));
    };
    let (sql, params) = f.compile_prefixed("n.")?;
    let offset = next - 1;
    Ok((
        format!("AND {}", renumber_placeholders(&sql, offset)),
        params,
    ))
}

/// Convert owned JSON values into rusqlite parameters.
fn to_sql_params(values: &[Value]) -> Vec<Box<dyn rusqlite::ToSql + '_>> {
    values
        .iter()
        .map(|v| -> Box<dyn rusqlite::ToSql + '_> {
            match v {
                Value::String(s) => Box::new(s.clone()),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Box::new(i)
                    } else {
                        Box::new(n.as_f64().unwrap_or(0.0))
                    }
                }
                Value::Bool(b) => Box::new(*b),
                Value::Null => Box::new(Option::<String>::None),
                other => Box::new(other.to_string()),
            }
        })
        .collect()
}

fn load_rows(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, Row>> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let mut unique: Vec<i64> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    let holes: Vec<String> = (1..=unique.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "SELECT {ROW_COLS} FROM nodes WHERE id IN ({})",
        holes.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(unique.iter()), row_of)?;
    for row in rows {
        let row = row?;
        out.insert(row.id, row);
    }
    Ok(out)
}

/// Degrees for many ids in TWO queries, not two per row (review #26/#59).
///
/// Measured on this repo's live graph (279 685 edges, 200 sampled nodes):
/// - correlated subselects per id: 261 ms extrapolated for 200 hits
/// - `LEFT JOIN (SELECT ... GROUP BY)` twice: 3 051 ms — worse, the GROUP BY scans the
///   whole table before the join filters it
/// - one indexed `GROUP BY` per direction with `IN (ids)`: **3.6 ms**
///
/// Only the last form uses `idx_edges_target` / `idx_edges_source`; the others do a
/// full pass per direction. Two queries, one per direction, keeps both indexes hot.
fn degrees_batch(
    conn: &Connection,
    project: &str,
    ids: &[i64],
) -> Result<HashMap<i64, (usize, usize)>> {
    let mut out: HashMap<i64, (usize, usize)> = ids.iter().map(|id| (*id, (0, 0))).collect();
    if ids.is_empty() {
        return Ok(out);
    }
    let holes: Vec<String> = (2..=ids.len() + 1).map(|i| format!("?{i}")).collect();
    for (position, column) in [(0usize, "target"), (1usize, "source")] {
        let sql = format!(
            "SELECT {column}, count(*) FROM edges WHERE project = ?1 AND {column} IN ({}) \
             GROUP BY {column}",
            holes.join(", ")
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(ids.len() + 1);
        bound.push(&project);
        for id in ids {
            bound.push(id);
        }
        let rows = stmt.query_map(bound.as_slice(), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (id, count) = row?;
            if let Some(slot) = out.get_mut(&id) {
                if position == 0 {
                    slot.0 = count as usize;
                } else {
                    slot.1 = count as usize;
                }
            }
        }
    }
    Ok(out)
}

/// Apply MMR diversity reranking, using split-name features as the similarity
/// basis (no embeddings needed for the MVP).
fn apply_mmr(hits: Vec<SearchHit>, limit: usize, lambda: f32) -> Vec<SearchHit> {
    if hits.is_empty() {
        return hits;
    }
    let scores: Vec<f32> = hits.iter().map(|h| h.score).collect();
    let normalized = min_max_normalize(&scores);
    let candidates: Vec<MmrCandidate> = hits
        .iter()
        .zip(normalized.iter())
        .map(|(h, &rel)| MmrCandidate {
            id: h.id,
            relevance: rel,
            features: feature_vector(&h.name, &h.qualified_name),
        })
        .collect();
    let by_id: HashMap<i64, SearchHit> = hits.into_iter().map(|h| (h.id, h)).collect();
    let mut selected_ids: HashSet<i64> = HashSet::new();
    let ordered = crate::hybrid::mmr_rerank(&candidates, limit, lambda);
    let mut out = Vec::with_capacity(ordered.len());
    for cand in ordered {
        if let Some(hit) = by_id.get(&cand.id) {
            selected_ids.insert(cand.id);
            out.push(hit.clone());
        }
    }
    out
}

/// Bag-of-split-tokens feature vector used by MMR.
fn feature_vector(name: &str, qn: &str) -> Vec<f32> {
    // Hash each token into a fixed-width vector so vectors are comparable.
    const DIM: usize = 64;
    let mut tokens: Vec<String> = split_for_fts(name)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    tokens.extend(split_for_fts(qn).split_whitespace().map(str::to_string));
    tokens.sort_unstable();
    tokens.dedup();
    let mut v = vec![0.0f32; DIM];
    for t in tokens {
        let bucket = fnv1a(t.as_bytes()) % DIM as u64;
        // `bucket < DIM` by construction, so the narrowing cast is exact.
        #[allow(clippy::cast_possible_truncation)]
        let idx = bucket as usize;
        v[idx] += 1.0;
    }
    v
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::GraphStore;
    use crate::types::{EdgeType, Node};
    use serde_json::json;

    fn seed(store: &GraphStore) {
        store.register_project("p", "/tmp").expect("project");
        let mut ids = Vec::new();
        for (name, qn, doc) in [
            ("parse_config", "p::parse_config", "Parse the config file"),
            ("load_config", "p::load_config", ""),
            ("render_widget", "p::render_widget", "Draw a widget"),
        ] {
            let node = Node {
                id: 0,
                project: "p".into(),
                label: Label::Function,
                name: name.into(),
                qualified_name: qn.into(),
                file_path: "src/lib.rs".into(),
                start_line: 1,
                end_line: 5,
                props: json!({ "doc": doc }),
            };
            let id = store.upsert_node(&node).expect("node");
            store
                .index_fts(id, name, qn, &split_for_fts(name), doc, "Function")
                .expect("fts");
            ids.push(id);
        }
        // parse_config -> load_config, so the latter gains in-degree.
        store
            .insert_edge(
                "p",
                ids[0],
                ids[1],
                EdgeType::Calls,
                &json!({"confidence":0.9}),
            )
            .expect("edge");
    }

    fn open() -> GraphStore {
        let store = GraphStore::open_in_memory().expect("store");
        seed(&store);
        store
    }

    /// The degree subqueries must be project-scoped so SQLite can seek
    /// `idx_edges_target(project, target, edge_type)` instead of scanning it.
    /// Unscoped, the graph channel took 2,669 ms on the live store where the
    /// scoped form takes 2.2 ms — a plan property, so it is asserted on the plan
    /// rather than on a clock.
    #[test]
    fn the_degree_subqueries_seek_the_project_scoped_indexes() {
        let store = open();
        search(store.connection(), "load_config", &SearchOptions::new("p")).expect("warm");
        let mut stmt = store
            .connection()
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT n.id, {DEGREE_EXPR} AS deg
                 FROM symbols_fts f JOIN nodes n ON n.id = f.rowid
                 WHERE f.symbols_fts MATCH 'load_config' AND n.project = 'p'"
            ))
            .expect("prepare");
        let plan: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(3))
            .expect("plan")
            .map(|r| r.expect("plan row"))
            .collect();
        let edges_steps: Vec<&String> = plan.iter().filter(|s| s.contains(" e ")).collect();
        assert!(
            !edges_steps.is_empty(),
            "expected the plan to mention the edges table: {plan:?}"
        );
        assert!(
            edges_steps
                .iter()
                .any(|s| s.starts_with("SEARCH") && s.contains("idx_edges_target")),
            "in-degree must seek idx_edges_target, plan was: {plan:?}"
        );
        assert!(
            edges_steps.iter().all(|s| !s.starts_with("SCAN")),
            "no edges access may be a full scan, plan was: {plan:?}"
        );
    }

    #[test]
    fn fts_search_finds_symbol_by_name() {
        let store = open();
        let hits =
            search(store.connection(), "parse_config", &SearchOptions::new("p")).expect("search");
        assert!(!hits.is_empty(), "expected a hit");
        assert_eq!(hits[0].name, "parse_config");
        assert!(hits[0].channels.name.is_some());
    }

    #[test]
    fn fts_search_matches_split_camel_tokens() {
        let store = open();
        // Query "widget" must reach `render_widget` via the split column.
        let hits = search(store.connection(), "widget", &SearchOptions::new("p")).expect("search");
        assert!(hits.iter().any(|h| h.name == "render_widget"), "{hits:?}");
    }

    #[test]
    fn body_channel_matches_docstring() {
        let store = open();
        let hits = search(store.connection(), "widget", &SearchOptions::new("p")).expect("search");
        let widget = hits
            .iter()
            .find(|h| h.name == "render_widget")
            .expect("widget");
        // "Draw a widget" is the body; the token channel also matches, so at
        // least one of the two must have ranked it.
        assert!(widget.channels.name.is_some() || widget.channels.body.is_some());
    }

    /// Seed a dedicated `big` project so a 20-hit page is reachable at all: the
    /// three-node fixture is too small to show page-scaling work (it made an earlier
    /// version of these tests pass on the broken code).
    fn seed_wide(store: &mut GraphStore, count: usize) {
        for i in 0..count {
            store
                .register_project("big", "/tmp")
                .or_else(|_| Ok::<(), crate::GraphError>(()))
                .ok();
            let name = format!("widget_{i}");
            let node = Node {
                id: 0,
                project: "big".into(),
                label: Label::Function,
                name: name.clone(),
                qualified_name: format!("big::{name}"),
                file_path: "src/big.rs".into(),
                start_line: 1,
                end_line: 2,
                props: json!({ "doc": "widget" }),
            };
            let id = store.upsert_node(&node).expect("node");
            store
                .index_fts(
                    id,
                    &name,
                    &format!("big::{name}"),
                    &format!("widget {i}"),
                    "widget",
                    "Function",
                )
                .expect("fts");
        }
    }

    /// Statements recorded by the statement hook, and the lock that serialises the
    /// tests sharing it (rusqlite's `trace` hook is a plain `fn` pointer, so it has no
    /// closure state to carry its own sink).
    static RECORDED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    static HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// What SQL one `search` call issued, as far as the two review findings care.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Tallies {
        /// Statements projecting rows out of `nodes`.
        node_reads: usize,
        /// Most ids any one of those bound.
        node_ids: usize,
        /// Statements counting edges for the page (`COUNT(*) FROM EDGES`).
        degree_reads: usize,
        /// Most ids any one of those bound.
        degree_ids: usize,
        /// FTS channel statements that also projected the row columns.
        fts_row_projections: usize,
    }

    /// Classify the statements the hook recorded.
    fn tallies_of(statements: &[String]) -> Tallies {
        let mut tallies = Tallies::default();
        for sql in statements {
            let upper = sql.to_ascii_uppercase();
            let ids = bound_ids(sql);
            if upper.contains("FROM NODES") && upper.contains("SELECT ID,") {
                tallies.node_reads += 1;
                tallies.node_ids = tallies.node_ids.max(ids);
            }
            if upper.contains("COUNT(*) FROM EDGES") {
                tallies.degree_reads += 1;
                tallies.degree_ids = tallies.degree_ids.max(ids);
            }
            if upper.contains("FROM SYMBOLS_FTS")
                && (upper.contains("N.NAME") || upper.contains("N.FILE_PATH"))
            {
                tallies.fts_row_projections += 1;
            }
        }
        tallies
    }

    /// How many ids a statement's `IN (...)` list binds.
    ///
    /// The legacy `trace` hook reports the statement with its parameters EXPANDED
    /// (`IN (12, 13, 14)`), so the ids are the list's items; a `?`-placeholder form is
    /// counted the same way, because a placeholder is one item too.
    fn bound_ids(sql: &str) -> usize {
        let Some((_, rest)) = sql.split_once(" IN (") else {
            return 0;
        };
        let Some((inner, _)) = rest.split_once(')') else {
            return 0;
        };
        inner
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .count()
    }

    /// Run `run` with a statement hook installed and classify what it issued.
    ///
    /// The hook is a plain `fn` pointer (rusqlite's `trace` takes no closure state),
    /// so its output goes through a static buffer and a lock serialises the tests
    /// that share it.
    fn count_statements(
        store: &mut GraphStore,
        run: impl FnOnce(&rusqlite::Connection),
    ) -> Tallies {
        let _serialised = HOOK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        RECORDED.lock().expect("recorded").clear();
        let conn = store.connection_mut();
        #[allow(deprecated)]
        conn.trace(Some(|sql: &str| {
            let flat: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            // FTS5's own index probes are the virtual table's business, not the query
            // builder's: `--`-prefixed statements are emitted by the extension.
            if !flat.starts_with("--") {
                RECORDED.lock().expect("recorded").push(flat);
            }
        }));
        run(conn);
        #[allow(deprecated)]
        conn.trace(None);
        let statements = RECORDED.lock().expect("recorded").clone();
        tallies_of(&statements)
    }

    /// Review #26: degree counts are a function of the PAGE, and only of the rows
    /// that survive the cut.
    ///
    /// Before the fix both counts were computed for every fused candidate — 152 ids
    /// for a 20-hit page on this repo's graph — and the surplus was thrown away by
    /// the truncate. The oracle reads the SQL rather than a clock, so the property is
    /// "no degree statement binds more ids than the page has rows", which pool-sized
    /// work cannot satisfy.
    #[test]
    fn degrees_are_counted_for_the_page_and_not_for_the_candidate_pool() {
        let mut store = open();
        seed_wide(&mut store, 30);
        let mut run = |limit: usize| {
            let mut hits = 0;
            let tallies = count_statements(&mut store, |conn| {
                hits = search(
                    conn,
                    "config OR widget OR render",
                    &SearchOptions::new("big").with_limit(limit),
                )
                .expect("search")
                .len();
            });
            (hits, tallies)
        };
        let (small_hits, small) = run(2);
        let (big_hits, big) = run(20);

        assert!(
            big_hits > small_hits,
            "the fixture must return more hits for the bigger limit, or the test proves \
             nothing: {small_hits} vs {big_hits}"
        );
        assert_eq!(
            big.degree_reads, small.degree_reads,
            "degree queries grew with the page: {small:?} vs {big:?}"
        );
        assert!(
            big.degree_ids <= big_hits,
            "degrees were counted for {} ids to return {big_hits} hits: {big:?}",
            big.degree_ids
        );
        assert!(
            small.degree_ids <= small_hits,
            "degrees were counted for {} ids to return {small_hits} hits: {small:?}",
            small.degree_ids
        );
        // The same cut governs the row read: nothing before it looks at a row, so the
        // page's rows are the only rows the query needs (review #25's shape).
        assert!(
            big.node_ids <= big_hits,
            "rows were loaded for {} ids to return {big_hits} hits: {big:?}",
            big.node_ids
        );
    }

    /// The row columns are read by `load_rows` and by nothing else.
    ///
    /// Both FTS channels rank and return ids; the six string columns they used to
    /// project were copied for every candidate and then read from `load_rows` anyway.
    /// Measured on this repo's graph (59 813 nodes): a 1 000-row pool went 6.8 ms ->
    /// 5.1 ms per channel once the projection was the id alone.
    #[test]
    fn the_channels_project_ids_and_not_rows() {
        let mut store = open();
        seed_wide(&mut store, 30);
        let tallies = count_statements(&mut store, |conn| {
            search(
                conn,
                "config OR widget OR render",
                &SearchOptions::new("big").with_limit(20),
            )
            .expect("search");
        });
        assert!(
            tallies.fts_row_projections == 0,
            "an FTS channel re-projected the row columns instead of the id: {tallies:?}"
        );
    }

    /// Review #59: a regex-filtered search materialises the candidate rows once.
    ///
    /// The regex needs `name`/`qualified_name`/`file_path` and the page needs the same
    /// columns, so the filter loaded them and then the page loaded them again. The
    /// oracle is the number of `nodes` reads: the filter's map must be the page's map.
    #[test]
    fn a_regex_search_reads_the_candidate_rows_once() {
        let mut store = open();
        seed_wide(&mut store, 30);
        let mut hits = 0;
        let tallies = count_statements(&mut store, |conn| {
            let opts = SearchOptions::new("big")
                .with_limit(20)
                .with_regex("^widget_1");
            hits = search(conn, "config OR widget OR render", &opts)
                .expect("search")
                .len();
        });
        assert!(hits > 1, "the regex must keep more than one hit: {hits}");
        assert_eq!(
            tallies.node_reads, 1,
            "the regex filter re-materialised the candidate rows: {tallies:?}"
        );
        // The pool is `limit * candidate_multiplier`; a second load would add the
        // surviving page's ids on top of the pool the filter already had.
        assert!(
            tallies.node_ids <= 20 * 4,
            "read {} candidate ids for a pool of 80: {tallies:?}",
            tallies.node_ids
        );
    }

    #[test]
    fn degrees_are_reported() {
        let store = open();
        let hits =
            search(store.connection(), "load_config", &SearchOptions::new("p")).expect("search");
        let load = hits.iter().find(|h| h.name == "load_config").expect("load");
        assert_eq!(load.in_degree, 1);
        assert_eq!(load.out_degree, 0);
    }

    #[test]
    fn regex_filters_results() {
        let store = open();
        let opts = SearchOptions::new("p").with_regex("^render_");
        let hits = search(store.connection(), "config OR widget", &opts).expect("search");
        assert!(
            hits.iter().all(|h| h.name.starts_with("render_")),
            "{hits:?}"
        );
    }

    #[test]
    fn empty_query_is_rejected() {
        let store = open();
        assert!(search(store.connection(), "   ", &SearchOptions::new("p")).is_err());
    }

    #[test]
    fn label_filter_restricts_results() {
        let store = open();
        let opts = SearchOptions::new("p").with_labels(vec![Label::Method]);
        let hits = search(store.connection(), "config", &opts).expect("search");
        assert!(hits.is_empty(), "no methods seeded: {hits:?}");
    }

    #[test]
    fn payload_filter_narrows_by_prop() {
        let store = open();
        let opts = SearchOptions::new("p").with_filter(Filter::contains("name", "parse"));
        let hits = search(store.connection(), "config", &opts).expect("search");
        assert!(hits.iter().all(|h| h.name.contains("parse")), "{hits:?}");
    }

    #[test]
    fn mmr_rerank_returns_bounded_results() {
        let store = open();
        let opts = SearchOptions::new("p").with_mmr(0.5).with_limit(2);
        let hits = search(store.connection(), "config", &opts).expect("search");
        assert!(hits.len() <= 2);
    }

    /// The cascade's first consumer: an identifier-shaped query stops at exact
    /// name lookup, so the docstring/comment pass never runs.
    #[test]
    fn an_identifier_query_does_not_run_the_body_channel() {
        let store = open();
        let (hits, report) =
            search_report(store.connection(), "parse_config", &SearchOptions::new("p"))
                .expect("search");
        assert_eq!(report.shape, QueryShape::Identifier);
        assert_eq!(report.running, vec![Tier::Name]);
        assert_eq!(report.skip_reason(Tier::Signals), Some(SkipReason::Shape));
        // The gate, observed where it is visible: no hit carries a body rank,
        // even though "Parse the config file" would match "config".
        assert!(
            hits.iter().all(|h| h.channels.body.is_none()),
            "identifier query ran the body channel: {hits:?}"
        );
    }

    /// The other half of the gate: a broad question still pays for both passes.
    #[test]
    fn a_broad_query_runs_the_body_channel() {
        let store = open();
        let (hits, report) = search_report(
            store.connection(),
            "config widget render draw",
            &SearchOptions::new("p"),
        )
        .expect("search");
        assert_eq!(report.shape, QueryShape::Broad);
        assert!(report.running.contains(&Tier::Signals), "{report:?}");
        let widget = hits
            .iter()
            .find(|h| h.name == "render_widget")
            .expect("widget");
        assert!(
            widget.channels.body.is_some(),
            "broad query skipped the body channel: {hits:?}"
        );
    }

    /// `details.cascade`: the decision is reported, not implied. The ignored
    /// tiers say why they were ignored, and `cost` is the running tiers' sum.
    #[test]
    fn the_report_names_the_skipped_tiers_and_their_reasons() {
        let store = open();
        let (_, report) =
            search_report(store.connection(), "parse_config", &SearchOptions::new("p"))
                .expect("search");
        assert_eq!(
            report.to_json(),
            json!({
                "shape": "identifier",
                "running": ["name"],
                "skipped": {"signals": "shape", "structure": "shape", "community": "shape"},
                "cost": 1,
            })
        );
    }

    /// The gate is a gate on work, not on ranking: for a phrase-shaped query,
    /// where both channels run, the plan adds no weight and no reordering, so
    /// the order is the one the three-channel fusion already produced.
    #[test]
    fn the_plan_does_not_change_ranking() {
        let store = open();
        let (hits, report) =
            search_report(store.connection(), "config file", &SearchOptions::new("p"))
                .expect("search");
        assert!(report.running.contains(&Tier::Signals), "{report:?}");
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["parse_config", "load_config"], "{hits:?}");
        assert_eq!(report.cost, 401, "name + signals: {report:?}");
    }

    #[test]
    fn limit_is_respected() {
        let store = open();
        let opts = SearchOptions::new("p").with_limit(1);
        let hits = search(store.connection(), "config", &opts).expect("search");
        assert_eq!(hits.len(), 1);
    }
}
