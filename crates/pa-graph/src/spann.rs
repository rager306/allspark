//! R16 — SPANN-style spilling: replicated boundary points across clusters.
//!
//! SPANN (Space-filling Partitioned ANN) trades a little storage for recall:
//! points near a cluster boundary are **replicated** into every nearby
//! cluster, so a query that probes only its own cluster still finds a
//! boundary point that the "true" nearest-neighbour search would have placed
//! in a sibling cluster.
//!
//! This module keeps the reference idea with deterministic, dependency-free
//! pieces:
//!
//! - [`spann_build`] runs a fixed-seed k-means-lite over the points. Centroids
//!   are initialized at evenly spaced point indices (no randomness), so the
//!   partition is reproducible. Every point is assigned to its nearest
//!   centroid; additionally, every *other* centroid within `spill_eps` also
//!   gets the point — that is the spill.
//! - [`spann_search`] probes a caller-supplied list of clusters (the top-k
//!   clusters for a query) and returns distinct points, de-duplicating the
//!   replicas so a spilled point is reported once.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

/// A point identifier.
pub type PointId = i64;

/// A cluster identifier.
pub type ClusterId = usize;

/// A point: an id plus its feature vector.
pub type Point = (PointId, Vec<f32>);

/// The result of [`spann_build`]: cluster assignments and the learned
/// centroids.
#[derive(Debug, Clone, PartialEq)]
pub struct Partitions {
    /// For each point, the ascending cluster ids it belongs to (≥ 1; ≥ 2 for a
    /// spilled boundary point).
    pub assign: BTreeMap<PointId, Vec<ClusterId>>,
    /// The final centroids, indexed by cluster id.
    pub centroids: Vec<Vec<f32>>,
}

impl Partitions {
    /// The clusters a point belongs to, or an empty slice if unknown.
    #[must_use]
    pub fn clusters_of(&self, point: PointId) -> &[ClusterId] {
        self.assign.get(&point).map_or(&[][..], Vec::as_slice)
    }

    /// Cluster id → the points assigned to it, ascending.
    #[must_use]
    pub fn content(&self) -> BTreeMap<ClusterId, Vec<PointId>> {
        let mut out: BTreeMap<ClusterId, BTreeSet<PointId>> = BTreeMap::new();
        for (&p, cs) in &self.assign {
            for &c in cs {
                out.entry(c).or_default().insert(p);
            }
        }
        out.into_iter()
            .map(|(c, ps)| (c, ps.into_iter().collect()))
            .collect()
    }
}

/// Squared Euclidean distance (monotone with the distance, so fine for
/// argmin/`≤ eps` comparisons).
fn dist2(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum()
}

/// Fixed-seed k-means-lite over `points`, then spill boundary points.
///
/// `k` is clamped to `1..=points.len()`. Centroids start at evenly spaced
/// point indices and are refined for a fixed 32 iterations (or until they
/// stop moving), so the result is deterministic. A point is assigned to its
/// nearest centroid and to every other centroid within `spill_eps`
/// (Euclidean).
#[must_use]
pub fn spann_build(points: &[Point], k: usize, spill_eps: f32) -> Partitions {
    let n = points.len();
    let mut assign: BTreeMap<PointId, Vec<ClusterId>> = BTreeMap::new();
    if n == 0 {
        return Partitions {
            assign,
            centroids: Vec::new(),
        };
    }
    let k = k.clamp(1, n);
    let dim = points[0].1.len();

    // Deterministic init: evenly spaced point indices.
    let mut centroids: Vec<Vec<f32>> = (0..k)
        .map(|i| {
            let idx = i * n / k;
            points[idx].1.clone()
        })
        .collect();

    let mut labels: Vec<usize> = vec![0; n];
    for _ in 0..32 {
        let mut moved = false;
        for (pi, (_, feat)) in points.iter().enumerate() {
            let mut best = 0_usize;
            let mut best_d = f32::INFINITY;
            for (ci, c) in centroids.iter().enumerate() {
                let d = dist2(feat, c);
                if d < best_d - 1e-6 || ((d - best_d).abs() <= 1e-6 && ci < best) {
                    best_d = d;
                    best = ci;
                }
            }
            if labels[pi] != best {
                labels[pi] = best;
                moved = true;
            }
        }

        let mut sums = vec![vec![0.0_f32; dim]; k];
        let mut counts = vec![0_usize; k];
        for (pi, (_, feat)) in points.iter().enumerate() {
            let c = labels[pi];
            counts[c] += 1;
            for (s, v) in sums[c].iter_mut().zip(feat.iter()) {
                *s += *v;
            }
        }
        for ci in 0..k {
            if counts[ci] > 0 {
                let inv = 1.0 / counts[ci] as f32;
                for (c, s) in centroids[ci].iter_mut().zip(sums[ci].iter()) {
                    *c = s * inv;
                }
            }
        }
        if !moved {
            break;
        }
    }

    let eps2 = spill_eps * spill_eps;
    for (pid, feat) in points {
        let mut near: Vec<ClusterId> = Vec::new();
        let mut best = 0_usize;
        let mut best_d = f32::INFINITY;
        for (ci, c) in centroids.iter().enumerate() {
            let d = dist2(feat, c);
            if d < best_d - 1e-6 || ((d - best_d).abs() <= 1e-6 && ci < best) {
                best_d = d;
                best = ci;
            }
            if d <= eps2 {
                near.push(ci);
            }
        }
        if !near.contains(&best) {
            near.push(best);
        }
        near.sort_unstable();
        near.dedup();
        assign.insert(*pid, near);
    }

    Partitions { assign, centroids }
}

