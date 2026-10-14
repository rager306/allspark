//! M4 — `TARL` trajectory-aware reward ledger (ruvector `agent-memory`).
//!
//! A ledger of quality signals attached to agent actions. Each
//! [`TarlRecord`] carries the action, a scalar reward, a timestamp, and an
//! [`Outcome`] (`Accepted` / `Pending` / `Rejected`). [`TarlLedger`] appends
//! records and aggregates them with a plain mean and an exponential moving
//! average (EMA) so a consumer can read either a whole-run summary or a
//! recency-weighted one.

/// The acceptance state of a recorded action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The action was accepted.
    Accepted,
    /// The action is still pending review.
    Pending,
    /// The action was rejected.
    Rejected,
}

impl Outcome {
    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Pending => "pending",
            Self::Rejected => "rejected",
        }
    }
}

/// One recorded reward signal.
#[derive(Debug, Clone, PartialEq)]
pub struct TarlRecord {
    /// The action the reward is attached to.
    pub action: String,
    /// Scalar reward.
    pub reward: f64,
    /// Logical timestamp (caller-supplied).
    pub timestamp: u64,
    /// Acceptance state.
    pub outcome: Outcome,
}

/// Aggregate statistics over a set of records.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LedgerStats {
    /// Number of records aggregated.
    pub count: usize,
    /// Sum of rewards.
    pub sum: f64,
    /// Arithmetic mean reward.
    pub mean: f64,
    /// Minimum reward.
    pub min: f64,
    /// Maximum reward.
    pub max: f64,
}

/// An append-only ledger of reward signals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TarlLedger {
    records: Vec<TarlRecord>,
}

impl TarlLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the ledger is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The raw records, in append order.
    #[must_use]
    pub fn records(&self) -> &[TarlRecord] {
        &self.records
    }

    /// Append a record.
    pub fn record(
        &mut self,
        action: impl Into<String>,
        reward: f64,
        timestamp: u64,
        outcome: Outcome,
    ) {
        self.records.push(TarlRecord {
            action: action.into(),
            reward,
            timestamp,
            outcome,
        });
    }

    /// Aggregate every record.
    #[must_use]
    pub fn aggregate(&self) -> LedgerStats {
        Self::stats_of(&self.records)
    }

    /// Aggregate only the records for `action`.
    #[must_use]
    pub fn aggregate_action(&self, action: &str) -> Option<LedgerStats> {
        let filtered: Vec<TarlRecord> = self
            .records
            .iter()
            .filter(|r| r.action == action)
            .cloned()
            .collect();
        if filtered.is_empty() {
            None
        } else {
            Some(Self::stats_of(&filtered))
        }
    }

    /// Exponential moving average of the rewards in append order.
    ///
    /// `alpha` is clamped to `[0, 1]`; `alpha == 1` tracks the latest reward
    /// and `alpha == 0` freezes on the first. An empty ledger yields `0.0`.
    #[must_use]
    pub fn ema(&self, alpha: f64) -> f64 {
        let alpha = alpha.clamp(0.0, 1.0);
        let mut iter = self.records.iter();
        let Some(first) = iter.next() else {
            return 0.0;
        };
        let mut value = first.reward;
        for record in iter {
            value = alpha * record.reward + (1.0 - alpha) * value;
        }
        value
    }

    /// Sum of all rewards.
    #[must_use]
    pub fn total_reward(&self) -> f64 {
        self.records.iter().map(|r| r.reward).sum()
    }

    /// Count of records with the given outcome.
    #[must_use]
    pub fn count_outcome(&self, outcome: Outcome) -> usize {
        self.records.iter().filter(|r| r.outcome == outcome).count()
    }

    fn stats_of(records: &[TarlRecord]) -> LedgerStats {
        if records.is_empty() {
            return LedgerStats::default();
        }
        let mut sum = 0.0_f64;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for record in records {
            sum += record.reward;
            min = min.min(record.reward);
            max = max.max(record.reward);
        }
        let count = records.len();
        LedgerStats {
            count,
            sum,
            mean: sum / count as f64,
            min,
            max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_reports_mean_min_max() {
        let mut ledger = TarlLedger::new();
        ledger.record("edit", 1.0, 1, Outcome::Accepted);
        ledger.record("edit", 3.0, 2, Outcome::Accepted);
        ledger.record("edit", 2.0, 3, Outcome::Rejected);
        let stats = ledger.aggregate();
        assert_eq!(stats.count, 3);
        assert!((stats.mean - 2.0).abs() < 1e-12);
        assert!((stats.min - 1.0).abs() < 1e-12);
        assert!((stats.max - 3.0).abs() < 1e-12);
    }

    #[test]
    fn per_action_aggregation_filters() {
        let mut ledger = TarlLedger::new();
        ledger.record("edit", 1.0, 1, Outcome::Accepted);
        ledger.record("search", 5.0, 2, Outcome::Accepted);
        let edit = ledger.aggregate_action("edit").expect("edit present");
        assert_eq!(edit.count, 1);
        assert!((edit.mean - 1.0).abs() < 1e-12);
        assert!(ledger.aggregate_action("missing").is_none());
    }

    #[test]
    fn ema_tracks_recent_rewards() {
        let mut ledger = TarlLedger::new();
        ledger.record("a", 0.0, 1, Outcome::Accepted);
        ledger.record("a", 10.0, 2, Outcome::Accepted);
        // alpha=1 -> latest reward; alpha=0 -> first reward.
        assert!((ledger.ema(1.0) - 10.0).abs() < 1e-12);
        assert!((ledger.ema(0.0) - 0.0).abs() < 1e-12);
        // alpha=0.5 -> 0.5*10 + 0.5*0 = 5.
        assert!((ledger.ema(0.5) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn empty_ledger_is_well_formed() {
        let ledger = TarlLedger::new();
        assert!(ledger.is_empty());
        assert_eq!(ledger.aggregate(), LedgerStats::default());
        assert!((ledger.ema(0.5) - 0.0).abs() < 1e-12);
    }

    #[test]
    fn outcome_counting() {
        let mut ledger = TarlLedger::new();
        ledger.record("a", 1.0, 1, Outcome::Accepted);
        ledger.record("a", 1.0, 2, Outcome::Rejected);
        ledger.record("a", 1.0, 3, Outcome::Rejected);
        assert_eq!(ledger.count_outcome(Outcome::Accepted), 1);
        assert_eq!(ledger.count_outcome(Outcome::Rejected), 2);
        assert_eq!(ledger.count_outcome(Outcome::Pending), 0);
    }
}
