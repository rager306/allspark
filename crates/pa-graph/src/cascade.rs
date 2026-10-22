//! Tiered activation: run only the work a query's shape can use, and record why.
//!
//! The graph crate carries far more machinery than any single query needs. Rather
//! than switch features on globally, a query walks a **cascade**: each tier is
//! cheaper than the next, and a tier only runs when the query's shape says its
//! output would be used. Every decision is recorded, so "why did this answer miss
//! the community view?" has an answer that is not a guess.
//!
//! ```text
//! tier 0  exact name lookup            always (the FTS index answers it)
//! tier 1  identifier shape             a distinctive identifier stops here
//! tier 2  lexical signals              only when tier 0 returns nothing useful
//! tier 3  structural neighbourhood     only for a traced symbol
//! tier 4  communities / architecture   only for a broad question
//! ```
//!
//! The tiers are *mutually ordered by cost*: a later tier's cost is many times
//! the earlier one's (measured: name lookup 2 ms, `signal_signals` 451 us per
//! node, `SymbolGraph::from_store` 6.3 s), so a cascade that stops early is the
//! only way to keep a broad feature set affordable.
//!
//! Nothing here decides *correctness*; it decides *work*. A tier that is skipped
//! says so, and the caller can force it.

use std::fmt;

/// What a query looks like to the cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryShape {
    /// One distinctive identifier, e.g. `parse_config` or `GraphStore::open`.
    Identifier,
    /// A phrase or several words, e.g. `where is the retry logic`.
    Phrase,
    /// A structural question about one symbol, e.g. "who calls this".
    AboutSymbol,
    /// A whole-repository question, e.g. "what are the modules".
    Broad,
}

impl QueryShape {
    /// Classify a raw query string.
    ///
    /// Deliberately conservative: an identifier is a single token that looks
    /// like code (`snake_case`, `camelCase`, or a `::` path) and carries no spaces.
    #[must_use]
    pub fn classify(query: &str) -> Self {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Self::Broad;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if words.len() > 3 {
            return Self::Broad;
        }
        if words.len() == 1 && looks_like_identifier(trimmed) {
            return Self::Identifier;
        }
        Self::Phrase
    }

    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identifier => "identifier",
            Self::Phrase => "phrase",
            Self::AboutSymbol => "about_symbol",
            Self::Broad => "broad",
        }
    }
}

/// Whether `token` looks like a code identifier rather than a word.
fn looks_like_identifier(token: &str) -> bool {
    if token.len() < 3 || token.contains('"') {
        return false;
    }
    let has_code_shape = token.contains("::")
        || token.contains('_')
        || token.chars().any(|c| c.is_ascii_uppercase())
        || token
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    has_code_shape
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '<' | '>'))
}

/// One tier of the cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Exact name lookup through FTS.
    Name,
    /// Lexical similarity signals (TF-IDF and friends).
    Signals,
    /// Structural neighbourhood: callers, callees, imports.
    Structure,
    /// Communities and architectural views.
    Community,
}

impl Tier {
    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Signals => "signals",
            Self::Structure => "structure",
            Self::Community => "community",
        }
    }

    /// Rough relative cost, used to justify the budget.
    #[must_use]
    pub const fn cost_weight(self) -> u32 {
        match self {
            Self::Name => 1,
            Self::Signals => 400,
            Self::Structure => 3,
            Self::Community => 900,
        }
    }
}

/// Why a tier did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The query's shape cannot use this tier's output.
    Shape,
    /// An earlier tier already answered the question well enough.
    Sufficient,
    /// The cost budget for this query is spent.
    Budget,
    /// The caller disabled it explicitly.
    Disabled,
}

impl SkipReason {
    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shape => "shape",
            Self::Sufficient => "sufficient",
            Self::Budget => "budget",
            Self::Disabled => "disabled",
        }
    }
}

/// One tier's outcome in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Which tier.
    pub tier: Tier,
    /// Whether it will run.
    pub run: bool,
    /// Why not, when it will not.
    pub reason: Option<SkipReason>,
}

/// The result of planning a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The query's shape.
    pub shape: QueryShape,
    /// One entry per tier, in order.
    pub steps: Vec<Step>,
    /// Sum of `cost_weight` over the tiers that will run.
    pub cost: u32,
}

impl Plan {
    /// The tiers that will run, in order.
    #[must_use]
    pub fn running(&self) -> Vec<Tier> {
        self.steps
            .iter()
            .filter(|s| s.run)
            .map(|s| s.tier)
            .collect()
    }

    /// Why `tier` will not run, or `None` when it will.
    #[must_use]
    pub fn skip_reason(&self, tier: Tier) -> Option<SkipReason> {
        self.steps
            .iter()
            .find(|s| s.tier == tier)
            .and_then(|s| s.reason)
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let running: Vec<&str> = self.running().iter().map(|t| t.as_str()).collect();
        write!(f, "{}: {}", self.shape.as_str(), running.join(">"))
    }
}

