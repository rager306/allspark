//! R9 — delta graph: node/edge deltas and delta-aware traversal.
//!
//! Where [`crate::delta`] is a generic keyed event log, this module models the
//! graph change directly: [`NodeDelta`] (a node appeared/vanished/changed),
//! [`EdgeDelta`] (an edge appeared/vanished), and [`GraphDelta`] (a bag of
//! both). The headline operation is [`affected_nodes`]: given a delta, which
//! nodes must be re-examined?
//!
//! [`DeltaAwareTraversal`] runs BFS/DFS/Dijkstra over an *adjacency snapshot*,
//! and can seed from a delta's touched nodes so a repair walks only the
//! neighbourhood a change actually disturbed. Dijkstra uses non-negative
//! integer edge weights (edge properties may carry a `weight`).

use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::types::EdgeType;

/// What happened to a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeOp {
    /// The node was created.
    Add,
    /// The node's properties/position changed.
    Update,
    /// The node was removed.
    Remove,
}

impl NodeOp {
    /// Stable wire name.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Update => "update",
            Self::Remove => "remove",
        }
    }
}

/// A node-level change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDelta {
    /// Node row id.
    pub id: i64,
    /// What happened.
    pub op: NodeOp,
    /// Qualified name (stable across id churn).
    pub qualified_name: String,
}

impl NodeDelta {
    /// A node add.
    #[must_use]
    pub fn add(id: i64, qn: impl Into<String>) -> Self {
        Self {
            id,
            op: NodeOp::Add,
            qualified_name: qn.into(),
        }
    }

    /// A node update.
    #[must_use]
    pub fn update(id: i64, qn: impl Into<String>) -> Self {
        Self {
            id,
            op: NodeOp::Update,
            qualified_name: qn.into(),
        }
    }

    /// A node removal.
    #[must_use]
    pub fn remove(id: i64, qn: impl Into<String>) -> Self {
        Self {
            id,
            op: NodeOp::Remove,
            qualified_name: qn.into(),
        }
    }
}

/// What happened to an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeOp {
    /// The edge was created.
    Add,
    /// The edge was removed.
    Remove,
}

/// An edge-level change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDelta {
    /// Source node id.
    pub source: i64,
    /// Target node id.
    pub target: i64,
    /// Edge kind.
    pub edge_type: EdgeType,
    /// What happened.
    pub op: EdgeOp,
}

impl EdgeDelta {
    /// An edge add.
    #[must_use]
    pub fn add(source: i64, target: i64, edge_type: EdgeType) -> Self {
        Self {
            source,
            target,
            edge_type,
            op: EdgeOp::Add,
        }
    }

    /// An edge removal.
    #[must_use]
    pub fn remove(source: i64, target: i64, edge_type: EdgeType) -> Self {
        Self {
            source,
            target,
            edge_type,
            op: EdgeOp::Remove,
        }
    }
}

/// A batch of node and edge changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphDelta {
    /// Node changes.
    pub nodes: Vec<NodeDelta>,
    /// Edge changes.
    pub edges: Vec<EdgeDelta>,
}

impl GraphDelta {
    /// An empty delta.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the delta contains no changes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// Total number of changes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len() + self.edges.len()
    }

    /// Add a node change.
    pub fn push_node(&mut self, delta: NodeDelta) {
        self.nodes.push(delta);
    }

    /// Add an edge change.
    pub fn push_edge(&mut self, delta: EdgeDelta) {
        self.edges.push(delta);
    }

    /// Merge `other` into `self`.
    pub fn merge(&mut self, other: GraphDelta) {
        self.nodes.extend(other.nodes);
        self.edges.extend(other.edges);
    }
}

/// The set of nodes a delta touches (directly or, optionally, via one hop).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AffectedNodes {
    /// Nodes named directly by the delta.
    pub direct: Vec<i64>,
    /// Nodes adjacent to a directly-affected node (one hop out).
    pub adjacent: Vec<i64>,
}

impl AffectedNodes {
    /// Every affected node id (direct ∪ adjacent), sorted and deduped.
    #[must_use]
    pub fn all(&self) -> Vec<i64> {
        let mut out: Vec<i64> = self
            .direct
            .iter()
            .chain(self.adjacent.iter())
            .copied()
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Total affected node count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.all().len()
    }

