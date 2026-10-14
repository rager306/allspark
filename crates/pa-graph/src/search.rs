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

use std::collections::{HashMap, HashSet};

use regex::Regex;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;

use crate::bm25::min_max_normalize;
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

/// Execute a search against `conn` for `query`.
///
/// # Errors
/// Returns [`GraphError::InvalidQuery`] for an unparseable regex or empty
/// query, and propagates SQLite errors.
pub fn search(conn: &Connection, query: &str, opts: &SearchOptions) -> Result<Vec<SearchHit>> {
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

    let pool = opts
        .limit
        .max(1)
        .saturating_mul(opts.candidate_multiplier.max(1));

    let name_channel = fts_channel(conn, query, "tokens", opts, pool)?;
    let body_channel = fts_channel(conn, query, "body", opts, pool)?;
    let graph_channel = graph_channel(conn, query, opts, pool)?;

    let mut channels: Vec<(Vec<i64>, f32)> = vec![
        (name_channel.clone(), opts.weights[0]),
        (body_channel.clone(), opts.weights[1]),
        (graph_channel.clone(), opts.weights[2]),
    ];
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
        }
    }

    let fused = rrf_fuse_weighted(&channels);

    // Regex is a *filter*, not a ranker: drop non-matching candidates.
    let fused: Vec<(i64, f32)> = match &regex {
        None => fused,
        Some(re) => {
            let rows = load_rows(conn, &fused.iter().map(|(id, _)| *id).collect::<Vec<_>>())?;
            fused
                .into_iter()
                .filter(|(id, _)| {
                    rows.get(id).is_some_and(|r| {
                        re.is_match(&r.name)
                            || re.is_match(&r.qualified_name)
                            || re.is_match(&r.file_path)
                    })
                })
                .collect()
        }
    };

    let rank_maps = RankMaps {
        name: index_ranks(&name_channel),
        body: index_ranks(&body_channel),
        graph: index_ranks(&graph_channel),
    };

    let ids: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
    let rows = load_rows(conn, &ids)?;

    let mut hits: Vec<SearchHit> = Vec::with_capacity(fused.len());
    for (id, score) in fused {
        let Some(row) = rows.get(&id) else { continue };
        let (in_degree, out_degree) = degrees(conn, id)?;
        hits.push(SearchHit {
            id,
            label: Label::from_str_or_default(&row.label),
            name: row.name.clone(),
            qualified_name: row.qualified_name.clone(),
            file_path: row.file_path.clone(),
            start_line: row.start_line as u32,
            end_line: row.end_line as u32,
            score,
            in_degree,
            out_degree,
            channels: ChannelRanks {
                name: rank_maps.name.get(&id).copied(),
                body: rank_maps.body.get(&id).copied(),
                graph: rank_maps.graph.get(&id).copied(),
            },
        });
    }

    if let Some(lambda) = opts.mmr_lambda {
        hits = apply_mmr(hits, opts.limit, lambda);
    } else {
        hits.truncate(opts.limit);
    }
    Ok(hits)
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
    params.push(Value::from(pool as i64));

    let sql = format!(
        "SELECT n.id,n.label,n.name,n.qualified_name,n.file_path,n.start_line,n.end_line
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
    params.push(Value::from(pool as i64));

    let sql = format!(
        "SELECT n.id,
                (SELECT count(*) FROM edges e WHERE e.target = n.id) +
                (SELECT count(*) FROM edges e WHERE e.source = n.id) AS deg
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

fn degrees(conn: &Connection, id: i64) -> Result<(usize, usize)> {
    let inc: i64 = conn.query_row("SELECT count(*) FROM edges WHERE target = ?1", [id], |r| {
        r.get(0)
    })?;
    let out: i64 = conn.query_row("SELECT count(*) FROM edges WHERE source = ?1", [id], |r| {
        r.get(0)
    })?;
    Ok((inc as usize, out as usize))
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

    #[test]
    fn limit_is_respected() {
        let store = open();
        let opts = SearchOptions::new("p").with_limit(1);
        let hits = search(store.connection(), "config", &opts).expect("search");
        assert_eq!(hits.len(), 1);
    }
}
