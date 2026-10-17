//! R6 — value-of-information escalation ladder.
//!
//! When an incident fires, an agent can spend more and more effort narrowing
//! it down. Each rung of the ladder buys a better decision, but costs more to
//! run:
//!
//! ```text
//! Stream → Detector → Risk → Verifier → Investigator → Halt
//! ```
//!
//! The rule is value-of-information: escalate to the next rung only while the
//! *expected loss* of not knowing (`P · damage`) exceeds that rung's cost.
//! When the expected loss drops below the next rung's price, escalation stops
//! — spending more would cost more than the loss it prevents.
//!
//! A [`Policy`] also carries a mandatory **floor**: a minimum rung that always
//! runs regardless of the cost comparison, so cheap insurance (e.g. a
//! verifier) is never skipped even for a seemingly harmless incident.

#![forbid(unsafe_code)]

/// A rung of the escalation ladder, cheapest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Passive streaming / monitoring only.
    Stream,
    /// Run a detector over the stream.
    Detector,
    /// Quantify the risk.
    Risk,
    /// Independently verify the finding.
    Verifier,
    /// Open a full investigation.
    Investigator,
    /// Halt and escalate to a human / kill-switch.
    Halt,
}

impl Stage {
    /// All rungs, cheapest first.
    pub const ALL: [Self; 6] = [
        Self::Stream,
        Self::Detector,
        Self::Risk,
        Self::Verifier,
        Self::Investigator,
        Self::Halt,
    ];

    /// Position on the ladder (`Stream == 0`, `Halt == 5`).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Stream => 0,
            Self::Detector => 1,
            Self::Risk => 2,
            Self::Verifier => 3,
            Self::Investigator => 4,
            Self::Halt => 5,
        }
    }

    /// The next, more expensive rung (`None` at [`Stage::Halt`]).
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Stream => Some(Self::Detector),
            Self::Detector => Some(Self::Risk),
            Self::Risk => Some(Self::Verifier),
            Self::Verifier => Some(Self::Investigator),
            Self::Investigator => Some(Self::Halt),
            Self::Halt => None,
        }
    }
}

/// The incident under assessment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Incident {
    /// Probability the incident is real, in `[0.0, 1.0]`.
    pub p: f32,
    /// Damage magnitude if it is real (arbitrary currency).
    pub damage: f32,
}

impl Incident {
    /// A new incident.
    #[must_use]
    pub fn new(p: f32, damage: f32) -> Self {
        Self {
            p: p.clamp(0.0, 1.0),
            damage,
        }
    }

    /// Expected loss `P · damage`.
    #[must_use]
    pub fn expected_loss(self) -> f32 {
        self.p * self.damage
    }
}

/// Per-rung costs, indexed by [`Stage`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Costs {
    /// Cost of the stream rung (usually `0.0`).
    pub stream: f32,
    /// Cost of the detector rung.
    pub detector: f32,
    /// Cost of the risk rung.
    pub risk: f32,
    /// Cost of the verifier rung.
    pub verifier: f32,
    /// Cost of the investigator rung.
    pub investigator: f32,
    /// Cost of the halt rung.
    pub halt: f32,
}

impl Costs {
    /// The cost of running `stage`.
    #[must_use]
    pub const fn of(&self, stage: Stage) -> f32 {
        match stage {
            Stage::Stream => self.stream,
            Stage::Detector => self.detector,
            Stage::Risk => self.risk,
            Stage::Verifier => self.verifier,
            Stage::Investigator => self.investigator,
            Stage::Halt => self.halt,
        }
    }
}

impl Default for Costs {
    fn default() -> Self {
        Self {
            stream: 0.0,
            detector: 1.0,
            risk: 5.0,
            verifier: 10.0,
            investigator: 50.0,
            halt: 200.0,
        }
    }
}

/// Escalation policy: rung costs plus the mandatory floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    /// Cost of each rung.
    pub costs: Costs,
    /// Minimum rung that always runs, whatever the cost comparison says.
    pub floor: Stage,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            costs: Costs::default(),
            floor: Stage::Stream,
        }
    }
}

impl Policy {
    /// A policy with the default costs and the given floor.
    #[must_use]
    pub fn with_floor(floor: Stage) -> Self {
        Self {
            costs: Costs::default(),
            floor,
        }
    }
}