    /// Whether nothing is affected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.direct.is_empty() && self.adjacent.is_empty()
    }
}

/// Which nodes a delta affects, using `adj` for one-hop expansion.
///
/// Directly affected: every node named by a node delta, plus both endpoints of
/// every edge delta. Adjacent: the one-hop neighbours (in either direction) of
/// the directly-affected set, excluding the set itself.
#[must_use]
pub fn affected_nodes(delta: &GraphDelta, adj: &Adjacency) -> AffectedNodes {
    let mut direct: Vec<i64> = Vec::new();
    for node in &delta.nodes {
        direct.push(node.id);
    }
    for edge in &delta.edges {
        direct.push(edge.source);
        direct.push(edge.target);
    }
    direct.sort_unstable();
    direct.dedup();

    let direct_set: HashSet<i64> = direct.iter().copied().collect();
    let mut adjacent: Vec<i64> = Vec::new();
    for id in &direct {
        for nbr in adj.neighbors(*id) {
            if !direct_set.contains(&nbr) {
                adjacent.push(nbr);
            }
        }
    }
    adjacent.sort_unstable();
    adjacent.dedup();
    AffectedNodes { direct, adjacent }
}

/// A weighted adjacency snapshot the traversal runs on.
#[derive(Debug, Clone, Default)]
pub struct Adjacency {
    out: HashMap<i64, Vec<(i64, u32)>>,
    inc: HashMap<i64, Vec<(i64, u32)>>,
}

impl Adjacency {
    /// An empty adjacency.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a directed edge with weight `w`.
    pub fn add_edge(&mut self, from: i64, to: i64, w: u32) {
        self.out.entry(from).or_default().push((to, w));
        self.inc.entry(to).or_default().push((from, w));
    }

    /// Remove a directed edge (first match).
    pub fn remove_edge(&mut self, from: i64, to: i64) {
        if let Some(v) = self.out.get_mut(&from) {
            v.retain(|(t, _)| *t != to);
        }
        if let Some(v) = self.inc.get_mut(&to) {
            v.retain(|(s, _)| *s != from);
        }
    }

    /// Out-neighbours of `id` (with weights).
    #[must_use]
    pub fn out_neighbors(&self, id: i64) -> &[(i64, u32)] {
        self.out.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Undirected neighbours of `id` (both directions).
    #[must_use]
    pub fn neighbors(&self, id: i64) -> Vec<i64> {
        let mut out: Vec<i64> = Vec::new();
        if let Some(v) = self.out.get(&id) {
            out.extend(v.iter().map(|(t, _)| *t));
        }
        if let Some(v) = self.inc.get(&id) {
            out.extend(v.iter().map(|(s, _)| *s));
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Number of nodes with at least one edge.
    #[must_use]
    pub fn node_count(&self) -> usize {
        let mut set: HashSet<i64> = HashSet::new();
        set.extend(self.out.keys().copied());
        set.extend(self.inc.keys().copied());
        set.len()
    }
}

/// The traversal strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalOrder {
    /// Breadth-first (queue).
    Bfs,
    /// Depth-first (stack).
    Dfs,
    /// Dijkstra over non-negative weights.
    Dijkstra,
}

/// The result of a traversal: nodes in visit order and their distances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraversalResult {
    /// Nodes in visit order.
    pub order: Vec<i64>,
    /// Hop count (BFS/DFS) or shortest distance (Dijkstra) per node.
    pub distance: HashMap<i64, u32>,
}

impl TraversalResult {
    /// Distance to `id`, if reached.
    #[must_use]
    pub fn distance(&self, id: i64) -> Option<u32> {
        self.distance.get(&id).copied()
    }

    /// Whether `id` was reached.
    #[must_use]
    pub fn contains(&self, id: i64) -> bool {
        self.distance.contains_key(&id)
    }
}

/// A traversal that can be seeded from a [`GraphDelta`] so only the disturbed
/// neighbourhood is walked.
#[derive(Debug, Clone, Copy)]
pub struct DeltaAwareTraversal<'a> {
    adj: &'a Adjacency,
}

