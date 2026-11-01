//! Giving a slot back: settle with its AIMD signal, plain release, a dead
//! holder's leases, the FIFO dispatch that refills freed capacity, and the
//! observability rows the daemon serves.
//!
//! This is the second half of `impl ProviderAdmission` (the first is
//! [`super::admit`]). It calls four members of that half — `lock`, `mint`,
//! `telemetry_client`, `track_breaker_transition` — which are `pub(super)`
//! there because this module is a SIBLING, not a descendant, so private
//! visibility would not reach them. In the other direction `admit::settle` needs
//! exactly one member of this half: `dispatch`, which hands a freed slot on.

use std::sync::PoisonError;

use pa_telemetry::AdmissionSnapshot;

use super::breaker::{breaker_snapshot, BreakerState};
use super::{
    BreakerSnapshot, Grant, ProviderAdmission, ProviderKey, ProviderSnapshot, RegistryState,
};

/// One per-`(provider, model)` observability row (the M5 snapshot wire):
/// the AIMD standing plus the provider's breaker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRow {
    pub provider: String,
    pub model: String,
    pub limit: usize,
    pub in_flight: usize,
    pub queued: usize,
    pub cap: usize,
    pub consecutive_429: u32,
    pub breaker: BreakerSnapshot,
}

impl ProviderAdmission {
    /// Give a lease back without an AIMD signal (an aborted turn, a
    /// dispatch that never reached the provider, a dead worker).
    pub fn release(&self, lease: u64) {
        let mut state = self.lock();
        let Some(record) = state.leases.remove(&lease) else {
            return;
        };
        if let Some(entry) = state.providers.get_mut(&record.key) {
            entry.in_flight = entry.in_flight.saturating_sub(1);
        }
        // A dropped probe re-arms the cooldown (M4): the breaker must
        // never wedge in half-open on a probe that never answered.
        let now = self.clock.now_ms();
        if let Some(breaker) = state.breakers.get_mut(&record.key.provider) {
            if breaker.probe_lease == Some(lease) {
                breaker.probe_lease = None;
                breaker.state = BreakerState::Open;
                breaker.opened_at_ms = now;
                let failures = breaker.failures();
                tracing::info!(
                    provider = %record.key.provider,
                    "provider admission: breaker re-opened, the probe dropped without a verdict"
                );
                self.track_breaker_transition(
                    &record.key.provider,
                    BreakerState::HalfOpen,
                    BreakerState::Open,
                    failures,
                );
            }
        }
        self.dispatch(&mut state, &record.key);
    }

    /// Whether `holder` owns `lease`.
    ///
    /// Settle and release are MUTATIONS of provider capacity, and the wire only carried the
    /// lease id plus a shape-checked token — so any same-user local process could settle or
    /// release another worker's lease (review #14). Forged rate-limited settles drive the
    /// provider limit to the floor; foreign releases corrupt in-flight accounting. The holder
    /// is already recorded at mint; this is what reads it.
    ///
    /// An UNKNOWN lease answers `false`: a lease that is gone cannot be authoritatively
    /// settled by anyone, and pretending otherwise would let a stale id win a race with a
    /// reused one.
    pub fn lease_held_by(&self, lease: u64, holder: &str) -> bool {
        let state = self.lock();
        state
            .leases
            .get(&lease)
            .is_some_and(|record| record.holder == holder)
    }

    /// Drop every lease a holder still owns: a worker that died mid-stream
    /// must not shrink the pool forever.
    pub fn release_holder(&self, holder: &str) -> usize {
        let leases: Vec<u64> = {
            let state = self.lock();
            state
                .leases
                .iter()
                .filter(|(_, record)| record.holder == holder)
                .map(|(id, _)| *id)
                .collect()
        };
        for lease in &leases {
            self.release(*lease);
        }
        if !leases.is_empty() {
            tracing::info!(
                holder,
                leases = leases.len(),
                "provider admission: released a dead holder's leases"
            );
        }
        leases.len()
    }

