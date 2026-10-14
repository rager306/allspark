//! R18 — conformal predictor: distribution-free prediction intervals.
//!
//! Given a calibration set of `(prediction, actual)` pairs, a conformal
//! predictor measures the *nonconformity* of each pair (here `|actual −
//! prediction|`) and stores the `ceil((n+1)(1−α))`-th smallest score as the
//! interval half-width. At inference, `predict_interval` returns
//! `[ŷ − q, ŷ + q]`, which covers the true value with probability `≥ 1 − α`
//! under exchangeability — no distributional assumption.
//!
//! This is the split-conformal / inductive variant; the calibration scores are
//! sorted once so both calibration and prediction are cheap.

/// A split-conformal predictor over absolute residuals.
#[derive(Debug, Clone, PartialEq)]
pub struct ConformalPredictor {
    /// Sorted calibration residuals.
    scores: Vec<f64>,
    /// Target miscoverage (`α`).
    alpha: f64,
}

impl ConformalPredictor {
    /// Calibrate from `(prediction, actual)` pairs at miscoverage `alpha`.
    ///
    /// `alpha` is clamped to `(0, 1)`. Residuals are `|actual − prediction|`.
    /// Returns a predictor with an empty calibration when `pairs` is empty.
    #[must_use]
    pub fn calibrate(pairs: &[(f64, f64)], alpha: f64) -> Self {
        let mut scores: Vec<f64> = pairs
            .iter()
            .map(|(prediction, actual)| (actual - prediction).abs())
            .collect();
        scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            scores,
            alpha: alpha.clamp(f64::EPSILON, 1.0 - f64::EPSILON),
        }
    }

    /// The number of calibration points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.scores.len()
    }

    /// Whether the predictor has no calibration points.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scores.is_empty()
    }

    /// The target miscoverage.
    #[must_use]
    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    /// The conformal quantile (interval half-width).
    ///
    /// Uses the finite-sample-corrected rank `ceil((n + 1)(1 − α))`, clamped to
    /// `[1, n]`. An empty calibration yields `f64::INFINITY` (no information ⇒
    /// an unbounded interval).
    #[must_use]
    pub fn quantile(&self) -> f64 {
        let n = self.scores.len();
        if n == 0 {
            return f64::INFINITY;
        }
        let rank = (((n + 1) as f64) * (1.0 - self.alpha)).ceil() as usize;
        let index = rank.clamp(1, n) - 1;
        self.scores[index]
    }

    /// The prediction interval `[ŷ − q, ŷ + q]` for a point prediction `y_hat`.
    #[must_use]
    pub fn predict_interval(&self, y_hat: f64) -> (f64, f64) {
        let q = self.quantile();
        (y_hat - q, y_hat + q)
    }

    /// Whether `actual` falls inside the interval for `y_hat`.
    #[must_use]
    pub fn covers(&self, y_hat: f64, actual: f64) -> bool {
        let (lo, hi) = self.predict_interval(y_hat);
        actual >= lo && actual <= hi
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantile_uses_finite_sample_rank() {
        // 9 residuals 0..8, alpha = 0.1 -> rank = ceil(10 * 0.9) = 9 -> max.
        let pairs: Vec<(f64, f64)> = (0..9).map(|i| (0.0, f64::from(i))).collect();
        let predictor = ConformalPredictor::calibrate(&pairs, 0.1);
        assert_eq!(predictor.len(), 9);
        assert!((predictor.quantile() - 8.0).abs() < 1e-12);
    }

    #[test]
    fn interval_is_symmetric() {
        let pairs: Vec<(f64, f64)> = (0..9).map(|i| (0.0, f64::from(i))).collect();
        let predictor = ConformalPredictor::calibrate(&pairs, 0.5);
        let (lo, hi) = predictor.predict_interval(100.0);
        assert!((lo - 100.0).abs() - (hi - 100.0).abs() < 1e-12);
        assert!(lo < hi);
    }

    #[test]
    fn coverage_on_the_calibration_set_is_at_least_one_minus_alpha() {
        let pairs: Vec<(f64, f64)> = (0..20)
            .map(|i| (f64::from(i), f64::from(i) + (f64::from(i) % 3.0)))
            .collect();
        let predictor = ConformalPredictor::calibrate(&pairs, 0.1);
        let covered = pairs
            .iter()
            .filter(|(prediction, actual)| predictor.covers(*prediction, *actual))
            .count();
        let ratio = covered as f64 / pairs.len() as f64;
        assert!(ratio >= 0.9, "coverage {ratio} below the target");
    }

    #[test]
    fn empty_calibration_is_unbounded() {
        let predictor = ConformalPredictor::calibrate(&[], 0.1);
        assert!(predictor.is_empty());
        assert!(predictor.quantile().is_infinite());
        let (lo, hi) = predictor.predict_interval(0.0);
        assert!(lo.is_infinite() && hi.is_infinite());
    }

    #[test]
    fn larger_alpha_shrinks_the_interval() {
        let pairs: Vec<(f64, f64)> = (0..9).map(|i| (0.0, f64::from(i))).collect();
        let tight = ConformalPredictor::calibrate(&pairs, 0.5).quantile();
        let wide = ConformalPredictor::calibrate(&pairs, 0.05).quantile();
        assert!(tight <= wide);
    }
}
