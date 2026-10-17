//! G17 — global min-cut, community detection, and cut certification.
//!
//! Three small, deterministic graph primitives over the undirected weighted
//! graph type the crate already speaks ([`WeightedEdge`] / `i64` node ids):
//!
//! - [`stoer_wagner`] — the Stoer–Wagner global min-cut: repeatedly contracts
//!   the two most tightly coupled super-nodes found by a maximum-adjacency
//!   search, keeping the lightest cut seen. It runs in `O(V^3)` with a dense
//!   adjacency matrix, which is exactly the "safe, obviously-correct" posture
//!   this crate wants (no heaps, no unsafe, fully deterministic).
//! - [`label_propagation`] — deterministic community detection: each node
//!   adopts the label carrying the most edge weight among its neighbours,
//!   sweeping nodes in ascending id order and breaking ties toward the
//!   *largest* label (so a high-id cluster seeds first and is not absorbed by
//!   an earlier one). Identical input always yields identical communities.
//! - [`certify_cut`] — a witness check for a proposed [`Cut`]: the reported
//!   crossing weight must equal the measured sum of the edges crossing the
//!   partition, and each side must be internally connected. A forged cut
//!   (wrong weight, overlapping sides, a dangling node, a split side) is
//!   rejected with `Err`.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use crate::sparsifier::WeightedEdge;

/// A community identifier assigned by [`label_propagation`].
pub type CommunityId = usize;

/// Float tolerance for the deterministic comparisons in this module.
const EPS: f64 = 1e-9;

/// A proposed cut: a partition of the node set into two sides plus the
/// reported weight of the edges crossing between them.
#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    /// Reported total weight of the edges crossing the partition.
    pub weight: f64,
    /// One side of the partition.
    pub a: Vec<i64>,
    /// The other side of the partition.
    pub b: Vec<i64>,
}

impl Cut {
    /// Build a cut, sorting both sides so equality is order-independent.
    #[must_use]
    pub fn new(weight: f64, mut a: Vec<i64>, mut b: Vec<i64>) -> Self {
        a.sort_unstable();
        a.dedup();
        b.sort_unstable();
        b.dedup();
        Self { weight, a, b }
    }
}

/// A verified cut produced by [`certify_cut`].
#[derive(Debug, Clone, PartialEq)]
pub struct CutWitness {
    /// The verified crossing weight (equal to the reported cut weight).
    pub cut_weight: f64,
    /// The crossing edges, each listed once as `(min_endpoint, max_endpoint)`.
    pub crossing_edges: Vec<(i64, i64)>,
    /// Whether side `a` is internally connected.
    pub a_connected: bool,
    /// Whether side `b` is internally connected.
    pub b_connected: bool,
}

/// Sorted, de-duplicated node ids touched by `edges`.
fn nodes_of(edges: &[WeightedEdge]) -> Vec<i64> {
    let mut set: BTreeSet<i64> = BTreeSet::new();
    for e in edges {
        set.insert(e.a);
        set.insert(e.b);
    }
    set.into_iter().collect()
}