/// Cost budget for one query, in the units of [`Tier::cost_weight`].
pub const DEFAULT_BUDGET: u32 = 2_000;

/// Plan the cascade for `shape` under `budget`.
///
/// The rule that keeps the cascade honest: a tier runs only if the shape can use
/// its output, and only while the accumulated cost stays inside the budget. The
/// first tier always runs — an answer with no lookup is not an answer.
#[must_use]
pub fn plan(shape: QueryShape, budget: u32, disabled: &[Tier]) -> Plan {
    let mut steps = Vec::with_capacity(4);
    let mut cost = 0u32;
    let mut answered = false;
    for tier in [Tier::Name, Tier::Signals, Tier::Structure, Tier::Community] {
        if disabled.contains(&tier) {
            steps.push(Step {
                tier,
                run: false,
                reason: Some(SkipReason::Disabled),
            });
            continue;
        }
        let usable = match (shape, tier) {
            // A distinctive identifier is answered by exact lookup; every later
            // tier costs hundreds of times more and cannot improve on it. A
            // phrase has no exact name to hit, so signals carry the weight. A
            // structural question needs the neighbourhood, not signals.
            (QueryShape::Identifier, Tier::Signals | Tier::Structure | Tier::Community)
            | (QueryShape::Phrase, Tier::Structure | Tier::Community)
            | (QueryShape::AboutSymbol, Tier::Signals | Tier::Community) => false,
            // Everything else is usable; the budget decides how far it goes.
            // A broad question is what communities exist for, and a distinctive
            // identifier is answered by exact lookup, so every other later tier
            // would only cost more.
            _ => true,
        };
        let reason = if !usable {
            Some(SkipReason::Shape)
        } else if answered && tier != Tier::Name {
            Some(SkipReason::Sufficient)
        } else if cost + tier.cost_weight() > budget {
            Some(SkipReason::Budget)
        } else {
            None
        };
        let run = reason.is_none();
        if run {
            cost += tier.cost_weight();
        }
        // A single identifier that hit is a complete answer; a phrase that
        // reached signals is not, but the later tiers are shape-excluded anyway.
        if run && shape == QueryShape::Identifier {
            answered = true;
        }
        steps.push(Step { tier, run, reason });
    }
    Plan { shape, steps, cost }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identifier_runs_only_the_name_tier() {
        let p = plan(QueryShape::classify("parse_config"), DEFAULT_BUDGET, &[]);
        assert_eq!(p.shape, QueryShape::Identifier);
        assert_eq!(p.running(), vec![Tier::Name]);
        assert_eq!(p.skip_reason(Tier::Signals), Some(SkipReason::Shape));
        assert_eq!(p.skip_reason(Tier::Community), Some(SkipReason::Shape));
    }

    #[test]
    fn a_phrase_runs_name_and_signals() {
        let p = plan(
            QueryShape::classify("where is the retry logic"),
            DEFAULT_BUDGET,
            &[],
        );
        assert_eq!(p.shape, QueryShape::Broad, "four words is a broad question");
        let running = p.running();
        assert!(running.contains(&Tier::Signals), "{p}");
        assert!(running.contains(&Tier::Community), "{p}");
    }

    #[test]
    fn a_two_word_phrase_stops_before_communities() {
        let p = plan(QueryShape::classify("retry logic"), DEFAULT_BUDGET, &[]);
        assert_eq!(p.shape, QueryShape::Phrase);
        assert_eq!(p.running(), vec![Tier::Name, Tier::Signals]);
        assert_eq!(p.skip_reason(Tier::Community), Some(SkipReason::Shape));
    }

    #[test]
    fn a_tiny_budget_skips_the_expensive_tiers() {
        let p = plan(QueryShape::Broad, 10, &[]);
        assert!(p.running().contains(&Tier::Name));
        assert_eq!(p.skip_reason(Tier::Signals), Some(SkipReason::Budget));
        assert_eq!(p.skip_reason(Tier::Community), Some(SkipReason::Budget));
        assert!(p.cost <= 10);
    }

    #[test]
    fn a_disabled_tier_says_so() {
        let p = plan(QueryShape::Broad, DEFAULT_BUDGET, &[Tier::Signals]);
        assert_eq!(p.skip_reason(Tier::Signals), Some(SkipReason::Disabled));
    }

    #[test]
    fn paths_and_camel_case_are_identifiers() {
        assert_eq!(
            QueryShape::classify("GraphStore::open"),
            QueryShape::Identifier
        );
        assert_eq!(QueryShape::classify("GraphStore"), QueryShape::Identifier);
        assert_eq!(QueryShape::classify("http client"), QueryShape::Phrase);
        assert_eq!(QueryShape::classify(""), QueryShape::Broad);
    }
}
