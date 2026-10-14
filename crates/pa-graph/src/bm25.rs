//! Robertson BM25 scoring (Robertson & Zaragoza, 2009).
//!
//! `pa-graph` ranks candidates from several channels; this module is the
//! shared scorer for channels whose scores are computed in Rust (rather than
//! delegated to SQLite's FTS5 `bm25()`).
//!
//! ```text
//! BM25(q, d) = Σ_{t∈q} IDF(t) · TF_norm(t, d)
//! IDF(t)     = ln((N − df(t) + 0.5) / (df(t) + 0.5) + 1)
//! TF_norm    = tf · (k1 + 1) / (tf + k1 · (1 − b + b · |d| / avgdl))
//! ```

/// Robertson default term-frequency saturation.
pub const K1: f32 = 1.2;
/// Robertson default length-normalization.
pub const B: f32 = 0.75;

/// A scored document id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scored {
    /// Caller-assigned document id.
    pub id: i64,
    /// BM25 score (higher is better).
    pub score: f32,
}

/// Compute BM25 for a single term against a corpus summary.
///
/// `n_docs` is the corpus size, `df` the document frequency of the term,
/// `tf` the term frequency in this document, `doc_len`/`avg_doc_len` the
/// length normalization inputs.
#[must_use]
pub fn bm25_term(n_docs: u32, df: u32, tf: u32, doc_len: u32, avg_doc_len: f32) -> f32 {
    let n = f64::from(n_docs);
    let df = f64::from(df);
    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
    let tf = f64::from(tf);
    let dl = f64::from(doc_len);
    let avgdl = f64::from(avg_doc_len).max(f64::EPSILON);
    let k1 = f64::from(K1);
    let b = f64::from(B);
    let tf_norm = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avgdl));
    // The accumulator is f64 for precision; the public score is f32 by
    // contract (callers compare/normalize in f32). The cast is intentional.
    #[allow(clippy::cast_possible_truncation)]
    {
        (idf * tf_norm) as f32
    }
}

/// Score every term of a query against the same document and sum.
#[must_use]
pub fn bm25_sum(terms: &[(u32, u32)], n_docs: u32, doc_len: u32, avg_doc_len: f32) -> f32 {
    terms
        .iter()
        .map(|&(df, tf)| bm25_term(n_docs, df, tf, doc_len, avg_doc_len))
        .sum()
}

/// Normalize a score list to `[0, 1]` by its own min/max.
///
/// Used by MMR and RSF-style fusion; a single-element list maps to `0.0`
/// (there is no spread to normalize against).
#[must_use]
pub fn min_max_normalize(scores: &[f32]) -> Vec<f32> {
    if scores.is_empty() {
        return Vec::new();
    }
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &s in scores {
        min = min.min(s);
        max = max.max(s);
    }
    let range = (max - min).max(1e-10);
    scores.iter().map(|s| (s - min) / range).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idf_floor_keeps_scores_positive() {
        // df == N → IDF = ln(0.5/(N+0.5)+1) > 0, never negative.
        let s = bm25_term(10, 10, 1, 5, 5.0);
        assert!(s > 0.0, "score {s} should be positive");
    }

    #[test]
    fn higher_tf_scores_higher() {
        let low = bm25_term(10, 2, 1, 5, 5.0);
        let high = bm25_term(10, 2, 5, 5, 5.0);
        assert!(
            high > low,
            "tf saturation must be monotonic: {high} vs {low}"
        );
    }

    #[test]
    fn rarer_term_scores_higher() {
        let common = bm25_term(100, 90, 1, 5, 5.0);
        let rare = bm25_term(100, 2, 1, 5, 5.0);
        assert!(rare > common);
    }

    #[test]
    fn length_normalization_penalizes_long_docs() {
        let short = bm25_term(10, 2, 3, 5, 10.0);
        let long = bm25_term(10, 2, 3, 50, 10.0);
        assert!(short > long, "short doc should win: {short} vs {long}");
    }

    #[test]
    fn sum_over_terms_is_additive() {
        let terms = [(2, 1), (5, 3)];
        let total = bm25_sum(&terms, 10, 8, 8.0);
        let expected = bm25_term(10, 2, 1, 8, 8.0) + bm25_term(10, 5, 3, 8, 8.0);
        assert!((total - expected).abs() < 1e-5);
    }

    #[test]
    fn normalize_maps_to_unit_range() {
        let n = min_max_normalize(&[1.0, 3.0, 5.0]);
        assert_eq!(n[0], 0.0);
        assert!((n[2] - 1.0).abs() < 1e-6);
        assert!((n[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn normalize_single_is_zero() {
        assert_eq!(min_max_normalize(&[7.0]), vec![0.0]);
        assert_eq!(min_max_normalize(&[]).len(), 0);
    }
}
