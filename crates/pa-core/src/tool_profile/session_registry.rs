//! S19: an LRU + idle-TTL session registry.
//!
//! Serena's `SessionRegistry` keeps live sessions in an LRU keyed by
//! session id and evicts on two axes: a hard cap (max 100) and an idle
//! timeout (48h). This port keeps the same contract, injecting the clock so
//! the TTL is testable without sleeping.

use std::collections::BTreeMap;
use std::time::Duration;

/// Default hard cap on live sessions.
pub const DEFAULT_MAX_SESSIONS: usize = 100;
/// Default idle timeout before a session is evicted.
pub const DEFAULT_IDLE_TTL: Duration = Duration::from_hours(48);

/// The outcome of one registry operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eviction {
    /// Session ids evicted by this operation, least-recently-used first.
    pub evicted: Vec<String>,
}

impl Eviction {
    /// Whether anything was evicted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.evicted.is_empty()
    }
}

#[derive(Debug, Clone)]
struct Entry {
    last_used_ms: u64,
}

/// S19: an LRU + idle-TTL session registry.
///
/// `now_ms` is supplied by the caller (a monotonic millisecond clock), so the
/// TTL and the LRU order are deterministic under test.
#[derive(Debug, Clone)]
pub struct SessionRegistry {
    entries: BTreeMap<String, Entry>,
    max_sessions: usize,
    idle_ttl: Duration,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_SESSIONS, DEFAULT_IDLE_TTL)
    }
}

impl SessionRegistry {
    /// A registry with an explicit cap and idle TTL.
    #[must_use]
    pub fn new(max_sessions: usize, idle_ttl: Duration) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_sessions,
            idle_ttl,
        }
    }

    /// The number of live sessions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a session is live.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    /// Touch/insert a session, refreshing its LRU position and last-used time,
    /// then evict idle and over-cap sessions.
    pub fn touch(&mut self, id: &str, now_ms: u64) -> Eviction {
        self.entries.insert(
            id.to_string(),
            Entry {
                last_used_ms: now_ms,
            },
        );
        self.evict(now_ms)
    }

    /// Remove a session explicitly.
    pub fn remove(&mut self, id: &str) -> bool {
        self.entries.remove(id).is_some()
    }

    /// Evict idle and over-cap sessions, least-recently-used first.
    pub fn evict(&mut self, now_ms: u64) -> Eviction {
        let ttl_ms = self.idle_ttl.as_millis() as u64;
        let mut evicted: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| now_ms.saturating_sub(entry.last_used_ms) >= ttl_ms)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &evicted {
            self.entries.remove(id);
        }
        while self.entries.len() > self.max_sessions {
            let Some(lru) = self
                .entries
                .iter()
                .min_by(|(id_a, a), (id_b, b)| {
                    a.last_used_ms
                        .cmp(&b.last_used_ms)
                        .then_with(|| id_a.cmp(id_b))
                })
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.entries.remove(&lru);
            evicted.push(lru);
        }
        Eviction { evicted }
    }

    /// The live session ids, most-recently-used first.
    #[must_use]
    pub fn ids_by_recency(&self) -> Vec<String> {
        let mut ids: Vec<(String, u64)> = self
            .entries
            .iter()
            .map(|(id, entry)| (id.clone(), entry.last_used_ms))
            .collect();
        ids.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ids.into_iter().map(|(id, _)| id).collect()
    }

    /// Drop everything (used on teardown).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
