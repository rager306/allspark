//! R20 — mcp-brain: a shared, in-process collective memory.
//!
//! A "brain" is memory that outlives one agent: knowledge one agent learns is
//! contributed, ranked by the group, and made available to the others. The
//! reference implementation is a networked MCP service; here the same
//! operations are an in-process store behind `Arc<RwLock<…>>`, so several
//! agents in one process share one brain with no network at all.
//!
//! The operations map to the reference's tools:
//!
//! - [`SharedBrain::share`] — contribute a memory (embedding-free: a simple
//!   term-vector is used for search).
//! - [`SharedBrain::search`] — rank memories against a query by term overlap
//!   and quality.
//! - [`SharedBrain::vote`] — up/down vote, folded into a Bayesian quality
//!   score so one vote cannot dominate.
//! - [`SharedBrain::transfer`] — move a memory into another scope.
//! - [`SharedBrain::drift`] — flag memories whose scope's quality has moved
//!   away from its long-run mean.
//! - [`SharedBrain::partition`] — group memories by scope with a coherence
//!   score.
//!
//! ```no_run
//! use pa_graph::SharedBrain;
//!
//! let brain = SharedBrain::new();
//! let id = brain.share("team", "the parser rejects tabs");
//! let _ = brain.vote(&id, true);
//! let hits = brain.search("parser tabs", 5);
//! # let _ = hits;
//! ```

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A single shared memory.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainEntry {
    /// Stable identifier, assigned on [`SharedBrain::share`].
    pub id: String,
    /// The scope (team/agent/project) that owns this memory.
    pub scope: String,
    /// The knowledge text.
    pub content: String,
    /// Up votes.
    pub up: u32,
    /// Down votes.
    pub down: u32,
    /// Monotonic insertion sequence (used to break quality ties stably).
    pub seq: u64,
}

impl BrainEntry {
    /// The Bayesian quality score in `[0, 1]` (Laplace-smoothed up fraction).
    ///
    /// With no votes the score is `0.5` — an unvoted memory is neither trusted
    /// nor distrusted. Each vote moves it, but never past the smoothed bounds,
    /// so a single vote cannot drive a memory to a perfect or zero score.
    #[must_use]
    pub fn quality(&self) -> f64 {
        let up = f64::from(self.up);
        let total = f64::from(self.up + self.down);
        (up + 1.0) / (total + 2.0)
    }

    /// Total votes cast on this memory.
    #[must_use]
    pub fn votes(&self) -> u32 {
        self.up + self.down
    }
}

/// One ranked search hit.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainHit {
    /// The memory.
    pub entry: BrainEntry,
    /// Number of distinct query terms matched.
    pub score: usize,
}

/// A drift finding for one scope.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftFinding {
    /// The scope that drifted.
    pub scope: String,
    /// The scope's mean quality.
    pub mean_quality: f64,
    /// The scope's quality standard deviation.
    pub std_dev: f64,
    /// The coefficient of variation (`std_dev / mean_quality`).
    pub coefficient_of_variation: f64,
    /// Whether the variation exceeds the drift threshold.
    pub drifting: bool,
    /// How many memories the scope holds.
    pub memories: usize,
}

/// One knowledge partition (a scope cluster).
#[derive(Debug, Clone, PartialEq)]
pub struct BrainPartition {
    /// The scope.
    pub scope: String,
    /// Ids of the memories in this partition.
    pub ids: Vec<String>,
    /// Number of memories.
    pub size: usize,
    /// Mean quality of the partition's memories.
    pub mean_quality: f64,
    /// 1 − normalised quality variance; higher is more coherent.
    pub coherence: f64,
}

/// A shared collective memory backed by `Arc<RwLock<…>>`.
///
/// Cloning a [`SharedBrain`] shares the same store, so it can be handed to
/// several agents and every one of them sees the others' contributions.
#[derive(Debug, Clone)]
pub struct SharedBrain {
    inner: Arc<RwLock<BrainState>>,
    threshold: f64,
}

#[derive(Debug, Default)]
struct BrainState {
    entries: BTreeMap<String, BrainEntry>,
    seq: u64,
    next_id: u64,
}