/// The result of running the ladder.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The last rung reached.
    pub final_stage: Stage,
    /// The rungs executed, cheapest first.
    pub executed: Vec<Stage>,
    /// Whether escalation continued beyond the mandatory floor.
    pub escalated_past_floor: bool,
    /// Expected loss that drove the decision.
    pub expected_loss: f32,
}

impl Outcome {
    /// Whether the ladder ran `stage`.
    #[must_use]
    pub fn ran(&self, stage: Stage) -> bool {
        self.executed.contains(&stage)
    }
}

/// The escalation ladder.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ladder;

impl Ladder {
    /// Run the ladder for `incident` under `policy`.
    ///
    /// Starts at [`Stage::Stream`], always advances to at least `policy.floor`,
    /// then keeps escalating while `expected_loss > cost(next_rung)`.
    #[must_use]
    pub fn run(incident: Incident, policy: Policy) -> Outcome {
        let loss = incident.expected_loss();
        let floor_index = policy.floor.index();
        let mut stage = Stage::Stream;
        let mut executed = vec![stage];
        let mut escalated_past_floor = false;

        while let Some(next) = stage.next() {
            let below_floor = stage.index() < floor_index;
            let worth_it = loss > policy.costs.of(next);
            if !below_floor && !worth_it {
                break;
            }
            if !below_floor && worth_it {
                escalated_past_floor = true;
            }
            stage = next;
            executed.push(stage);
        }

        Outcome {
            final_stage: stage,
            executed,
            escalated_past_floor,
            expected_loss: loss,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_order_is_a_chain() {
        for pair in Stage::ALL.windows(2) {
            assert_eq!(pair[0].next(), Some(pair[1]));
        }
        assert_eq!(Stage::Halt.next(), None);
        assert_eq!(Stage::Stream.index(), 0);
        assert_eq!(Stage::Halt.index(), 5);
    }

    #[test]
    fn low_loss_stops_at_the_floor() {
        // loss = 1.0; investigator costs 50 → no escalation past Verifier.
        let incident = Incident::new(0.1, 10.0);
        assert_eq!(incident.expected_loss(), 1.0);
        let outcome = Ladder::run(incident, Policy::with_floor(Stage::Verifier));
        assert_eq!(outcome.final_stage, Stage::Verifier);
        assert_eq!(
            outcome.executed,
            vec![Stage::Stream, Stage::Detector, Stage::Risk, Stage::Verifier]
        );
        assert!(!outcome.escalated_past_floor);
        assert!(!outcome.ran(Stage::Investigator));
    }

    #[test]
    fn high_loss_reaches_investigator_then_halt() {
        // loss = 90: > investigator (50), < halt (200).
        let outcome = Ladder::run(
            Incident::new(0.9, 100.0),
            Policy::with_floor(Stage::Verifier),
        );
        assert_eq!(outcome.final_stage, Stage::Investigator);
        assert!(outcome.ran(Stage::Verifier));
        assert!(!outcome.ran(Stage::Halt));
        assert!(outcome.escalated_past_floor);

        // loss = 1000: exceeds every rung → halt.
        let extreme = Ladder::run(
            Incident::new(1.0, 1000.0),
            Policy::with_floor(Stage::Stream),
        );
        assert_eq!(extreme.final_stage, Stage::Halt);
        assert_eq!(extreme.executed.len(), Stage::ALL.len());
        assert_eq!(extreme.executed.last(), Some(&Stage::Halt));
    }

    #[test]
    fn floor_is_never_skipped_even_for_a_trivial_incident() {
        // Zero expected loss, but a Risk floor forces Detector and Risk to run.
        let outcome = Ladder::run(Incident::new(0.0, 0.0), Policy::with_floor(Stage::Risk));
        assert_eq!(outcome.final_stage, Stage::Risk);
        assert!(outcome.ran(Stage::Detector));
        assert!(outcome.ran(Stage::Risk));
        assert!(!outcome.ran(Stage::Verifier));
        assert!(!outcome.escalated_past_floor);
        assert_eq!(outcome.expected_loss, 0.0);
    }

    #[test]
    fn incident_clamps_probability() {
        assert_eq!(Incident::new(1.5, 10.0).p, 1.0);
        assert_eq!(Incident::new(-1.0, 10.0).p, 0.0);
        assert_eq!(Costs::default().of(Stage::Investigator), 50.0);
    }
}
