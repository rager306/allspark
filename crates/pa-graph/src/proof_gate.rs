//! R17 — proof gate: accept a result only when its evidence checks out.
//!
//! Downstream consumers (a patch applier, a report emitter) should not trust a
//! result on its own word. [`ProofGate`] requires two independent conditions:
//!
//! * **receipts** — every witness receipt verifies (hash matches its content)
//!   and the chain is unbroken;
//! * **envelope** — the quality envelope raises no degradation flag.
//!
//! Anything else is a [`GateDecision::Reject`] carrying the first failing
//! [`RejectReason`], so the caller can report *why* rather than a bare "no".

use crate::quality::QualityEnvelope;
use crate::witness::{ReceiptChain, WitnessReceipt};

/// Why a result was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// No receipt was supplied.
    NoReceipts,
    /// A receipt failed its own hash check.
    InvalidReceipt,
    /// The receipt chain is broken.
    BrokenChain,
    /// The quality envelope is degraded.
    DegradedQuality,
}

impl RejectReason {
    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoReceipts => "no_receipts",
            Self::InvalidReceipt => "invalid_receipt",
            Self::BrokenChain => "broken_chain",
            Self::DegradedQuality => "degraded_quality",
        }
    }
}

/// The gate's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// The result is accepted.
    Accept,
    /// The result is rejected for the given reason.
    Reject(RejectReason),
}

impl GateDecision {
    /// Whether the decision accepts.
    #[must_use]
    pub fn is_accept(&self) -> bool {
        matches!(self, Self::Accept)
    }

    /// The rejection reason, or `None` when the decision accepts.
    #[must_use]
    pub const fn reason(&self) -> Option<RejectReason> {
        match self {
            Self::Accept => None,
            Self::Reject(reason) => Some(*reason),
        }
    }
}

/// A gate that requires valid receipts and a healthy envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofGate {
    /// Whether a healthy envelope is required in addition to receipts.
    pub require_envelope: bool,
}

impl Default for ProofGate {
    fn default() -> Self {
        Self {
            require_envelope: true,
        }
    }
}

impl ProofGate {
    /// A gate requiring both receipts and a healthy envelope.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A gate that checks only receipts.
    #[must_use]
    pub fn receipts_only() -> Self {
        Self {
            require_envelope: false,
        }
    }

    /// Check `receipts` (already-recorded hashes) and `envelope`.
    ///
    /// `recorded` is the hash the caller expects each receipt to have; the
    /// receipts are verified in order and the chain link must hold.
    #[must_use]
    pub fn check(
        &self,
        receipts: &[(WitnessReceipt, String)],
        envelope: Option<&QualityEnvelope>,
    ) -> GateDecision {
        if receipts.is_empty() {
            return GateDecision::Reject(RejectReason::NoReceipts);
        }
        let mut prev = String::new();
        for (receipt, expected_hash) in receipts {
            if !receipt.verify(expected_hash) {
                return GateDecision::Reject(RejectReason::InvalidReceipt);
            }
            if receipt.chain_prev_hash != prev {
                return GateDecision::Reject(RejectReason::BrokenChain);
            }
            prev = receipt.hash();
        }
        if self.require_envelope {
            if let Some(envelope) = envelope {
                if envelope.is_degraded() {
                    return GateDecision::Reject(RejectReason::DegradedQuality);
                }
            }
        }
        GateDecision::Accept
    }

    /// Convenience: check a [`ReceiptChain`] plus an envelope. The chain's own
    /// `verify_chain` is the receipt condition.
    #[must_use]
    pub fn check_chain(
        &self,
        chain: &ReceiptChain,
        envelope: Option<&QualityEnvelope>,
    ) -> GateDecision {
        if chain.is_empty() {
            return GateDecision::Reject(RejectReason::NoReceipts);
        }
        if !chain.verify_chain() {
            return GateDecision::Reject(RejectReason::BrokenChain);
        }
        if self.require_envelope {
            if let Some(envelope) = envelope {
                if envelope.is_degraded() {
                    return GateDecision::Reject(RejectReason::DegradedQuality);
                }
            }
        }
        GateDecision::Accept
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::QualityPolicy;

    fn healthy_envelope() -> QualityEnvelope {
        let policy = QualityPolicy::new()
            .with_min_acceptable(0.5)
            .with_max_acceptable(0.5)
            .with_max_spread(1.0)
            .with_mean_floor(0.5);
        QualityEnvelope::from_samples(&[0.9, 0.95, 1.0], &policy)
    }

    fn chain_of_two() -> ReceiptChain {
        let mut chain = ReceiptChain::new();
        chain.append("q1", "r1", 1);
        chain.append("q2", "r2", 2);
        chain
    }

    #[test]
    fn accepts_valid_receipts_and_healthy_envelope() {
        let chain = chain_of_two();
        let decision = ProofGate::new().check_chain(&chain, Some(&healthy_envelope()));
        assert_eq!(decision, GateDecision::Accept);
        assert!(decision.is_accept());
    }

    #[test]
    fn rejects_without_receipts() {
        let decision = ProofGate::new().check_chain(&ReceiptChain::new(), None);
        assert_eq!(decision, GateDecision::Reject(RejectReason::NoReceipts));
    }

    #[test]
    fn rejects_a_tampered_chain() {
        let mut chain = chain_of_two();
        chain.receipts_mut()[0].result_hash = "forged".into();
        let decision = ProofGate::new().check_chain(&chain, Some(&healthy_envelope()));
        assert_eq!(decision, GateDecision::Reject(RejectReason::BrokenChain));
    }

    #[test]
    fn rejects_degraded_quality() {
        let chain = chain_of_two();
        let policy = QualityPolicy::new().with_min_acceptable(0.99);
        let degraded = QualityEnvelope::from_samples(&[0.1, 0.2], &policy);
        let decision = ProofGate::new().check_chain(&chain, Some(&degraded));
        assert_eq!(
            decision,
            GateDecision::Reject(RejectReason::DegradedQuality)
        );
    }

    #[test]
    fn receipts_only_gate_ignores_quality() {
        let chain = chain_of_two();
        let policy = QualityPolicy::new().with_min_acceptable(0.99);
        let degraded = QualityEnvelope::from_samples(&[0.1], &policy);
        let decision = ProofGate::receipts_only().check_chain(&chain, Some(&degraded));
        assert_eq!(decision, GateDecision::Accept);
    }

    #[test]
    fn explicit_receipt_pairs_are_verified() {
        let chain = chain_of_two();
        let pairs: Vec<(WitnessReceipt, String)> = chain
            .receipts()
            .iter()
            .map(|r| (r.clone(), r.hash()))
            .collect();
        assert!(ProofGate::new().check(&pairs, None).is_accept());
        let bad = vec![(chain.receipts()[0].clone(), "deadbeef".to_string())];
        assert_eq!(
            ProofGate::new().check(&bad, None),
            GateDecision::Reject(RejectReason::InvalidReceipt)
        );
    }
}
