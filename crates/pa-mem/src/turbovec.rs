//! V7 — TurboVec-style multi-bit scalar quantization, 2 or 4 bits per
//! dimension (pure `std`).
//!
//! Each dimension is replaced by the nearest of `2^bits` uniform reconstruction
//! levels spread across the vector's own `[min, max]` range — a Lloyd-style
//! fixed-rate codebook that adapts to the data with a single range pass. The
//! levels sit at cell midpoints,
//!
//! ```text
//! step    = (max - min) / 2^bits
//! level_j = min + (j + 0.5) * step,   j in [0, 2^bits)
//! ```
//!
//! so the reconstruction error is at most half a step — the optimal uniform
//! (nearest-level) quantizer. Codes are packed little-endian, `bits` per
//! dimension (`4` codes per byte at 2 bits, `2` per byte at 4 bits), and the
//! dot product de-quantizes each code on the fly.

/// A multi-bit quantized vector: packed codes plus the affine reconstruction
/// parameters (`v ≈ min + (code + 0.5) * step`).
#[derive(Clone, Debug, PartialEq)]
pub struct MultiBitVector {
    codes: Vec<u8>,
    min: f32,
    step: f32,
    bits: u8,
    dim: usize,
}

impl MultiBitVector {
    /// The number of quantized dimensions.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// The bit width per dimension (`2` or `4`).
    #[must_use]
    pub fn bits(&self) -> u8 {
        self.bits
    }

    /// The packed code bytes.
    #[must_use]
    pub fn codes(&self) -> &[u8] {
        &self.codes
    }

    /// The lower end of the reconstruction range.
    #[must_use]
    pub fn min(&self) -> f32 {
        self.min
    }

    /// The reconstruction step between adjacent level midpoints.
    #[must_use]
    pub fn step(&self) -> f32 {
        self.step
    }

    /// Reads the `bits`-wide code stored at dimension `i`.
    fn code_at(&self, i: usize) -> u32 {
        let per_byte = 8 / usize::from(self.bits);
        let byte = self.codes[i / per_byte];
        let slot = i % per_byte;
        let mask = (1_u32 << self.bits) - 1;
        (u32::from(byte) >> (slot * usize::from(self.bits))) & mask
    }

    /// The de-quantized value at dimension `i`.
    fn value_at(&self, i: usize) -> f32 {
        self.min + (self.code_at(i) as f32 + 0.5) * self.step
    }

    /// The de-quantized dot product with a full-precision query.
    ///
    /// # Panics
    ///
    /// Panics if `q.len()` differs from the encoded dimension.
    #[must_use]
    pub fn dot(&self, q: &[f32]) -> f32 {
        assert_eq!(
            q.len(),
            self.dim,
            "multibit dot: dimension mismatch ({} vs {})",
            q.len(),
            self.dim
        );
        let mut acc = 0.0_f32;
        for (i, &qi) in q.iter().enumerate() {
            acc = self.value_at(i).mul_add(qi, acc);
        }
        acc
    }
}

/// The multi-bit quantizer, parameterized by bit width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultiBitQuantizer {
    bits: u8,
}

impl MultiBitQuantizer {
    /// Creates a quantizer for `bits` bits per dimension.
    ///
    /// # Panics
    ///
    /// Panics unless `bits` is `2` or `4`.
    #[must_use]
    pub fn new(bits: u8) -> Self {
        assert!(
            bits == 2 || bits == 4,
            "MultiBitQuantizer supports 2 or 4 bits, got {bits}"
        );
        Self { bits }
    }

    /// The configured bit width.
    #[must_use]
    pub fn bits(&self) -> u8 {
        self.bits
    }

