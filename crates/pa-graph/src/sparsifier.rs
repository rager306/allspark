//! R13 — graph sparsification: top-k edges plus an MST backbone.
//!
//! A dense graph is expensive to traverse and to store. [`sparsify`] keeps the
//! strongest `keep_ratio` fraction of edges by weight *and* a minimum spanning
//! forest over the whole node set, so the result stays connected (no node is
//! orphaned by a pure top-k cut) while still dropping the weak majority.
//!
//! The graph type here is deliberately small and self-contained: an undirected
//! weighted graph over `i64` node ids, which is what a symbol graph looks like
//! once the edge `weight` property is read.

use std::collections::{BTreeMap, BTreeSet};

/// An undirected weighted edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeightedEdge {
    /// One endpoint.
    pub a: i64,
    /// The other endpoint.
    pub b: i64,
    /// Edge weight (higher is stronger; the MST maximizes, not minimizes, so a
    /// heavy backbone is preferred).
    pub weight: f64,
}

impl WeightedEdge {
    /// A new edge.
    #[must_use]
    pub fn new(a: i64, b: i64, weight: f64) -> Self {
        Self { a, b, weight }
    }
}

/// An undirected weighted graph.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    /// Node ids.
    pub nodes: Vec<i64>,
    /// Edges.
    pub edges: Vec<WeightedEdge>,
}

impl Graph {
    /// An empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node (idempotent).
    pub fn add_node(&mut self, id: i64) {
        if !self.nodes.contains(&id) {
            self.nodes.push(id);
            self.nodes.sort_unstable();
        }
    }

    /// Add an edge (endpoints are added as nodes).
    pub fn add_edge(&mut self, a: i64, b: i64, weight: f64) {
        self.add_node(a);
        self.add_node(b);
        self.edges.push(WeightedEdge::new(a, b, weight));
    }

    /// Number of nodes.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of edges.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

/// Sparsify `graph`: keep the strongest `keep_ratio` fraction of edges plus a
/// maximum-spanning-forest backbone over all nodes.
///
/// `keep_ratio` is clamped to `[0, 1]`. The union is deduplicated by unordered
/// endpoint pair (a pair that is both in the top-k and the backbone appears
/// once), so the result never exceeds `top_k + (nodes − components)` edges.
#[must_use]
pub fn sparsify(graph: &Graph, keep_ratio: f64) -> Graph {
    let keep_ratio = keep_ratio.clamp(0.0, 1.0);
    let mut ranked = graph.edges.clone();
    ranked.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.a.cmp(&b.a))
            .then_with(|| a.b.cmp(&b.b))
    });
    let keep = ((ranked.len() as f64) * keep_ratio).round() as usize;
    let top_k: Vec<WeightedEdge> = ranked.into_iter().take(keep).collect();

    let backbone = maximum_spanning_forest(graph);
    let mut out = Graph {
        nodes: graph.nodes.clone(),
        edges: Vec::new(),
    };
    let mut seen: BTreeSet<(i64, i64)> = BTreeSet::new();
    for edge in top_k.into_iter().chain(backbone) {
        let pair = if edge.a <= edge.b {
            (edge.a, edge.b)
        } else {
            (edge.b, edge.a)
        };
        if seen.insert(pair) {
            out.edges.push(edge);
        }
    }
    out.edges.sort_by(|a, b| {
        a.a.cmp(&b.a).then_with(|| a.b.cmp(&b.b)).then_with(|| {
            a.weight
                .partial_cmp(&b.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    out
}

/// Kruskal maximum-spanning forest (per connected component).
fn maximum_spanning_forest(graph: &Graph) -> Vec<WeightedEdge> {
    let mut parent: BTreeMap<i64, i64> = graph.nodes.iter().map(|&id| (id, id)).collect();
    let mut ranked = graph.edges.clone();
    ranked.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.a.cmp(&b.a))
            .then_with(|| a.b.cmp(&b.b))
    });
    let mut forest = Vec::new();
    for edge in ranked {
        let ra = find(&mut parent, edge.a);
        let rb = find(&mut parent, edge.b);
        if ra != rb {
            parent.insert(ra, rb);
            forest.push(edge);
        }
    }
    forest
}

/// Union-find `find` with path compression.
fn find(parent: &mut BTreeMap<i64, i64>, mut x: i64) -> i64 {
    while parent.get(&x).copied().unwrap_or(x) != x {
        let next = parent.get(&x).copied().unwrap_or(x);
        parent.insert(x, parent.get(&next).copied().unwrap_or(next));
        x = next;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> Graph {
        let mut g = Graph::new();
        g.add_edge(1, 2, 1.0);
        g.add_edge(2, 3, 2.0);
        g.add_edge(3, 4, 3.0);
        g.add_edge(1, 4, 0.1);
        g.add_edge(1, 3, 0.2);
        g
    }

    #[test]
    fn keep_ratio_zero_keeps_only_the_backbone() {
        let sparse = sparsify(&graph(), 0.0);
        // A spanning tree over 4 nodes has 3 edges (the strongest ones here).
        assert_eq!(sparse.edge_count(), 3);
        assert_eq!(sparse.node_count(), 4);
    }

    #[test]
    fn keep_ratio_one_keeps_everything() {
        let sparse = sparsify(&graph(), 1.0);
        assert_eq!(sparse.edge_count(), graph().edge_count());
    }

    #[test]
    fn every_node_stays_connected() {
        let sparse = sparsify(&graph(), 0.0);
        // The backbone must reach all four nodes.
        let mut reached: BTreeSet<i64> = BTreeSet::new();
        reached.insert(sparse.nodes[0]);
        loop {
            let before = reached.len();
            for edge in &sparse.edges {
                if reached.contains(&edge.a) {
                    reached.insert(edge.b);
                }
                if reached.contains(&edge.b) {
                    reached.insert(edge.a);
                }
            }
            if reached.len() == before {
                break;
            }
        }
        assert_eq!(reached.len(), 4, "all nodes remain reachable");
    }

    #[test]
    fn dedupes_shared_backbone_and_topk_edges() {
        let sparse = sparsify(&graph(), 0.6);
        let mut pairs: Vec<(i64, i64)> = sparse
            .edges
            .iter()
            .map(|e| if e.a <= e.b { (e.a, e.b) } else { (e.b, e.a) })
            .collect();
        let before = pairs.len();
        pairs.sort_unstable();
        pairs.dedup();
        assert_eq!(pairs.len(), before, "no duplicate pairs");
    }

    #[test]
    fn empty_graph_sparsifies_to_empty() {
        let sparse = sparsify(&Graph::new(), 0.5);
        assert_eq!(sparse.node_count(), 0);
        assert_eq!(sparse.edge_count(), 0);
    }
}