    /// Fill the freed capacity from the key's FIFO queue, one waiter per
    /// freed slot (a waiter's grant takes exactly one slot, so the loop
    /// re-checks the limit each pass).
    pub(super) fn dispatch(&self, state: &mut RegistryState, key: &ProviderKey) {
        loop {
            let Some(entry) = state.providers.get_mut(key) else {
                return;
            };
            // An open breaker refuses new admissions; the queue drains only
            // through a half-open probe (finding #5: the settle path used
            // to dispatch through an open breaker, so the fail-fast never
            // stopped the stream of fresh leases into a dead provider).
            //
            // HALF-OPEN IS THE SAME REFUSAL (review #9). `breaker_gate` answers `Refuse` for
            // half-open, but it runs on the ACQUIRE path only — `dispatch` is called from
            // `settle` and `release`, which never pass through it. So a backlog queued while
            // the breaker was CLOSED (a waiter's budget is 600 s, the cooldown 30 s) was
            // granted as soon as the cooldown expired: non-probe leases beside the single
            // probe, whose settles land in the `BreakerState::Open | HalfOpen => {}` no-op arm
            // and teach the breaker nothing, while fresh traffic stays refused.
            //
            // The earlier note here claimed this arm was dead code. It was not: the test that
            // "proved" it only covered a waiter arriving AFTER the probe was taken, which
            // `breaker_gate` does refuse. Pre-trip backlog was the uncovered half.
            if let Some(breaker) = state.breakers.get(&key.provider) {
                if matches!(breaker.state, BreakerState::Open | BreakerState::HalfOpen) {
                    return;
                }
            }
            if entry.in_flight >= entry.limit {
                return;
            }
            let Some(waiter) = entry.waiters.pop_front() else {
                return;
            };
            entry.in_flight += 1;
            let mut slot = waiter.grant.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(tx) = slot.take() else {
                // The receiver dropped before the pop: return the slot and
                // try the next waiter.
                if let Some(entry) = state.providers.get_mut(key) {
                    entry.in_flight = entry.in_flight.saturating_sub(1);
                }
                continue;
            };
            drop(slot);
            let stagger_ms = self.stagger.next_delay_ms();
            let grant = Grant {
                lease: Self::mint(state, key, &waiter.holder),
                stagger_ms,
            };
            let queued = state
                .providers
                .get(key)
                .map_or(0, |entry| entry.waiters.len());
            tracing::info!(
                provider = %key.provider,
                model = %key.model,
                holder = %waiter.holder,
                queued,
                stagger_ms,
                lease = grant.lease,
                "provider admission: dequeue"
            );
            if tx.send(grant).is_err() {
                // The acquirer gave up between the pop and the send: the
                // slot returns to the pool and the next waiter takes it.
                if let Some(record) = state.leases.remove(&grant.lease) {
                    if let Some(entry) = state.providers.get_mut(&record.key) {
                        entry.in_flight = entry.in_flight.saturating_sub(1);
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self, key: &ProviderKey) -> ProviderSnapshot {
        let state = self.lock();
        let breaker = breaker_snapshot(&state.breakers, &key.provider);
        match state.providers.get(key) {
            Some(entry) => ProviderSnapshot {
                limit: entry.limit,
                in_flight: entry.in_flight,
                queued: entry.waiters.len(),
                cap: self.cap,
                consecutive_429: entry.consecutive_429,
                breaker,
            },
            None => ProviderSnapshot {
                limit: self.start,
                in_flight: 0,
                queued: 0,
                cap: self.cap,
                consecutive_429: 0,
                breaker,
            },
        }
    }

    /// The total queued acquirers across every provider (the daemon log's
    /// queue-depth line).
    #[must_use]
    pub fn queued_total(&self) -> usize {
        self.lock()
            .providers
            .values()
            .map(|entry| entry.waiters.len())
            .sum()
    }

    /// Every provider's observability rows (M5): one per `(provider,
    /// model)` pair the registry has seen, sorted, each carrying its
    /// provider's breaker standing. The daemon's state snapshot wire —
    /// one served ask also rides the telemetry seam as an
    /// `admission_snapshot` rollup (counters only).
    #[must_use]
    pub fn provider_rows(&self) -> Vec<ProviderRow> {
        let state = self.lock();
        let mut rows: Vec<ProviderRow> = state
            .providers
            .iter()
            .map(|(key, entry)| ProviderRow {
                provider: key.provider.clone(),
                model: key.model.clone(),
                limit: entry.limit,
                in_flight: entry.in_flight,
                queued: entry.waiters.len(),
                cap: self.cap,
                consecutive_429: entry.consecutive_429,
                breaker: breaker_snapshot(&state.breakers, &key.provider),
            })
            .collect();
        rows.sort_by(|left, right| {
            (&left.provider, &left.model).cmp(&(&right.provider, &right.model))
        });
        if let Some(client) = self.telemetry_client() {
            AdmissionSnapshot {
                providers_count: rows.len() as u64,
                total_in_flight: rows.iter().map(|row| row.in_flight).sum::<usize>() as u64,
                total_queued: rows.iter().map(|row| row.queued).sum::<usize>() as u64,
                any_open_breakers: rows
                    .iter()
                    .any(|row| row.breaker.state == BreakerState::Open),
            }
            .track(&client);
        }
        rows
    }
}
