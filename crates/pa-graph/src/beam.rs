//! R15 — diverse beam search over a graph.
//!
//! Classic beam search keeps the `width` highest-scoring partial paths, but
//! they often collapse onto the same trunk. [`diverse_beam`] borrows the
//! maximal-marginal-relevance idea from reranking: when a ray picks its next
//! node it maximizes
//!
//! ```text
//! score(next) − λ · max cosine(feature(next), feature(already chosen))
//! ```
//!
//! where "already chosen" is the ray's own path *and* the nodes the earlier
//! rays picked this step. With `λ = 0` the penalty vanishes and every ray walks
//! the greedy top-score path (the rays degenerate). With `λ = 1` a ray is
//! strongly pushed off its siblings' choices, so the beam covers distinct
//! nodes. Similarity is [`crate::hybrid::cosine`] over caller-supplied
//! features; ties break toward the smaller node id, keeping the search
//! deterministic.

#![forbid(unsafe_code)]

use crate::hybrid::cosine;

/// A node id in the searched graph.
pub type NodeId = i64;

/// Diverse beam search.
///
/// - `start` — the common first node of every ray.
/// - `expand_fn` — the successors of a node.
/// - `score_fn` — a node's relevance (higher is better).
/// - `feature_fn` — the node's feature vector (used for the diversity term).
/// - `beam_width` — how many rays to keep (the returned `Vec` length).
/// - `lambda` — the diversity weight, clamped to `[0, 1]`.
/// - `steps` — how many expansion steps to run.
///
/// Returns `beam_width` rays, each a path of up to `steps + 1` nodes starting
/// at `start`. A ray whose frontier is exhausted keeps its last node.
#[must_use]
pub fn diverse_beam<F, S, G>(
    start: NodeId,
    expand_fn: F,
    score_fn: S,
    feature_fn: G,
    beam_width: usize,
    lambda: f32,
    steps: usize,
) -> Vec<Vec<NodeId>>
where
    F: Fn(NodeId) -> Vec<NodeId>,
    S: Fn(NodeId) -> f32,
    G: Fn(NodeId) -> Vec<f32>,
{
    let lambda = lambda.clamp(0.0, 1.0);
    if beam_width == 0 {
        return Vec::new();
    }
    let mut beam: Vec<Vec<NodeId>> = vec![vec![start]; beam_width];

    for _ in 0..steps {
        // Features of the nodes the rays chose this step; the penalty for a
        // later ray measures similarity against these plus its own path.
        let mut chosen_features: Vec<Vec<f32>> = Vec::new();
        for path_slot in &mut beam {
            let path = path_slot.clone();
            let last = *path.last().unwrap_or(&start);
            let mut best: Option<(f32, NodeId)> = None;
            for cand in expand_fn(last) {
                if path.contains(&cand) {
                    continue;
                }
                let feat = feature_fn(cand);
                let mut redundancy = 0.0_f32;
                if lambda > 0.0 {
                    for prev in &path {
                        redundancy = redundancy.max(cosine(&feat, &feature_fn(*prev)));
                    }
                    for cf in &chosen_features {
                        redundancy = redundancy.max(cosine(&feat, cf));
                    }
                }
                let score = score_fn(cand) - lambda * redundancy;
                let better = match best {
                    None => true,
                    Some((bs, bid)) => {
                        score > bs + 1e-6 || ((score - bs).abs() <= 1e-6 && cand < bid)
                    }
                };
                if better {
                    best = Some((score, cand));
                }
            }
            if let Some((_, chosen)) = best {
                chosen_features.push(feature_fn(chosen));
                path_slot.push(chosen);
            }
        }
    }

    beam
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A tiny DAG: 0 fans out to {1, 2, 3}; each of those flows to 4.
    fn expand(n: NodeId) -> Vec<NodeId> {
        match n {
            0 => vec![1, 2, 3],
            1..=3 => vec![4],
            _ => Vec::new(),
        }
    }

    /// Equal scores, so the *only* thing separating the rays is the diversity
    /// penalty — the cleanest way to isolate the λ effect.
    fn score(_n: NodeId) -> f32 {
        3.0
    }

    /// Cosine similarities between siblings 1, 2, 3 are set to 0.2 / 0.9 / 0.6
    /// so a ray that already picked 1 prefers 2 (least similar) next, then 3.
    /// The start (0) and sink (4) are the zero vector, adding no penalty.
    fn features(n: NodeId) -> Vec<f32> {
        match n {
            1 => vec![1.0, 0.0],
            2 => vec![0.2, 0.979_795_9],
            3 => vec![0.9, 0.435_889_9],
            _ => vec![0.0, 0.0],
        }
    }

    #[test]
    fn lambda_zero_degenerates_to_greedy() {
        let rays = diverse_beam(0, expand, score, features, 3, 0.0, 2);
        assert_eq!(rays.len(), 3);
        for ray in &rays {
            assert_eq!(ray, &vec![0, 1, 4], "every ray walks the greedy path");
        }
    }

    #[test]
    fn lambda_one_forces_diversity() {
        // One step: all three rays branch off the same node but must pick
        // distinct successors, unlike the greedy collapse at λ = 0.
        let rays = diverse_beam(0, expand, score, features, 3, 1.0, 1);
        assert_eq!(rays.len(), 3, "width is respected");
        let firsts: BTreeSet<NodeId> = rays.iter().map(|r| r[1]).collect();
        assert_eq!(firsts, BTreeSet::from([1, 2, 3]), "distinct nodes chosen");
        for ray in &rays {
            assert_eq!(ray[0], 0, "all rays start at the start node");
        }
    }

    #[test]
    fn width_and_length_are_honoured() {
        // The DAG is depth-2 (0 → {1,2,3} → 4), so three steps still yield
        // three nodes per ray: the frontier is exhausted at depth 2.
        let rays = diverse_beam(0, expand, score, features, 2, 0.5, 3);
        assert_eq!(rays.len(), 2);
        for ray in &rays {
            assert_eq!(ray.len(), 3, "depth-2 DAG caps the ray at three nodes");
            assert_eq!(ray[0], 0);
            assert_eq!(ray[2], 4);
        }
    }

    #[test]
    fn dead_end_keeps_the_ray_in_place() {
        let rays = diverse_beam(4, expand, score, features, 1, 0.0, 3);
        assert_eq!(rays, vec![vec![4]]);
    }
}
