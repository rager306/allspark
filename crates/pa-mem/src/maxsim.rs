//! V12 — late-interaction `MaxSim` (pure `std`).
//!
//! ColBERT-style late interaction keeps one vector *per token* on both sides.
//! The score between a query and a document is the sum, over query tokens, of
//! the best cosine similarity that query token finds among the document tokens:
//!
//! ```text
//! MaxSim(q, d) = Σ_i max_j cos(q_i, d_j)
//! ```
//!
//! Unlike a single pooled embedding, this preserves token-level matching: a
//! document that contains one token exactly matching a query token earns a full
//! `1.0` contribution for it even if the rest of the document is unrelated.
//!
//! [`maxsim_batch`] scores a whole candidate set and returns `(index, score)`
//! pairs sorted descending — a stable, deterministic ranking.

/// Cosine similarity between two equal-length vectors, `0.0` when either is
/// all-zero.
///
/// # Panics
///
/// Panics if the vectors have different lengths.
#[must_use]
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(
        a.len(),
        b.len(),
        "cosine: dimension mismatch ({} vs {})",
        a.len(),
        b.len()
    );
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for (&x, &y) in a.iter().zip(b) {
        dot = x.mul_add(y, dot);
        na = x.mul_add(x, na);
        nb = y.mul_add(y, nb);
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom > 0.0 {
        dot / denom
    } else {
        0.0
    }
}

/// The late-interaction `MaxSim` score between a query token set and a document
/// token set.
///
/// Returns `0.0` if either side is empty. Every query token contributes the
/// maximum cosine it finds across the document tokens.
#[must_use]
pub fn maxsim(query: &[Vec<f32>], doc: &[Vec<f32>]) -> f32 {
    if query.is_empty() || doc.is_empty() {
        return 0.0;
    }
    let mut total = 0.0_f32;
    for q in query {
        let mut best = f32::NEG_INFINITY;
        for d in doc {
            let sim = cosine(q, d);
            if sim > best {
                best = sim;
            }
        }
        total += best;
    }
    total
}

/// Scores each document in `docs` against `query` and returns `(index, score)`
/// pairs sorted by score descending (stable, so ties keep input order).
#[must_use]
pub fn maxsim_batch(query: &[Vec<f32>], docs: &[Vec<Vec<f32>>]) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = docs
        .iter()
        .enumerate()
        .map(|(i, doc)| (i, maxsim(query, doc)))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared xorshift32 test family.
    struct XorShift32(u32);

    impl XorShift32 {
        fn new(seed: u32) -> Self {
            Self(seed | 1)
        }

        fn next_u32(&mut self) -> u32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            x
        }

        /// A value in `[-1, 1)`.
        fn next_unit(&mut self) -> f32 {
            (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    #[test]
    fn the_two_by_two_hand_example() {
        // q0 = e0, q1 = e1; d0 = e0, d1 = (0, 1).
        let query = vec![vec![1.0_f32, 0.0], vec![0.0, 1.0]];
        let doc = vec![vec![1.0_f32, 0.0], vec![0.0, 1.0]];
        // q0 matches d0 exactly (1.0), q1 matches d1 exactly (1.0).
        assert!((maxsim(&query, &doc) - 2.0).abs() < 1e-6);

        // Reversing the query token order does not change MaxSim.
        let query_rev = vec![vec![0.0_f32, 1.0], vec![1.0, 0.0]];
        assert!((maxsim(&query_rev, &doc) - 2.0).abs() < 1e-6);

        // A document with only d0 loses the q1 contribution (best is 0.0).
        let doc_half = vec![vec![1.0_f32, 0.0]];
        assert!((maxsim(&query, &doc_half) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_exact_token_match_scores_higher() {
        let mut rng = XorShift32::new(0x1357_9bdf);
        let dim = 16;
        let query: Vec<Vec<f32>> = (0..4)
            .map(|_| (0..dim).map(|_| rng.next_unit()).collect())
            .collect();

        // doc_exact embeds the query's own tokens; doc_noise is unrelated.
        let doc_exact = query.clone();
        let doc_noise: Vec<Vec<f32>> = (0..4)
            .map(|_| (0..dim).map(|_| rng.next_unit()).collect())
            .collect();

        let s_exact = maxsim(&query, &doc_exact);
        let s_noise = maxsim(&query, &doc_noise);
        assert!(s_exact > s_noise);
        assert!((s_exact - query.len() as f32).abs() < 1e-5);
    }

    #[test]
    fn batch_ranks_the_matching_document_first() {
        let mut rng = XorShift32::new(0x2468_ace0);
        let dim = 8;
        let query: Vec<Vec<f32>> = (0..3)
            .map(|_| (0..dim).map(|_| rng.next_unit()).collect())
            .collect();
        let noise: Vec<Vec<f32>> = (0..3)
            .map(|_| (0..dim).map(|_| rng.next_unit()).collect())
            .collect();
        let docs = vec![noise.clone(), query.clone(), noise];
        let ranked = maxsim_batch(&query, &docs);
        assert_eq!(ranked[0].0, 1);
        for pair in ranked.windows(2) {
            assert!(pair[0].1 >= pair[1].1);
        }
    }

    #[test]
    fn batch_over_no_documents_is_empty() {
        let query = vec![vec![1.0_f32, 0.0]];
        assert_eq!(maxsim_batch(&query, &[]).len(), 0);
    }

    #[test]
    fn empty_inputs_score_zero() {
        let q = vec![vec![1.0_f32, 0.0]];
        assert_eq!(maxsim(&[], &q).to_bits(), 0.0_f32.to_bits());
        assert_eq!(maxsim(&q, &[]).to_bits(), 0.0_f32.to_bits());
        assert_eq!(
            cosine(&[0.0, 0.0], &[1.0, 0.0]).to_bits(),
            0.0_f32.to_bits()
        );
    }
}
