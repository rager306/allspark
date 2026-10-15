//! R22 — topological data analysis: 0-dimensional persistent homology.
//!
//! Build a Vietoris–Rips filtration over a point cloud: at radius `ε` every
//! pair of points closer than `ε` is connected. Tracking connected components
//! (Betti-0) as `ε` grows yields a **persistence diagram** — each component is
//! born (in H0 every point is born at `ε = 0`) and dies when it merges into
//! another, older component. A component that never dies gets an infinite
//! death.
//!
//! This module implements the standard union-find sweep:
//!
//! 1. enumerate all pairwise distances;
//! 2. sort the edges by increasing distance;
//! 3. union in that order — each successful union *kills* the younger
//!    component, recording `(birth, death) = (0, edge_distance)`;
//! 4. whatever is still standing at the end dies at infinity.
//!
//! The result always has exactly `n` bars (one per input point). Long bars are
//! real structure (well-separated clusters); short bars are noise.
//!
//! [`bottleneck`] compares two diagrams with a greedy (sorted, index-wise)
//! matching under the `L∞` metric, with unmatched points charged their distance
//! to the diagonal. It is a metric: `bottleneck(d, d) == 0` and it is
//! symmetric.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

/// Euclidean distance between two 2-D points.
#[must_use]
pub fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

/// Disjoint-set forest with path compression and union by size.
#[derive(Debug, Clone)]
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    /// Union the sets containing `a` and `b`. Returns `true` if they were
    /// distinct (a merge happened), `false` if they were already joined.
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (mut ra, mut rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        // Union by size; ties break on the smaller root id for determinism.
        if self.size[ra] < self.size[rb] || (self.size[ra] == self.size[rb] && ra > rb) {
            std::mem::swap(&mut ra, &mut rb);
        }
        self.parent[rb] = ra;
        self.size[ra] += self.size[rb];
        true
    }
}

/// H0 persistence of a 2-D point cloud.
///
/// Returns `n` bars `(birth, death)` — one per input point — with `birth == 0`
/// for every bar and `death` the filtration radius at which the component
/// merged (or [`f32::INFINITY`] for the last surviving components).
///
/// `eps_steps` bounds the filtration: only the `eps_steps` shortest edges are
/// added, so components still alive after that budget are reported with an
/// infinite death. Pass `0` (or a value at least `n·(n-1)/2`) to run the full
/// filtration.
#[must_use]
pub fn h0_persistence(points: &[(f32, f32)], eps_steps: usize) -> Vec<(f32, f32)> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }

    let mut edges: Vec<(f32, usize, usize)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            edges.push((distance(points[i], points[j]), i, j));
        }
    }
    edges.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });

    let take = if eps_steps == 0 {
        edges.len()
    } else {
        eps_steps.min(edges.len())
    };

    let mut uf = UnionFind::new(n);
    let mut bars: Vec<(f32, f32)> = Vec::new();
    for &(d, i, j) in edges.iter().take(take) {
        if uf.union(i, j) {
            // A merge kills one component at radius `d`.
            bars.push((0.0, d));
        }
    }

    // Every still-open component dies at infinity.
    let roots: BTreeSet<usize> = (0..n).map(|i| uf.find(i)).collect();
    for _ in roots {
        bars.push((0.0, f32::INFINITY));
    }
    bars
}

/// The Betti-0 curve: the number of connected components as a function of the
/// filtration radius.
///
/// `eps_steps` is the number of intervals the range `[0, max_distance]` is
/// split into; the curve has `eps_steps + 1` samples from `ε = 0` (all points
/// isolated) up to `ε = max_distance` (everything joined). Returns an empty
/// vector for an empty cloud.
#[must_use]
pub fn betti0_curve(points: &[(f32, f32)], eps_steps: usize) -> Vec<(f32, usize)> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(0.0, 1)];
    }
    let steps = eps_steps.max(1);

    let mut edges: Vec<(f32, usize, usize)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            edges.push((distance(points[i], points[j]), i, j));
        }
    }
    let max_dist = edges.iter().fold(0.0_f32, |m, e| m.max(e.0));

    let mut curve = Vec::with_capacity(steps + 1);
    for k in 0..=steps {
        let eps = max_dist * (k as f32) / (steps as f32);
        let mut uf = UnionFind::new(n);
        for &(d, i, j) in &edges {
            if d <= eps {
                uf.union(i, j);
            }
        }
        let count: BTreeSet<usize> = (0..n).map(|i| uf.find(i)).collect();
        curve.push((eps, count.len()));
    }
    curve
}

/// Total order on persistence pairs: by birth, then death (`INFINITY` last).
fn cmp_pair(a: (f32, f32), b: (f32, f32)) -> std::cmp::Ordering {
    a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1))
}

/// `L∞` cost of matching two bars, treating two infinite deaths as identical.
fn pair_cost(a: (f32, f32), b: (f32, f32)) -> f32 {
    let birth = (a.0 - b.0).abs();
    let death = match (a.1.is_infinite(), b.1.is_infinite()) {
        (true, true) => 0.0,
        (true, false) | (false, true) => f32::INFINITY,
        (false, false) => (a.1 - b.1).abs(),
    };
    birth.max(death)
}

