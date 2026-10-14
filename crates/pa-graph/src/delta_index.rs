
//! R10 — incremental ANN index maintenance with repair policies.
//!
//! An approximate-nearest-neighbour index (HNSW-shaped) degrades as vectors are
//! inserted and deleted: deleted nodes leave dead links, and inserts create
//! locally over-dense regions. Repairing after *every* mutation is wasteful;
//! never repairing loses recall. This module separates the two concerns:
//!
//! * [`IncrementalUpdater`] applies [`Delta`](crate::delta::Delta) mutations to
//!   an abstract ANN graph and records the disturbance.
//! * a repair step decides *when* to repair, under one of four policies:
//!   [`RepairPolicy::Lazy`] (only when recall drops below a floor),
//!   [`RepairPolicy::Eager`] (after every batch),
//!   [`RepairPolicy::Batched`] (every `N` mutations), and
//!   [`RepairPolicy::Adaptive`] (when the drift estimate crosses a threshold).
//! * [`QualityMonitor`] tracks a recall proxy and dead-link ratio so the
//!   policies have a signal to act on.
//!
//! The ANN graph here is deliberately abstract (adjacency lists of node ids,
//! no vectors): the point is the *repair policy*, which is identical whether
//! the neighbours were chosen by `HNSW`, `DiskANN`, or a plain k-NN graph.

use std::collections::{HashMap, HashSet};

/// How the index repairs itself after mutations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RepairPolicy {
    /// Repair only when recall drops below the floor.
    Lazy,
    /// Repair after every applied batch.
    Eager,
    /// Repair once every `n` mutations have accumulated.
    Batched {
        /// Mutations between repairs.
        n: usize,
    },
    /// Repair when the drift estimate crosses `threshold`.
    Adaptive {
        /// Drift (dead-link fraction + density skew) that triggers a repair.
        threshold: f32,
    },
}

impl RepairPolicy {
    /// Whether a repair is due, given the monitor and pending mutation count.
    #[must_use]
    pub fn should_repair(&self, monitor: &QualityMonitor, pending: usize) -> bool {
        match self {
            Self::Eager => pending > 0,
            Self::Batched { n } => *n > 0 && pending >= *n,
            Self::Lazy => monitor.recall_proxy() < monitor.recall_floor(),
            Self::Adaptive { threshold } => monitor.drift() >= *threshold,
        }
    }

    /// Stable wire name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Lazy => "lazy",
            Self::Eager => "eager",
            Self::Batched { .. } => "batched",
            Self::Adaptive { .. } => "adaptive",
        }
    }
}

/// Tracks index health: a recall proxy and dead-link ratio.
#[derive(Debug, Clone)]
pub struct QualityMonitor {
    /// Sum of per-node live-neighbour counts observed.
    live_links: usize,
    /// Total links (live + dead) observed.
    total_links: usize,
    /// Number of nodes the last sweep found reachable from a random probe.
    reachable: usize,
    /// Number of nodes probed.
    probed: usize,
    /// Recall floor below which [`RepairPolicy::Lazy`] repairs.
    recall_floor: f32,
}

impl Default for QualityMonitor {
    fn default() -> Self {
        Self {
            live_links: 0,
            total_links: 0,
            reachable: 0,
            probed: 0,
            recall_floor: 0.9,
        }
    }
}

impl QualityMonitor {
    /// A fresh monitor with the default recall floor (0.9).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the recall floor.
    #[must_use]
    pub fn with_recall_floor(mut self, floor: f32) -> Self {
        self.recall_floor = floor;
        self
    }

    /// The recall floor.
    #[must_use]
    pub fn recall_floor(&self) -> f32 {
        self.recall_floor
    }

    /// Fraction of links pointing at live nodes (`1.0` when no links seen).
    #[must_use]
    pub fn live_link_ratio(&self) -> f32 {
        if self.total_links == 0 {
            1.0
        } else {
            self.live_links as f32 / self.total_links as f32
        }
    }

    /// Fraction of probed nodes reachable (`1.0` when never probed).
    #[must_use]
    pub fn recall_proxy(&self) -> f32 {
        if self.probed == 0 {
            1.0
        } else {
            self.reachable as f32 / self.probed as f32
        }
    }

    /// A combined drift estimate: dead links plus unreachable mass.
    #[must_use]
    pub fn drift(&self) -> f32 {
        (1.0 - self.live_link_ratio()).max(0.0) + (1.0 - self.recall_proxy()).max(0.0)
    }

    /// Observe a node's link set: `live` links to live nodes out of `total`.
    pub fn observe_links(&mut self, live: usize, total: usize) {
        self.live_links += live;
        self.total_links += total;
    }

    /// Observe a reachability probe.
    pub fn observe_probe(&mut self, reachable: usize, probed: usize) {
        self.reachable += reachable;
        self.probed += probed;
    }
}