/// Global minimum cut of an undirected weighted graph via Stoer–Wagner.
///
/// Returns `(cut_weight, side_a, side_b)`: the weight of the lightest cut and
/// the two sides of one such partition, both sorted ascending. Isolated nodes
/// and self-loops are handled: self-loops never cross a cut, and a node with
/// no incident edge can be split off at zero cost.
#[must_use]
#[allow(clippy::many_single_char_names, clippy::needless_range_loop)]
pub fn stoer_wagner(nodes: &[i64], edges: &[WeightedEdge]) -> (f64, Vec<i64>, Vec<i64>) {
    let mut all: BTreeSet<i64> = nodes.iter().copied().collect();
    for e in edges {
        all.insert(e.a);
        all.insert(e.b);
    }
    let node_list: Vec<i64> = all.into_iter().collect();
    let n = node_list.len();
    if n == 0 {
        return (0.0, Vec::new(), Vec::new());
    }
    if n == 1 {
        return (0.0, node_list, Vec::new());
    }

    let index: BTreeMap<i64, usize> = node_list.iter().enumerate().map(|(i, &v)| (v, i)).collect();
    let mut w = vec![vec![0.0_f64; n]; n];
    for e in edges {
        let i = index.get(&e.a).copied().unwrap_or(0);
        let j = index.get(&e.b).copied().unwrap_or(0);
        if i == j {
            continue;
        }
        w[i][j] += e.weight;
        w[j][i] += e.weight;
    }

    // `active` holds the current super-node representatives (original indices);
    // `groups[r]` lists the original indices collapsed into representative `r`.
    let mut active: Vec<usize> = (0..n).collect();
    let mut groups: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut best = f64::INFINITY;
    let mut best_side: Vec<usize> = Vec::new();

    while active.len() > 1 {
        let mut in_tree = vec![false; n];
        let mut weight_to_tree = vec![0.0_f64; n];
        let mut prev = usize::MAX;
        let mut last = usize::MAX;

        // Maximum-adjacency search: pull the active node most tightly bound to
        // the growing tree. Ties break toward the smaller original index
        // because `active` is scanned in ascending order with a strict `>`.
        for _ in 0..active.len() {
            let mut sel = usize::MAX;
            let mut best_w = f64::NEG_INFINITY;
            for &v in &active {
                if !in_tree[v] && weight_to_tree[v] > best_w {
                    best_w = weight_to_tree[v];
                    sel = v;
                }
            }
            in_tree[sel] = true;
            prev = last;
            last = sel;
            for &u in &active {
                if !in_tree[u] {
                    weight_to_tree[u] += w[sel][u];
                }
            }
        }

        let s = prev;
        let t = last;
        let cut = weight_to_tree[t];
        if cut < best - EPS {
            best = cut;
            best_side.clone_from(&groups[t]);
        }

        // Contract `t` into `s`: fold the edges and merge the groups.
        for &v in &active {
            if v != t {
                w[s][v] += w[t][v];
                w[v][s] = w[s][v];
            }
        }
        let merged = std::mem::take(&mut groups[t]);
        groups[s].extend(merged);
        active.retain(|&v| v != t);
    }

    let side_a: BTreeSet<i64> = best_side.iter().map(|&i| node_list[i]).collect();
    let side_b: Vec<i64> = node_list
        .iter()
        .copied()
        .filter(|v| !side_a.contains(v))
        .collect();
    (best, side_a.into_iter().collect(), side_b)
}

/// Deterministic label propagation over `edges`.
///
/// Returns one [`CommunityId`] per node, aligned with the ascending, unique
/// node list derived from `edges` (see [`community_map`] for the id-keyed
/// form). `rounds` bounds the sweeps; the sweep stops early once nothing
/// changes. Ties are broken toward the smallest label, so the result is
/// reproducible across runs and platforms.
#[must_use]
pub fn label_propagation(edges: &[WeightedEdge], rounds: usize) -> Vec<CommunityId> {
    let map = community_map(edges, rounds);
    nodes_of(edges).iter().map(|n| map[n]).collect()
}

