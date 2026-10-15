//! V13 — GNN-style reranking by score propagation over the graph.
//!
//! A first-stage retriever assigns each node a base relevance score from the
//! query alone. Those scores ignore structure: a node whose neighbours are all
//! highly relevant is likely relevant too, even if its own base score is
//! middling. One or two rounds of message passing capture that signal cheaply:
//!
//! ```text
//! propagated(v) = α · own(v) + (1 − α) · mean(own(neighbours of v))
//! final(v)      = β · base(v) + (1 − β) · propagated(v)
//! ```
//!
//! Each round recomputes every node from the previous round's values
//! (Jacobi-style, not in-place), so the update is order-independent and
//! deterministic. Only neighbours present in the scored set contribute, and a
//! node with no such neighbour keeps its own value.

use std::collections::{BTreeMap, BTreeSet};

/// Rerank `base` scores with `rounds` of neighbour-mean propagation.
///
/// `base` is the `(node, score)` list from the first stage; `edges` are
/// undirected `(a, b)` pairs. `alpha` weights a node's own score against the
/// mean of its neighbours (`0.0` = pure neighbour mean, `1.0` = no
/// propagation); `beta` weights the original base score against the propagated
/// score in the final blend. Returns `(node, final_score)` sorted by
/// descending score, ties broken by ascending id. `rounds == 0` returns the
/// base scores (still blended, which is the identity when `beta == 1.0`).
#[must_use]
pub fn gnn_rerank(
    base: &[(i64, f32)],
    edges: &[(i64, i64)],
    rounds: usize,
    alpha: f32,
    beta: f32,
) -> Vec<(i64, f32)> {
    if base.is_empty() {
        return Vec::new();
    }

    let base_map: BTreeMap<i64, f32> = base.iter().copied().collect();

    // Undirected adjacency restricted to scored nodes, sorted for determinism.
    let mut adjacency: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for &(a, b) in edges {
        if a == b || !base_map.contains_key(&a) || !base_map.contains_key(&b) {
            continue;
        }
        adjacency.entry(a).or_default().push(b);
        adjacency.entry(b).or_default().push(a);
    }
    for nbrs in adjacency.values_mut() {
        nbrs.sort_unstable();
        nbrs.dedup();
    }

    let mut current = base_map.clone();
    for _ in 0..rounds {
        let mut next: BTreeMap<i64, f32> = BTreeMap::new();
        for (&node, &own) in &current {
            let neighbor_mean = match adjacency.get(&node) {
                Some(nbrs) if !nbrs.is_empty() => {
                    let sum: f32 = nbrs.iter().map(|n| current[n]).sum();
                    sum / nbrs.len() as f32
                }
                _ => own,
            };
            next.insert(node, alpha.mul_add(own, (1.0 - alpha) * neighbor_mean));
        }
        current = next;
    }

    let mut out: Vec<(i64, f32)> = base_map
        .iter()
        .map(|(&node, &base_score)| {
            let propagated = current[&node];
            let final_score = beta.mul_add(base_score, (1.0 - beta) * propagated);
            (node, final_score)
        })
        .collect();
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}

/// Convenience: nodes reachable in `edges`, for callers building the graph.
#[must_use]
pub fn edge_nodes(edges: &[(i64, i64)]) -> BTreeSet<i64> {
    let mut set = BTreeSet::new();
    for &(a, b) in edges {
        set.insert(a);
        set.insert(b);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_with_strong_neighbors_rises() {
        // Node 1 has a mediocre base score but two strong neighbours; node 2
        // has a slightly higher base but weak neighbours.
        let base = vec![(1, 0.4), (2, 0.5), (3, 0.9), (4, 0.9)];
        let edges = vec![(1, 3), (1, 4), (2, 5)];
        let ranked = gnn_rerank(&base, &edges, 2, 0.5, 0.3);
        let score = |id: i64| ranked.iter().find(|(n, _)| *n == id).unwrap().1;
        assert!(
            score(1) > score(2),
            "node 1 (strong neighbours) must overtake node 2 (isolated)"
        );
    }

    #[test]
    fn propagation_is_deterministic() {
        let base = vec![(1, 0.2), (2, 0.8), (3, 0.5)];
        let edges = vec![(1, 2), (2, 3)];
        let a = gnn_rerank(&base, &edges, 2, 0.5, 0.5);
        let b = gnn_rerank(&base, &edges, 2, 0.5, 0.5);
        assert_eq!(a, b);
    }

    #[test]
    fn converges_within_two_rounds() {
        // On a fixed graph, the ranking after 2 rounds matches the ranking
        // after 20 rounds (the propagation has effectively settled).
        let base = vec![(1, 0.1), (2, 0.9), (3, 0.3), (4, 0.6)];
        let edges = vec![(1, 2), (2, 3), (3, 4), (1, 4)];
        let two = gnn_rerank(&base, &edges, 2, 0.5, 0.5);
        let many = gnn_rerank(&base, &edges, 20, 0.5, 0.5);
        let order = |v: &[(i64, f32)]| v.iter().map(|(n, _)| *n).collect::<Vec<_>>();
        assert_eq!(order(&two), order(&many), "ranking stabilised by round 2");
    }

    #[test]
    fn alpha_one_is_identity_propagation() {
        let base = vec![(1, 0.2), (2, 0.8)];
        let edges = vec![(1, 2)];
        // alpha = 1 means no neighbour influence; beta = 1 keeps base.
        let out = gnn_rerank(&base, &edges, 5, 1.0, 1.0);
        assert_eq!(out, vec![(2, 0.8), (1, 0.2)]);
    }

    #[test]
    fn edges_to_unscored_nodes_are_ignored() {
        let base = vec![(1, 0.5)];
        let edges = vec![(1, 999)];
        let out = gnn_rerank(&base, &edges, 3, 0.5, 0.0);
        assert_eq!(out, vec![(1, 0.5)], "no scored neighbour → own score kept");
    }

    #[test]
    fn empty_base_is_empty() {
        assert_eq!(gnn_rerank(&[], &[(1, 2)], 2, 0.5, 0.5), Vec::<(i64, f32)>::new());
    }

    #[test]
    fn edge_nodes_collects_endpoints() {
        let nodes = edge_nodes(&[(1, 2), (2, 3)]);
        assert_eq!(nodes.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }
}
