//! G18 — namespace routing.
//!
//! Memory is sharded, and each shard holds one or more namespaces. A query
//! targets a namespace; which shards should it search, and in what order?
//! [`NsRouter`] answers with a *connectivity* ranking built from observed
//! co-access: namespaces that are frequently accessed together (high edge
//! weight) are strongly connected, so a query into one should also reach the
//! shards of its close neighbours.
//!
//! Connectivity between two namespaces is the **widest-path** value (the
//! maximum over all paths of the minimum edge weight along the path) — a
//! min-cut-flavoured measure: it is exactly the bottleneck a path can carry,
//! and it collapses to `0` when the namespaces are in different connected
//! components, i.e. isolated namespaces never route.
//!
//! Ordering rules:
//! * the query's own shard always ranks first (self-connectivity `∞`);
//! * other shards rank by the best connectivity of any of their namespaces;
//! * ties break on the smaller [`ShardId`] so the order is deterministic;
//! * shards whose best connectivity is `0` are excluded.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

/// Identifier of a memory shard.
pub type ShardId = u32;

/// Namespace → shard routing table plus co-access weights.
#[derive(Debug, Clone, Default)]
pub struct NsRouter {
    /// namespace → shard.
    shard_of: BTreeMap<String, ShardId>,
    /// namespace → shard ids it has been co-accessed with → weight.
    co_access: BTreeMap<String, BTreeMap<String, f64>>,
}

impl NsRouter {
    /// An empty router.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register that `ns` lives in `shard` (last write wins).
    pub fn register(&mut self, ns: &str, shard: ShardId) {
        self.shard_of.insert(ns.to_string(), shard);
    }

    /// Record a symmetric co-access edge of `weight` between two namespaces.
    /// Non-positive weights are ignored. Weights combine by `max` so repeated
    /// observations of the same pair do not inflate the graph.
    pub fn co_access(&mut self, a: &str, b: &str, weight: f64) {
        if weight <= 0.0 || a == b {
            return;
        }
        let entry = self
            .co_access
            .entry(a.to_string())
            .or_default()
            .entry(b.to_string())
            .or_insert(0.0);
        if weight > *entry {
            *entry = weight;
        }
        let reverse = self
            .co_access
            .entry(b.to_string())
            .or_default()
            .entry(a.to_string())
            .or_insert(0.0);
        if weight > *reverse {
            *reverse = weight;
        }
    }

    /// The shard a namespace is registered to.
    #[must_use]
    pub fn shard_of(&self, ns: &str) -> Option<ShardId> {
        self.shard_of.get(ns).copied()
    }

    /// Widest-path connectivity from `from` to every reachable namespace.
    ///
    /// Uses a max-heap Dijkstra on the "maximin" semiring: a namespace's value
    /// is the best bottleneck path found so far. `from` itself scores `∞`.
    fn connectivity(&self, from: &str) -> BTreeMap<String, f64> {
        let mut best: BTreeMap<String, f64> = BTreeMap::new();
        best.insert(from.to_string(), f64::INFINITY);

        // Simple selection loop: repeatedly pick the unvisited namespace with
        // the highest known connectivity and relax its neighbours. Small
        // namespace graphs make the O(n²) form preferable to a heap (and it is
        // trivially deterministic).
        let mut done: BTreeSet<String> = BTreeSet::new();
        loop {
            let mut current: Option<(String, f64)> = None;
            for (ns, &score) in &best {
                if done.contains(ns) {
                    continue;
                }
                let better = current.as_ref().is_none_or(|(_, s)| score > *s);
                if better {
                    current = Some((ns.clone(), score));
                }
            }
            let Some((ns, score)) = current else { break };
            done.insert(ns.clone());
            if let Some(neighbours) = self.co_access.get(&ns) {
                for (nbr, &w) in neighbours {
                    let candidate = score.min(w);
                    let slot = best.entry(nbr.clone()).or_insert(0.0);
                    if candidate > *slot {
                        *slot = candidate;
                    }
                }
            }
        }
        best
    }

