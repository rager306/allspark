//! V4 — int8 vector quantization, symmetric and asymmetric (pure `std`).
//!
//! Both modes store one `i8` code per dimension plus the affine parameters
//! that map a code back to `f32`:
//!
//! - **Symmetric** — `scale = max|v| / 127`, `zero_point = 0`; codes land in
//!   `[-127, 127]` and `v ≈ scale * q`.
//! - **Asymmetric** — `scale = (max - min) / 255`, a `zero_point` that pins the
//!   vector's minimum to `-128` and its maximum to `127`; `v ≈ scale * (q - zp)`.
//!
//! The dot product accumulates in `i32` (the codes are integers, so the
//! integer dot is exact for any realistic dimension) and is de-quantized once
//! at the end. Expanding `(q1 - z1)(q2 - z2)` gives the integer form
//!
//! ```text
//! <a, b> = s1 * s2 * (Σ q1 q2 - z2 Σ q1 - z1 Σ q2 + n z1 z2)
//! ```
//!
//! so a single pass over the codes collects all three integer sums.

/// A quantized vector: signed codes plus the affine parameters that invert the
/// quantization (`v ≈ scale * (q - zero_point)`).
#[derive(Clone, Debug, PartialEq)]
pub struct Int8Vector {
    data: Vec<i8>,
    scale: f32,
    zero_point: i32,
}

impl Int8Vector {
    /// The number of quantized dimensions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the vector has no dimensions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The per-vector scale applied on de-quantization.
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The asymmetric zero point (`0` for symmetric quantization).
    #[must_use]
    pub fn zero_point(&self) -> i32 {
        self.zero_point
    }

    /// The raw `i8` codes.
    #[must_use]
    pub fn codes(&self) -> &[i8] {
        &self.data
    }

    /// De-quantizes back to `f32` (`v = scale * (q - zero_point)`).
    #[must_use]
    pub fn dequantize(&self) -> Vec<f32> {
        let zp = self.zero_point as f32;
        self.data
            .iter()
            .map(|&q| self.scale * (f32::from(q) - zp))
            .collect()
    }

    /// The de-quantized dot product with another vector, accumulated in `i32`.
    ///
    /// # Panics
    ///
    /// Panics if the two vectors have different dimensions.
    #[must_use]
    pub fn dot(&self, other: &Self) -> f32 {
        assert_eq!(
            self.data.len(),
            other.data.len(),
            "int8 dot: dimension mismatch ({} vs {})",
            self.data.len(),
            other.data.len()
        );
        let mut acc_dot: i32 = 0;
        let mut sum_self: i32 = 0;
        let mut sum_other: i32 = 0;
        for (&a, &b) in self.data.iter().zip(&other.data) {
            let (ai, bi) = (i32::from(a), i32::from(b));
            acc_dot += ai * bi;
            sum_self += ai;
            sum_other += bi;
        }
        let dim = self.data.len() as i32;
        let z1 = self.zero_point;
        let z2 = other.zero_point;
        let total = acc_dot - z2 * sum_self - z1 * sum_other + z1 * z2 * dim;
        self.scale * other.scale * total as f32
    }
}

/// The int8 quantizer. Both entry points are associated functions: symmetric
/// quantization needs no state, and asymmetric quantization derives its
/// `zero_point` from the vector being encoded.
#[derive(Clone, Copy, Debug, Default)]
pub struct Int8Quantizer;

impl Int8Quantizer {
    /// Symmetric quantization: `scale = max|v| / 127`, codes in `[-127, 127]`.
    ///
    /// An all-zero (or empty) vector yields `scale = 1.0` and all-zero codes.
    #[must_use]
    pub fn quantize_sym(v: &[f32]) -> Int8Vector {
        let max_abs = v.iter().fold(0.0_f32, |m, &x| m.max(x.abs()));
        let scale = if max_abs > 0.0 { max_abs / 127.0 } else { 1.0 };
        let inv = 1.0 / scale;
        let data = v
            .iter()
            .map(|&x| (x * inv).round().clamp(-127.0, 127.0) as i8)
            .collect();
        Int8Vector {
            data,
            scale,
            zero_point: 0,
        }
    }

