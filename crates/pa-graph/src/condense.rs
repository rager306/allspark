//! G15 — structure-preserving condensation of a graph into super-nodes.
//!
//! Given a partition of the nodes into communities (typically produced by
//! [`crate::mincut::label_propagation`]), [`condense`] collapses each community
//! into a single **super-node**:
//!
//! - intra-community edges disappear (both endpoints share one super-node);
//! - inter-community edges are aggregated, their weights summed, so the
//!   condensed graph keeps the same flow *between* communities;
//! - the mapping super-node → original nodes is preserved as **provenance**, so
//!   any super-node can be [`CondensedGraph::expand`]ed back to its members.
//!
//! The result is a smaller, structure-preserving view: traversing the condensed
//! graph answers community-level questions at a fraction of the edge count,
//! while provenance keeps the answer explainable.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crate::mincut::CommunityId;
use crate::sparsifier::WeightedEdge;

/// A node id in the source graph.
pub type NodeId = i64;

/// A condensed node: the community it stands for.
pub type SuperNode = CommunityId;

/// An aggregated edge between two super-nodes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SuperEdge {
    /// One super-node endpoint.
    pub a: SuperNode,
    /// The other super-node endpoint.
    pub b: SuperNode,
    /// Summed weight of the source edges that crossed this pair.
    pub weight: f64,
}

/// A condensed graph: super-nodes, aggregated edges, and provenance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CondensedGraph {
    /// Super-node ids, ascending.
    pub supernodes: Vec<SuperNode>,
    /// Aggregated inter-community edges, sorted by `(a, b)`.
    pub edges: Vec<SuperEdge>,
    /// Provenance: each super-node mapped to its original nodes, ascending.
    pub provenance: BTreeMap<SuperNode, Vec<NodeId>>,
}

impl CondensedGraph {
    /// The original nodes collapsed into `supernode`, or an empty slice if the
    /// id is unknown.
    #[must_use]
    pub fn expand(&self, supernode: SuperNode) -> &[NodeId] {
        self.provenance
            .get(&supernode)
            .map_or(&[][..], Vec::as_slice)
    }

    /// Number of super-nodes.
    #[must_use]
    pub fn supernode_count(&self) -> usize {
        self.supernodes.len()
    }
}

/// Condense `nodes`/`edges` by the given `communities` map.
///
/// Every node in `nodes` (and every endpoint of `edges`) lands in exactly one
/// super-node. Nodes missing from `communities` are given singleton super-nodes
/// numbered after the highest community id, in ascending node order, so the
/// result is total and deterministic.
#[must_use]
pub fn condense(
    nodes: &[NodeId],
    edges: &[WeightedEdge],
    communities: &BTreeMap<NodeId, CommunityId>,
) -> CondensedGraph {
    let mut node_set: Vec<NodeId> = nodes.to_vec();
    for e in edges {
        node_set.push(e.a);
        node_set.push(e.b);
    }
    node_set.sort_unstable();
    node_set.dedup();

    // Resolve every node to a super-node id, minting singletons for the nodes
    // the community map does not cover.
    let mut next_singleton = communities
        .values()
        .copied()
        .max()
        .map_or(0, |m| m + 1);
    let mut super_of: BTreeMap<NodeId, SuperNode> = BTreeMap::new();
    for &n in &node_set {
        let s = if let Some(&c) = communities.get(&n) {
            c
        } else {
            let s = next_singleton;
            next_singleton += 1;
            s
        };
        super_of.insert(n, s);
    }

    // Provenance: super-node → members, both ascending.
    let mut provenance: BTreeMap<SuperNode, Vec<NodeId>> = BTreeMap::new();
    for &n in &node_set {
        provenance.entry(super_of[&n]).or_default().push(n);
    }

    // Aggregate inter-community edges; intra-community edges collapse away.
    let mut agg: BTreeMap<(SuperNode, SuperNode), f64> = BTreeMap::new();
    for e in edges {
        let sa = super_of[&e.a];
        let sb = super_of[&e.b];
        if sa == sb {
            continue;
        }
        let key = if sa <= sb { (sa, sb) } else { (sb, sa) };
        *agg.entry(key).or_insert(0.0) += e.weight;
    }
    let mut agg_edges: Vec<SuperEdge> = agg
        .into_iter()
        .map(|((a, b), weight)| SuperEdge { a, b, weight })
        .collect();
    agg_edges.sort_by_key(|e| (e.a, e.b));

    CondensedGraph {
        supernodes: provenance.keys().copied().collect(),
        edges: agg_edges,
        provenance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Vec<NodeId>, Vec<WeightedEdge>, BTreeMap<NodeId, CommunityId>) {
        let nodes: Vec<NodeId> = (0..6).collect();
        let edges = vec![
            // intra-community (both clusters)
            WeightedEdge::new(0, 1, 1.0),
            WeightedEdge::new(1, 2, 2.0),
            WeightedEdge::new(3, 4, 3.0),
            // inter-community, two parallel edges that must sum
            WeightedEdge::new(2, 3, 4.0),
            WeightedEdge::new(2, 3, 6.0),
        ];
        let mut communities = BTreeMap::new();
        for n in [0, 1, 2] {
            communities.insert(n, 0);
        }
        for n in [3, 4, 5] {
            communities.insert(n, 1);
        }
        (nodes, edges, communities)
    }

    #[test]
    fn provenance_covers_every_node_exactly_once() {
        let (nodes, edges, communities) = sample();
        let cg = condense(&nodes, &edges, &communities);
        let mut seen: Vec<NodeId> = Vec::new();
        for members in cg.provenance.values() {
            seen.extend_from_slice(members);
        }
        seen.sort_unstable();
        assert_eq!(seen, nodes, "each node appears in exactly one super-node");
        assert_eq!(cg.supernode_count(), 2);
    }

    #[test]
    fn inter_community_weights_are_summed() {
        let (nodes, edges, communities) = sample();
        let cg = condense(&nodes, &edges, &communities);
        assert_eq!(cg.edges.len(), 1);
        let e = cg.edges[0];
        assert_eq!((e.a, e.b), (0, 1));
        assert!((e.weight - 10.0).abs() < 1e-9, "4 + 6 parallel edges sum");
    }

    #[test]
    fn expand_round_trips_membership() {
        let (nodes, edges, communities) = sample();
        let cg = condense(&nodes, &edges, &communities);
        assert_eq!(cg.expand(0), &[0, 1, 2]);
        assert_eq!(cg.expand(1), &[3, 4, 5]);
        assert!(cg.expand(99).is_empty());
    }

    #[test]
    fn uncovered_nodes_become_singletons() {
        let nodes = vec![0, 1, 2];
        let edges = vec![WeightedEdge::new(0, 1, 1.0), WeightedEdge::new(1, 2, 1.0)];
        let mut communities = BTreeMap::new();
        communities.insert(0, 0);
        communities.insert(1, 0);
        // node 2 is not in the map → its own super-node.
        let cg = condense(&nodes, &edges, &communities);
        assert_eq!(cg.expand(0), &[0, 1]);
        assert_eq!(cg.expand(1), &[2]);
        assert_eq!(cg.edges.len(), 1);
        assert_eq!((cg.edges[0].a, cg.edges[0].b), (0, 1));
    }
}