impl<'a> DeltaAwareTraversal<'a> {
    /// Wrap an adjacency snapshot.
    #[must_use]
    pub fn new(adj: &'a Adjacency) -> Self {
        Self { adj }
    }

    /// Traverse from `start` in `order`, bounded by `max_depth` hops
    /// (Dijkstra instead bounds by `max_depth` as a weight budget).
    #[must_use]
    pub fn traverse(&self, start: i64, order: TraversalOrder, max_depth: u32) -> TraversalResult {
        match order {
            TraversalOrder::Bfs => self.bfs(start, max_depth),
            TraversalOrder::Dfs => self.dfs(start, max_depth),
            TraversalOrder::Dijkstra => self.dijkstra(start, max_depth),
        }
    }

    /// Traverse starting from every node a delta affects (direct + adjacent).
    #[must_use]
    pub fn traverse_delta(
        &self,
        delta: &GraphDelta,
        order: TraversalOrder,
        max_depth: u32,
    ) -> TraversalResult {
        let seeds = affected_nodes(delta, self.adj).all();
        let mut order_out: Vec<i64> = Vec::new();
        let mut distance: HashMap<i64, u32> = HashMap::new();
        for seed in seeds {
            let result = self.traverse(seed, order, max_depth);
            for id in result.order {
                if distance
                    .insert(id, *result.distance.get(&id).unwrap_or(&0))
                    .is_none()
                {
                    order_out.push(id);
                }
            }
        }
        TraversalResult {
            order: order_out,
            distance,
        }
    }

    fn bfs(self, start: i64, max_depth: u32) -> TraversalResult {
        let mut order = Vec::new();
        let mut distance = HashMap::new();
        let mut queue = VecDeque::new();
        queue.push_back((start, 0u32));
        distance.insert(start, 0);
        while let Some((node, depth)) = queue.pop_front() {
            order.push(node);
            if depth >= max_depth {
                continue;
            }
            for (nbr, _) in self.adj.out_neighbors(node) {
                if !distance.contains_key(nbr) {
                    distance.insert(*nbr, depth + 1);
                    queue.push_back((*nbr, depth + 1));
                }
            }
        }
        TraversalResult { order, distance }
    }

    fn dfs(self, start: i64, max_depth: u32) -> TraversalResult {
        let mut order = Vec::new();
        let mut distance = HashMap::new();
        let mut stack = vec![(start, 0u32)];
        while let Some((node, depth)) = stack.pop() {
            if distance.contains_key(&node) {
                continue;
            }
            distance.insert(node, depth);
            order.push(node);
            if depth >= max_depth {
                continue;
            }
            // Push in reverse so the smallest neighbour is visited first.
            let mut nbrs: Vec<i64> = self
                .adj
                .out_neighbors(node)
                .iter()
                .map(|(t, _)| *t)
                .collect();
            nbrs.sort_unstable_by(|a, b| b.cmp(a));
            for nbr in nbrs {
                if !distance.contains_key(&nbr) {
                    stack.push((nbr, depth + 1));
                }
            }
        }
        TraversalResult { order, distance }
    }

