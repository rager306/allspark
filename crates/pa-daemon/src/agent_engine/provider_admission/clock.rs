//! The admission plane's injected time and randomness: the breaker cooldown's
//! clock and the spawn-dispatch stagger.
//!
//! Each is a trait with a production implementation and a scripted one, so a
//! breaker transition or a dispatch spread is testable without sleeping — the
//! ADR's injectable-time rule.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use super::SPAWN_STAGGER_MAX_MS;

/// The breaker cooldown's time source. Injectable so breaker
/// transitions test without sleeping (the ADR's injectable-time rule).
pub trait ClockSource: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The production clock: wall-clock epoch milliseconds.
#[derive(Debug, Default)]
pub struct SystemClock;

impl ClockSource for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }
}

/// A scripted clock (tests): `advance` moves time without sleeping.
#[derive(Debug)]
pub struct ScriptedClock {
    now_ms: AtomicU64,
}

impl ScriptedClock {
    #[must_use]
    pub fn new(start_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            now_ms: AtomicU64::new(start_ms),
        })
    }

    pub fn advance(&self, by_ms: u64) {
        self.now_ms.fetch_add(by_ms, Ordering::Relaxed);
    }
}

impl ClockSource for ScriptedClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::Relaxed)
    }
}

/// The randomized spawn-dispatch delay source. Injectable so the stagger is
/// testable without sleeping (the ADR's "мок-время или injectable rand").
pub trait StaggerSource: Send + Sync {
    /// The delay to wait before dispatching the next queued spawn.
    fn next_delay_ms(&self) -> u64;
}

/// The production stagger: a splitmix walk over a clock seed plus an atomic
/// counter, bounded by [`SPAWN_STAGGER_MAX_MS`].
#[derive(Debug, Default)]
pub struct JitteredStagger {
    counter: AtomicU64,
}

impl JitteredStagger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl StaggerSource for JitteredStagger {
    fn next_delay_ms(&self) -> u64 {
        let tick = self.counter.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| u64::from(elapsed.subsec_nanos()));
        let mut seed = nanos
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(tick.wrapping_mul(0xBF58_476D_1CE4_E5B9))
            .wrapping_add(0x94D0_49BB_1331_11EB);
        seed ^= seed >> 30;
        seed = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        seed ^= seed >> 27;
        seed = seed.wrapping_mul(0x94D0_49BB_1331_11EB);
        seed ^= seed >> 31;
        seed % (SPAWN_STAGGER_MAX_MS + 1)
    }
}

/// A stagger source that hands back a fixed script (tests): the last value
/// repeats once the script runs out.
#[derive(Debug)]
pub struct ScriptedStagger {
    delays: Mutex<VecDeque<u64>>,
    fallback: u64,
}

impl ScriptedStagger {
    #[must_use]
    pub fn new(delays: impl IntoIterator<Item = u64>) -> Arc<Self> {
        let delays: VecDeque<u64> = delays.into_iter().collect();
        let fallback = delays.back().copied().unwrap_or(0);
        Arc::new(Self {
            delays: Mutex::new(delays),
            fallback,
        })
    }
}

impl StaggerSource for ScriptedStagger {
    fn next_delay_ms(&self) -> u64 {
        self.delays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or(self.fallback)
    }
}
