//! O11 — snapshot compaction: fold incremental snapshots into a base.
//!
//! An event-sourced store keeps a base snapshot plus a stream of incremental
//! snapshots. Replaying every increment from the base is O(total), so
//! [`SnapCompact`] folds them: for each node/edge key the *last* operation
//! wins, and the result is a single compacted [`GraphDelta`] that, applied to
//! the base, reaches the same state without touching a key twice.
//!
//! The fold is stable: nodes are emitted sorted by id and edges by
//! `(source, target, edge_type)`, so equal inputs always compact to byte-equal
//! output.

use std::collections::BTreeMap;

use crate::delta_graph::{EdgeDelta, EdgeOp, GraphDelta, NodeDelta, NodeOp};
use crate::types::EdgeType;

/// Compaction of graph snapshots.
#[derive(Debug, Clone, Copy, Default)]
pub struct SnapCompact;

impl SnapCompact {
    /// Fold `increments` (in order) into `base`, last write per key wins.
    #[must_use]
    pub fn compact(base: &GraphDelta, increments: &[GraphDelta]) -> GraphDelta {
        let mut nodes: BTreeMap<i64, NodeDelta> = BTreeMap::new();
        // `EdgeType` is not `Ord`; key on its stable wire name instead.
        let mut edges: BTreeMap<(i64, i64, &'static str), EdgeDelta> = BTreeMap::new();

        for delta in std::iter::once(base).chain(increments.iter()) {
            for node in &delta.nodes {
                nodes.insert(node.id, node.clone());
            }
            for edge in &delta.edges {
                edges.insert((edge.source, edge.target, edge.edge_type.as_str()), edge.clone());
            }
        }

        GraphDelta {
            nodes: nodes.into_values().collect(),
            edges: edges.into_values().collect(),
        }
    }

    /// Fold a whole sequence of snapshots (the first is the base).
    #[must_use]
    pub fn compact_all(snapshots: &[GraphDelta]) -> GraphDelta {
        match snapshots.split_first() {
            Some((base, rest)) => Self::compact(base, rest),
            None => GraphDelta::new(),
        }
    }

    /// Whether a compacted delta still removes a node (its last op is `Remove`).
    #[must_use]
    pub fn is_node_removed(delta: &GraphDelta, id: i64) -> bool {
        delta
            .nodes
            .iter()
            .find(|n| n.id == id)
            .is_some_and(|n| n.op == NodeOp::Remove)
    }

    /// Whether a compacted delta still adds/updates an edge.
    #[must_use]
    pub fn is_edge_added(delta: &GraphDelta, source: i64, target: i64, edge_type: EdgeType) -> bool {
        delta
            .edges
            .iter()
            .find(|e| e.source == source && e.target == target && e.edge_type == edge_type)
            .is_some_and(|e| e.op == EdgeOp::Add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_node(id: i64) -> NodeDelta {
        NodeDelta::add(id, format!("n{id}"))
    }

    fn remove_node(id: i64) -> NodeDelta {
        NodeDelta::remove(id, format!("n{id}"))
    }

    #[test]
    fn last_write_wins_per_node() {
        let base = GraphDelta {
            nodes: vec![add_node(1)],
            edges: vec![],
        };
        let increments = vec![
            GraphDelta {
                nodes: vec![add_node(2)],
                edges: vec![],
            },
            GraphDelta {
                nodes: vec![remove_node(1)],
                edges: vec![],
            },
        ];
        let compacted = SnapCompact::compact(&base, &increments);
        assert_eq!(compacted.nodes.len(), 2, "two distinct node keys");
        assert!(SnapCompact::is_node_removed(&compacted, 1));
        assert!(!SnapCompact::is_node_removed(&compacted, 2));
    }

    #[test]
    fn last_write_wins_per_edge() {
        let base = GraphDelta {
            nodes: vec![],
            edges: vec![EdgeDelta::add(1, 2, EdgeType::Calls)],
        };
        let increments = vec![GraphDelta {
            nodes: vec![],
            edges: vec![EdgeDelta::remove(1, 2, EdgeType::Calls)],
        }];
        let compacted = SnapCompact::compact(&base, &increments);
        assert_eq!(compacted.edges.len(), 1, "same edge key collapses to one");
        assert!(!SnapCompact::is_edge_added(&compacted, 1, 2, EdgeType::Calls));
    }

    #[test]
    fn distinct_edges_are_kept() {
        let base = GraphDelta::new();
        let increments = vec![GraphDelta {
            nodes: vec![],
            edges: vec![
                EdgeDelta::add(1, 2, EdgeType::Calls),
                EdgeDelta::add(1, 2, EdgeType::Imports),
            ],
        }];
        let compacted = SnapCompact::compact(&base, &increments);
        assert_eq!(compacted.edges.len(), 2, "edge type is part of the key");
    }

    #[test]
    fn compaction_is_order_stable() {
        let base = GraphDelta {
            nodes: vec![add_node(3), add_node(1)],
            edges: vec![EdgeDelta::add(2, 1, EdgeType::Calls)],
        };
        let increments = vec![GraphDelta {
            nodes: vec![add_node(2)],
            edges: vec![EdgeDelta::add(1, 2, EdgeType::Calls)],
        }];
        let compacted = SnapCompact::compact(&base, &increments);
        let ids: Vec<i64> = compacted.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![1, 2, 3], "nodes emitted sorted by id");
    }

    #[test]
    fn compact_all_on_empty_is_empty() {
        assert!(SnapCompact::compact_all(&[]).is_empty());
    }

    #[test]
    fn compact_all_folds_from_the_first_snapshot() {
        let snapshots = vec![
            GraphDelta {
                nodes: vec![add_node(1)],
                edges: vec![],
            },
            GraphDelta {
                nodes: vec![add_node(2)],
                edges: vec![],
            },
        ];
        let compacted = SnapCompact::compact_all(&snapshots);
        assert_eq!(compacted.nodes.len(), 2);
    }
}
