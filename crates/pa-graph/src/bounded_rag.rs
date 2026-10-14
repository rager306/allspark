//! M8 — bounded RAG context selection.
//!
//! Retrieval returns more fragments than fit in a prompt. [`bounded_select`]
//! greedily takes the highest-scoring fragments that still fit a token budget,
//! skipping any that would overflow. The result is deterministic (score
//! descending, ties by id) and never exceeds the budget.

/// One retrieval candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// Stable candidate id.
    pub id: String,
    /// Relevance score (higher is better).
    pub score: f64,
    /// Token cost of including this fragment.
    pub tokens: usize,
}

impl Candidate {
    /// A new candidate.
    #[must_use]
    pub fn new(id: impl Into<String>, score: f64, tokens: usize) -> Self {
        Self {
            id: id.into(),
            score,
            tokens,
        }
    }
}

/// The context budget: a token ceiling and an optional fragment count cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RagBudget {
    /// Maximum total tokens.
    pub tokens: usize,
    /// Maximum number of fragments (`usize::MAX` = no cap).
    pub max_fragments: usize,
}

impl RagBudget {
    /// A budget with a token ceiling and no fragment cap.
    #[must_use]
    pub fn tokens(tokens: usize) -> Self {
        Self {
            tokens,
            max_fragments: usize::MAX,
        }
    }

    /// A budget with both a token ceiling and a fragment cap.
    #[must_use]
    pub fn new(tokens: usize, max_fragments: usize) -> Self {
        Self {
            tokens,
            max_fragments,
        }
    }
}

/// Greedily select candidates by descending score within `budget`.
///
/// A candidate that does not fit the remaining budget is skipped (not fatal),
/// so a small high-score fragment after a large one is still admitted.
#[must_use]
pub fn bounded_select(candidates: &[Candidate], budget: RagBudget) -> Vec<Candidate> {
    let mut ordered: Vec<Candidate> = candidates.to_vec();
    ordered.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut used = 0usize;
    let mut selected = Vec::new();
    for candidate in ordered {
        if selected.len() >= budget.max_fragments {
            break;
        }
        if used + candidate.tokens <= budget.tokens {
            used += candidate.tokens;
            selected.push(candidate);
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_highest_scores_within_budget() {
        let candidates = vec![
            Candidate::new("a", 0.9, 10),
            Candidate::new("b", 0.8, 10),
            Candidate::new("c", 0.7, 10),
        ];
        let selected = bounded_select(&candidates, RagBudget::tokens(25));
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].id, "a");
        assert_eq!(selected[1].id, "b");
    }

    #[test]
    fn skips_fragments_that_do_not_fit() {
        let candidates = vec![
            Candidate::new("big", 0.9, 100),
            Candidate::new("small", 0.8, 5),
        ];
        let selected = bounded_select(&candidates, RagBudget::tokens(10));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "small", "the small one still fits");
    }

    #[test]
    fn respects_the_fragment_cap() {
        let candidates = vec![
            Candidate::new("a", 0.9, 1),
            Candidate::new("b", 0.8, 1),
            Candidate::new("c", 0.7, 1),
        ];
        let selected = bounded_select(&candidates, RagBudget::new(100, 2));
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn ties_break_by_id_for_determinism() {
        let candidates = vec![Candidate::new("b", 0.5, 1), Candidate::new("a", 0.5, 1)];
        let selected = bounded_select(&candidates, RagBudget::tokens(10));
        assert_eq!(selected[0].id, "a");
    }

    #[test]
    fn empty_and_zero_budget_are_safe() {
        assert!(bounded_select(&[], RagBudget::tokens(10)).is_empty(), "empty input");
        let candidates = vec![Candidate::new("a", 1.0, 5)];
        assert!(bounded_select(&candidates, RagBudget::tokens(0)).is_empty(), "zero budget");
    }
}
