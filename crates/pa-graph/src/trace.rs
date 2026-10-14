//! Graph tracing: callers, callees, data-flow, and cross-service edges.
//!
//! [`trace`] resolves a symbol, walks the call graph breadth-first up to
//! `depth` hops in the requested [`Direction`], and reports every node reached
//! together with the path length, accumulated confidence, and a risk label.
//! Confidence is the product of edge confidences along the path (the indexer
//! stores one on every `CALLS` edge); risk labels flag edges whose confidence
//! is low or whose strategy is heuristic.

use std::collections::{HashMap, HashSet, VecDeque};

use rusqlite::Connection;
use serde_json::Value;

use crate::error::{GraphError, Result};
use crate::store::GraphStore;
use crate::types::EdgeType;

/// Which way to walk the call graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Follow incoming edges (who calls this symbol).
    In,
    /// Follow outgoing edges (what this symbol calls).
    Out,
    /// Follow both directions.
    Both,
}

/// A single hop in a trace result.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceStep {
    /// Node id of the reached symbol.
    pub id: i64,
    /// Short name.
    pub name: String,
    /// Qualified name.
    pub qualified_name: String,
    /// File path.
    pub file_path: String,
    /// 1-based start line.
    pub start_line: u32,
    /// BFS distance from the origin (1 = direct neighbour).
    pub depth: usize,
    /// Edge type traversed to reach this node.
    pub via: EdgeType,
    /// Product of edge confidences along the path.
    pub confidence: f32,
    /// Resolution strategy of the traversed edge.
    pub strategy: String,
    /// Risk labels attached to this step.
    pub risk_labels: Vec<String>,
    /// Argument expressions (data-flow), when the edge carried them.
    pub args: Vec<Value>,
}

/// Options for a [`trace`] call.
#[derive(Debug, Clone)]
pub struct TraceOptions {
    /// Project to search.
    pub project: String,
    /// Symbol to start from (qualified name, or a unique short name).
    pub symbol: String,
    /// Walk direction.
    pub direction: Direction,
    /// Maximum BFS depth (hops).
    pub depth: usize,
    /// Edge types to traverse (empty = CALLS only).
    pub edge_types: Vec<EdgeType>,
    /// Maximum nodes to return.
    pub limit: usize,
    /// Confidence below which a step is labelled `low_confidence`.
    pub confidence_floor: f32,
}

impl TraceOptions {
    /// Default options (outgoing CALLS, depth 2).
    #[must_use]
    pub fn new(project: impl Into<String>, symbol: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            symbol: symbol.into(),
            direction: Direction::Out,
            depth: 2,
            edge_types: Vec::new(),
            limit: 200,
            confidence_floor: 0.6,
        }
    }

    /// Set the walk direction.
    #[must_use]
    pub fn with_direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Set the BFS depth.
    #[must_use]
    pub fn with_depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    /// Restrict traversal to specific edge types.
    #[must_use]
    pub fn with_edge_types(mut self, types: Vec<EdgeType>) -> Self {
        self.edge_types = types;
        self
    }
}

/// The full trace result.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceResult {
    /// Origin node id.
    pub origin_id: i64,
    /// Origin qualified name.
    pub origin: String,
    /// Every reached step, ordered by depth then confidence (descending).
    pub steps: Vec<TraceStep>,
}

impl TraceResult {
    /// Steps at exactly one hop.
    pub fn direct(&self) -> impl Iterator<Item = &TraceStep> {
        self.steps.iter().filter(|s| s.depth == 1)
    }
}