/// [`label_propagation`] keyed by node id.
#[must_use]
#[allow(clippy::many_single_char_names, clippy::needless_range_loop)]
pub fn community_map(edges: &[WeightedEdge], rounds: usize) -> BTreeMap<i64, CommunityId> {
    let node_list = nodes_of(edges);
    let n = node_list.len();
    let index: BTreeMap<i64, usize> = node_list.iter().enumerate().map(|(i, &v)| (v, i)).collect();

    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for e in edges {
        let i = index.get(&e.a).copied().unwrap_or(0);
        let j = index.get(&e.b).copied().unwrap_or(0);
        if i == j {
            continue;
        }
        adj[i].push((j, e.weight));
        adj[j].push((i, e.weight));
    }

    let mut label: Vec<usize> = (0..n).collect();
    for _ in 0..rounds {
        let mut changed = false;
        for i in 0..n {
            let mut counts: BTreeMap<usize, f64> = BTreeMap::new();
            for &(j, wt) in &adj[i] {
                *counts.entry(label[j]).or_insert(0.0) += wt;
            }
            let mut best_label = label[i];
            let mut best_w = f64::NEG_INFINITY;
            for (&lab, &wt) in &counts {
                // `>=` keeps the largest label among the max-weight ones
                // (BTreeMap iterates ascending).
                if wt >= best_w {
                    best_w = wt;
                    best_label = lab;
                }
            }
            if best_label != label[i] {
                label[i] = best_label;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Canonicalize labels to a contiguous `0..k` in order of first appearance.
    let mut canonical: BTreeMap<usize, CommunityId> = BTreeMap::new();
    let mut next = 0_usize;
    for i in 0..n {
        canonical.entry(label[i]).or_insert_with(|| {
            let id = next;
            next += 1;
            id
        });
    }

    node_list
        .iter()
        .enumerate()
        .map(|(i, &v)| (v, canonical.get(&label[i]).copied().unwrap_or(0)))
        .collect()
}

/// Path-compressed find for the connectivity union-find.
fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// Union-find over the members of one side, used to test internal connectivity.
#[allow(clippy::many_single_char_names)]
fn is_connected(nodes: &BTreeSet<i64>, edges: &[WeightedEdge]) -> bool {
    let list: Vec<i64> = nodes.iter().copied().collect();
    if list.len() <= 1 {
        return true;
    }
    let index: BTreeMap<i64, usize> = list.iter().enumerate().map(|(i, &v)| (v, i)).collect();
    let mut parent: Vec<usize> = (0..list.len()).collect();

    for e in edges {
        if let (Some(&i), Some(&j)) = (index.get(&e.a), index.get(&e.b)) {
            let (ri, rj) = (uf_find(&mut parent, i), uf_find(&mut parent, j));
            if ri != rj {
                parent[ri] = rj;
            }
        }
    }
    let root = uf_find(&mut parent, 0);
    (0..list.len()).all(|i| uf_find(&mut parent, i) == root)
}

/// Certify a proposed [`Cut`] against the graph `edges`.
///
/// The cut is accepted only when the reported weight equals the measured sum
/// of the edges crossing the partition and each side is internally connected.
///
/// # Errors
///
/// Returns `Err` if the two sides overlap, if some node touched by `edges` is
/// on neither side, if the reported weight differs from the measured crossing
/// weight, or if a side with more than one node is not connected.
pub fn certify_cut(cut: &Cut, edges: &[WeightedEdge]) -> Result<CutWitness, String> {
    let node_list = nodes_of(edges);
    let a: BTreeSet<i64> = cut.a.iter().copied().collect();
    let b: BTreeSet<i64> = cut.b.iter().copied().collect();

    if !a.is_disjoint(&b) {
        return Err("cut sides overlap".to_string());
    }
    for n in &node_list {
        if !a.contains(n) && !b.contains(n) {
            return Err(format!("node {n} is on neither side of the cut"));
        }
    }

    let mut crossing: Vec<(i64, i64)> = Vec::new();
    let mut total = 0.0_f64;
    for e in edges {
        let xa = a.contains(&e.a);
        let ya = a.contains(&e.b);
        let xb = b.contains(&e.a);
        let yb = b.contains(&e.b);
        if (xa && yb) || (xb && ya) {
            let (lo, hi) = if e.a <= e.b { (e.a, e.b) } else { (e.b, e.a) };
            crossing.push((lo, hi));
            total += e.weight;
        }
    }
    crossing.sort_unstable();

    if (total - cut.weight).abs() > EPS {
        return Err(format!(
            "reported cut weight {} does not match measured {total}",
            cut.weight
        ));
    }

    let a_connected = is_connected(&a, edges);
    let b_connected = is_connected(&b, edges);
    if !a_connected || !b_connected {
        return Err("a cut side is not internally connected".to_string());
    }

    Ok(CutWitness {
        cut_weight: total,
        crossing_edges: crossing,
        a_connected,
        b_connected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles joined by a single bridge edge of weight 1: the global
    /// min-cut is the bridge.
    fn two_clusters_with_bridge() -> Vec<WeightedEdge> {
        vec![
            WeightedEdge::new(0, 1, 1.0),
            WeightedEdge::new(1, 2, 1.0),
            WeightedEdge::new(0, 2, 1.0),
            WeightedEdge::new(3, 4, 1.0),
            WeightedEdge::new(4, 5, 1.0),
            WeightedEdge::new(3, 5, 1.0),
            WeightedEdge::new(2, 3, 1.0),
        ]
    }

    #[test]
    fn stoer_wagner_finds_the_bridge_cut() {
        let edges = two_clusters_with_bridge();
        let nodes: Vec<i64> = (0..6).collect();
        let (weight, a, b) = stoer_wagner(&nodes, &edges);
        assert!((weight - 1.0).abs() < 1e-9, "bridge weighs 1, got {weight}");
        let mut sides = vec![a, b];
        sides.sort();
        assert_eq!(sides, vec![vec![0, 1, 2], vec![3, 4, 5]]);
    }

    #[test]
    fn stoer_wagner_handles_a_single_node_and_empty_graph() {
        assert_eq!(stoer_wagner(&[7], &[]), (0.0, vec![7], vec![]));
        let (w, a, b) = stoer_wagner(&[], &[]);
        assert!(w.abs() < 1e-9);
        assert!(a.is_empty() && b.is_empty());
    }

    #[test]
    fn label_propagation_splits_the_two_clusters() {
        let edges = two_clusters_with_bridge();
        let comms = community_map(&edges, 20);
        assert_eq!(comms[&0], comms[&1]);
        assert_eq!(comms[&1], comms[&2]);
        assert_eq!(comms[&3], comms[&4]);
        assert_eq!(comms[&4], comms[&5]);
        assert_ne!(
            comms[&2], comms[&3],
            "the bridge must not merge the clusters"
        );
        // Two communities, canonically numbered 0 and 1.
        let ids: BTreeSet<usize> = comms.values().copied().collect();
        assert_eq!(ids, BTreeSet::from([0, 1]));
    }

    #[test]
    fn label_propagation_is_deterministic() {
        let edges = two_clusters_with_bridge();
        assert_eq!(community_map(&edges, 20), community_map(&edges, 20));
    }

    #[test]
    fn certify_accepts_a_real_cut() {
        let edges = two_clusters_with_bridge();
        let cut = Cut::new(1.0, vec![0, 1, 2], vec![3, 4, 5]);
        let witness = certify_cut(&cut, &edges).expect("valid cut certifies");
        assert!((witness.cut_weight - 1.0).abs() < 1e-9);
        assert_eq!(witness.crossing_edges, vec![(2, 3)]);
        assert!(witness.a_connected && witness.b_connected);
    }

    #[test]
    fn certify_rejects_a_forged_weight() {
        let edges = two_clusters_with_bridge();
        let forged = Cut::new(5.0, vec![0, 1, 2], vec![3, 4, 5]);
        assert!(certify_cut(&forged, &edges).is_err());
    }

    #[test]
    fn certify_rejects_overlap_and_dangling_nodes() {
        let edges = two_clusters_with_bridge();
        let overlap = Cut::new(1.0, vec![0, 1, 2, 3], vec![3, 4, 5]);
        assert!(certify_cut(&overlap, &edges).is_err());
        let dangling = Cut::new(1.0, vec![0, 1, 2], vec![3, 4]);
        assert!(certify_cut(&dangling, &edges).is_err());
    }

    #[test]
    fn certify_rejects_a_split_side() {
        // Side a = {0, 2} touches node 1 only through edges, so it is not
        // internally connected even though the crossing weight is stated.
        let edges = vec![WeightedEdge::new(0, 1, 1.0), WeightedEdge::new(1, 2, 1.0)];
        let cut = Cut::new(2.0, vec![0, 2], vec![1]);
        assert!(certify_cut(&cut, &edges).is_err());
    }
}