    /// Quantizes `v` to its per-vector uniform codebook.
    ///
    /// A constant (or empty) vector yields `step = 0` (exact reconstruction).
    #[must_use]
    pub fn quantize(&self, v: &[f32]) -> MultiBitVector {
        let dim = v.len();
        if dim == 0 {
            return MultiBitVector {
                codes: Vec::new(),
                min: 0.0,
                step: 0.0,
                bits: self.bits,
                dim: 0,
            };
        }
        let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let levels = 1_u32 << self.bits;
        let range = hi - lo;
        let step = if range > 0.0 {
            range / levels as f32
        } else {
            0.0
        };
        let inv = if step > 0.0 { 1.0 / step } else { 0.0 };
        let max_code = levels - 1;

        let per_byte = 8 / usize::from(self.bits);
        let mut codes = vec![0_u8; dim.div_ceil(per_byte)];
        for (i, &x) in v.iter().enumerate() {
            let code = if step > 0.0 {
                ((x - lo) * inv).floor().clamp(0.0, max_code as f32) as u32
            } else {
                0
            };
            let byte = i / per_byte;
            let slot = i % per_byte;
            codes[byte] |= (code as u8) << (slot * usize::from(self.bits));
        }
        MultiBitVector {
            codes,
            min: lo,
            step,
            bits: self.bits,
            dim,
        }
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

        /// A value in `[-1, 1)`.
        fn next_unit(&mut self) -> f32 {
            (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(&x, &y)| x * y).sum()
    }

    fn norm(a: &[f32]) -> f32 {
        a.iter().map(|&x| x * x).sum::<f32>().sqrt()
    }

    /// The dot error normalized by `||a|| * ||b||` (bounded, unlike the raw
    /// relative error, which blows up when the true dot is near zero).
    fn norm_dot_error(approx: f32, exact: f32, na: f32, nb: f32) -> f32 {
        (approx - exact).abs() / (na * nb).max(1e-6)
    }

    #[test]
    fn four_bits_beat_two_bits() {
        let mut rng = XorShift32::new(0x00c0_ffee);
        let dim = 128;
        let mut err2 = 0.0_f32;
        let mut err4 = 0.0_f32;
        let trials = 64;
        for _ in 0..trials {
            let a: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let b: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let exact = dot(&a, &b);
            let (na, nb) = (norm(&a), norm(&b));
            let q2 = MultiBitQuantizer::new(2).quantize(&a);
            let q4 = MultiBitQuantizer::new(4).quantize(&a);
            err2 += norm_dot_error(q2.dot(&b), exact, na, nb);
            err4 += norm_dot_error(q4.dot(&b), exact, na, nb);
        }
        assert!(
            err4 < err2,
            "4-bit mean error {err4} should be below 2-bit {err2}"
        );
    }

    #[test]
    fn dot_error_is_under_three_percent() {
        let mut rng = XorShift32::new(0x1234_abcd);
        let dim = 256;
        for _ in 0..64 {
            let a: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let b: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let exact = dot(&a, &b);
            let err = norm_dot_error(
                MultiBitQuantizer::new(4).quantize(&a).dot(&b),
                exact,
                norm(&a),
                norm(&b),
            );
            assert!(err < 0.03, "4-bit normalized dot error {err} exceeds 3%");
        }
    }

    #[test]
    fn levels_sit_at_cell_midpoints() {
        // Range [0, 16] with 4 bits -> step 1, level j at j + 0.5.
        let v = vec![0.0_f32, 16.0];
        let q = MultiBitQuantizer::new(4).quantize(&v);
        assert_eq!(q.min(), 0.0);
        assert!((q.step() - 1.0).abs() < 1e-6);
        // The minimum reconstructs to the first midpoint, 0.5; the maximum to
        // the last, 15.5.
        assert!((q.dot(&[1.0, 0.0]) - 0.5).abs() < 1e-6);
        assert!((q.dot(&[0.0, 1.0]) - 15.5).abs() < 1e-6);
    }

    #[test]
    fn codes_are_packed_tightly() {
        let v = vec![0.1_f32; 32];
        let q2 = MultiBitQuantizer::new(2).quantize(&v);
        let q4 = MultiBitQuantizer::new(4).quantize(&v);
        assert_eq!(q2.codes().len(), 8); // 32 * 2 / 8
        assert_eq!(q4.codes().len(), 16); // 32 * 4 / 8
        assert_eq!(q2.bits(), 2);
        assert_eq!(q4.bits(), 4);
    }

    #[test]
    fn a_single_dimension_round_trips_exactly() {
        let q = MultiBitQuantizer::new(4).quantize(&[0.75_f32]);
        assert_eq!(q.dim(), 1);
        assert!((q.dot(&[1.0]) - 0.75).abs() < 1e-6);
    }
}
