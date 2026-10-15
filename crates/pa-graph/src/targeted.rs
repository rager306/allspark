//! V16 — adaptive recall targeting.
//!
//! An ANN search exposes a knob (`ef_search` in `HNSW`, `search_width` in
//! `DiskANN`) that trades latency for recall. The relationship between the
//! knob and the achieved recall is index- and data-dependent, so a fixed value
//! either over-spends on easy queries or under-serves hard ones. Given a few
//! calibration samples — `(ef, measured_recall)` pairs from probing the live
//! index — [`RecallTargeter::calibrate`] suggests the smallest `ef` that meets
//! a requested recall, and [`RecallTargeter::search_with_target`] runs the
//! search at that width.
//!
//! Calibration is *monotone by construction*: the suggested `ef` is
//! non-decreasing in the target recall. To keep that property even when the
//! samples are noisy (recall is not perfectly increasing in `ef`), the
//! suggestion is taken against the running maximum recall, so a lucky low-`ef`
//! sample can never make a *higher* target cheaper than a lower one.

/// Suggests a search width (`ef`) for a target recall, from calibration
/// samples.
#[derive(Debug, Clone, Default)]
pub struct RecallTargeter {
    samples: Vec<(usize, f32)>,
}

impl RecallTargeter {
    /// A targeter with no samples yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A targeter seeded with `(ef, recall)` samples.
    #[must_use]
    pub fn from_samples(samples: Vec<(usize, f32)>) -> Self {
        Self { samples }
    }

    /// Record one `(ef, recall)` observation.
    pub fn observe(&mut self, ef: usize, recall: f32) {
        self.samples.push((ef, recall.clamp(0.0, 1.0)));
    }

    /// The recorded samples.
    #[must_use]
    pub fn samples(&self) -> &[(usize, f32)] {
        &self.samples
    }

    /// Suggest the smallest sampled `ef` whose recall reaches `target_recall`.
    ///
    /// Samples are sorted by `ef` and compared against the running-maximum
    /// recall, so the result is monotone non-decreasing in `target_recall`. When
    /// no sample reaches the target, the largest sampled `ef` is returned (the
    /// caller can widen further). With no samples the fallback width is `0`.
    #[must_use]
    pub fn calibrate(samples: &[(usize, f32)], target_recall: f32) -> usize {
        if samples.is_empty() {
            return 0;
        }
        let mut sorted: Vec<(usize, f32)> = samples
            .iter()
            .map(|&(ef, r)| (ef, r.clamp(0.0, 1.0)))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));

        let mut best_recall = f32::NEG_INFINITY;
        for &(ef, recall) in &sorted {
            best_recall = best_recall.max(recall);
            if best_recall >= target_recall {
                return ef;
            }
        }
        sorted.last().map_or(0, |&(ef, _)| ef)
    }

    /// Suggest an `ef` from this targeter's samples.
    #[must_use]
    pub fn ef_for(&self, target_recall: f32) -> usize {
        Self::calibrate(&self.samples, target_recall)
    }

    /// Run `search` at the width suggested for `target_recall`.
    ///
    /// Returns `(ef_used, results)`. The `search` closure receives the suggested
    /// width and returns the node ids it found at that width.
    #[must_use]
    pub fn search_with_target(
        &self,
        target_recall: f32,
        search: impl Fn(usize) -> Vec<i64>,
    ) -> (usize, Vec<i64>) {
        let ef = self.ef_for(target_recall);
        (ef, search(ef))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plausible recall curve: more `ef` → more recall, saturating.
    fn curve() -> Vec<(usize, f32)> {
        vec![
            (10, 0.55),
            (20, 0.72),
            (40, 0.86),
            (80, 0.94),
            (160, 0.99),
        ]
    }

    #[test]
    fn higher_target_needs_higher_ef() {
        let samples = curve();
        let low = RecallTargeter::calibrate(&samples, 0.7);
        let high = RecallTargeter::calibrate(&samples, 0.95);
        assert!(high > low, "0.95 must cost more than 0.7: {high} vs {low}");
        assert_eq!(low, 20, "0.7 met at ef=20");
        assert_eq!(high, 160, "0.95 met at ef=160");
    }

    #[test]
    fn calibration_is_monotone_in_target() {
        let samples = curve();
        let mut previous = 0usize;
        let mut target = 0.0_f32;
        while target <= 1.0 {
            let ef = RecallTargeter::calibrate(&samples, target);
            assert!(
                ef >= previous,
                "ef must not decrease as target grows (target={target}, ef={ef})"
            );
            previous = ef;
            target += 0.05;
        }
    }

    #[test]
    fn unreachable_target_returns_widest_sample() {
        let samples = curve();
        assert_eq!(RecallTargeter::calibrate(&samples, 0.9999), 160);
    }

    #[test]
    fn no_samples_falls_back_to_zero() {
        assert_eq!(RecallTargeter::calibrate(&[], 0.9), 0);
    }

    #[test]
    fn noisy_samples_keep_monotonicity() {
        // ef=10 has an anomalously high sample and ef=20 an anomalously low
        // one. The running-max rule still makes a higher target no cheaper
        // than a lower one across the whole target range.
        let samples = vec![(10, 0.99), (20, 0.60), (30, 0.95), (40, 0.99)];
        let mut previous = 0usize;
        let mut target = 0.0_f32;
        while target <= 1.0 {
            let ef = RecallTargeter::calibrate(&samples, target);
            assert!(
                ef >= previous,
                "ef must not decrease as target grows (target={target}, ef={ef})"
            );
            previous = ef;
            target += 0.05;
        }
        assert_eq!(RecallTargeter::calibrate(&samples, 0.5), 10);
    }

    #[test]
    fn search_with_target_uses_calibrated_width() {
        let targeter = RecallTargeter::from_samples(curve());
        let (ef, results) = targeter.search_with_target(0.9, |width| {
            assert!(width >= 80, "0.9 requires at least ef=80");
            vec![1, 2, 3]
        });
        assert_eq!(ef, 80);
        assert_eq!(results, vec![1, 2, 3]);
    }
}
