//! V11 — Matryoshka prefix representations (pure `std`).
//!
//! A Matryoshka embedding is trained so that its first `k` dimensions are
//! themselves a usable embedding: truncating to `64`, `128`, `256`, `768`
//! dimensions yields progressively coarser but *rank-preserving* scores. This
//! module implements the runtime half — prefix cosine and prefix ranking — over
//! any dense `f32` vector, so a search can score cheap short prefixes and only
//! fall back to the full width when the top of the list is tight.
//!
//! [`MatryoshkaVector::score_prefix`] is cosine similarity over the first `k`
//! dimensions; [`MatryoshkaVector::rank_stable`] sorts a candidate set by that
//! prefix score, returning `(index, score)` pairs in descending order.

/// The canonical prefix widths a Matryoshka model exposes.
pub const DEFAULT_PREFIXES: [usize; 4] = [64, 128, 256, 768];

/// A dense vector viewed through its Matryoshka prefixes.
#[derive(Clone, Debug, PartialEq)]
pub struct MatryoshkaVector {
    data: Vec<f32>,
}

impl MatryoshkaVector {
    /// Wraps a dense vector.
    #[must_use]
    pub fn new(data: &[f32]) -> Self {
        Self {
            data: data.to_vec(),
        }
    }

    /// The full dimensionality.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.data.len()
    }

    /// The full-precision vector.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.data
    }

    /// Cosine similarity over the first `min(k, dim)` dimensions.
    ///
    /// Returns `0.0` if either prefix is all-zero (undefined direction) or if
    /// `k == 0`.
    #[must_use]
    pub fn score_prefix(&self, other: &Self, k: usize) -> f32 {
        let k = k.min(self.dim()).min(other.dim());
        if k == 0 {
            return 0.0;
        }
        let mut dot = 0.0_f32;
        let mut na = 0.0_f32;
        let mut nb = 0.0_f32;
        for (&a, &b) in self.data[..k].iter().zip(&other.data[..k]) {
            dot = a.mul_add(b, dot);
            na = a.mul_add(a, na);
            nb = b.mul_add(b, nb);
        }
        let denom = na.sqrt() * nb.sqrt();
        if denom > 0.0 {
            dot / denom
        } else {
            0.0
        }
    }

    /// Ranks `others` against `self` by the `k`-dimension prefix score,
    /// descending. Ties keep input order (the sort is stable), so the returned
    /// list is deterministic for a fixed input.
    #[must_use]
    pub fn rank_stable(&self, others: &[Self], k: usize) -> Vec<(usize, f32)> {
        let mut scored: Vec<(usize, f32)> = others
            .iter()
            .enumerate()
            .map(|(i, o)| (i, self.score_prefix(o, k)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }
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

        /// A standard-normal sample (Box–Muller) from the fixed generator.
        fn next_gauss(&mut self) -> f32 {
            let u1 = (self.next_u32() as f32 + 1.0) / (u32::MAX as f32 + 1.0);
            let u2 = (self.next_u32() as f32 + 1.0) / (u32::MAX as f32 + 1.0);
            let r = (-2.0 * u1.ln()).sqrt();
            r * (std::f32::consts::TAU * u2).cos()
        }
    }

    /// Builds a Matryoshka-like corpus: energy concentrated in the first
    /// `principal` dimensions (the "trained" prefix), with a decaying tail.
    fn corpus(
        rng: &mut XorShift32,
        n: usize,
        dim: usize,
        principal: usize,
    ) -> Vec<MatryoshkaVector> {
        (0..n)
            .map(|_| {
                let mut v = vec![0.0_f32; dim];
                for (i, x) in v.iter_mut().enumerate() {
                    let scale = if i < principal { 1.0 } else { 0.15 };
                    *x = rng.next_gauss() * scale;
                }
                MatryoshkaVector::new(&v)
            })
            .collect()
    }

    fn spearman(left: &[usize], right: &[usize]) -> f32 {
        // Ranks of an item in each ordering; both are permutations of 0..n,
        // so an item's rank is its position.
        let n = left.len();
        let mut rank_left = vec![0.0_f32; n];
        let mut rank_right = vec![0.0_f32; n];
        for (pos, &item) in left.iter().enumerate() {
            rank_left[item] = pos as f32;
        }
        for (pos, &item) in right.iter().enumerate() {
            rank_right[item] = pos as f32;
        }
        let mean = (n as f32 - 1.0) / 2.0;
        let mut num = 0.0_f32;
        let mut var_left = 0.0_f32;
        let mut var_right = 0.0_f32;
        for item in 0..n {
            let x = rank_left[item] - mean;
            let y = rank_right[item] - mean;
            num = x.mul_add(y, num);
            var_left = x.mul_add(x, var_left);
            var_right = y.mul_add(y, var_right);
        }
        num / (var_left.sqrt() * var_right.sqrt())
    }

    fn order_of(ranked: &[(usize, f32)]) -> Vec<usize> {
        ranked.iter().map(|&(i, _)| i).collect()
    }

    #[test]
    fn prefix_score_is_cosine_on_the_prefix() {
        let a = MatryoshkaVector::new(&[1.0, 0.0, 5.0, 5.0]);
        let b = MatryoshkaVector::new(&[1.0, 0.0, -5.0, -5.0]);
        // Over the first two dims the vectors are identical -> cosine 1.
        assert!((a.score_prefix(&b, 2) - 1.0).abs() < 1e-6);
        // Over all four dims only the tail is opposite: dot = 1 - 50 = -49,
        // ||a||^2 = ||b||^2 = 51 -> cosine -49/51.
        let expected = -49.0_f32 / 51.0;
        assert!((a.score_prefix(&b, 4) - expected).abs() < 1e-6);
        // k beyond dim clamps to the full width.
        assert!((a.score_prefix(&b, 99) - expected).abs() < 1e-6);
        assert_eq!(a.score_prefix(&b, 0), 0.0);
    }

    #[test]
    fn prefix_128_ranking_correlates_with_the_full_ranking() {
        let mut rng = XorShift32::new(0x2024_0601);
        let dim = 768;
        let principal = 128;
        let docs = corpus(&mut rng, 64, dim, principal);
        let query = MatryoshkaVector::new(
            &(0..dim)
                .map(|i| rng.next_gauss() * if i < principal { 1.0 } else { 0.15 })
                .collect::<Vec<f32>>(),
        );

        let full = order_of(&query.rank_stable(&docs, dim));
        let short = order_of(&query.rank_stable(&docs, 128));
        let rho = spearman(&full, &short);
        assert!(rho > 0.9, "Spearman on prefix-128 is {rho}, want > 0.9");
    }

    #[test]
    fn wider_prefixes_are_more_faithful() {
        let mut rng = XorShift32::new(0x5151_5151);
        let dim = 768;
        let docs = corpus(&mut rng, 48, dim, 256);
        let query = MatryoshkaVector::new(
            &(0..dim)
                .map(|i| rng.next_gauss() * if i < 256 { 1.0 } else { 0.2 })
                .collect::<Vec<f32>>(),
        );
        let full = order_of(&query.rank_stable(&docs, dim));
        let rho64 = spearman(&full, &order_of(&query.rank_stable(&docs, 64)));
        let rho256 = spearman(&full, &order_of(&query.rank_stable(&docs, 256)));
        assert!(
            rho256 >= rho64,
            "prefix-256 ({rho256}) should be at least as faithful as prefix-64 ({rho64})"
        );
    }

    #[test]
    fn accessors_expose_the_full_vector() {
        let v = MatryoshkaVector::new(&[1.0, 2.0, 3.0]);
        assert_eq!(v.dim(), 3);
        assert_eq!(v.as_slice(), &[1.0, 2.0, 3.0]);
        // A zero-length prefix scores 0 for every candidate.
        let others = vec![MatryoshkaVector::new(&[1.0, 0.0, 0.0])];
        assert_eq!(v.rank_stable(&others, 0)[0].1, 0.0);
    }

    #[test]
    fn ranking_is_deterministic_and_descending() {
        let docs = vec![
            MatryoshkaVector::new(&[1.0, 0.0]),
            MatryoshkaVector::new(&[0.0, 1.0]),
            MatryoshkaVector::new(&[1.0, 1.0]),
        ];
        let query = MatryoshkaVector::new(&[1.0, 0.0]);
        let ranked = query.rank_stable(&docs, 2);
        assert_eq!(ranked[0].0, 0); // exact match first
        for pair in ranked.windows(2) {
            assert!(pair[0].1 >= pair[1].1);
        }
    }
}