    /// Route a query for `query_ns`: return up to `k` shard ids ordered by
    /// connectivity (best first). An unregistered `query_ns` yields an empty
    /// list. Isolated shards (connectivity `0`) are excluded.
    #[must_use]
    pub fn route(&self, query_ns: &str, k: usize) -> Vec<ShardId> {
        if k == 0 {
            return Vec::new();
        }
        let Some(&self_shard) = self.shard_of.get(query_ns) else {
            return Vec::new();
        };

        let conn = self.connectivity(query_ns);

        // Best connectivity per shard.
        let mut by_shard: BTreeMap<ShardId, f64> = BTreeMap::new();
        for (ns, score) in &conn {
            if *score <= 0.0 {
                continue;
            }
            if let Some(&shard) = self.shard_of.get(ns) {
                let slot = by_shard.entry(shard).or_insert(f64::NEG_INFINITY);
                if *score > *slot {
                    *slot = *score;
                }
            }
        }

        // The query's own shard always ranks first.
        let self_score = by_shard.get(&self_shard).copied().unwrap_or(f64::INFINITY);
        by_shard.insert(self_shard, self_score.max(f64::INFINITY));

        let mut ranked: Vec<(ShardId, f64)> = by_shard.into_iter().collect();
        ranked.sort_by(|a, b| {
            b.1.total_cmp(&a.1) // higher connectivity first
                .then_with(|| a.0.cmp(&b.0)) // then smaller shard id
        });
        ranked.into_iter().take(k).map(|(s, _)| s).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_namespace_outranks_weak_and_isolated_is_excluded() {
        let mut r = NsRouter::new();
        r.register("q", 0);
        r.register("strong", 1);
        r.register("weak", 2);
        r.register("island", 3);

        r.co_access("q", "strong", 10.0);
        r.co_access("q", "weak", 1.0);
        // "island" has no edge at all.

        let route = r.route("q", 10);
        assert_eq!(route[0], 0, "query's own shard first");
        assert_eq!(route[1], 1, "strong before weak");
        assert_eq!(route[2], 2);
        assert!(!route.contains(&3), "isolated shard is not routed");
    }

    #[test]
    fn connectivity_flows_through_intermediates() {
        let mut r = NsRouter::new();
        r.register("q", 0);
        r.register("mid", 1);
        r.register("far", 2);
        r.co_access("q", "mid", 4.0);
        r.co_access("mid", "far", 9.0);

        // widest path q→mid→far is min(4, 9) = 4, so `far` still routes.
        let route = r.route("q", 10);
        assert_eq!(route, vec![0, 1, 2]);
    }

    #[test]
    fn tie_break_and_k_limit_are_deterministic() {
        let mut r = NsRouter::new();
        r.register("q", 5);
        r.register("a", 2);
        r.register("b", 1);
        r.co_access("q", "a", 3.0);
        r.co_access("q", "b", 3.0);
        // Equal connectivity: shard id breaks the tie (1 before 2).
        assert_eq!(r.route("q", 10), vec![5, 1, 2]);
        assert_eq!(r.route("q", 2), vec![5, 1]);
        assert_eq!(r.route("q", 0), Vec::<ShardId>::new());
        assert!(r.route("unknown", 10).is_empty());
    }

    #[test]
    fn co_access_is_symmetric_and_takes_the_max() {
        let mut r = NsRouter::new();
        r.register("a", 0);
        r.register("b", 1);
        r.co_access("a", "b", 2.0);
        r.co_access("b", "a", 7.0);
        r.co_access("a", "b", 1.0); // weaker, ignored
        // From either side the edge is 7.
        assert_eq!(r.route("a", 10), vec![0, 1]);
        assert_eq!(r.route("b", 10), vec![1, 0]);
        // Self-edges and non-positive weights are ignored.
        r.co_access("a", "a", 100.0);
        r.co_access("a", "b", -5.0);
        assert_eq!(r.route("a", 10), vec![0, 1]);
    }
}
