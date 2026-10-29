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
    /// How many definitions shared this callee's name when the edge was resolved.
    ///
    /// A value of `1` means the name was unique. A large number means the edge is a
    /// guess among that many same-named definitions: measured worst case on a real
    /// repository is `new` with 400 candidates and 3,466 edges bound to one of them.
    /// `GitNexus` records this shape as its `wrong edge, recorded at full confidence`
    /// bug (issue #3038); surfacing the count is what lets a reader discount it.
    pub candidates: u32,
    /// Risk labels attached to this step.
    pub risk_labels: Vec<String>,
    /// Argument expressions (data-flow), when the edge carried them.
    pub args: Vec<Value>,
    /// The WITNESS PATH: the qualified names from the origin down to this node.
    ///
    /// `via` names the last edge only, which says how this node was reached but not WHY.
    /// The review's §7 asks for the path to be preserved ("сохранять witness path"), and
    /// `impact` cannot answer "why is my module affected" without it: "A -> B -> C" is an
    /// explanation, "C, via CALLS" is not.
    ///
    /// Bounded by construction: the BFS stores one parent per visited node, so the path is
    /// at most `depth` entries, and `depth` is capped by the caller.
    pub witness: Vec<String>,
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
    /// Keep only steps at least this confident, or `None` to keep them all.
    ///
    /// A step's confidence is the product of the edge confidences along its path
    /// from the origin — the number `trace` prints and puts in each step — so it
    /// only falls with depth and one guessed edge discounts everything below it.
    ///
    /// `None` — the default — drops nothing: a step below
    /// [`DEFAULT_CONFIDENCE_FLOOR`] is labelled `low_confidence` instead, because
    /// a caller that asked for no floor must not silently lose steps it never
    /// asked to hide.
    ///
    /// `Some(floor)` drops every step below `floor` and counts them in
    /// [`TraceResult::filtered`]; the floor also becomes the labelling threshold,
    /// since a step the caller chose to keep is not "low" by the caller's own
    /// standard.
    ///
    /// A step whose edge recorded **no** confidence is not "below" anything: the
    /// indexer writes a confidence on every edge it resolves, so an absent value
    /// belongs to an edge that never claimed one — a hand-built seed, a `USAGE`
    /// edge, an `IMPORTS` edge. Such an edge is read as full confidence (1.0, in
    /// `read_call_props`), so no floor drops it.
    pub confidence_floor: Option<f32>,
    /// Maximum edges EXAMINED during the walk, or `None` for no ceiling.
    ///
    /// [`Self::limit`] caps the answer; this caps the WORK. They are different
    /// budgets, and a walk can spend the second without producing the first: a
    /// node below `confidence_floor` is counted in [`TraceResult::filtered`] and
    /// NOT in `steps`, yet its subtree is still expanded (see the loop's
    /// comment — pruning there would hide a step that passes the floor). A
    /// high-fanout hub therefore costs the full walk and returns almost nothing,
    /// which is exactly the case the graph review's §3.3 calls out: "Trace limit
    /// считает выданные steps… небольшой ответ может стоить большого обхода".
    ///
    /// A capped walk sets [`TraceResult::truncated`] and states the reason, so a
    /// short answer is never read as the complete reachable set.
    pub work_budget: Option<usize>,
    /// A cancellation token and a deadline, so a walk can be STOPPED.
    ///
    /// `work_budget` bounds a walk that runs to completion; this bounds one that should
    /// not finish at all — the caller lost interest (a client disconnected, a turn was
    /// aborted) or the answer is no longer worth its cost. Without it a `trace` on a
    /// pathological graph could hold a turn open for its whole budget with no way out.
    ///
    /// `Cancelled` is reported in [`TraceResult::stopped_by`] rather than as an error: a
    /// cancelled walk still has the steps it found, and throwing them away would make
    /// cancellation cost more than waiting.
    pub cancel: Option<crate::lease::CancelToken>,
    /// Wall-clock ceiling for the walk, checked between edges.
    ///
    /// A deadline is the cooperative form: the walk stops where it is and reports what it
    /// reached, so a caller gets a partial answer instead of a timeout error.
    pub deadline: Option<std::time::Instant>,
}