/// An abstract ANN graph: node id → neighbour ids.
#[derive(Debug, Clone, Default)]
pub struct AnnGraph {
    adjacency: HashMap<i64, Vec<i64>>,
    live: HashSet<i64>,
}

impl AnnGraph {
    /// An empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a node with no neighbours yet.
    pub fn insert(&mut self, id: i64) {
        self.adjacency.entry(id).or_default();
        self.live.insert(id);
    }

    /// Remove a node (its links become dead).
    pub fn remove(&mut self, id: i64) {
        self.live.remove(&id);
        self.adjacency.remove(&id);
    }

    /// Link `from` → `to` (both must exist).
    pub fn link(&mut self, from: i64, to: i64) {
        self.adjacency.entry(from).or_default().push(to);
    }

    /// Whether `id` is live.
    #[must_use]
    pub fn is_live(&self, id: i64) -> bool {
        self.live.contains(&id)
    }

    /// Number of live nodes.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    /// Number of stored adjacency entries.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.adjacency.values().map(Vec::len).sum()
    }

    /// Neighbours of `id`.
    #[must_use]
    pub fn neighbors(&self, id: i64) -> &[i64] {
        self.adjacency.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Every live node id.
    #[must_use]
    pub fn live_nodes(&self) -> Vec<i64> {
        let mut out: Vec<i64> = self.live.iter().copied().collect();
        out.sort_unstable();
        out
    }

    /// Count links to live nodes vs total links.
    #[must_use]
    pub fn link_health(&self) -> (usize, usize) {
        let mut live = 0;
        let mut total = 0;
        for neighbours in self.adjacency.values() {
            for n in neighbours {
                total += 1;
                if self.live.contains(n) {
                    live += 1;
                }
            }
        }
        (live, total)
    }

    /// Repair: drop dead links and add reciprocal links between mutually
    /// nearest live nodes (a cheap density fix).
    pub fn repair(&mut self) {
        // 1. Prune dead links.
        for neighbours in self.adjacency.values_mut() {
            neighbours.retain(|n| self.live.contains(n));
            neighbours.sort_unstable();
            neighbours.dedup();
        }
        // 2. Reconnect isolated live nodes to their nearest live peer (lowest
        //    id) so no node is unreachable.
        let live: Vec<i64> = self.live_nodes();
        if live.len() < 2 {
            return;
        }
        for &id in &live {
            let empty = self.adjacency.get(&id).is_none_or(Vec::is_empty);
            if empty {
                let peer = live.iter().copied().find(|&p| p != id);
                if let Some(peer) = peer {
                    self.adjacency.entry(id).or_default().push(peer);
                }
            }
        }
    }
}

/// Applies deltas to an [`AnnGraph`] and decides when to repair.
#[derive(Debug, Clone)]
pub struct IncrementalUpdater {
    policy: RepairPolicy,
    monitor: QualityMonitor,
    pending: usize,
    repairs: usize,
}

impl IncrementalUpdater {
    /// A new updater with `policy`.
    #[must_use]
    pub fn new(policy: RepairPolicy) -> Self {
        Self {
            policy,
            monitor: QualityMonitor::new(),
            pending: 0,
            repairs: 0,
        }
    }

    /// The policy in force.
    #[must_use]
    pub fn policy(&self) -> RepairPolicy {
        self.policy
    }

    /// The monitor.
    #[must_use]
    pub fn monitor(&self) -> &QualityMonitor {
        &self.monitor
    }

    /// Number of repairs performed.
    #[must_use]
    pub fn repairs(&self) -> usize {
        self.repairs
    }

    /// Mutations since the last repair.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending
    }

    /// Insert a node, recording one pending mutation.
    pub fn insert(&mut self, graph: &mut AnnGraph, id: i64) {
        graph.insert(id);
        self.pending += 1;
    }

    /// Remove a node, recording one pending mutation.
    pub fn remove(&mut self, graph: &mut AnnGraph, id: i64) {
        graph.remove(id);
        self.pending += 1;
    }

    /// Refresh the monitor from the graph, then repair if the policy is due.
    /// Returns `true` when a repair ran.
    pub fn maybe_repair(&mut self, graph: &mut AnnGraph) -> bool {
        let (live, total) = graph.link_health();
        self.monitor.observe_links(live, total);
        // Reachability probe: BFS from the first live node.
        let nodes = graph.live_nodes();
        if let Some(&start) = nodes.first() {
            let reached = reachable_count(graph, start);
            self.monitor.observe_probe(reached, nodes.len());
        }
        if self.policy.should_repair(&self.monitor, self.pending) {
            graph.repair();
            self.pending = 0;
            self.repairs += 1;
            return true;
        }
        false
    }
}

