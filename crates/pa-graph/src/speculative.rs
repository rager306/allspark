//! V15 — speculative ANN search: cheap over-fetch, exact recheck.
//!
//! A two-stage retriever can answer `k`-nearest cheaply by first ranking a
//! large candidate pool with a *coarse* (approximate) score and then
//! recomputing the **exact** score for only the most promising slice. Fetching
//! more than `k` candidates (`k · OVER_FETCH`) before the exact pass is what
//! buys recall: a candidate that the coarse score mis-ranks slightly still
//! survives into the exact recheck as long as it stays inside the over-fetched
//! window.
//!
//! ```text
//! speculative_search(q, candidates, exact, k):
//!   coarse   = sort candidates by coarse score, descending
//!   window   = coarse[.. min(len, k · OVER_FETCH)]
//!   rescored = exact(node, q) for node in window
//!   return   = top-k of rescored by exact score
//! ```
//!
//! When the coarse score is a faithful approximation, the returned top-k is
//! identical to a full brute-force exact top-k (the property the tests pin
//! down); the cost saved is the exact evaluation of everything outside the
//! window.

use std::collections::HashSet;

/// Over-fetch factor: how many coarse candidates survive to the exact pass.
pub const OVER_FETCH: usize = 4;

/// Speculative search with the default over-fetch factor ([`OVER_FETCH`]).
///
/// See [`speculative_search_with_factor`] for the mechanics. Returns the
/// `(node, exact_score)` top-k, sorted by descending exact score with ties
/// broken by ascending node id.
#[must_use]
pub fn speculative_search(
    query: &[f32],
    candidates: &[(i64, f32)],
    exact: impl Fn(i64, &[f32]) -> f32,
    k: usize,
) -> Vec<(i64, f32)> {
    speculative_search_with_factor(query, candidates, exact, k, OVER_FETCH)
}

/// Speculative search with an explicit over-fetch factor.
///
/// `candidates` are `(node, coarse_score)` pairs; `exact(node, query)` returns
/// the authoritative score. A `factor` of `0` is treated as `1`. `k == 0` or an
/// empty candidate pool yields an empty result.
#[must_use]
pub fn speculative_search_with_factor(
    query: &[f32],
    candidates: &[(i64, f32)],
    exact: impl Fn(i64, &[f32]) -> f32,
    k: usize,
    factor: usize,
) -> Vec<(i64, f32)> {
    if k == 0 || candidates.is_empty() {
        return Vec::new();
    }

    // Rank the candidate pool by coarse score (desc), ties by id (asc).
    let mut coarse: Vec<(i64, f32)> = candidates.to_vec();
    coarse.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let window = k.saturating_mul(factor.max(1)).min(coarse.len());
    let mut seen: HashSet<i64> = HashSet::with_capacity(window);
    let mut rescored: Vec<(i64, f32)> = Vec::with_capacity(window);
    for &(node, _coarse_score) in coarse.iter().take(window) {
        if seen.insert(node) {
            rescored.push((node, exact(node, query)));
        }
    }

    rescored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    rescored.truncate(k);
    rescored
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic corpus: `n` unit-ish vectors whose exact dot with the query
    /// decreases with the node id, so the exact top-k is `0, 1, .., k-1`.
    fn corpus(n: i64) -> (Vec<f32>, Vec<Vec<f32>>) {
        let query = vec![1.0_f32, 0.0, 0.0];
        let vectors = (0..n)
            .map(|i| vec![100.0 - i as f32, 0.0, 0.0])
            .collect();
        (query, vectors)
    }

    fn exact_for(vectors: &[Vec<f32>]) -> impl Fn(i64, &[f32]) -> f32 + '_ {
        move |node, query| {
            let v = &vectors[node as usize];
            v.iter().zip(query).map(|(a, b)| a * b).sum()
        }
    }

    #[test]
    fn matches_brute_force_top_k() {
        let (query, vectors) = corpus(40);
        let exact = exact_for(&vectors);

        // Coarse scores are the exact score plus bounded deterministic noise,
        // so the coarse order is a small perturbation of the true order.
        let candidates: Vec<(i64, f32)> = (0..40)
            .map(|i| {
                let noise = ((i * 37) % 11) as f32 / 11.0 - 0.5; // in [-0.5, 0.5)
                (i, exact(i, &query) + noise)
            })
            .collect();

        for k in [1usize, 3, 5, 10] {
            let got = speculative_search(&query, &candidates, exact_for(&vectors), k);
            let mut brute: Vec<(i64, f32)> =
                (0..40).map(|i| (i, exact(i, &query))).collect();
            brute.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });
            brute.truncate(k);
            assert_eq!(got, brute, "speculative top-{k} must equal brute force");
        }
    }

    #[test]
    fn small_pool_is_fully_rescored() {
        let query = vec![1.0_f32];
        let vectors = vec![vec![3.0_f32], vec![1.0], vec![2.0]];
        let exact = exact_for(&vectors);
        let candidates = vec![(0, 0.0), (1, 0.0), (2, 0.0)];
        let got = speculative_search(&query, &candidates, exact, 2);
        assert_eq!(got, vec![(0, 3.0), (2, 2.0)]);
    }

    #[test]
    fn zero_k_and_empty_pool_are_empty() {
        let query = vec![1.0_f32];
        let exact = |_: i64, _: &[f32]| 0.0_f32;
        assert_eq!(speculative_search(&query, &[(1, 1.0)], exact, 0), Vec::<(i64, f32)>::new());
        assert_eq!(speculative_search(&query, &[], exact, 3), Vec::<(i64, f32)>::new());
    }

    #[test]
    fn only_window_is_exactly_scored() {
        // The exact closure counts how many nodes it was asked about; the
        // over-fetch window bounds that to k · factor.
        let query = vec![1.0_f32];
        let candidates: Vec<(i64, f32)> = (0..100).map(|i| (i, 100.0 - i as f32)).collect();
        let calls = std::cell::Cell::new(0usize);
        let exact = |_: i64, _: &[f32]| {
            calls.set(calls.get() + 1);
            0.0_f32
        };
        let _ = speculative_search_with_factor(&query, &candidates, exact, 5, 4);
        assert_eq!(calls.get(), 20, "k·factor = 5·4 candidates rescored");
    }
}
