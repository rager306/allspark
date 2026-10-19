//! R11 — drifting index: detect neighbour-set drift and queue repairs.
//!
//! A static `ANN` graph is a snapshot of neighbourhoods that were optimal when
//! it was built. As the underlying vectors shift (new documents, updated
//! embeddings), a node's *true* nearest neighbours change while its stored
//! adjacency does not — the index *drifts*. [`DriftMonitor`] keeps a per-node
//! snapshot of its neighbours and a version counter; [`check`](DriftMonitor::check)
//! compares the snapshot against freshly computed neighbours and, when the
//! Jaccard distance exceeds the threshold, enqueues the node for repair. After
//! [`repair`](DriftMonitor::repair) the snapshot is refreshed and the node
//! leaves the queue.
//!
//! *When* a drifted node is enqueued is governed by the crate's existing
//! [`RepairPolicy`] (R10), reinterpreted for drift:
//!
//! * [`RepairPolicy::Eager`] — enqueue the node on the first drift over threshold;
//! * [`RepairPolicy::Batched`] — enqueue once `n` drifted nodes have accumulated;
//! * [`RepairPolicy::Lazy`] — enqueue only when the *fraction* of observed nodes
//!   currently drifted exceeds the threshold (defer until the index is broadly off);
//! * [`RepairPolicy::Adaptive`] — use the policy's own threshold for the node.

use std::collections::{BTreeMap, BTreeSet};

use crate::delta_index::RepairPolicy;

/// Jaccard distance between two neighbour sets: `1 − |A ∩ B| / |A ∪ B|`,
/// in `[0, 1]`. Two empty sets are identical (`0.0`).
#[must_use]
pub fn jaccard_distance(a: &[i64], b: &[i64]) -> f32 {
    let set_a: BTreeSet<i64> = a.iter().copied().collect();
    let set_b: BTreeSet<i64> = b.iter().copied().collect();
    if set_a.is_empty() && set_b.is_empty() {
        return 0.0;
    }
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        0.0
    } else {
        1.0 - (intersection as f32 / union as f32)
    }
}

/// Watches neighbour-set drift across an `ANN` index.
#[derive(Debug, Clone)]
pub struct DriftMonitor {
    threshold: f32,
    policy: RepairPolicy,
    snapshots: BTreeMap<i64, Vec<i64>>,
    observed: BTreeMap<i64, Vec<i64>>,
    versions: BTreeMap<i64, u32>,
    queue: BTreeSet<i64>,
    pending_drifted: BTreeSet<i64>,
}

impl Default for DriftMonitor {
    fn default() -> Self {
        Self {
            threshold: 0.3,
            policy: RepairPolicy::Eager,
            snapshots: BTreeMap::new(),
            observed: BTreeMap::new(),
            versions: BTreeMap::new(),
            queue: BTreeSet::new(),
            pending_drifted: BTreeSet::new(),
        }
    }
}