/// Probe `query_assign` clusters in `clusters_content` and return up to `k`
/// distinct points.
///
/// Clusters are visited in the order given (nearest first, by convention);
/// points inside a cluster keep their stored order. Replicas of a spilled
/// point that appear in several probed clusters are returned **once**.
#[must_use]
pub fn spann_search(
    query_assign: &[ClusterId],
    clusters_content: &BTreeMap<ClusterId, Vec<PointId>>,
    k: usize,
) -> Vec<PointId> {
    let mut seen: BTreeSet<PointId> = BTreeSet::new();
    let mut out: Vec<PointId> = Vec::new();
    for c in query_assign {
        if let Some(pts) = clusters_content.get(c) {
            for &p in pts {
                if seen.insert(p) {
                    out.push(p);
                    if out.len() == k {
                        return out;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two tight groups (around 0 and around 10) plus one midpoint at 5.0.
    fn line_points() -> Vec<Point> {
        vec![
            (0, vec![0.0]),
            (1, vec![0.1]),
            (2, vec![0.2]),
            (3, vec![0.3]),
            (4, vec![10.0]),
            (5, vec![10.1]),
            (6, vec![10.2]),
            (7, vec![10.3]),
            (8, vec![5.0]),
        ]
    }

    #[test]
    fn every_point_lands_in_at_least_one_cluster() {
        let parts = spann_build(&line_points(), 2, 0.0);
        assert_eq!(parts.assign.len(), 9);
        for cs in parts.assign.values() {
            assert!(!cs.is_empty(), "a point must belong to a cluster");
        }
        assert_eq!(parts.centroids.len(), 2);
    }

    #[test]
    fn boundary_point_spills_into_two_clusters() {
        // eps = 6.0 puts the midpoint (5.0) within reach of both centroids
        // (≈1.1 and ≈10.2), so it is replicated; the tight-group points are not.
        let parts = spann_build(&line_points(), 2, 6.0);
        assert_eq!(parts.clusters_of(8), &[0, 1], "mid point spills into both");
        assert_eq!(parts.clusters_of(0), &[0]);
        assert_eq!(parts.clusters_of(4), &[1]);
        // Zero eps → no spill.
        let strict = spann_build(&line_points(), 2, 0.0);
        assert_eq!(strict.clusters_of(8).len(), 1);
    }

    #[test]
    fn search_finds_a_point_from_any_of_its_clusters() {
        let parts = spann_build(&line_points(), 2, 6.0);
        let content = parts.content();
        // Point 8 is in clusters 0 and 1; probing either one finds it.
        let from0 = spann_search(&[0], &content, 10);
        let from1 = spann_search(&[1], &content, 10);
        assert!(from0.contains(&8));
        assert!(from1.contains(&8));
    }

    #[test]
    fn search_dedups_spilled_replicas() {
        let parts = spann_build(&line_points(), 2, 6.0);
        let content = parts.content();
        let hits = spann_search(&[0, 1], &content, 20);
        let unique: BTreeSet<PointId> = hits.iter().copied().collect();
        assert_eq!(hits.len(), unique.len(), "no duplicate point ids");
        assert_eq!(hits.len(), 9, "all nine points found once");
        // k bounds the result.
        let top2 = spann_search(&[0, 1], &content, 2);
        assert_eq!(top2.len(), 2);
    }
}
