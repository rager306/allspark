//! S17: anti-grep / anti-read hooks.
//!
//! Serena's CLI hooks count *consecutive* grep and whole-file read calls and,
//! once a threshold is crossed, deny the next call with a reminder to use the
//! symbolic tools. The counters gate the gap between neighbouring calls (a
//! non-grep call resets the streak), and a deny is rate-limited so the model
//! is not spammed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Consecutive-grep threshold before the first deny.
pub const GREP_THRESHOLD: u32 = 3;
/// Consecutive-whole-file-read threshold before the first deny.
pub const READ_THRESHOLD: u32 = 3;
/// Hard ceiling on consecutive reads (deny regardless of rate limit).
pub const READ_HARD_THRESHOLD: u32 = 4;
/// Minimum gap between two denies for the same kind.
pub const MIN_DENY_INTERVAL: Duration = Duration::from_secs(120);

/// The kind of call being tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GuardKind {
    /// A grep/search-over-file call.
    Grep,
    /// A whole-file read call.
    Read,
}

impl GuardKind {
    fn threshold(self) -> u32 {
        match self {
            GuardKind::Grep => GREP_THRESHOLD,
            GuardKind::Read => READ_THRESHOLD,
        }
    }

    fn reminder(self) -> &'static str {
        match self {
            GuardKind::Grep => {
                "Repeated grep detected. Use `find_symbol` / `get_symbols_overview` \
                 instead of grepping the tree."
            }
            GuardKind::Read => {
                "Repeated whole-file reads detected. Use `find_symbol` with \
                 `include_body`, or `get_symbols_overview`, instead of reading whole files."
            }
        }
    }
}

/// The gate's verdict for one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecision {
    /// Allow the call.
    Allow,
    /// Deny the call with a reminder.
    Deny(String),
}

impl HookDecision {
    /// Whether the call is denied.
    #[must_use]
    pub fn is_deny(&self) -> bool {
        matches!(self, HookDecision::Deny(_))
    }
}

#[derive(Debug, Clone, Default)]
struct Counter {
    streak: u32,
    last_deny: Option<Instant>,
}

/// S17: the anti-grep/anti-read gate.
///
/// Feed each observed call through [`AntiGrepHook::observe`]; a non-tracked
/// call resets both streaks via [`AntiGrepHook::reset`].
#[derive(Debug, Default)]
pub struct AntiGrepHook {
    counters: BTreeMap<GuardKind, Counter>,
    denies: BTreeMap<GuardKind, u32>,
}

impl AntiGrepHook {
    /// A fresh gate.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify a tool call into a tracked kind.
    ///
    /// `grep`/`search_for_pattern` count as grep; `read`/`read_file` count as
    /// read. Anything else returns `None`.
    #[must_use]
    pub fn classify(tool: &str) -> Option<GuardKind> {
        match tool {
            "grep" | "search_for_pattern" | "rg" => Some(GuardKind::Grep),
            "read" | "read_file" | "view" => Some(GuardKind::Read),
            _ => None,
        }
    }

    /// Record an untracked call, resetting both streaks.
    pub fn reset(&mut self) {
        for counter in self.counters.values_mut() {
            counter.streak = 0;
        }
    }

    /// Observe a call and return the gate's verdict.
    ///
    /// `now` is injected so tests can drive the rate limit deterministically.
    pub fn observe_at(&mut self, kind: GuardKind, now: Instant) -> HookDecision {
        let threshold = kind.threshold();
        let counter = self.counters.entry(kind).or_default();
        counter.streak += 1;
        let hard = kind == GuardKind::Read && counter.streak >= READ_HARD_THRESHOLD;
        if counter.streak < threshold && !hard {
            return HookDecision::Allow;
        }
        // Rate-limit: suppress a repeat deny within the interval unless the
        // hard ceiling is crossed.
        let cooled = counter
            .last_deny
            .is_none_or(|last| now.duration_since(last) >= MIN_DENY_INTERVAL);
        if !cooled && !hard {
            return HookDecision::Allow;
        }
        counter.last_deny = Some(now);
        let total = self.denies.entry(kind).or_insert(0);
        *total += 1;
        HookDecision::Deny(format!(
            "{} (deny #{}, streak {})",
            kind.reminder(),
            *total,
            counter.streak
        ))
    }

    /// Observe a call against the wall clock.
    pub fn observe(&mut self, kind: GuardKind) -> HookDecision {
        self.observe_at(kind, Instant::now())
    }

    /// Observe a tool name: untracked names reset the streaks.
    pub fn observe_tool(&mut self, tool: &str) -> HookDecision {
        if let Some(kind) = Self::classify(tool) {
            self.observe(kind)
        } else {
            self.reset();
            HookDecision::Allow
        }
    }

    /// How many denies the gate has issued for a kind.
    #[must_use]
    pub fn deny_count(&self, kind: GuardKind) -> u32 {
        self.denies.get(&kind).copied().unwrap_or(0)
    }
}