    /// Asymmetric quantization: `scale = (max - min) / 255` and a `zero_point`
    /// that maps the minimum to code `-128` and the maximum to code `127`.
    ///
    /// A constant (or empty) vector yields `scale = 1.0` and a `zero_point`
    /// chosen so the single value round-trips exactly.
    #[must_use]
    pub fn quantize_asym(v: &[f32]) -> Int8Vector {
        if v.is_empty() {
            return Int8Vector {
                data: Vec::new(),
                scale: 1.0,
                zero_point: 0,
            };
        }
        let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let range = hi - lo;
        if range <= 0.0 {
            // A constant vector: one code, and the scale itself carries the
            // value. `zero_point = -1` makes the code `0` de-quantize to
            // `scale * (0 - (-1)) = scale = lo`, so it round-trips exactly.
            let scale = if lo == 0.0 { 1.0 } else { lo };
            return Int8Vector {
                data: vec![0; v.len()],
                scale,
                zero_point: -1,
            };
        }
        let scale = range / 255.0;
        let inv = 1.0 / scale;
        // q = clamp(round(v / scale) + zp); zp is pinned so lo -> -128, hi -> 127.
        let zp = (-128.0 - lo * inv).round();
        let zp_int = zp as i32;
        let data = v
            .iter()
            .map(|&x| ((x * inv) + zp).round().clamp(-128.0, 127.0) as i8)
            .collect();
        Int8Vector {
            data,
            scale,
            zero_point: zp_int,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny xorshift32 generator — the fixed test family (same seed family
    /// across the whole quantization suite), no external data.
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

    /// A smooth vector: a low-frequency sinusoid plus a little jitter.
    fn smooth(dim: usize, freq: f32, phase: f32) -> Vec<f32> {
        (0..dim)
            .map(|i| {
                let t = i as f32 / dim as f32;
                (std::f32::consts::TAU * freq * t + phase).sin()
            })
            .collect()
    }

    fn rel_error(approx: &[f32], exact: &[f32]) -> f32 {
        let mut num = 0.0_f32;
        let mut den = 0.0_f32;
        for (&a, &b) in approx.iter().zip(exact) {
            num = (a - b).mul_add(a - b, num);
            den = b.mul_add(b, den);
        }
        (num / den).sqrt()
    }

    #[test]
    fn symmetric_error_is_under_one_percent() {
        for (freq, phase) in [(1.0_f32, 0.0_f32), (3.0, 0.7), (7.0, 2.1)] {
            let v = smooth(256, freq, phase);
            let q = Int8Quantizer::quantize_sym(&v);
            let back = q.dequantize();
            assert!(
                rel_error(&back, &v) < 0.01,
                "symmetric relative error {} exceeds 1%",
                rel_error(&back, &v)
            );
        }
    }

    #[test]
    fn asymmetric_error_is_under_one_percent() {
        for (freq, phase) in [(1.0_f32, 0.3_f32), (5.0, 1.9)] {
            let v = smooth(256, freq, phase);
            let q = Int8Quantizer::quantize_asym(&v);
            let back = q.dequantize();
            assert!(rel_error(&back, &v) < 0.01);
        }
    }

    #[test]
    fn codes_stay_inside_the_signed_range() {
        let mut rng = XorShift32::new(0x1234_5678);
        let v: Vec<f32> = (0..512).map(|_| rng.next_unit() * 3.0).collect();
        for q in [
            Int8Quantizer::quantize_sym(&v),
            Int8Quantizer::quantize_asym(&v),
        ] {
            assert!(q.codes().iter().all(|&c| (-128..=127).contains(&c)));
        }
    }

    #[test]
    fn dot_tracks_the_f32_dot_product() {
        let mut rng = XorShift32::new(0xfeed_beef);
        for dim in [32usize, 128, 384] {
            let a: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let b: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
            let exact: f32 = a.iter().zip(&b).map(|(&x, &y)| x * y).sum();

            let qa = Int8Quantizer::quantize_sym(&a);
            let qb = Int8Quantizer::quantize_sym(&b);
            let sym = qa.dot(&qb);

            let aa = Int8Quantizer::quantize_asym(&a);
            let ab = Int8Quantizer::quantize_asym(&b);
            let asym = aa.dot(&ab);

            // The int8 dot is the de-quantized dot; check it against the
            // de-quantized codes rather than raw f32 (which carries the same
            // quantization error either way).
            let expected: f32 = qa
                .dequantize()
                .iter()
                .zip(&qb.dequantize())
                .map(|(&x, &y)| x * y)
                .sum();
            assert!((sym - expected).abs() <= 1e-3 * expected.abs().max(1.0));

            let expected: f32 = aa
                .dequantize()
                .iter()
                .zip(&ab.dequantize())
                .map(|(&x, &y)| x * y)
                .sum();
            assert!((asym - expected).abs() <= 1e-3 * expected.abs().max(1.0));

            // And the quantized dot is close to the true f32 dot.
            assert!((sym - exact).abs() < 0.05 * exact.abs().max(1.0));
            assert!((asym - exact).abs() < 0.05 * exact.abs().max(1.0));
        }
    }

    #[test]
    fn asymmetric_uses_the_full_code_range() {
        let v = vec![-4.0_f32, -1.0, 0.0, 1.0, 4.0];
        let q = Int8Quantizer::quantize_asym(&v);
        assert_eq!(q.codes()[0], -128); // the minimum
        assert!(q.codes()[4] >= 126); // the maximum (127 up to rounding)
        assert!((q.scale() - 8.0 / 255.0).abs() < 1e-9);
        // The zero point pins the minimum to -128, so it is close to 0 here.
        assert!(q.zero_point().abs() <= 1);
    }

    #[test]
    fn symmetric_scale_is_max_abs_over_127() {
        let v = vec![3.0_f32, -6.0, 1.5];
        let q = Int8Quantizer::quantize_sym(&v);
        assert!((q.scale() - 6.0 / 127.0).abs() < 1e-7);
        assert_eq!(q.zero_point(), 0);
        assert_eq!(q.codes()[1], -127); // the largest magnitude
    }

    #[test]
    fn constant_and_empty_vectors_round_trip() {
        let flat = vec![2.5_f32; 64];
        let q = Int8Quantizer::quantize_asym(&flat);
        for value in q.dequantize() {
            assert!((value - 2.5).abs() < 1e-3);
        }

        let empty = Int8Quantizer::quantize_sym(&[]);
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
    }
}
