
//! G13 — tri-modal hybrid retrieval: ANN vector + BM25 + graph traversal,
//! fused with reciprocal rank fusion (RRF).
//!
//! `pa-graph`'s default [`crate::search`] fuses BM25 (name + body) with a
//! graph-*degree* channel. Tri-modal retrieval is different in two ways:
//!
//! 1. the third channel is a real **graph traversal** seeded from the fused
//!    top-k (callers/callees of the best lexical/vector hits), not a static
//!    degree count — so a query that lands on one function can surface the
//!    helpers it calls;
//! 2. the dense channel is an explicit [`Embedder`] seam. The crate ships
//!    [`NoopEmbedder`], so the module is fully functional (two real channels +
//!    traversal) without a model, and gains a dense channel the moment one is
//!    plugged in.
//!
//! Fusion is RRF (rank-based), which is robust to the wildly different score
//! scales of BM25 and cosine (see [`crate::hybrid`]).

use std::collections::HashSet;

use rusqlite::Connection;

use crate::error::Result;
use crate::hybrid::{rrf_fuse_weighted, RankedList};
use crate::search::Embedder;
use crate::store::GraphStore;
use crate::tokenize::fts_match_expr;
use crate::types::{EdgeType, Label};

/// Options for a tri-modal search.
pub struct TriModalOptions {
    /// Project to search.
    pub project: String,
    /// Number of final results.
    pub limit: usize,
    /// Dense-vector channel (optional; [`NoopEmbedder`] disables it).
    pub embedder: Option<std::sync::Arc<dyn Embedder>>,
    /// How far to expand the graph-traversal channel (hops).
    pub traversal_depth: usize,
    /// Over-fetch factor for the traversal seed (`k * this`).
    pub over_fetch: usize,
    /// Per-channel RRF weights `[bm25, dense, traversal]`.
    pub weights: [f32; 3],
}

impl std::fmt::Debug for TriModalOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TriModalOptions")
            .field("project", &self.project)
            .field("limit", &self.limit)
            .field("embedder", &self.embedder.as_ref().map(|_| "<embedder>"))
            .field("traversal_depth", &self.traversal_depth)
            .field("over_fetch", &self.over_fetch)
            .field("weights", &self.weights)
            .finish()
    }
}

impl TriModalOptions {
    /// Defaults for `project`: `limit = 20`, 1-hop traversal, `4×` over-fetch.
    #[must_use]
    pub fn new(project: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            limit: 20,
            embedder: None,
            traversal_depth: 1,
            over_fetch: 4,
            weights: [1.0, 1.0, 0.5],
        }
    }

    /// Attach a dense embedder.
    #[must_use]
    pub fn with_embedder(mut self, embedder: std::sync::Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Set the result limit.
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// One tri-modal result.
#[derive(Debug, Clone, PartialEq)]
pub struct TriModalHit {
    /// Node id.
    pub id: i64,
    /// Node label.
    pub label: Label,
    /// Short name.
    pub name: String,
    /// Qualified name.
    pub qualified_name: String,
    /// Defining file.
    pub file_path: String,
    /// Fused RRF score.
    pub score: f32,
    /// Whether the node came from the graph-traversal channel.
    pub via_traversal: bool,
}

/// Run a tri-modal search against `store`.
///
/// # Errors
/// Propagates SQLite errors and [`crate::GraphError::InvalidQuery`] for an
/// empty query.
pub fn tri_modal_search(
    store: &GraphStore,
    query: &str,
    opts: &TriModalOptions,
) -> Result<Vec<TriModalHit>> {
    tri_modal_search_conn(store.connection(), query, opts)
}

/// Run a tri-modal search against a raw connection.
///
/// # Errors
/// Propagates SQLite errors and [`crate::GraphError::InvalidQuery`].
pub fn tri_modal_search_conn(
    conn: &Connection,
    query: &str,
    opts: &TriModalOptions,
) -> Result<Vec<TriModalHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(crate::GraphError::InvalidQuery("empty query".into()));
    }
    let pool = opts.limit.max(1) * opts.over_fetch.max(1);

    // Channel 1 — BM25 over the split-token column.
    let bm25 = bm25_channel(conn, &opts.project, query, pool)?;

    // Channel 2 — dense ANN (no-op when no embedder is configured).
    let dense: RankedList = match &opts.embedder {
        Some(embedder) => embedder.rank(query, &bm25),
        None => Vec::new(),
    };

    // Seed the traversal from the fusion of the two *direct* channels.
    let seed_channels: Vec<(RankedList, f32)> = vec![
        (bm25.clone(), opts.weights[0]),
        (dense.clone(), opts.weights[1]),
    ];
    let seeds: Vec<i64> = rrf_fuse_weighted(&seed_channels)
        .into_iter()
        .take(opts.limit.max(1))
        .map(|(id, _)| id)
        .collect();

    // Channel 3 — graph traversal from the seeds.
    let traversal = traversal_channel(conn, &opts.project, &seeds, opts.traversal_depth, pool)?;
    let traversal_set: HashSet<i64> = traversal.iter().copied().collect();

    // Fuse all three.
    let fused = rrf_fuse_weighted(&[
        (bm25, opts.weights[0]),
        (dense, opts.weights[1]),
        (traversal, opts.weights[2]),
    ]);

    let ids: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
    let rows = load_hits(conn, &ids)?;
    let mut out = Vec::with_capacity(fused.len());
    for (id, score) in fused {
        let Some(hit) = rows.get(&id) else { continue };
        out.push(TriModalHit {
            id,
            label: hit.0,
            name: hit.1.clone(),
            qualified_name: hit.2.clone(),
            file_path: hit.3.clone(),
            score,
            via_traversal: traversal_set.contains(&id),
        });
        if out.len() >= opts.limit {
            break;
        }
    }
    Ok(out)
}

