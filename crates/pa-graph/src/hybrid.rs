//! Reciprocal Rank Fusion (RRF) and Maximal Marginal Relevance (MMR).
//!
//! RRF merges several ranked channels without comparing their raw scores —
//! it works on rank position only, which is exactly what fusing BM25, regex,
//! and graph-degree channels needs.
//!
//! ```text
//! RRF(d) = Σ_channels 1 / (k + rank_channel(d))         k = 60
//! MMR(d) = λ · sim(q, d) − (1 − λ) · max_{s∈selected} sim(d, s)
//! ```

use std::collections::HashMap;

/// RRF rank constant; 60 is the value Cormack et al. (2009) found optimal.
pub const RRF_K: f32 = 60.0;

/// One ranked channel: ids best-first.
pub type RankedList = Vec<i64>;

/// Fuse ranked channels with reciprocal rank fusion.
///
/// Channels may have different lengths and need not be score-comparable.
/// Returns `(id, rrf_score)` sorted by descending score, ties broken by id
/// for determinism.
#[must_use]
pub fn rrf_fuse(channels: &[RankedList]) -> Vec<(i64, f32)> {
    let mut scores: HashMap<i64, f32> = HashMap::new();
    for channel in channels {
        for (rank, &id) in channel.iter().enumerate() {
            // rank is 0-based; RRF uses 1-based rank.
            *scores.entry(id).or_insert(0.0) += 1.0 / (RRF_K + rank_of(rank) + 1.0);
        }
    }
    let mut merged: Vec<(i64, f32)> = scores.into_iter().collect();
    merged.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    merged
}

/// Convert a rank index to `f32` for the RRF denominator. Ranks are bounded
/// by the candidate pool (a few thousand at most), so the f32 mantissa is
/// never the limiting factor; the conversion is exact for any realistic rank.
#[allow(clippy::cast_precision_loss)]
fn rank_of(rank: usize) -> f32 {
    rank as f32
}

/// Fuse channels carrying explicit weights: `Σ w_i / (k + rank_i)`.
#[must_use]
pub fn rrf_fuse_weighted(channels: &[(RankedList, f32)]) -> Vec<(i64, f32)> {
    let mut scores: HashMap<i64, f32> = HashMap::new();
    for (channel, weight) in channels {
        for (rank, &id) in channel.iter().enumerate() {
            *scores.entry(id).or_insert(0.0) += weight / (RRF_K + rank_of(rank) + 1.0);
        }
    }
    let mut merged: Vec<(i64, f32)> = scores.into_iter().collect();
    merged.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    merged
}

/// A candidate for MMR reranking: relevance plus its feature vector.
#[derive(Debug, Clone)]
pub struct MmrCandidate {
    /// Caller-assigned id.
    pub id: i64,
    /// Query-relevance score (already normalized to `[0, 1]` by the caller).
    pub relevance: f32,
    /// Feature vector used to measure inter-candidate similarity.
    pub features: Vec<f32>,
}

/// Cosine similarity of two equal-length vectors; `0.0` on any degenerate input.
#[must_use]
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom <= f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

