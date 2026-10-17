//! V5 — RaBitQ-style 1-bit quantization (pure `std`).
//!
//! Each dimension keeps a single sign bit: `b_i = (v_i >= 0)`. The bits are
//! packed into `u64` words (bit `i` lives in word `i / 64`, position
//! `i % 64`) — no `unsafe`, no `Vec<bool>` overhead. Alongside the codes the
//! encoder stores the vector's L2 norm and a per-vector scale
//!
//! ```text
//! scale = ||v|| / sqrt(D)
//! ```
//!
//! so that the reconstruction `v_hat_i = scale * (2 b_i - 1)` has the same
//! norm as `v`: `||v_hat||^2 = D * scale^2 = ||v||^2`.
//!
//! The dot-product estimator is then just the signed sum of the query entries
//! weighted by the scale:
//!
//! ```text
//! estimate = scale * Σ (2 b_i - 1) * q_i
//! ```
//!
//! This is the classic sign-random-projection inner-product estimator. Its
//! error is bounded rigorously by Cauchy–Schwarz: since `|est - <v,q>| =
//! |<v_hat - v, q>| <= ||v_hat - v|| * ||q||`, [`RabitqVector::error_bound`]
//! returns `||v_hat - v||`, so `|est - <v,q>| <= error_bound() * ||q||`.

/// A 1-bit (sign) quantized vector: packed sign bits plus the scale, norm and
/// `L1` norm needed to reconstruct and to bound the estimation error.
#[derive(Clone, Debug, PartialEq)]
pub struct RabitqVector {
    words: Vec<u64>,
    scale: f32,
    norm: f32,
    l1: f32,
    dim: usize,
}

impl RabitqVector {
    /// Encodes `v` into sign bits, a per-vector scale, and the norm.
    ///
    /// An empty vector yields an empty code with zero scale.
    #[must_use]
    pub fn encode(v: &[f32]) -> Self {
        let dim = v.len();
        let words = dim.div_ceil(64);
        let mut bits = vec![0_u64; words];
        let mut sq = 0.0_f32;
        let mut l1 = 0.0_f32;
        for (i, &x) in v.iter().enumerate() {
            if x >= 0.0 {
                bits[i / 64] |= 1_u64 << (i % 64);
            }
            sq = x.mul_add(x, sq);
            l1 += x.abs();
        }
        let norm = sq.sqrt();
        let scale = if dim > 0 {
            norm / (dim as f32).sqrt()
        } else {
            0.0
        };
        Self {
            words: bits,
            scale,
            norm,
            l1,
            dim,
        }
    }

    /// The number of encoded dimensions.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// The per-vector scale (`||v|| / sqrt(D)`).
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The L2 norm of the encoded vector.
    #[must_use]
    pub fn norm(&self) -> f32 {
        self.norm
    }

    /// The packed sign bits (bit `i` set means `v_i >= 0`).
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// The sign of dimension `i` as `+1.0` / `-1.0` (out-of-range reads are
    /// treated as `+1.0`, which never happens for well-formed use).
    fn sign_at(&self, i: usize) -> f32 {
        let set = (self.words[i / 64] >> (i % 64)) & 1 == 1;
        if set {
            1.0
        } else {
            -1.0
        }
    }

    /// Estimates the dot product `<v, q>` for a full-precision query `q`.
    ///
    /// # Panics
    ///
    /// Panics if `q.len()` differs from the encoded dimension.
    #[must_use]
    pub fn estimate_dot(&self, q: &[f32]) -> f32 {
        assert_eq!(
            q.len(),
            self.dim,
            "rabitq estimate_dot: dimension mismatch ({} vs {})",
            q.len(),
            self.dim
        );
        let mut acc = 0.0_f32;
        for (i, &qi) in q.iter().enumerate() {
            acc = self.sign_at(i).mul_add(qi, acc);
        }
        self.scale * acc
    }