    fn dijkstra(self, start: i64, budget: u32) -> TraversalResult {
        let mut order = Vec::new();
        let mut distance: HashMap<i64, u32> = HashMap::new();
        let mut heap: BinaryHeap<std::cmp::Reverse<(u32, i64)>> = BinaryHeap::new();
        heap.push(std::cmp::Reverse((0, start)));
        while let Some(std::cmp::Reverse((dist, node))) = heap.pop() {
            // Skip a stale heap entry whose node already has a shorter path.
            if distance.get(&node).is_some_and(|&best| best < dist) {
                continue;
            }
            if distance.insert(node, dist).is_none() {
                order.push(node);
            }
            for (nbr, w) in self.adj.out_neighbors(node) {
                let next = dist.saturating_add(*w);
                if next > budget {
                    continue;
                }
                let better = distance.get(nbr).is_none_or(|&d| next < d);
                if better {
                    heap.push(std::cmp::Reverse((next, *nbr)));
                }
            }
        }
        TraversalResult { order, distance }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a → b → c, a → d.
    fn sample_adj() -> Adjacency {
        let mut adj = Adjacency::new();
        adj.add_edge(1, 2, 1);
        adj.add_edge(2, 3, 1);
        adj.add_edge(1, 4, 5);
        adj
    }

    #[test]
    fn bfs_visits_in_breadth_order() {
        let adj = sample_adj();
        let t = DeltaAwareTraversal::new(&adj);
        let r = t.traverse(1, TraversalOrder::Bfs, 10);
        assert_eq!(r.order, vec![1, 2, 4, 3]);
        assert_eq!(r.distance(3), Some(2));
        assert_eq!(r.distance(4), Some(1));
    }

    #[test]
    fn dfs_visits_depth_first() {
        let adj = sample_adj();
        let t = DeltaAwareTraversal::new(&adj);
        let r = t.traverse(1, TraversalOrder::Dfs, 10);
        assert_eq!(r.order[0], 1);
        assert!(r.contains(3) && r.contains(4));
    }

    #[test]
    fn max_depth_bounds_the_walk() {
        let adj = sample_adj();
        let t = DeltaAwareTraversal::new(&adj);
        let r = t.traverse(1, TraversalOrder::Bfs, 1);
        assert_eq!(r.order, vec![1, 2, 4]);
        assert!(!r.contains(3), "c is two hops away");
    }

    #[test]
    fn dijkstra_prefers_the_cheaper_path() {
        let mut adj = Adjacency::new();
        adj.add_edge(1, 2, 10);
        adj.add_edge(1, 3, 1);
        adj.add_edge(3, 2, 1);
        let t = DeltaAwareTraversal::new(&adj);
        let r = t.traverse(1, TraversalOrder::Dijkstra, 100);
        assert_eq!(r.distance(2), Some(2), "1→3→2 costs 2 < 10");
    }

    #[test]
    fn affected_nodes_includes_both_edge_endpoints() {
        let adj = sample_adj();
        let mut delta = GraphDelta::new();
        delta.push_edge(EdgeDelta::remove(1, 2, EdgeType::Calls));
        let affected = affected_nodes(&delta, &adj);
        assert!(affected.direct.contains(&1));
        assert!(affected.direct.contains(&2));
        // 3 is adjacent to 2; 4 is adjacent to 1.
        assert!(affected.adjacent.contains(&3), "{affected:?}");
        assert!(affected.adjacent.contains(&4), "{affected:?}");
    }

    #[test]
    fn affected_nodes_expands_one_hop() {
        let adj = sample_adj();
        let mut delta = GraphDelta::new();
        delta.push_node(NodeDelta::update(2, "m::b"));
        let affected = affected_nodes(&delta, &adj);
        assert_eq!(affected.direct, vec![2]);
        // 2's neighbours are 1 and 3.
        assert_eq!(affected.adjacent, vec![1, 3]);
        assert_eq!(affected.all(), vec![1, 2, 3]);
    }

    #[test]
    fn delta_aware_traversal_seeds_from_the_delta() {
        let adj = sample_adj();
        let t = DeltaAwareTraversal::new(&adj);
        let mut delta = GraphDelta::new();
        delta.push_node(NodeDelta::add(2, "m::new"));
        let r = t.traverse_delta(&delta, TraversalOrder::Bfs, 2);
        // Seeded at 1, 2, 3 (affected); union covers the whole chain.
        assert!(r.contains(1) && r.contains(2) && r.contains(3) && r.contains(4));
    }

    #[test]
    fn graph_delta_merge_and_counts() {
        let mut a = GraphDelta::new();
        a.push_node(NodeDelta::add(1, "x"));
        let mut b = GraphDelta::new();
        b.push_edge(EdgeDelta::add(1, 2, EdgeType::Calls));
        a.merge(b);
        assert_eq!(a.len(), 2);
        assert!(!a.is_empty());
    }

    #[test]
    fn adjacency_remove_edge_updates_both_directions() {
        let mut adj = sample_adj();
        adj.remove_edge(1, 2);
        assert!(!adj.neighbors(1).contains(&2));
        assert!(!adj.neighbors(2).contains(&1));
        // 1→4 keeps node 1 alive; 2→3 keeps 2 and 3.
        assert_eq!(adj.node_count(), 4);
    }
}