/// BFS reachable-node count from `start`.
fn reachable_count(graph: &AnnGraph, start: i64) -> usize {
    let mut seen: HashSet<i64> = HashSet::new();
    let mut stack = vec![start];
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        for nbr in graph.neighbors(n) {
            if graph.is_live(*nbr) && !seen.contains(nbr) {
                stack.push(*nbr);
            }
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(n: i64) -> AnnGraph {
        let mut g = AnnGraph::new();
        for i in 0..n {
            g.insert(i);
        }
        for i in 0..n - 1 {
            g.link(i, i + 1);
            g.link(i + 1, i);
        }
        g
    }

    #[test]
    fn eager_repairs_after_every_batch() {
        let mut g = chain(4);
        let mut up = IncrementalUpdater::new(RepairPolicy::Eager);
        up.remove(&mut g, 2);
        assert!(up.maybe_repair(&mut g), "eager must repair immediately");
        assert_eq!(up.repairs(), 1);
        assert_eq!(up.pending(), 0);
    }

    #[test]
    fn batched_waits_for_n_mutations() {
        let mut g = chain(6);
        let mut up = IncrementalUpdater::new(RepairPolicy::Batched { n: 3 });
        up.remove(&mut g, 5);
        assert!(!up.maybe_repair(&mut g), "one mutation < n");
        up.remove(&mut g, 4);
        assert!(!up.maybe_repair(&mut g), "two mutations < n");
        up.remove(&mut g, 3);
        assert!(up.maybe_repair(&mut g), "three mutations == n");
        assert_eq!(up.repairs(), 1);
    }

    #[test]
    fn lazy_repairs_only_when_recall_drops() {
        // Healthy graph: lazy does not repair.
        let mut g = chain(5);
        let mut up = IncrementalUpdater::new(RepairPolicy::Lazy);
        assert!(!up.maybe_repair(&mut g), "healthy graph needs no repair");
        assert_eq!(up.repairs(), 0);

        // Remove the bridge node: the chain splits and reachability drops, so
        // the recall proxy falls below the floor and a repair fires.
        up.remove(&mut g, 2);
        assert!(
            up.maybe_repair(&mut g),
            "disconnected graph must trigger a lazy repair"
        );
        assert_eq!(up.repairs(), 1);
    }

    #[test]
    fn adaptive_repairs_when_drift_crosses_threshold() {
        let mut g = chain(4);
        let mut up = IncrementalUpdater::new(RepairPolicy::Adaptive { threshold: 0.01 });
        up.remove(&mut g, 2);
        assert!(up.maybe_repair(&mut g), "dead links create drift ≥ 0.01");
    }

    #[test]
    fn repair_prunes_dead_links() {
        let mut g = chain(4);
        g.remove(2);
        let (live, total) = g.link_health();
        assert!(live < total, "dead links to 2 exist before repair");
        g.repair();
        let (live, total) = g.link_health();
        assert_eq!(live, total, "all links live after repair");
        assert!(!g.neighbors(1).contains(&2));
    }

    #[test]
    fn repair_reconnects_isolated_nodes() {
        let mut g = AnnGraph::new();
        g.insert(1);
        g.insert(2);
        g.insert(3);
        // 3 is isolated.
        g.link(1, 2);
        g.repair();
        assert!(!g.neighbors(3).is_empty(), "isolated node must be reconnected");
    }

    #[test]
    fn monitor_drift_and_recall_proxy() {
        let mut m = QualityMonitor::new();
        assert_eq!(m.recall_proxy(), 1.0);
        assert_eq!(m.live_link_ratio(), 1.0);
        m.observe_links(3, 4);
        m.observe_probe(5, 10);
        assert!((m.live_link_ratio() - 0.75).abs() < 1e-6);
        assert!((m.recall_proxy() - 0.5).abs() < 1e-6);
        assert!(m.drift() > 0.5);
    }

    #[test]
    fn policy_names_are_stable() {
        assert_eq!(RepairPolicy::Lazy.name(), "lazy");
        assert_eq!(RepairPolicy::Eager.name(), "eager");
        assert_eq!(RepairPolicy::Batched { n: 5 }.name(), "batched");
        assert_eq!(RepairPolicy::Adaptive { threshold: 0.5 }.name(), "adaptive");
    }

    #[test]
    fn insert_and_remove_track_live_count() {
        let mut g = AnnGraph::new();
        let mut up = IncrementalUpdater::new(RepairPolicy::Batched { n: 100 });
        up.insert(&mut g, 1);
        up.insert(&mut g, 2);
        assert_eq!(g.live_count(), 2);
        up.remove(&mut g, 1);
        assert_eq!(g.live_count(), 1);
        assert_eq!(up.pending(), 3);
    }
}