/// Trace the call graph from a symbol.
///
/// # Errors
/// Returns [`GraphError::SymbolNotFound`] when `symbol` matches nothing, and
/// propagates SQLite errors.
pub fn trace(store: &GraphStore, opts: &TraceOptions) -> Result<TraceResult> {
    let origin = resolve_symbol(store, &opts.project, &opts.symbol)?;
    let conn = store.connection();
    let edge_types = if opts.edge_types.is_empty() {
        vec![EdgeType::Calls]
    } else {
        opts.edge_types.clone()
    };

    let mut visited: HashSet<i64> = HashSet::new();
    visited.insert(origin);
    let mut steps = Vec::new();
    let mut queue: VecDeque<(i64, usize, f32)> = VecDeque::new();
    queue.push_back((origin, 0, 1.0));

    while let Some((node_id, depth, path_conf)) = queue.pop_front() {
        if depth >= opts.depth || steps.len() >= opts.limit {
            continue;
        }
        for edge in outgoing_edges(conn, node_id, &edge_types, opts.direction)? {
            if !visited.insert(edge.target) {
                continue;
            }
            let (confidence, strategy, args) = read_call_props(&edge.props);
            let path_conf = path_conf * confidence;
            if let Some(node) = store.node_by_id(edge.target)? {
                let risk = risk_labels(confidence, &strategy, opts.confidence_floor);
                steps.push(TraceStep {
                    id: node.id,
                    name: node.name.clone(),
                    qualified_name: node.qualified_name.clone(),
                    file_path: node.file_path.clone(),
                    start_line: node.start_line,
                    depth: depth + 1,
                    via: edge.edge_type,
                    confidence: path_conf,
                    strategy,
                    risk_labels: risk,
                    args,
                });
                queue.push_back((edge.target, depth + 1, path_conf));
            }
        }
    }

    steps.sort_by(|a, b| {
        a.depth.cmp(&b.depth).then_with(|| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    Ok(TraceResult {
        origin_id: origin,
        origin: opts.symbol.clone(),
        steps,
    })
}

/// A cross-service HTTP call edge, if the project has any.
#[derive(Debug, Clone, PartialEq)]
pub struct CrossServiceEdge {
    /// Caller qualified name.
    pub source: String,
    /// Callee/service qualified name.
    pub target: String,
    /// HTTP path, when recorded.
    pub url_path: Option<String>,
}

/// Collect every edge whose properties carry a `url_path` (`HTTP_CALLS` shape).
///
/// # Errors
/// Propagates SQLite errors.
pub fn cross_service(store: &GraphStore, project: &str) -> Result<Vec<CrossServiceEdge>> {
    let conn = store.connection();
    let mut stmt = conn.prepare(
        "SELECT sn.qualified_name, tn.qualified_name, e.props_json
         FROM edges e
         JOIN nodes sn ON sn.id = e.source
         JOIN nodes tn ON tn.id = e.target
         WHERE e.project = ?1
           AND json_extract(e.props_json, '$.url_path') IS NOT NULL",
    )?;
    let rows = stmt.query_map([project], |r| {
        let props: String = r.get(2)?;
        let url_path = serde_json::from_str::<Value>(&props).ok().and_then(|v| {
            v.get("url_path")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        Ok(CrossServiceEdge {
            source: r.get(0)?,
            target: r.get(1)?,
            url_path,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn resolve_symbol(store: &GraphStore, project: &str, symbol: &str) -> Result<i64> {
    if let Some(node) = store.node_by_qn(project, symbol)? {
        return Ok(node.id);
    }
    let matches = store.nodes_by_name(project, symbol)?;
    match matches.len() {
        0 => Err(GraphError::SymbolNotFound(symbol.to_string())),
        1 => Ok(matches[0].id),
        _ => {
            // Prefer a File/Module-free definition; otherwise take the first
            // deterministically by qualified name.
            let mut sorted = matches;
            sorted.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
            Ok(sorted[0].id)
        }
    }
}

/// Fetch edges to walk, honouring direction.
fn outgoing_edges(
    conn: &Connection,
    node_id: i64,
    edge_types: &[EdgeType],
    direction: Direction,
) -> Result<Vec<EdgeRow>> {
    let types: Vec<&str> = edge_types.iter().map(|t| t.as_str()).collect();
    let type_holes: Vec<String> = (2..=types.len() + 1).map(|i| format!("?{i}")).collect();
    let type_clause = format!("AND edge_type IN ({})", type_holes.join(", "));

    let mut out = Vec::new();
    if matches!(direction, Direction::Out | Direction::Both) {
        let sql = format!(
            "SELECT target, edge_type, props_json FROM edges WHERE source = ?1 {type_clause}"
        );
        out.extend(query_edges(conn, &sql, node_id, &types, true)?);
    }
    if matches!(direction, Direction::In | Direction::Both) {
        let sql = format!(
            "SELECT source, edge_type, props_json FROM edges WHERE target = ?1 {type_clause}"
        );
        out.extend(query_edges(conn, &sql, node_id, &types, false)?);
    }
    Ok(out)
}

struct EdgeRow {
    target: i64,
    edge_type: EdgeType,
    props: Value,
}

fn query_edges(
    conn: &Connection,
    sql: &str,
    node_id: i64,
    types: &[&str],
    _outgoing: bool,
) -> Result<Vec<EdgeRow>> {
    let mut stmt = conn.prepare(sql)?;
    let mut params: Vec<Box<dyn rusqlite::ToSql + '_>> = vec![Box::new(node_id)];
    for t in types {
        params.push(Box::new(t.to_string()));
    }
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        let kind: String = r.get(1)?;
        let props: String = r.get(2)?;
        Ok(EdgeRow {
            target: r.get(0)?,
            edge_type: EdgeType::from_str_or_default(&kind),
            props: serde_json::from_str(&props).unwrap_or(Value::Null),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn read_call_props(props: &Value) -> (f32, String, Vec<Value>) {
    let confidence = props
        .get("confidence")
        .and_then(Value::as_f64)
        .map_or(1.0, |c| c as f32);
    let strategy = props
        .get("strategy")
        .and_then(Value::as_str)
        .unwrap_or("explicit")
        .to_string();
    let args = props
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (confidence, strategy, args)
}

fn risk_labels(confidence: f32, strategy: &str, floor: f32) -> Vec<String> {
    let mut labels = Vec::new();
    if confidence < floor {
        labels.push("low_confidence".to_string());
    }
    match strategy {
        "suffix" => labels.push("heuristic_resolution".to_string()),
        "unique" => labels.push("unique_name".to_string()),
        _ => {}
    }
    labels
}

/// Data-flow view: the argument expressions of every call *into* `symbol`.
///
/// # Errors
/// Propagates SQLite errors and [`GraphError::SymbolNotFound`].
pub fn data_flow(
    store: &GraphStore,
    project: &str,
    symbol: &str,
) -> Result<Vec<(String, Vec<Value>)>> {
    let origin = resolve_symbol(store, project, symbol)?;
    let conn = store.connection();
    let mut stmt = conn.prepare(
        "SELECT sn.qualified_name, e.props_json
         FROM edges e JOIN nodes sn ON sn.id = e.source
         WHERE e.target = ?1 AND e.edge_type = 'CALLS'",
    )?;
    let rows = stmt.query_map([origin], |r| {
        let props: String = r.get(1)?;
        let caller: String = r.get(0)?;
        let args = serde_json::from_str::<Value>(&props)
            .ok()
            .and_then(|v| v.get("args").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        Ok((caller, args))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Degree summary used by the `graph` channel ranking and diagnostics.
#[must_use]
pub fn degree_map(store: &GraphStore, project: &str) -> HashMap<i64, (usize, usize)> {
    let mut out = HashMap::new();
    let Ok(conn) = Ok::<_, GraphError>(store.connection()) else {
        return out;
    };
    let Ok(mut stmt) = conn.prepare("SELECT id FROM nodes WHERE project = ?1") else {
        return out;
    };
    let Ok(ids) = stmt.query_map([project], |r| r.get::<_, i64>(0)) else {
        return out;
    };
    for id in ids.flatten() {
        if let Ok((i, o)) = store.degree(id) {
            out.insert(id, (i, o));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::GraphStore;
    use crate::types::{Label, Node};
    use serde_json::json;

    fn seed() -> GraphStore {
        let store = GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        let mk = |name: &str| {
            let node = Node {
                id: 0,
                project: "p".into(),
                label: Label::Function,
                name: name.into(),
                qualified_name: format!("p::{name}"),
                file_path: "src/lib.rs".into(),
                start_line: 1,
                end_line: 3,
                props: json!({}),
            };
            store.upsert_node(&node).expect("node")
        };
        let main = mk("main");
        let parse = mk("parse");
        let load = mk("load");
        // main -> parse (0.9), parse -> load (0.5, suffix)
        store
            .insert_edge(
                "p",
                main,
                parse,
                EdgeType::Calls,
                &json!({"confidence":0.9,"strategy":"same_module","args":[{"i":0,"e":"path"}]}),
            )
            .expect("e1");
        store
            .insert_edge(
                "p",
                parse,
                load,
                EdgeType::Calls,
                &json!({"confidence":0.5,"strategy":"suffix"}),
            )
            .expect("e2");
        store
    }

    #[test]
    fn trace_out_follows_callees() {
        let store = seed();
        let r = trace(&store, &TraceOptions::new("p", "p::main").with_depth(2)).expect("trace");
        let names: Vec<&str> = r.steps.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"parse"), "{names:?}");
        assert!(names.contains(&"load"), "{names:?}");
        let parse = r.steps.iter().find(|s| s.name == "parse").expect("parse");
        assert_eq!(parse.depth, 1);
        assert_eq!(parse.args.len(), 1);
        assert_eq!(parse.strategy, "same_module");
    }

    #[test]
    fn trace_in_follows_callers() {
        let store = seed();
        let r = trace(
            &store,
            &TraceOptions::new("p", "p::load")
                .with_direction(Direction::In)
                .with_depth(1),
        )
        .expect("trace");
        assert_eq!(r.steps.len(), 1);
        assert_eq!(r.steps[0].name, "parse");
    }

    #[test]
    fn trace_accumulates_confidence_and_risk() {
        let store = seed();
        let r = trace(&store, &TraceOptions::new("p", "p::main").with_depth(2)).expect("trace");
        let load = r.steps.iter().find(|s| s.name == "load").expect("load");
        // 0.9 * 0.5 = 0.45, below the 0.6 floor → low_confidence.
        assert!(
            (load.confidence - 0.45).abs() < 1e-5,
            "conf {}",
            load.confidence
        );
        assert!(load.risk_labels.contains(&"low_confidence".to_string()));
        assert!(load
            .risk_labels
            .contains(&"heuristic_resolution".to_string()));
    }

    #[test]
    fn trace_resolves_short_name() {
        let store = seed();
        let r = trace(&store, &TraceOptions::new("p", "main")).expect("trace");
        assert_eq!(
            r.origin_id,
            store
                .node_by_qn("p", "p::main")
                .expect("qn")
                .expect("some")
                .id
        );
    }

    #[test]
    fn trace_unknown_symbol_errors() {
        let store = seed();
        assert!(trace(&store, &TraceOptions::new("p", "nope")).is_err());
    }

    #[test]
    fn depth_zero_returns_nothing() {
        let store = seed();
        let r = trace(&store, &TraceOptions::new("p", "p::main").with_depth(0)).expect("trace");
        assert_eq!(r.steps.len(), 0);
    }

    #[test]
    fn data_flow_reports_caller_args() {
        let store = seed();
        let flow = data_flow(&store, "p", "p::parse").expect("flow");
        assert_eq!(flow.len(), 1);
        assert_eq!(flow[0].0, "p::main");
        assert_eq!(flow[0].1.len(), 1);
    }

    #[test]
    fn cross_service_reads_url_path() {
        let store = seed();
        let a = store.node_by_qn("p", "p::main").expect("qn").expect("a");
        let b = store.node_by_qn("p", "p::load").expect("qn").expect("b");
        store
            .insert_edge(
                "p",
                a.id,
                b.id,
                EdgeType::Uses,
                &json!({"url_path":"/api/load"}),
            )
            .expect("http");
        let edges = cross_service(&store, "p").expect("cross");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].url_path.as_deref(), Some("/api/load"));
    }
}