/// Cost of leaving a bar unmatched (matched to the diagonal).
fn diagonal_cost(p: (f32, f32)) -> f32 {
    if p.1.is_infinite() {
        f32::INFINITY
    } else {
        (p.1 - p.0).abs() / 2.0
    }
}

/// Greedy bottleneck distance between two persistence diagrams.
///
/// Both diagrams are sorted by `(birth, death)` and matched index-wise; any
/// leftover bar is charged its distance to the diagonal. The bottleneck is the
/// largest matched cost. The result is symmetric and `bottleneck(d, d) == 0`.
#[must_use]
pub fn bottleneck(d1: &[(f32, f32)], d2: &[(f32, f32)]) -> f32 {
    let mut a = d1.to_vec();
    let mut b = d2.to_vec();
    a.sort_by(|x, y| cmp_pair(*x, *y));
    b.sort_by(|x, y| cmp_pair(*x, *y));

    let len = a.len().max(b.len());
    let mut worst = 0.0_f32;
    for i in 0..len {
        let cost = match (a.get(i), b.get(i)) {
            (Some(&p), Some(&q)) => pair_cost(p, q),
            (Some(&p), None) => diagonal_cost(p),
            (None, Some(&q)) => diagonal_cost(q),
            (None, None) => break,
        };
        if cost > worst {
            worst = cost;
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two tight clusters far apart: (0,0),(1,0) and (10,0),(11,0).
    fn two_clusters() -> Vec<(f32, f32)> {
        vec![(0.0, 0.0), (1.0, 0.0), (10.0, 0.0), (11.0, 0.0)]
    }

    #[test]
    fn two_clusters_give_two_long_lived_components() {
        let bars = h0_persistence(&two_clusters(), 0);
        assert_eq!(bars.len(), 4, "one bar per point");
        assert!(bars.iter().all(|(b, _)| *b == 0.0));

        let mut finite: Vec<f32> = bars
            .iter()
            .filter(|(_, d)| d.is_finite())
            .map(|(_, d)| *d)
            .collect();
        finite.sort_by(f32::total_cmp);
        // Short intra-cluster merges at distance 1.0 (two of them), then the
        // long inter-cluster merge: (1,0)→(10,0) is distance 9.0.
        assert_eq!(finite, vec![1.0, 1.0, 9.0]);

        let infinite = bars.iter().filter(|(_, d)| d.is_infinite()).count();
        assert_eq!(infinite, 1, "one cluster survives to infinity");

        // Long-lived components: the finite bar of length 9 and the infinite
        // one. Everything else is a length-1 noise bar.
        let long: Vec<&(f32, f32)> = bars.iter().filter(|(b, d)| d - b >= 5.0).collect();
        assert_eq!(long.len(), 2, "two long-lived components");
    }

    #[test]
    fn single_point_and_empty_cloud() {
        assert!(h0_persistence(&[], 0).is_empty());
        let one = h0_persistence(&[(3.0, 4.0)], 0);
        assert_eq!(one, vec![(0.0, f32::INFINITY)]);
        assert!(betti0_curve(&[], 5).is_empty());
        assert_eq!(betti0_curve(&[(0.0, 0.0)], 5), vec![(0.0, 1)]);
    }

    #[test]
    fn betti0_curve_starts_high_and_ends_at_one() {
        let curve = betti0_curve(&two_clusters(), 4);
        assert_eq!(curve.len(), 5);
        assert_eq!(curve[0].1, 4, "all isolated at ε = 0");
        assert_eq!(curve.last().unwrap().1, 1, "fully connected at max ε");
        // Betti-0 is monotonically non-increasing in ε.
        for pair in curve.windows(2) {
            assert!(pair[0].1 >= pair[1].1);
        }
    }

    #[test]
    fn bottleneck_of_a_diagram_with_itself_is_zero() {
        let d = h0_persistence(&two_clusters(), 0);
        assert_eq!(bottleneck(&d, &d), 0.0);
    }

    #[test]
    fn bottleneck_is_symmetric_and_reflects_a_small_perturbation() {
        let d1 = vec![(0.0, 1.0), (0.0, 5.0), (0.0, f32::INFINITY)];
        let d2 = vec![(0.0, 1.5), (0.0, 5.0), (0.0, f32::INFINITY)];
        let ab = bottleneck(&d1, &d2);
        let ba = bottleneck(&d2, &d1);
        assert_eq!(ab, ba, "symmetric");
        assert!((ab - 0.5).abs() < 1e-6, "only the 1.0→1.5 bar moved");

        // Two infinite bars match at zero cost; a lone infinite bar would be
        // infinitely far from a finite diagram.
        assert_eq!(
            bottleneck(&[(0.0, f32::INFINITY)], &[(0.0, f32::INFINITY)]),
            0.0
        );
        assert!(bottleneck(&[(0.0, f32::INFINITY)], &[(0.0, 2.0)]).is_infinite());
    }
}