    /// The theoretical error bound: `||v_hat - v||`, the residual norm of the
    /// sign reconstruction. For any query `q`,
    /// `|estimate_dot(q) - <v, q>| <= error_bound() * ||q||`.
    #[must_use]
    pub fn error_bound(&self) -> f32 {
        if self.dim == 0 {
            return 0.0;
        }
        // ||v_hat - v||^2 = 2||v|| (||v|| - ||v||_1 / sqrt(D)).
        let excess = (self.norm - self.l1 / (self.dim as f32).sqrt()).max(0.0);
        (2.0 * self.norm * excess).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared xorshift32 test family (same seed family as the rest of the
    /// quantization suite).
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

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(&x, &y)| x * y).sum()
    }

    fn l2(a: &[f32]) -> f32 {
        a.iter().map(|&x| x * x).sum::<f32>().sqrt()
    }

    #[test]
    fn an_empty_vector_encodes_to_nothing() {
        let r = RabitqVector::encode(&[]);
        assert_eq!(r.dim(), 0);
        assert_eq!(r.words().len(), 0);
        assert_eq!(r.scale(), 0.0);
        assert_eq!(r.error_bound(), 0.0);
    }

    #[test]
    fn signs_pack_and_unpack_without_unsafe() {
        let v = [1.0_f32, -1.0, 0.0, -0.0, 2.0, -3.0];
        let r = RabitqVector::encode(&v);
        assert_eq!(r.dim(), 6);
        // Signs: + - + + + - (0.0 and -0.0 both count as >= 0).
        assert_eq!(r.words().len(), 1);
        // bit0..bit5 = 1,0,1,1,1,0 -> 0b011101.
        assert_eq!(r.words()[0] & 0b11_1111, 0b01_1101);
    }

    #[test]
    fn reconstruction_preserves_the_norm() {
        let mut rng = XorShift32::new(0x0bad_f00d);
        let v: Vec<f32> = (0..257).map(|_| rng.next_unit()).collect();
        let r = RabitqVector::encode(&v);
        // scale * sqrt(D) == norm.
        let recon_norm = r.scale() * (r.dim() as f32).sqrt();
        assert!((recon_norm - r.norm()).abs() < 1e-4 * r.norm().max(1.0));
    }

    #[test]
    fn sign_estimate_correlates_with_the_true_dot() {
        let mut rng = XorShift32::new(0x5eed_1234);
        let dim = 256;
        let mut ests = Vec::new();
        let mut trues = Vec::new();
        for _ in 0..400 {
            let v: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let q: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            ests.push(RabitqVector::encode(&v).estimate_dot(&q));
            trues.push(dot(&v, &q));
        }
        let corr = pearson(&ests, &trues);
        assert!(corr > 0.6, "sign correlation {corr} is too low");
    }

    #[test]
    fn estimate_stays_within_the_theoretical_bound() {
        let mut rng = XorShift32::new(0xc0ff_ee11);
        let dim = 192;
        for _ in 0..100 {
            let v: Vec<f32> = (0..dim).map(|_| rng.next_unit() * 2.0).collect();
            let q: Vec<f32> = (0..dim).map(|_| rng.next_unit() * 2.0).collect();
            let r = RabitqVector::encode(&v);
            let est = r.estimate_dot(&q);
            let true_dot = dot(&v, &q);
            let bound = r.error_bound() * l2(&q) * (1.0 + 1e-4) + 1e-3;
            assert!(
                (est - true_dot).abs() <= bound,
                "|{est} - {true_dot}| = {} exceeds bound {bound}",
                (est - true_dot).abs()
            );
        }
    }

    fn pearson(a: &[f32], b: &[f32]) -> f32 {
        let n = a.len() as f32;
        let ma = a.iter().sum::<f32>() / n;
        let mb = b.iter().sum::<f32>() / n;
        let mut cov = 0.0_f32;
        let mut va = 0.0_f32;
        let mut vb = 0.0_f32;
        for (&x, &y) in a.iter().zip(b) {
            let dx = x - ma;
            let dy = y - mb;
            cov = dx.mul_add(dy, cov);
            va = dx.mul_add(dx, va);
            vb = dy.mul_add(dy, vb);
        }
        cov / (va.sqrt() * vb.sqrt())
    }
}