/// Rerank `candidates` for diversity with maximal marginal relevance.
///
/// `lambda == 1.0` is pure relevance; `lambda == 0.0` is pure diversity.
/// The similarity between two candidates is the cosine of their feature
/// vectors. Returns at most `k` candidates, greedily selected.
#[must_use]
pub fn mmr_rerank(candidates: &[MmrCandidate], k: usize, lambda: f32) -> Vec<MmrCandidate> {
    let lambda = lambda.clamp(0.0, 1.0);
    if k == 0 || candidates.is_empty() {
        return Vec::new();
    }
    let k = k.min(candidates.len());
    let mut remaining: Vec<MmrCandidate> = candidates.to_vec();
    let mut selected: Vec<MmrCandidate> = Vec::with_capacity(k);

    while selected.len() < k {
        let mut best_idx = 0;
        let mut best_score = f32::NEG_INFINITY;
        for (idx, cand) in remaining.iter().enumerate() {
            let redundancy = selected
                .iter()
                .map(|s| cosine(&cand.features, &s.features))
                .fold(f32::NEG_INFINITY, f32::max);
            let redundancy = if selected.is_empty() { 0.0 } else { redundancy };
            let score = lambda * cand.relevance - (1.0 - lambda) * redundancy;
            if score > best_score {
                best_score = score;
                best_idx = idx;
            }
        }
        selected.push(remaining.remove(best_idx));
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_single_channel_ranks_in_order() {
        let fused = rrf_fuse(&[vec![10, 20, 30]]);
        assert_eq!(
            fused.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
        // rank 1 → 1/61, rank 2 → 1/62.
        assert!((fused[0].1 - 1.0 / 61.0).abs() < 1e-6);
        assert!((fused[1].1 - 1.0 / 62.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_rewards_agreement_across_channels() {
        // id 2 appears high in both channels; id 1 only in the first.
        let fused = rrf_fuse(&[vec![1, 2], vec![2, 3]]);
        assert_eq!(fused[0].0, 2, "the doubly-ranked id wins: {fused:?}");
    }

    #[test]
    fn rrf_formula_matches_reference() {
        let fused = rrf_fuse(&[vec![7], vec![7]]);
        let expected = 2.0 / 61.0;
        assert!((fused[0].1 - expected).abs() < 1e-6);
    }

    #[test]
    fn rrf_weighted_scales_channels() {
        let fused = rrf_fuse_weighted(&[(vec![1], 1.0), (vec![2], 3.0)]);
        // id 2 gets 3/61, id 1 gets 1/61.
        assert_eq!(fused[0].0, 2);
    }

    #[test]
    fn cosine_identity_and_orthogonal() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn mmr_pure_relevance_keeps_order() {
        let cands = vec![
            MmrCandidate {
                id: 1,
                relevance: 0.9,
                features: vec![1.0, 0.0],
            },
            MmrCandidate {
                id: 2,
                relevance: 0.8,
                features: vec![1.0, 0.0],
            },
            MmrCandidate {
                id: 3,
                relevance: 0.5,
                features: vec![0.0, 1.0],
            },
        ];
        let out = mmr_rerank(&cands, 2, 1.0);
        assert_eq!(out.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn mmr_diversity_avoids_duplicates() {
        // Two near-identical high-relevance candidates, one diverse lower one.
        let cands = vec![
            MmrCandidate {
                id: 1,
                relevance: 1.0,
                features: vec![1.0, 0.0],
            },
            MmrCandidate {
                id: 2,
                relevance: 0.99,
                features: vec![1.0, 0.0],
            },
            MmrCandidate {
                id: 3,
                relevance: 0.5,
                features: vec![0.0, 1.0],
            },
        ];
        let out = mmr_rerank(&cands, 2, 0.5);
        let ids: Vec<i64> = out.iter().map(|c| c.id).collect();
        assert_eq!(ids[0], 1);
        assert_eq!(
            ids[1], 3,
            "the diverse candidate should beat the near-duplicate: {ids:?}"
        );
    }

    #[test]
    fn mmr_lambda_zero_is_pure_diversity() {
        let cands = vec![
            MmrCandidate {
                id: 1,
                relevance: 1.0,
                features: vec![1.0, 0.0],
            },
            MmrCandidate {
                id: 2,
                relevance: 0.9,
                features: vec![1.0, 0.0],
            },
            MmrCandidate {
                id: 3,
                relevance: 0.1,
                features: vec![0.0, 1.0],
            },
        ];
        let out = mmr_rerank(&cands, 2, 0.0);
        assert_eq!(out[1].id, 3);
    }

    #[test]
    fn mmr_edge_cases() {
        assert_eq!(mmr_rerank(&[], 3, 0.5).len(), 0);
        let c = vec![MmrCandidate {
            id: 1,
            relevance: 1.0,
            features: vec![1.0],
        }];
        assert_eq!(mmr_rerank(&c, 0, 0.5).len(), 0);
        assert_eq!(mmr_rerank(&c, 5, 0.5).len(), 1);
    }
}
