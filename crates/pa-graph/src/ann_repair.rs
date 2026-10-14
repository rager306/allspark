//! M7 — ANN-graph repair: drop dead links and reconnect orphans.
//!
//! An `HNSW`-shaped index degrades as vectors are deleted: neighbour lists keep
//! pointing at removed nodes, and a deletion can strand a live node with no
//! inbound path. [`repair_ann`] rebuilds an [`AnnGraph`] with
//! * dead links pruned,
//! * self-links and duplicates removed,
//! * per-node out-degree capped at `max_degree` (lowest neighbour ids kept, for
//!   determinism), and
//! * orphaned live nodes reconnected to their lowest-id live peer.
//!
//! Unlike [`AnnGraph::repair`](crate::delta_index::AnnGraph::repair), the degree
//! cap is explicit and the operation returns a fresh graph plus a
//! [`RepairStats`] report, so a caller can gate on the disturbance.

use std::collections::BTreeSet;

use crate::delta_index::AnnGraph;

/// What a repair changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepairStats {
    /// Links removed because their target was no longer live.
    pub dead_links_removed: usize,
    /// Duplicate/self links removed.
    pub duplicate_links_removed: usize,
    /// Links removed to honour `max_degree`.
    pub degree_capped: usize,
    /// Orphaned live nodes that were reconnected.
    pub orphans_reconnected: usize,
}

impl RepairStats {
    /// Total links removed.
    #[must_use]
    pub fn removed(&self) -> usize {
        self.dead_links_removed + self.duplicate_links_removed + self.degree_capped
    }
}

/// Rebuild `graph`, pruning dead links and reconnecting orphans.
///
/// `max_degree == 0` means "no cap". The returned graph is always a superset of
/// the live nodes and contains no dead links.
#[must_use]
pub fn repair_ann(graph: &AnnGraph, max_degree: usize) -> (AnnGraph, RepairStats) {
    let live = graph.live_nodes();
    let live_set: BTreeSet<i64> = live.iter().copied().collect();
    let mut stats = RepairStats::default();
    let mut out = AnnGraph::new();
    for &id in &live {
        out.insert(id);
    }

    for &id in &live {
        let mut kept: BTreeSet<i64> = BTreeSet::new();
        for &neighbor in graph.neighbors(id) {
            if !live_set.contains(&neighbor) {
                stats.dead_links_removed += 1;
                continue;
            }
            if neighbor == id {
                stats.duplicate_links_removed += 1;
                continue;
            }
            if !kept.insert(neighbor) {
                stats.duplicate_links_removed += 1;
            }
        }
        let mut ordered: Vec<i64> = kept.into_iter().collect();
        if max_degree > 0 && ordered.len() > max_degree {
            stats.degree_capped += ordered.len() - max_degree;
            ordered.truncate(max_degree);
        }
        for neighbor in ordered {
            out.link(id, neighbor);
        }
    }

    // Reconnect orphans (live nodes with neither an outgoing nor an incoming
    // live link — truly isolated) to their lowest-id live peer.
    if live.len() >= 2 {
        for &id in &live {
            let has_incoming = out
                .live_nodes()
                .iter()
                .any(|&other| out.neighbors(other).contains(&id));
            if out.neighbors(id).is_empty() && !has_incoming {
                if let Some(&peer) = live.iter().find(|&&peer| peer != id) {
                    out.link(id, peer);
                    stats.orphans_reconnected += 1;
                }
            }
        }
    }

    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(n: i64) -> AnnGraph {
        let mut graph = AnnGraph::new();
        for i in 0..n {
            graph.insert(i);
        }
        for i in 0..n - 1 {
            graph.link(i, i + 1);
        }
        graph
    }

    #[test]
    fn dead_links_are_pruned() {
        let mut graph = chain(4);
        graph.remove(2);
        let (repaired, stats) = repair_ann(&graph, 0);
        assert!(stats.dead_links_removed >= 1, "link 1->2 pointed at a dead node");
        assert!(!repaired.neighbors(1).contains(&2));
        let (live_links, total) = repaired.link_health();
        assert_eq!(live_links, total, "no dead links remain");
    }

    #[test]
    fn orphaned_nodes_are_reconnected() {
        let mut graph = AnnGraph::new();
        graph.insert(1);
        graph.insert(2);
        graph.insert(3);
        graph.link(1, 2);
        // 3 has no links.
        let (repaired, stats) = repair_ann(&graph, 0);
        assert_eq!(stats.orphans_reconnected, 1, "only the isolated node 3");
        assert!(!repaired.neighbors(3).is_empty(), "orphan was reconnected");
    }

    #[test]
    fn degree_is_capped_deterministically() {
        let mut graph = AnnGraph::new();
        for i in 0..6 {
            graph.insert(i);
        }
        for target in [5, 4, 3, 2, 1] {
            graph.link(0, target);
        }
        let (repaired, stats) = repair_ann(&graph, 2);
        assert_eq!(stats.degree_capped, 3);
        assert_eq!(repaired.neighbors(0), &[1, 2], "lowest ids survive");
    }

    #[test]
    fn duplicate_and_self_links_are_removed() {
        let mut graph = AnnGraph::new();
        graph.insert(1);
        graph.insert(2);
        graph.link(1, 2);
        graph.link(1, 2);
        graph.link(1, 1);
        let (repaired, stats) = repair_ann(&graph, 0);
        assert_eq!(stats.duplicate_links_removed, 2);
        assert_eq!(repaired.neighbors(1), &[2]);
    }

    #[test]
    fn healthy_graph_is_unchanged_in_structure() {
        let graph = chain(4);
        let (repaired, stats) = repair_ann(&graph, 0);
        assert_eq!(stats, RepairStats::default());
        assert_eq!(repaired.live_count(), graph.live_count());
        assert_eq!(repaired.edge_count(), graph.edge_count());
    }

    #[test]
    fn stats_removed_sums_its_parts() {
        let stats = RepairStats {
            dead_links_removed: 1,
            duplicate_links_removed: 2,
            degree_capped: 3,
            orphans_reconnected: 4,
        };
        assert_eq!(stats.removed(), 6);
    }
}
