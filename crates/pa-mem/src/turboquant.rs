//! V8 — TurboQuant-style 4-bit quantization with per-block scales (pure
//! `std`).
//!
//! The vector is split into fixed 32-dimension blocks. Each block gets its own
//! `min`/`max` and therefore its own scale, so a block with a narrow value
//! range is not drowned out by a globally wide one — the main win over a single
//! per-vector scale. Every dimension is stored as a 4-bit code (two codes per
//! byte), and each block carries one `f32` `min` and one `f32` scale:
//!
//! ```text
//! size_bytes() = ceil(dim / 2) + n_blocks * 8
//! ```
//!
//! Level `j` sits at the cell midpoint `min + (j + 0.5) * step`, with
//! `step = (block_max - block_min) / 16`.

/// The block size: dimensions per per-block scale.
pub const BLOCK_DIM: usize = 32;

/// A 4-bit-per-dimension vector with one `(min, scale)` pair per 32-dim block.
#[derive(Clone, Debug, PartialEq)]
pub struct Block4Vector {
    codes: Vec<u8>,
    mins: Vec<f32>,
    steps: Vec<f32>,
    dim: usize,
}

impl Block4Vector {
    /// The number of encoded dimensions.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// The number of blocks (`ceil(dim / 32)`).
    #[must_use]
    pub fn blocks(&self) -> usize {
        self.mins.len()
    }

    /// The packed 4-bit codes (two per byte).
    #[must_use]
    pub fn codes(&self) -> &[u8] {
        &self.codes
    }

    /// The per-block lower bounds.
    #[must_use]
    pub fn mins(&self) -> &[f32] {
        &self.mins
    }

    /// The per-block reconstruction steps.
    #[must_use]
    pub fn steps(&self) -> &[f32] {
        &self.steps
    }

    /// The serialized size: packed codes plus 8 bytes per block (`min` + `step`).
    #[must_use]
    pub fn size_bytes(&self) -> usize {
        self.codes.len() + self.mins.len() * 8
    }

    /// Reads the 4-bit code stored at dimension `i`.
    fn code_at(&self, i: usize) -> u32 {
        let byte = self.codes[i / 2];
        if i.is_multiple_of(2) {
            u32::from(byte & 0x0f)
        } else {
            u32::from(byte >> 4)
        }
    }

    /// The de-quantized value at dimension `i`.
    fn value_at(&self, i: usize) -> f32 {
        let block = i / BLOCK_DIM;
        self.mins[block] + (self.code_at(i) as f32 + 0.5) * self.steps[block]
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
            "block4 dot: dimension mismatch ({} vs {})",
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

/// The per-block 4-bit quantizer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Block4Quantizer;

impl Block4Quantizer {
    /// Quantizes `v` into 32-dimension blocks, each with its own scale.
    #[must_use]
    pub fn quantize(v: &[f32]) -> Block4Vector {
        let dim = v.len();
        let blocks = dim.div_ceil(BLOCK_DIM);
        let mut codes = vec![0_u8; dim.div_ceil(2)];
        let mut mins = Vec::with_capacity(blocks);
        let mut steps = Vec::with_capacity(blocks);

        for block in 0..blocks {
            let start = block * BLOCK_DIM;
            let end = (start + BLOCK_DIM).min(dim);
            let slice = &v[start..end];
            let lo = slice.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = slice.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let range = hi - lo;
            let step = if range > 0.0 { range / 16.0 } else { 0.0 };
            let inv = if step > 0.0 { 1.0 / step } else { 0.0 };
            mins.push(lo);
            steps.push(step);
            for (offset, &x) in slice.iter().enumerate() {
                let code = if step > 0.0 {
                    ((x - lo) * inv).floor().clamp(0.0, 15.0) as u8
                } else {
                    0
                };
                let i = start + offset;
                if i.is_multiple_of(2) {
                    codes[i / 2] |= code;
                } else {
                    codes[i / 2] |= code << 4;
                }
            }
        }

        Block4Vector {
            codes,
            mins,
            steps,
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

    #[test]
    fn size_is_codes_plus_scales() {
        for dim in [1usize, 31, 32, 33, 128, 384, 768] {
            let v = vec![0.5_f32; dim];
            let q = Block4Quantizer::quantize(&v);
            let blocks = dim.div_ceil(BLOCK_DIM);
            assert_eq!(q.blocks(), blocks);
            assert_eq!(q.codes().len(), dim.div_ceil(2));
            assert_eq!(q.size_bytes(), dim.div_ceil(2) + blocks * 8);
        }
    }

    #[test]
    fn odd_dimensions_pack_the_trailing_code() {
        let v: Vec<f32> = (0..33).map(|i| i as f32 / 33.0).collect();
        let q = Block4Quantizer::quantize(&v);
        assert_eq!(q.blocks(), 2); // 32 + 1
        assert_eq!(q.codes().len(), 17); // ceil(33/2)
        assert_eq!(q.size_bytes(), 17 + 2 * 8);
        // The lone trailing dimension still reconstructs near its value.
        assert!(
            (q.dot(&[0.0; 32].into_iter().chain([1.0]).collect::<Vec<_>>()) - v[32]).abs() < 0.05
        );
    }

    #[test]
    fn dot_error_is_under_five_percent() {
        let mut rng = XorShift32::new(0x7777_3333);
        let dim = 256;
        for _ in 0..64 {
            let a: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let b: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let exact = dot(&a, &b);
            let approx = Block4Quantizer::quantize(&a).dot(&b);
            let err = (approx - exact).abs() / (norm(&a) * norm(&b)).max(1e-6);
            assert!(err < 0.05, "block4 normalized dot error {err} exceeds 5%");
        }
    }

    #[test]
    fn per_block_scales_beat_a_single_scale_on_heterogeneous_data() {
        // The first block is tiny, the second huge; a single global scale
        // would crush the tiny block into a couple of levels.
        let mut rng = XorShift32::new(0xabc0_1234);
        let mut v: Vec<f32> = (0..32).map(|_| rng.next_unit() * 0.01).collect();
        v.extend((0..32).map(|_| rng.next_unit() * 100.0));
        let q = Block4Quantizer::quantize(&v);
        assert_eq!(q.blocks(), 2);
        // Block steps must differ by ~4 orders of magnitude.
        assert!(q.steps()[1] > q.steps()[0] * 1000.0);

        // The tiny block is reconstructed within its own half-step.
        for (i, &x) in v[..32].iter().enumerate() {
            let half_step = q.steps()[0] * 0.5 + 1e-9;
            assert!((q.value_at(i) - x).abs() <= half_step);
        }
    }

    #[test]
    fn a_constant_block_round_trips_exactly() {
        let q = Block4Quantizer::quantize(&[3.25_f32; 40]);
        assert!((q.dot(&[1.0; 40]) - 3.25 * 40.0).abs() < 1e-3);
    }
}
