//! V17 — recall-bounded search with an early exit.
//!
//! A beam/best-first walk can stop before covering the whole component once it
//! is sure no unexplored node can still meet a relevance guarantee. The walk
//! keeps a max-heap of frontier candidates keyed by a *decayed* score — a node
//! one hop from a seed scores `HOP_DECAY ×` the seed, two hops `HOP_DECAY²×`,
//! and so on — and pops the strongest candidate first. When the strongest
//! remaining candidate already scores below `bound`, every node still queued
//! (and everything reachable behind it) is at least that weak, so the search
//! **stops**:
//!
//! ```text
//! best-first over (decayed score, node); pop max;
//!   if score < bound  → stop (guarantee can no longer be met)
//!   accept node; enqueue unvisited neighbours at score · HOP_DECAY
//! ```
//!
//! Because the heap is popped strongest-first, the first node accepted is the
//! true top-1, and the guarantee holds for every node in the result. On a
//! connected graph the decay drops distant nodes below `bound`, so the walk
//! returns well before a full traversal.

use std::collections::{BTreeMap, BinaryHeap, HashSet};

/// Per-hop score decay for the frontier priority. `0.85` keeps a useful depth
/// of reach while still pruning far, weakly-connected regions.
pub const HOP_DECAY: f32 = 0.85;

/// A frontier entry, ordered by score (descending) then node id (ascending) so
/// the heap is deterministic.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    score: f32,
    node: i64,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node && self.score == other.score
    }
}
impl Eq for Candidate {}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // `BinaryHeap` is a max-heap; higher score wins, ties broken by the
        // *smaller* node id (so reverse the id comparison).
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.node.cmp(&self.node))
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Best-first, recall-bounded walk from `start_scores`.
///
/// Returns the accepted node ids in strongest-first order, stopping as soon as
/// the strongest frontier candidate scores below `bound`. `adjacency` maps a
/// node to its neighbours; neighbours absent from the map are still accepted
/// but expand to nothing. Duplicate seeds keep their highest score.
#[must_use]
pub fn bounded_search(
    start_scores: &[(i64, f32)],
    adjacency: &BTreeMap<i64, Vec<i64>>,
    bound: f32,
) -> Vec<i64> {
    let mut frontier: BinaryHeap<Candidate> = BinaryHeap::new();
    for &(node, score) in start_scores {
        frontier.push(Candidate { score, node });
    }

    let mut visited: HashSet<i64> = HashSet::new();
    let mut results: Vec<i64> = Vec::new();

    while let Some(best) = frontier.pop() {
        if best.score < bound {
            // Nothing left can meet the guarantee.
            break;
        }
        if !visited.insert(best.node) {
            continue;
        }
        results.push(best.node);
        let next_score = best.score * HOP_DECAY;
        if let Some(neighbors) = adjacency.get(&best.node) {
            for &neighbor in neighbors {
                if !visited.contains(&neighbor) {
                    frontier.push(Candidate {
                        score: next_score,
                        node: neighbor,
                    });
                }
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connected path graph `0 — 1 — … — (n-1)`.
    fn path(n: i64) -> BTreeMap<i64, Vec<i64>> {
        let mut adjacency: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for i in 0..n {
            let mut nbrs = Vec::new();
            if i > 0 {
                nbrs.push(i - 1);
            }
            if i + 1 < n {
                nbrs.push(i + 1);
            }
            adjacency.insert(i, nbrs);
        }
        adjacency
    }

    #[test]
    fn stops_before_full_traversal() {
        let graph = path(40);
        let results = bounded_search(&[(0, 1.0)], &graph, 0.5);
        assert!(
            results.len() < 40,
            "decay + bound must prune the tail: visited {} of 40",
            results.len()
        );
        assert!(
            results.len() >= 3,
            "the near region is still covered: {}",
            results.len()
        );
    }

    #[test]
    fn result_contains_true_top1_first() {
        let graph = path(20);
        let results = bounded_search(&[(0, 1.0), (10, 0.4)], &graph, 0.3);
        assert_eq!(results.first(), Some(&0), "highest-scoring seed leads");
    }

    #[test]
    fn all_results_meet_the_bound() {
        // Every accepted node's decayed score must be >= bound. On a path the
        // deepest accepted node is at distance d with score 0.85^d.
        let graph = path(40);
        let bound = 0.5_f32;
        let results = bounded_search(&[(0, 1.0)], &graph, bound);
        for &node in &results {
            let score = HOP_DECAY.powi(node as i32); // distance == node id on a path from 0
            assert!(score >= bound, "node {node} scored {score} < {bound}");
        }
    }

    #[test]
    fn unreachable_bound_yields_empty() {
        let graph = path(10);
        // No seed meets the bound, so nothing is accepted.
        let results = bounded_search(&[(0, 0.1)], &graph, 0.5);
        assert_eq!(results, Vec::<i64>::new());
    }

    #[test]
    fn disconnected_start_does_not_cross_gap() {
        // Two components: 0-1-2 and 10-11-12. A single seed stays in its own
        // component.
        let mut graph: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        graph.insert(0, vec![1]);
        graph.insert(1, vec![0, 2]);
        graph.insert(2, vec![1]);
        graph.insert(10, vec![11]);
        graph.insert(11, vec![10, 12]);
        graph.insert(12, vec![11]);
        let results = bounded_search(&[(0, 1.0)], &graph, 0.5);
        assert!(results.iter().all(|n| *n < 10), "never leaves component A");
    }

    #[test]
    fn duplicate_seeds_are_accepted_once() {
        let graph = path(5);
        let results = bounded_search(&[(0, 1.0), (0, 0.9)], &graph, 0.1);
        assert_eq!(results.iter().filter(|&&n| n == 0).count(), 1);
    }
}