/// Why a [`trace`] walk stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truncation {
    /// [`TraceOptions::limit`] capped the returned steps.
    Limit,
    /// [`TraceOptions::work_budget`] capped the edges examined.
    WorkBudget,
    /// [`TraceOptions::cancel`] fired, or [`TraceOptions::deadline`] passed.
    Cancelled,
}

impl Truncation {
    /// The spelling the tool prints.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Limit => "step limit",
            Self::WorkBudget => "work budget",
            Self::Cancelled => "cancellation",
        }
    }
}

/// Confidence below which a step is labelled `low_confidence` when the caller
/// set no [`TraceOptions::confidence_floor`].
pub const DEFAULT_CONFIDENCE_FLOOR: f32 = 0.6;

/// Edges a default [`trace`] may examine.
///
/// Measured on this repo's own index (64 717 nodes / 261 988 edges), depth 4:
///
/// - a NORMAL symbol reaches the 200-step answer cap after 234–297 edges, so a
///   budget above that changes nothing it is supposed to change;
/// - a symbol whose edges fall below the floor is the case this exists for:
///   `create_session` returned **0 steps after examining 2 251 edges** (903
///   counted as filtered and still expanded), 226× the cost of its answer, in
///   140 ms instead of 27 ms.
///
/// 4 000 was the first value here and it **did not** stop that case — the review
/// lists 4–8k as an experiment start, and the experiment says the pathology fits
/// under it. 1 000 is ~4× the normal case's cost (so it never binds a healthy
/// walk) and half the measured pathology. A caller that wants the full walk
/// passes `work_budget: 0`.
pub const DEFAULT_WORK_BUDGET: usize = 1_000;

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
            confidence_floor: None,
            work_budget: Some(DEFAULT_WORK_BUDGET),
            cancel: None,
            deadline: None,
        }
    }

    /// Keep only steps whose path confidence is at least `floor` (`[0, 1]`).
    ///
    /// This is a filter over the confidence `trace` already computes, not a new
    /// signal: the value is the product of the edge confidences from the origin.
    /// [`None`] keeps every step and only labels the weak ones; see the field's
    /// documentation for what "no confidence recorded" means.
    #[must_use]
    pub fn with_confidence_floor(mut self, floor: Option<f32>) -> Self {
        self.confidence_floor = floor;
        self
    }

    /// The floor used for the `low_confidence` label: the caller's, or the
    /// default when the caller asked to filter nothing.
    #[must_use]
    pub fn label_floor(&self) -> f32 {
        self.confidence_floor.unwrap_or(DEFAULT_CONFIDENCE_FLOOR)
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

    /// Cap the number of returned steps.
    ///
    /// Reaching the cap sets [`TraceResult::truncated`], so a caller can tell a
    /// capped listing from the complete reachable set.
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Restrict traversal to specific edge types.
    #[must_use]
    pub fn with_edge_types(mut self, types: Vec<EdgeType>) -> Self {
        self.edge_types = types;
        self
    }

    /// Cap the edges examined; `None` lifts the ceiling.
    #[must_use]
    pub fn with_work_budget(mut self, budget: Option<usize>) -> Self {
        self.work_budget = budget;
        self
    }

    /// Stop the walk when `cancel` fires.
    #[must_use]
    pub fn with_cancel(mut self, cancel: crate::lease::CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Stop the walk at `deadline`.
    #[must_use]
    pub fn with_deadline(mut self, deadline: std::time::Instant) -> Self {
        self.deadline = Some(deadline);
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
    /// True when the walk stopped at `limit` while reachable nodes remained.
    ///
    /// A caller that reads only `steps` would otherwise treat a capped listing
    /// as the complete reachable set. gitnexus learned this the hard way: its
    /// `trace` reports `truncated: true` and the furthest reachable node, so a
    /// capped walk is never mistaken for an exhaustive one.
    pub truncated: bool,
    /// Steps whose confidence was below [`TraceOptions::confidence_floor`].
    ///
    /// Non-zero only when the caller set a floor. Reported so a short listing is
    /// attributable to the floor rather than reading as a symbol that calls
    /// almost nothing — the same reason [`TraceResult::truncated`] exists. A
    /// filtered step is still walked through, so its callees are judged on their
    /// own confidence and counted in `steps` if they pass.
    pub filtered: usize,
    /// Why the walk stopped early, or `None` when it reached its depth.
    ///
    /// [`Self::truncated`] says *that* the listing is a prefix; this says *which
    /// budget* made it one. A caller that only sees `truncated` cannot tell "the
    /// graph has more" from "I spent the walk on a hub", and the two call for
    /// different next moves — a lower depth versus a narrower query.
    pub stopped_by: Option<Truncation>,
    /// Edges examined, whatever their outcome.
    ///
    /// The cost of the answer, printed beside it: a 20-step listing that examined
    /// 4 000 edges is a different result from one that examined 30, and only this
    /// number tells them apart.
    pub edges_examined: usize,
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
#[allow(clippy::too_many_lines)] // one BFS with three stop conditions; splitting hides their order
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
    let mut filtered = 0usize;
    // The queue carries each node's WITNESS PATH alongside its depth and confidence: the
    // path is what turns "reached" into "why reached".
    let origin_name = store
        .node_by_id(origin)?
        .map_or_else(|| opts.symbol.clone(), |node| node.qualified_name);
    let mut queue: VecDeque<(i64, usize, f32, Vec<String>)> = VecDeque::new();
    queue.push_back((origin, 0, 1.0, vec![origin_name]));

    let mut stopped_by: Option<Truncation> = None;
    let mut edges_examined = 0usize;
    while let Some((node_id, depth, path_conf, witness)) = queue.pop_front() {
        if depth >= opts.depth {
            continue;
        }
        // Reaching the cap with nodes still queued means the listing is a
        // prefix, not the whole reachable set.
        if steps.len() >= opts.limit {
            stopped_by = Some(Truncation::Limit);
            break;
        }
        // The work budget is checked before expanding, and again per edge: one
        // node with hundreds of outgoing edges must not overshoot it the way it
        // used to overshoot `limit`.
        if opts
            .work_budget
            .is_some_and(|budget| edges_examined >= budget)
        {
            stopped_by = Some(Truncation::WorkBudget);
            break;
        }
        for edge in outgoing_edges(conn, node_id, &edge_types, opts.direction)? {
            // Check the cap per neighbour, not per dequeued node: one node can
            // have hundreds of outgoing edges, and testing only at the top of
            // the loop let a single fan-out overshoot the limit.
            if steps.len() >= opts.limit {
                stopped_by = Some(Truncation::Limit);
                break;
            }
            edges_examined += 1;
            if opts
                .work_budget
                .is_some_and(|budget| edges_examined >= budget)
            {
                stopped_by = Some(Truncation::WorkBudget);
                break;
            }
            // Cancellation is checked per EDGE, not per node: a node with hundreds of
            // outgoing edges must not delay a stop by a whole fan-out.
            // Cancellation is checked per EDGE, not per node: a node with hundreds of
            // outgoing edges must not delay a stop by a whole fan-out.
            if opts
                .cancel
                .as_ref()
                .is_some_and(crate::lease::CancelToken::is_cancelled)
                || opts
                    .deadline
                    .is_some_and(|at| std::time::Instant::now() >= at)
            {
                stopped_by = Some(Truncation::Cancelled);
                break;
            }
            if !visited.insert(edge.target) {
                continue;
            }
            let (confidence, strategy, candidates, args) = read_call_props(&edge.props);
            let path_conf = path_conf * confidence;
            // The floor filters the listing, not the walk: the node is still
            // queued, so its own callees are reached and judged on their own
            // confidence. Pruning the subtree would be wrong here, because
            // nothing guarantees an edge's recorded confidence is at most 1 —
            // the indexer's ladder is, a hand-built store's need not be — and a
            // pruned subtree could then hide a step that does pass the floor.
            if opts.confidence_floor.is_some_and(|floor| path_conf < floor) {
                filtered += 1;
                let mut next_witness = witness.clone();
                if let Some(node) = store.node_by_id(edge.target)? {
                    next_witness.push(node.qualified_name);
                }
                queue.push_back((edge.target, depth + 1, path_conf, next_witness));
                continue;
            }
            if let Some(node) = store.node_by_id(edge.target)? {
                let risk = risk_labels(confidence, &strategy, opts.label_floor());
                let mut next_witness = witness.clone();
                next_witness.push(node.qualified_name.clone());
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
                    candidates,
                    risk_labels: risk,
                    args,
                    witness: next_witness.clone(),
                });
                queue.push_back((edge.target, depth + 1, path_conf, next_witness));
            }
        }
        if stopped_by.is_some() {
            break;
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
        // `truncated` keeps its meaning: the listing is a prefix. The budget
        // that made it one travels separately, because the two call for
        // different next moves.
        truncated: stopped_by.is_some(),
        filtered,
        stopped_by,
        edges_examined,
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

fn read_call_props(props: &Value) -> (f32, String, u32, Vec<Value>) {
    let confidence = props
        .get("confidence")
        .and_then(Value::as_f64)
        .map_or(1.0, |c| c as f32);
    let strategy = props
        .get("strategy")
        .and_then(Value::as_str)
        .unwrap_or("explicit")
        .to_string();
    let candidates = props
        .get("candidates")
        .and_then(Value::as_u64)
        .map_or(1, |n| u32::try_from(n).unwrap_or(u32::MAX));
    let args = props
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (confidence, strategy, candidates, args)
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

    /// The graph review §3.3: "Trace limit считает выданные steps… небольшой
    /// ответ может стоить большого обхода". A hub whose edges all fall below the
    /// floor is the exact shape: every neighbour is counted as filtered and its
    /// subtree still expanded, so the answer stays tiny while the walk is huge.
    #[test]
    fn a_hub_below_the_floor_costs_the_budget_not_the_answer() {
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
        // 40 low-confidence callees, each with its own low-confidence callee:
        // 40 + 40 edges, none of which pass a 0.9 floor.
        let floor: f32 = 0.9;
        let mut hubs = Vec::new();
        for i in 0..40 {
            let hub = mk(&format!("hub{i}"));
            store
                .insert_edge(
                    "p",
                    main,
                    hub,
                    EdgeType::Calls,
                    &json!({"confidence":0.1,"strategy":"suffix"}),
                )
                .expect("edge");
            let leaf = mk(&format!("leaf{i}"));
            store
                .insert_edge(
                    "p",
                    hub,
                    leaf,
                    EdgeType::Calls,
                    &json!({"confidence":0.1,"strategy":"suffix"}),
                )
                .expect("edge");
            hubs.push(hub);
        }

        let unbounded = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_confidence_floor(Some(floor))
                .with_work_budget(None),
        )
        .expect("trace");
        assert_eq!(unbounded.steps.len(), 0, "every step is below the floor");
        assert_eq!(
            unbounded.edges_examined, 80,
            "but the walk still spent 80 edges"
        );
        assert_eq!(unbounded.stopped_by, None, "no ceiling was set");

        let bounded = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_confidence_floor(Some(floor))
                .with_work_budget(Some(10)),
        )
        .expect("trace");
        assert_eq!(bounded.steps.len(), 0);
        assert_eq!(
            bounded.stopped_by,
            Some(Truncation::WorkBudget),
            "the header must say the walk stopped on COST"
        );
        assert!(bounded.truncated, "a prefix is still a prefix");
        assert!(
            bounded.edges_examined <= 10,
            "the budget is a ceiling: examined {}",
            bounded.edges_examined
        );
        assert!(
            bounded.filtered < unbounded.filtered,
            "the capped walk saw fewer neighbours: {} vs {}",
            bounded.filtered,
            unbounded.filtered
        );
    }

    /// `limit` and `work_budget` are different budgets and must be reported
    /// separately: a caller that reads only "truncated" cannot tell "the graph
    /// has more" from "I spent the walk on a hub".
    #[test]
    fn the_stop_reason_distinguishes_the_step_cap_from_the_work_cap() {
        let store = seed();
        let by_limit = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_limit(1)
                .with_work_budget(None),
        )
        .expect("trace");
        assert_eq!(by_limit.stopped_by, Some(Truncation::Limit));
        assert_eq!(by_limit.steps.len(), 1);

        let by_work = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_limit(200)
                .with_work_budget(Some(1)),
        )
        .expect("trace");
        assert_eq!(by_work.stopped_by, Some(Truncation::WorkBudget));
        assert_eq!(by_work.edges_examined, 1);
    }

    /// A walk that reaches its depth is not truncated, and says so.
    #[test]
    fn a_complete_walk_reports_no_stop_reason() {
        let store = seed();
        let r = trace(&store, &TraceOptions::new("p", "p::main").with_depth(2)).expect("trace");
        assert_eq!(r.stopped_by, None);
        assert!(!r.truncated);
        assert_eq!(r.steps.len(), 2);
        assert!(r.edges_examined >= 2, "examined {}", r.edges_examined);
    }

    /// A walk must be STOPPABLE. `work_budget` bounds one that runs to completion; this
    /// bounds one that should not finish at all — the caller lost interest, or the answer
    /// is no longer worth its cost. Without it a `trace` on a pathological graph holds a
    /// turn open for its whole budget with no way out.
    #[test]
    fn a_cancelled_walk_stops_and_keeps_what_it_found() {
        let store = seed();
        let cancel = crate::lease::CancelToken::new();
        cancel.cancel();
        let r = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_cancel(cancel),
        )
        .expect("trace");
        assert_eq!(
            r.stopped_by,
            Some(Truncation::Cancelled),
            "an already-cancelled token stops the walk at the first edge"
        );
        assert!(r.truncated, "a prefix is still a prefix");
        // Cancellation is not an error: whatever was reached before the stop survives,
        // because throwing it away would make cancelling cost more than waiting.
        assert!(r.steps.len() <= 2, "the walk stopped early: {:?}", r.steps);
    }

    /// A deadline is the cooperative form: stop where you are and report what you
    /// reached, instead of failing the call.
    #[test]
    fn a_passed_deadline_stops_the_walk_without_an_error() {
        let store = seed();
        // `checked_sub` because a process that started moments ago may not have a
        // millisecond to subtract; `unwrap_or(now)` still lands in the past.
        let now = std::time::Instant::now();
        let past = now
            .checked_sub(std::time::Duration::from_millis(1))
            .unwrap_or(now);
        let r = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_deadline(past),
        )
        .expect("a deadline is not an error");
        assert_eq!(r.stopped_by, Some(Truncation::Cancelled));
        assert!(r.edges_examined <= 1, "it stopped at the first edge");
    }

    /// A live token and a future deadline change nothing: the guards must not fire on
    /// their own.
    #[test]
    fn a_live_token_and_a_future_deadline_do_not_stop_a_walk() {
        let store = seed();
        let r = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_cancel(crate::lease::CancelToken::new())
                .with_deadline(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        )
        .expect("trace");
        assert_eq!(r.stopped_by, None, "nothing stopped it");
        assert_eq!(r.steps.len(), 2);
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

    /// A floor filters the listing and counts what it dropped.
    ///
    /// Red-first: with the `confidence_floor` comparison removed from `trace`,
    /// both steps come back and `filtered` stays 0.
    #[test]
    fn the_confidence_floor_filters_steps_and_counts_them() {
        let store = seed();
        // main -> parse is 0.9, parse -> load is 0.9 * 0.5 = 0.45.
        let r = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_confidence_floor(Some(0.5)),
        )
        .expect("trace");
        let names: Vec<&str> = r.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["parse"], "load is below the floor: {names:?}");
        assert_eq!(r.filtered, 1, "the dropped step is counted");

        // A floor under every step keeps the whole walk and counts nothing.
        let all = trace(
            &store,
            &TraceOptions::new("p", "p::main")
                .with_depth(2)
                .with_confidence_floor(Some(0.0)),
        )
        .expect("trace");
        assert_eq!(all.steps.len(), 2);
        assert_eq!(all.filtered, 0);
    }

    /// An edge that recorded no confidence is not "below" anything.
    ///
    /// The indexer writes a confidence on every edge it resolves, so an absent
    /// one belongs to an edge that never claimed it (a hand-built seed, a
    /// `USAGE`/`IMPORTS` edge). It is read as full confidence, so even a floor of
    /// 1.0 keeps it — this is the decision the parameter's documentation states.
    #[test]
    fn a_step_without_a_recorded_confidence_is_never_below_the_floor() {
        let store = GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        let mk = |name: &str| {
            store
                .upsert_node(&Node {
                    id: 0,
                    project: "p".into(),
                    label: Label::Function,
                    name: name.into(),
                    qualified_name: format!("p::{name}"),
                    file_path: "src/lib.rs".into(),
                    start_line: 1,
                    end_line: 3,
                    props: json!({}),
                })
                .expect("node")
        };
        let root = mk("root");
        let silent = mk("silent");
        // No `confidence` key at all: the edge never claimed one.
        store
            .insert_edge("p", root, silent, EdgeType::Calls, &json!({}))
            .expect("edge");
        let r = trace(
            &store,
            &TraceOptions::new("p", "p::root")
                .with_depth(1)
                .with_confidence_floor(Some(1.0)),
        )
        .expect("trace");
        let names: Vec<&str> = r.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["silent"],
            "an unclaimed confidence is full confidence, not zero"
        );
        assert_eq!(r.filtered, 0);
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

    /// Hitting the node cap must be reported, not silently truncate.
    ///
    /// A caller that reads only `steps` cannot tell a capped listing from the
    /// complete reachable set, so a truncated walk used to look exhaustive.
    /// gitnexus treats this as a first-class contract (`truncated`,
    /// `enumeration`); we now match it.
    #[test]
    fn a_capped_walk_is_flagged_truncated() {
        let store = GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        let mk = |name: &str| {
            store
                .upsert_node(&Node {
                    id: 0,
                    project: "p".into(),
                    label: Label::Function,
                    name: name.into(),
                    qualified_name: format!("p::{name}"),
                    file_path: "src/lib.rs".into(),
                    start_line: 1,
                    end_line: 3,
                    props: json!({}),
                })
                .expect("node")
        };
        let root = mk("root");
        // A wide fan-out: 20 direct callees, walked with a limit of 5.
        for i in 0..20 {
            let callee = mk(&format!("callee{i}"));
            store
                .insert_edge(
                    "p",
                    root,
                    callee,
                    EdgeType::Calls,
                    &json!({"confidence": 0.9, "strategy": "same_module"}),
                )
                .expect("edge");
        }

        let capped = trace(
            &store,
            &TraceOptions::new("p", "root").with_depth(2).with_limit(5),
        )
        .expect("trace");
        assert_eq!(capped.steps.len(), 5, "the limit is honoured");
        assert!(
            capped.truncated,
            "a walk that stopped at the cap must say so"
        );

        // With room for every callee the flag stays false.
        let full = trace(
            &store,
            &TraceOptions::new("p", "root").with_depth(2).with_limit(100),
        )
        .expect("trace");
        assert_eq!(full.steps.len(), 20);
        assert!(!full.truncated, "a complete walk must not claim truncation");
    }
}