fn read(state: &RwLock<BrainState>) -> RwLockReadGuard<'_, BrainState> {
    state
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write(state: &RwLock<BrainState>) -> RwLockWriteGuard<'_, BrainState> {
    state
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Default for SharedBrain {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedBrain {
    /// A new, empty brain with the default drift threshold (0.5).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(BrainState::default())),
            threshold: 0.5,
        }
    }

    /// A new brain with an explicit drift coefficient-of-variation threshold.
    #[must_use]
    pub fn with_drift_threshold(threshold: f64) -> Self {
        Self {
            inner: Arc::new(RwLock::new(BrainState::default())),
            threshold,
        }
    }

    /// Contribute `content` to `scope`, returning the new memory's id.
    #[must_use]
    pub fn share(&self, scope: &str, content: &str) -> String {
        let mut state = write(&self.inner);
        let id = format!("mem-{}", state.next_id);
        state.next_id += 1;
        state.seq += 1;
        let seq = state.seq;
        state.entries.insert(
            id.clone(),
            BrainEntry {
                id: id.clone(),
                scope: scope.to_string(),
                content: content.to_string(),
                up: 0,
                down: 0,
                seq,
            },
        );
        id
    }

    /// Vote `up` (true) or down (false) on `id`. Returns `false` for no such id.
    #[must_use]
    pub fn vote(&self, id: &str, up: bool) -> bool {
        let mut state = write(&self.inner);
        match state.entries.get_mut(id) {
            Some(entry) => {
                if up {
                    entry.up += 1;
                } else {
                    entry.down += 1;
                }
                true
            }
            None => false,
        }
    }

    /// Rank memories against `query`, best first, at most `limit` results.
    ///
    /// A memory scores by how many distinct query terms its content contains,
    /// with quality as a tie-breaker (and insertion order below that), so the
    /// ranking is fully deterministic.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<BrainHit> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|t| !t.is_empty())
            .collect();
        if terms.is_empty() || limit == 0 {
            return Vec::new();
        }
        let state = read(&self.inner);
        let mut hits: Vec<BrainHit> = state
            .entries
            .values()
            .filter_map(|entry| {
                let lower = entry.content.to_lowercase();
                let score = terms.iter().filter(|t| lower.contains(t.as_str())).count();
                (score > 0).then(|| BrainHit {
                    entry: entry.clone(),
                    score,
                })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| {
                    b.entry
                        .quality()
                        .partial_cmp(&a.entry.quality())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.entry.seq.cmp(&b.entry.seq))
        });
        hits.truncate(limit);
        hits
    }

    /// Fetch one memory by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<BrainEntry> {
        read(&self.inner).entries.get(id).cloned()
    }

    /// Move the memory `id` into `to_scope`. Returns `false` for no such id.
    #[must_use]
    pub fn transfer(&self, id: &str, to_scope: &str) -> bool {
        let mut state = write(&self.inner);
        match state.entries.get_mut(id) {
            Some(entry) => {
                entry.scope = to_scope.to_string();
                true
            }
            None => false,
        }
    }

    /// Total number of memories in the brain.
    #[must_use]
    pub fn len(&self) -> usize {
        read(&self.inner).entries.len()
    }

    /// Whether the brain is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        read(&self.inner).entries.is_empty()
    }

    /// Every memory in the brain, in insertion order.
    #[must_use]
    pub fn entries(&self) -> Vec<BrainEntry> {
        let state = read(&self.inner);
        let mut entries: Vec<BrainEntry> = state.entries.values().cloned().collect();
        entries.sort_by_key(|e| e.seq);
        entries
    }

    /// Detect drift: scopes whose quality varies more than the threshold.
    ///
    /// For each scope the coefficient of variation (`σ / μ`) of member quality
    /// scores is computed; a scope whose CV exceeds the threshold is reported
    /// as drifting. A scope with fewer than two memories, or with a zero mean,
    /// cannot drift and is reported with `drifting: false`.
    #[must_use]
    pub fn drift(&self) -> Vec<DriftFinding> {
        let mut by_scope: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for entry in read(&self.inner).entries.values() {
            by_scope
                .entry(entry.scope.clone())
                .or_default()
                .push(entry.quality());
        }
        let mut findings = Vec::new();
        for (scope, qualities) in by_scope {
            let n = qualities.len();
            let mean = qualities.iter().sum::<f64>() / n as f64;
            let variance = qualities.iter().map(|q| (q - mean).powi(2)).sum::<f64>() / n as f64;
            let std_dev = variance.sqrt();
            let cv = if mean > 0.0 { std_dev / mean } else { 0.0 };
            findings.push(DriftFinding {
                scope,
                mean_quality: mean,
                std_dev,
                coefficient_of_variation: cv,
                drifting: n >= 2 && cv > self.threshold,
                memories: n,
            });
        }
        findings
    }

    /// Partition the brain by scope, with a coherence score per partition.
    ///
    /// Coherence is `1 − min(variance / 0.25, 1)`: identical qualities give 1,
    /// and the maximum possible variance of a `[0,1]`-bounded score (0.25)
    /// gives 0. Partitions are returned in scope-name order.
    #[must_use]
    pub fn partition(&self) -> Vec<BrainPartition> {
        let mut by_scope: BTreeMap<String, Vec<BrainEntry>> = BTreeMap::new();
        for entry in read(&self.inner).entries.values() {
            by_scope
                .entry(entry.scope.clone())
                .or_default()
                .push(entry.clone());
        }
        let mut partitions = Vec::new();
        for (scope, mut entries) in by_scope {
            entries.sort_by_key(|e| e.seq);
            let qualities: Vec<f64> = entries.iter().map(BrainEntry::quality).collect();
            let mean = qualities.iter().sum::<f64>() / qualities.len() as f64;
            let variance =
                qualities.iter().map(|q| (q - mean).powi(2)).sum::<f64>() / qualities.len() as f64;
            let coherence = 1.0 - (variance / 0.25).min(1.0);
            partitions.push(BrainPartition {
                scope,
                ids: entries.iter().map(|e| e.id.clone()).collect(),
                size: entries.len(),
                mean_quality: mean,
                coherence,
            });
        }
        partitions
    }

    /// Aggregate counts by scope.
    #[must_use]
    pub fn scope_counts(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for entry in read(&self.inner).entries.values() {
            *counts.entry(entry.scope.clone()).or_insert(0) += 1;
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_and_get_round_trip() {
        let brain = SharedBrain::new();
        let id = brain.share("team", "the parser rejects tabs");
        let entry = brain.get(&id).expect("entry");
        assert_eq!(entry.scope, "team");
        assert_eq!(entry.content, "the parser rejects tabs");
        assert_eq!(entry.quality(), 0.5, "unvoted memory is neutral");
    }

    #[test]
    fn clones_share_one_store() {
        let brain = SharedBrain::new();
        let clone = brain.clone();
        let _ = clone.share("team", "shared knowledge");
        assert_eq!(brain.len(), 1, "a clone writes into the same brain");
    }

    #[test]
    fn voting_moves_quality_within_bounds() {
        let brain = SharedBrain::new();
        let id = brain.share("team", "x");
        let _ = brain.vote(&id, true);
        let q1 = brain.get(&id).expect("entry").quality();
        assert!(
            q1 > 0.5 && q1 < 1.0,
            "one up vote lifts but does not saturate"
        );
        let _ = brain.vote(&id, false);
        let _ = brain.vote(&id, false);
        let q2 = brain.get(&id).expect("entry").quality();
        assert!(q2 < 0.5, "down votes lower the score");
        assert!(!brain.vote("missing", true));
    }

    #[test]
    fn search_ranks_by_term_overlap_then_quality() {
        let brain = SharedBrain::new();
        let weak = brain.share("team", "only parser here");
        let strong = brain.share("team", "parser and tabs together");
        let hits = brain.search("parser tabs", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].entry.id, strong);
        assert_eq!(hits[0].score, 2);
        assert_eq!(hits[1].entry.id, weak);
        // An up vote on the weak one does not let it outrank the two-term hit.
        let _ = brain.vote(&weak, true);
        let hits = brain.search("parser tabs", 10);
        assert_eq!(hits[0].entry.id, strong);
    }

    #[test]
    fn transfer_moves_between_scopes() {
        let brain = SharedBrain::new();
        let id = brain.share("team-a", "knowledge");
        assert!(brain.transfer(&id, "team-b"));
        assert_eq!(brain.get(&id).expect("entry").scope, "team-b");
        assert!(!brain.transfer("missing", "team-b"));
    }

    #[test]
    fn partition_groups_by_scope_with_coherence() {
        let brain = SharedBrain::new();
        let a1 = brain.share("team-a", "one");
        let a2 = brain.share("team-a", "two");
        let _ = brain.share("team-b", "three");
        // Both team-a memories get identical votes -> identical quality -> full
        // coherence.
        let _ = brain.vote(&a1, true);
        let _ = brain.vote(&a2, true);
        let parts = brain.partition();
        assert_eq!(parts.len(), 2);
        let team_a = parts.iter().find(|p| p.scope == "team-a").expect("a");
        assert_eq!(team_a.size, 2);
        assert!(
            (team_a.coherence - 1.0).abs() < 1e-9,
            "identical qualities are coherent"
        );
        assert_eq!(team_a.ids, vec![a1, a2]);
        let team_b = parts.iter().find(|p| p.scope == "team-b").expect("b");
        assert_eq!(team_b.size, 1);
    }

    #[test]
    fn drift_flags_scopes_with_variable_quality() {
        let brain = SharedBrain::with_drift_threshold(0.3);
        // A stable scope: every memory has the same quality.
        let s1 = brain.share("stable", "a");
        let s2 = brain.share("stable", "b");
        let _ = brain.vote(&s1, true);
        let _ = brain.vote(&s2, true);
        // A drifting scope: one highly-voted, one down-voted.
        let d1 = brain.share("drifty", "c");
        let d2 = brain.share("drifty", "d");
        for _ in 0..5 {
            let _ = brain.vote(&d1, true);
        }
        let _ = brain.vote(&d2, false);

        let findings = brain.drift();
        let stable = findings
            .iter()
            .find(|f| f.scope == "stable")
            .expect("stable");
        assert!(
            !stable.drifting,
            "uniform quality does not drift: {stable:?}"
        );
        let drifty = findings
            .iter()
            .find(|f| f.scope == "drifty")
            .expect("drifty");
        assert!(drifty.drifting, "varied quality drifts: {drifty:?}");
        assert!(drifty.coefficient_of_variation > 0.3);
    }

    #[test]
    fn a_single_memory_cannot_drift() {
        let brain = SharedBrain::with_drift_threshold(0.0);
        let _ = brain.share("solo", "only");
        let finding = &brain.drift()[0];
        assert!(!finding.drifting, "one memory has no variation to measure");
    }

    #[test]
    fn empty_brain_operations_are_safe() {
        let brain = SharedBrain::new();
        assert!(brain.is_empty());
        assert_eq!(brain.search("anything", 5).len(), 0);
        assert_eq!(brain.partition().len(), 0);
        assert_eq!(brain.drift().len(), 0);
        assert!(brain.get("nope").is_none());
    }

    #[test]
    fn concurrent_shares_are_all_recorded() {
        use std::thread;
        let brain = SharedBrain::new();
        let mut handles = Vec::new();
        for t in 0..4 {
            let brain = brain.clone();
            handles.push(thread::spawn(move || {
                for i in 0..25 {
                    let _ = brain.share(&format!("scope-{t}"), &format!("item {i}"));
                }
            }));
        }
        for handle in handles {
            handle.join().expect("join");
        }
        assert_eq!(brain.len(), 100);
        assert_eq!(brain.scope_counts().len(), 4);
    }

    #[test]
    fn entries_are_returned_in_insertion_order() {
        let brain = SharedBrain::new();
        let a = brain.share("s", "first");
        let b = brain.share("s", "second");
        let ids: Vec<String> = brain.entries().iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids, vec![a, b]);
    }
}