/// BM25 channel: FTS5 over the `tokens` column.
fn bm25_channel(
    conn: &Connection,
    project: &str,
    query: &str,
    pool: usize,
) -> Result<RankedList> {
    let Some(match_expr) = fts_match_expr(query) else {
        return Ok(Vec::new());
    };
    let column_match = format!("tokens : ({match_expr})");
    let sql = "SELECT n.id FROM symbols_fts f JOIN nodes n ON n.id = f.rowid
               WHERE f.symbols_fts MATCH ?1 AND n.project = ?2
               ORDER BY bm25(symbols_fts, 8.0, 4.0, 2.0, 1.0, 0.5) LIMIT ?3";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        rusqlite::params![column_match, project, pool as i64],
        |r| r.get::<_, i64>(0),
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Graph-traversal channel: BFS out from the seed nodes over CALLS/IMPORTS,
/// up to `depth` hops, excluding the seeds themselves.
fn traversal_channel(
    conn: &Connection,
    project: &str,
    seeds: &[i64],
    depth: usize,
    pool: usize,
) -> Result<RankedList> {
    if seeds.is_empty() || depth == 0 {
        return Ok(Vec::new());
    }
    let mut visited: HashSet<i64> = seeds.iter().copied().collect();
    let mut frontier: Vec<i64> = seeds.to_vec();
    let mut out: Vec<i64> = Vec::new();

    for _ in 0..depth {
        let mut next: Vec<i64> = Vec::new();
        for &node in &frontier {
            for target in out_neighbors(conn, project, node)? {
                if visited.insert(target) {
                    next.push(target);
                    out.push(target);
                    if out.len() >= pool {
                        return Ok(out);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(out)
}

/// Out-neighbours of `node` via CALLS or IMPORTS edges.
fn out_neighbors(conn: &Connection, project: &str, node: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT target FROM edges
         WHERE project = ?1 AND source = ?2 AND edge_type IN (?3, ?4)",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            project,
            node,
            EdgeType::Calls.as_str(),
            EdgeType::Imports.as_str()
        ],
        |r| r.get::<_, i64>(0),
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Project node rows into `(label, name, qn, file_path)`.
#[allow(clippy::type_complexity)]
fn load_hits(
    conn: &Connection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, (Label, String, String, String)>> {
    let mut out = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let mut unique: Vec<i64> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    let holes: Vec<String> = (1..=unique.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "SELECT id,label,name,qualified_name,file_path FROM nodes WHERE id IN ({})",
        holes.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(unique.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        let (id, label, name, qn, file) = row?;
        out.insert(
            id,
            (Label::from_str_or_default(&label), name, qn, file),
        );
    }
    Ok(out)
}

/// Convenience: tri-modal search with default options (no embedder).
///
/// # Errors
/// Propagates SQLite errors.
pub fn tri_modal(store: &GraphStore, project: &str, query: &str) -> Result<Vec<TriModalHit>> {
    tri_modal_search(store, query, &TriModalOptions::new(project))
}

/// Re-export so callers can pass an embedder without importing
/// [`crate::search`] directly.
pub use crate::search::NoopEmbedder;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenize::split_for_fts;
    use crate::types::Node;
    use serde_json::json;

    fn seed(store: &GraphStore) -> Vec<i64> {
        store.register_project("p", "/tmp").expect("project");
        let mut ids = Vec::new();
        for (name, qn) in [
            ("parse_config", "p::parse_config"),
            ("load_file", "p::load_file"),
            ("helper", "p::helper"),
            ("render_widget", "p::render_widget"),
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
                props: json!({}),
            };
            let id = store.upsert_node(&node).expect("node");
            store
                .index_fts(id, name, qn, &split_for_fts(name), "", "Function")
                .expect("fts");
            ids.push(id);
        }
        // parse_config -> load_file -> helper
        store
            .insert_edge("p", ids[0], ids[1], EdgeType::Calls, &json!({}))
            .expect("edge");
        store
            .insert_edge("p", ids[1], ids[2], EdgeType::Calls, &json!({}))
            .expect("edge");
        ids
    }

    #[test]
    fn finds_a_lexical_match() {
        let store = GraphStore::open_in_memory().expect("store");
        seed(&store);
        let hits = tri_modal(&store, "p", "parse_config").expect("search");
        assert!(hits.iter().any(|h| h.name == "parse_config"), "{hits:?}");
    }

    #[test]
    fn traversal_surfaces_callees_of_a_hit() {
        let store = GraphStore::open_in_memory().expect("store");
        seed(&store);
        // A query that lands on parse_config; traversal should pull load_file.
        let opts = TriModalOptions::new("p").with_limit(3);
        let hits = tri_modal_search(&store, "parse_config", &opts).expect("search");
        assert!(
            hits.iter().any(|h| h.via_traversal),
            "expected a traversal-sourced hit: {hits:?}"
        );
    }

    #[test]
    fn limit_is_respected() {
        let store = GraphStore::open_in_memory().expect("store");
        seed(&store);
        let opts = TriModalOptions::new("p").with_limit(1);
        let hits = tri_modal_search(&store, "helper OR parse_config", &opts).expect("search");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn empty_query_is_rejected() {
        let store = GraphStore::open_in_memory().expect("store");
        seed(&store);
        assert!(tri_modal(&store, "p", "   ").is_err());
    }

    #[test]
    fn noop_embedder_still_returns_results() {
        let store = GraphStore::open_in_memory().expect("store");
        seed(&store);
        let opts = TriModalOptions::new("p").with_embedder(std::sync::Arc::new(NoopEmbedder));
        let hits = tri_modal_search(&store, "parse_config", &opts).expect("search");
        assert_ne!(hits.len(), 0, "noop embedder must not suppress results");
    }

    #[test]
    fn traversal_channel_respects_depth() {
        let store = GraphStore::open_in_memory().expect("store");
        let ids = seed(&store);
        // 1 hop from parse_config reaches load_file only.
        let one = traversal_channel(store.connection(), "p", &[ids[0]], 1, 10).expect("chan");
        assert_eq!(one, vec![ids[1]]);
        // 2 hops also reaches helper.
        let two = traversal_channel(store.connection(), "p", &[ids[0]], 2, 10).expect("chan");
        assert!(two.contains(&ids[2]), "{two:?}");
    }
}
