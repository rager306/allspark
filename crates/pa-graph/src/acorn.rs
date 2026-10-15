//! V10 — `ACORN`: predicate-aware approximate nearest-neighbour search.
//!
//! Plain `ANN` indexes answer "what is near the query vector". Many real
//! queries are *hybrid*: "what is near the query vector **and** satisfies a
//! predicate" (a label, a time window, an ACL tag). The naive fix — filter the
//! result set after the fact — returns fewer than `k` rows when the predicate
//! is rare, because the graph walk never reaches enough passing nodes.
//!
//! `ACORN` keeps the walk graph unchanged and evaluates the predicate *during*
//! expansion: nodes that fail the predicate are **not** returned, but their
//! neighbours are still explored, so the search can travel through a region of
//! non-matching nodes to reach matching ones. The expansion budget bounds the
//! work, keeping the worst case finite on a cyclic graph.
//!
//! ```text
//! search(entries, predicate, k, expand):
//!   BFS from entries; a node counts toward the result iff predicate(node);
//!   every visited node's neighbours are enqueued regardless of the predicate;
//!   stop at k matches or after `expand` visited nodes.
//! ```

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A predicate-aware `ANN` index over an adjacency structure.
///
/// The index is vector-agnostic: the caller supplies the neighbour lists (as
/// produced by an `HNSW`/`DiskANN` build) and the predicate, and the index
/// performs the predicate-aware walk.
#[derive(Debug, Clone, Default)]
pub struct AcornIndex {
    adjacency: BTreeMap<i64, Vec<i64>>,
}

impl AcornIndex {
    /// Build an index from an adjacency map. Neighbour lists are sorted and
    /// de-duplicated so traversal order is deterministic.
    #[must_use]
    pub fn new(adjacency: BTreeMap<i64, Vec<i64>>) -> Self {
        let adjacency = adjacency
            .into_iter()
            .map(|(node, mut nbrs)| {
                nbrs.sort_unstable();
                nbrs.dedup();
                nbrs.retain(|n| *n != node);
                (node, nbrs)
            })
            .collect();
        Self { adjacency }
    }

    /// Build an index from a list of undirected edges. Both endpoints become
    /// nodes; a self-loop is ignored.
    #[must_use]
    pub fn from_edges(edges: &[(i64, i64)]) -> Self {
        let mut adjacency: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for &(a, b) in edges {
            if a == b {
                continue;
            }
            adjacency.entry(a).or_default().push(b);
            adjacency.entry(b).or_default().push(a);
        }
        Self::new(adjacency)
    }

    /// Number of nodes in the index.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.adjacency.len()
    }

    /// Neighbours of `node` (`[]` when absent).
    #[must_use]
    pub fn neighbors(&self, node: i64) -> &[i64] {
        self.adjacency.get(&node).map_or(&[], Vec::as_slice)
    }

    /// Predicate-aware search.
    ///
    /// Expands breadth-first from `entry_points`, returning up to `k` node ids
    /// that satisfy `predicate`, in discovery order. Nodes failing the
    /// predicate are still expanded, so a rare predicate can still fill `k`.
    /// At most `expand` nodes are visited, which bounds the work on cyclic
    /// graphs. `k == 0` or an empty entry set yields an empty result.
    #[must_use]
    pub fn search(
        &self,
        entry_points: &[i64],
        predicate: impl Fn(i64) -> bool,
        k: usize,
        expand: usize,
    ) -> Vec<i64> {
        if k == 0 || expand == 0 {
            return Vec::new();
        }
        let mut visited: BTreeSet<i64> = BTreeSet::new();
        let mut queue: VecDeque<i64> = VecDeque::new();
        for &entry in entry_points {
            if self.adjacency.contains_key(&entry) && visited.insert(entry) {
                queue.push_back(entry);
            }
        }
        let mut hits: Vec<i64> = Vec::new();
        let mut expanded = 0usize;
        while let Some(node) = queue.pop_front() {
            if expanded >= expand {
                break;
            }
            expanded += 1;
            if predicate(node) {
                hits.push(node);
                if hits.len() >= k {
                    break;
                }
            }
            for &neighbor in self.neighbors(node) {
                if visited.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connected graph where each node links to two deterministic peers.
    fn clustered(n: i64) -> AcornIndex {
        let mut edges = Vec::new();
        for i in 0..n {
            edges.push((i, (i + 1) % n));
            edges.push((i, (i + 7) % n));
        }
        AcornIndex::from_edges(&edges)
    }

    #[test]
    fn every_result_satisfies_the_predicate() {
        let index = clustered(30);
        let hits = index.search(&[0], |id| id % 10 == 0, 3, 30);
        assert_eq!(hits.len(), 3, "three multiples of ten exist");
        assert!(hits.iter().all(|id| id % 10 == 0), "all hits match");
    }

    #[test]
    fn rare_predicate_still_fills_k() {
        // Only one node in thirty matches, but expansion walks through the
        // failing nodes to reach it.
        let index = clustered(30);
        let hits = index.search(&[0], |id| id == 17, 1, 30);
        assert_eq!(hits, vec![17], "reached the lone matching node");
    }

    #[test]
    fn expansion_budget_bounds_the_walk() {
        let index = clustered(30);
        // Budget of 1 visits only the entry point; if it fails the predicate
        // the result is empty even though matches exist elsewhere.
        let hits = index.search(&[0], |id| id == 17, 1, 1);
        assert_eq!(hits, Vec::<i64>::new(), "budget too small to reach node 17");
    }

    #[test]
    fn zero_k_and_empty_entries_are_empty() {
        let index = clustered(10);
        assert_eq!(index.search(&[0], |_| true, 0, 10), Vec::<i64>::new());
        assert_eq!(index.search(&[], |_| true, 5, 10), Vec::<i64>::new());
    }

    #[test]
    fn self_loops_are_dropped_and_count_is_stable() {
        let index = AcornIndex::from_edges(&[(1, 1), (1, 2)]);
        assert_eq!(index.node_count(), 2);
        assert_eq!(index.neighbors(1), &[2]);
    }
}