impl DriftMonitor {
    /// A monitor with the default drift threshold (0.3) and
    /// [`RepairPolicy::Eager`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the drift threshold (clamped to `[0, 1]`).
    #[must_use]
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set the repair policy.
    #[must_use]
    pub fn with_policy(mut self, policy: RepairPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The drift threshold.
    #[must_use]
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// The repair policy.
    #[must_use]
    pub fn policy(&self) -> RepairPolicy {
        self.policy
    }

    /// Record the baseline neighbour snapshot for `node`. Bumps the node's
    /// version and clears any queued repair for it.
    pub fn snapshot(&mut self, node: i64, neighbors: &[i64]) {
        let mut sorted = neighbors.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.retain(|n| *n != node);
        self.snapshots.insert(node, sorted);
        *self.versions.entry(node).or_insert(0) += 1;
        self.queue.remove(&node);
        self.pending_drifted.remove(&node);
    }

    /// The stored snapshot for `node`, if any.
    #[must_use]
    pub fn snapshot_of(&self, node: i64) -> Option<&[i64]> {
        self.snapshots.get(&node).map(Vec::as_slice)
    }

    /// The snapshot version for `node` (0 if never snapshotted).
    #[must_use]
    pub fn version(&self, node: i64) -> u32 {
        self.versions.get(&node).copied().unwrap_or(0)
    }

    /// Drift of `node` against `current` (0.0 when no snapshot exists).
    ///
    /// `current` is normalized the same way a snapshot is (sorted, de-duplicated,
    /// self-links dropped) before comparison, so a caller can pass raw
    /// neighbour lists.
    #[must_use]
    pub fn drift_of(&self, node: i64, current: &[i64]) -> f32 {
        match self.snapshots.get(&node) {
            Some(snapshot) => {
                let mut observed = current.to_vec();
                observed.sort_unstable();
                observed.dedup();
                observed.retain(|n| *n != node);
                jaccard_distance(snapshot, &observed)
            }
            None => 0.0,
        }
    }

    /// Compare freshly computed neighbours for `node` against its snapshot and
    /// apply the policy. Returns `true` when `node` is now in the repair queue.
    pub fn check(&mut self, node: i64, current: &[i64]) -> bool {
        let mut observed = current.to_vec();
        observed.sort_unstable();
        observed.dedup();
        observed.retain(|n| *n != node);
        self.observed.insert(node, observed.clone());

        let threshold = match self.policy {
            RepairPolicy::Adaptive { threshold } => threshold,
            _ => self.threshold,
        };
        let drift = self.drift_of(node, &observed);
        if drift <= threshold {
            return self.queue.contains(&node);
        }

        match self.policy {
            RepairPolicy::Eager | RepairPolicy::Adaptive { .. } => {
                self.queue.insert(node);
            }
            RepairPolicy::Batched { n } => {
                self.pending_drifted.insert(node);
                if n == 0 || self.pending_drifted.len() >= n {
                    self.queue.extend(self.pending_drifted.iter().copied());
                    self.pending_drifted.clear();
                }
            }
            RepairPolicy::Lazy => {
                if self.drifted_fraction() > threshold {
                    // The index is broadly off; queue every drifted node seen.
                    for drifted in self.drifted_observed() {
                        self.queue.insert(drifted);
                    }
                }
            }
        }
        self.queue.contains(&node)
    }

    /// Nodes observed since the last snapshot whose neighbours exceed the
    /// threshold.
    fn drifted_observed(&self) -> Vec<i64> {
        self.observed
            .iter()
            .filter(|(node, current)| self.drift_of(**node, current) > self.threshold)
            .map(|(node, _)| *node)
            .collect()
    }

    /// Fraction of the monitored node set that has drifted. The denominator is
    /// the number of snapshotted nodes, so a single early observation cannot
    /// make the fraction look maximal.
    fn drifted_fraction(&self) -> f32 {
        if self.snapshots.is_empty() {
            return 0.0;
        }
        self.drifted_observed().len() as f32 / self.snapshots.len() as f32
    }

    /// Repair `node`: refresh its snapshot to the last observed neighbours,
    /// bump its version, and drop it from the queue.
    pub fn repair(&mut self, node: i64) {
        if let Some(observed) = self.observed.get(&node).cloned() {
            self.snapshots.insert(node, observed);
        }
        *self.versions.entry(node).or_insert(0) += 1;
        self.queue.remove(&node);
        self.pending_drifted.remove(&node);
    }

    /// Repair every queued node, refreshing each snapshot from its last
    /// observation. Afterwards the queue is empty.
    pub fn repair_all(&mut self) {
        let queued: Vec<i64> = self.queue.iter().copied().collect();
        for node in queued {
            self.repair(node);
        }
    }

    /// The current repair queue (ascending).
    #[must_use]
    pub fn repair_queue(&self) -> Vec<i64> {
        self.queue.iter().copied().collect()
    }

    /// Number of nodes awaiting repair.
    #[must_use]
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Whether `node` is queued.
    #[must_use]
    pub fn is_queued(&self, node: i64) -> bool {
        self.queue.contains(&node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neighbors(n: i64) -> Vec<i64> {
        (0..n).collect()
    }

    #[test]
    fn thirty_percent_change_queues_the_node() {
        let mut monitor = DriftMonitor::new().with_threshold(0.3);
        let base = neighbors(10);
        monitor.snapshot(7, &base);
        // Replace 3 of the 10 neighbours → Jaccard distance ≈ 0.46 > 0.3.
        let mut changed = base.clone();
        changed[7] = 100;
        changed[8] = 101;
        changed[9] = 102;
        assert!(monitor.check(7, &changed), "30% change must queue the node");
        assert!(monitor.is_queued(7));
        assert_eq!(monitor.repair_queue(), vec![7]);
    }

    #[test]
    fn repair_empties_the_queue() {
        let mut monitor = DriftMonitor::new().with_threshold(0.3);
        let base = neighbors(10);
        monitor.snapshot(7, &base);
        let mut changed = base.clone();
        changed[7] = 100;
        changed[8] = 101;
        changed[9] = 102;
        assert!(monitor.check(7, &changed));
        assert_eq!(monitor.queue_len(), 1);
        let before = monitor.version(7);
        monitor.repair(7);
        assert_eq!(
            monitor.repair_queue(),
            Vec::<i64>::new(),
            "queue drained after repair"
        );
        assert_eq!(monitor.version(7), before + 1, "repair bumps the version");
        // After repair the refreshed snapshot matches, so a recheck is clean.
        assert!(
            !monitor.check(7, &changed),
            "repaired node no longer drifts"
        );
    }

    #[test]
    fn identical_neighbors_do_not_drift() {
        let mut monitor = DriftMonitor::new();
        monitor.snapshot(1, &[1, 2, 3]);
        assert!(!monitor.check(1, &[1, 2, 3]), "no change → no drift");
        assert_eq!(monitor.repair_queue(), Vec::<i64>::new());
        assert_eq!(monitor.drift_of(1, &[1, 2, 3]).to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn unknown_node_has_zero_drift() {
        let monitor = DriftMonitor::new();
        assert_eq!(
            monitor.drift_of(42, &[1, 2, 3]).to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(monitor.version(42), 0);
    }

    #[test]
    fn batched_policy_defers_until_n_drifted() {
        let mut monitor = DriftMonitor::new().with_policy(RepairPolicy::Batched { n: 3 });
        for node in 0..5 {
            monitor.snapshot(node, &[10, 11, 12]);
        }
        // Three nodes drift; the batch fills on the third.
        assert!(!monitor.check(0, &[20, 21, 22]));
        assert!(!monitor.check(1, &[20, 21, 22]));
        assert!(
            monitor.check(2, &[20, 21, 22]),
            "third drift fills the batch"
        );
        assert_eq!(monitor.repair_queue(), vec![0, 1, 2]);
    }

    #[test]
    fn lazy_policy_needs_broad_drift() {
        let mut monitor = DriftMonitor::new()
            .with_policy(RepairPolicy::Lazy)
            .with_threshold(0.3);
        for node in 0..10 {
            monitor.snapshot(node, &[10, 11, 12, 13]);
        }
        // One node drifts out of ten → 10% < 30% threshold → deferred.
        assert!(!monitor.check(0, &[20, 21, 22, 23]));
        assert_eq!(
            monitor.repair_queue(),
            Vec::<i64>::new(),
            "broad drift not reached yet"
        );
        // Drive the majority over the line.
        for node in 1..6 {
            monitor.check(node, &[20, 21, 22, 23]);
        }
        assert_ne!(
            monitor.repair_queue(),
            Vec::<i64>::new(),
            "majority drift triggers"
        );
    }

    #[test]
    fn repair_all_clears_every_queued_node() {
        let mut monitor = DriftMonitor::new();
        for node in 0..4 {
            monitor.snapshot(node, &[1, 2, 3]);
            monitor.check(node, &[7, 8, 9]);
        }
        assert_eq!(monitor.queue_len(), 4);
        monitor.repair_all();
        assert_eq!(monitor.queue_len(), 0);
    }

    #[test]
    fn jaccard_distance_endpoints() {
        assert_eq!(jaccard_distance(&[], &[]).to_bits(), 0.0_f32.to_bits());
        assert_eq!(
            jaccard_distance(&[1, 2], &[1, 2]).to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(
            jaccard_distance(&[1, 2], &[3, 4]).to_bits(),
            1.0_f32.to_bits()
        );
        assert!((jaccard_distance(&[1, 2, 3], &[1, 2, 4]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn snapshot_drops_self_links_and_dedups() {
        let mut monitor = DriftMonitor::new();
        monitor.snapshot(1, &[1, 2, 2, 3]);
        assert_eq!(monitor.snapshot_of(1), Some(&[2, 3][..]));
    }
}
