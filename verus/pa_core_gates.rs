// MIRROR OF: crates/pa-core/src/autonomous/gates.rs (should_autonomously_continue decision kernel + gate_attempt_result). Update together with the source.
// Trusted surface: external_body wrappers below — minimal by design.
use vstd::prelude::*;

verus! {

// --- Pure decision kernel of pa-core/autonomous/gates.rs should_autonomously_continue ---
// The async wrapper (gate refresh, IO) is stripped; this is the logic over its results.

#[derive(PartialEq, Eq)]
pub enum GateResult { Passed, Failed, RetryExhausted }

#[derive(PartialEq, Eq)]
pub enum Reason { NotNeeded, LimitReached, GateFailed, MissingTerminalEvidence }

pub struct Decision {
    pub cont: bool,
    pub reason: Reason,
}

/// Contract properties of `should_autonomously_continue`:
/// - terminal_stop models `stop_reason == Some(Error) | Some(Aborted)`
/// - limit models `autonomous_limit_reason_now(state).is_some()`
pub fn decide(enabled: bool, terminal_stop: bool, gate: Option<GateResult>, limit: bool) -> (d: Decision)
    ensures
        // SAFETY: never continue when disabled or on a terminal error/abort stop
        d.cont ==> (enabled && !terminal_stop),
        // coherence: continue iff the reason is a continue-shaped reason
        d.cont == (d.reason == Reason::GateFailed || d.reason == Reason::MissingTerminalEvidence),
        // a passing gate always ends the autonomous stretch as NotNeeded
        gate == Some(GateResult::Passed) ==> (d.reason == Reason::NotNeeded && !d.cont),
        // retry exhaustion reports the limit, never a continue — but only when the
        // run is actually autonomous (disabled/error-abort exits earlier as NotNeeded)
        (gate == Some(GateResult::RetryExhausted) && enabled && !terminal_stop)
            ==> (d.reason == Reason::LimitReached && !d.cont),
        // disabled or terminal stop reports NotNeeded
        (!enabled || terminal_stop) ==> d.reason == Reason::NotNeeded,
        // limit reached never continues
        limit ==> !d.cont,
{
    if !enabled || terminal_stop {
        Decision { cont: false, reason: Reason::NotNeeded }
    } else {
        match gate {
            Some(GateResult::Passed) => Decision { cont: false, reason: Reason::NotNeeded },
            Some(GateResult::RetryExhausted) => Decision { cont: false, reason: Reason::LimitReached },
            None if limit => Decision { cont: false, reason: Reason::LimitReached },
            Some(GateResult::Failed) => {
                if limit {
                    Decision { cont: false, reason: Reason::LimitReached }
                } else {
                    Decision { cont: true, reason: Reason::GateFailed }
                }
            },
            None => Decision { cont: true, reason: Reason::MissingTerminalEvidence },
        }
    }
}

/// pa-mem-style: `gate_attempt_result` — retry exhaustion boundary.
pub fn gate_attempt_result(attempt: u64, max_retries: u64) -> (out: GateResult)
    ensures
        (out == GateResult::RetryExhausted) == (attempt > max_retries),
        (out == GateResult::Failed) == (attempt <= max_retries),
{
    if attempt > max_retries {
        GateResult::RetryExhausted
    } else {
        GateResult::Failed
    }
}

fn main() {}

}
