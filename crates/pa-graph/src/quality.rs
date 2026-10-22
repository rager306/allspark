//! M9 — quality envelope: min/median/max plus degradation flags.
//!
//! A single quality number hides regressions. [`QualityEnvelope`] summarizes a
//! sample as min / median / mean / max and raises [`DegradationFlag`]s when the
//! spread or the extremes cross the policy's thresholds, so a caller can gate
//! on "is this good enough?" rather than on a bare average.

/// A reason a quality envelope looks degraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationFlag {
    /// The minimum sample fell below `min_acceptable`.
    LowMinimum,
    /// The maximum sample fell below `max_acceptable` (everything is weak).
    WeakCeiling,
    /// The spread (max − min) exceeded `max_spread`.
    HighSpread,
    /// The mean fell below `mean_floor`.
    LowMean,
}

impl DegradationFlag {
    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LowMinimum => "low_minimum",
            Self::WeakCeiling => "weak_ceiling",
            Self::HighSpread => "high_spread",
            Self::LowMean => "low_mean",
        }
    }
}

/// Thresholds that decide which degradation flags fire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityPolicy {
    /// Minimum acceptable sample value.
    pub min_acceptable: f64,
    /// Minimum acceptable maximum (a weak ceiling means uniform weakness).
    pub max_acceptable: f64,
    /// Maximum acceptable spread (max − min).
    pub max_spread: f64,
    /// Minimum acceptable mean.
    pub mean_floor: f64,
}

impl Default for QualityPolicy {
    fn default() -> Self {
        Self {
            min_acceptable: 0.0,
            max_acceptable: 0.0,
            max_spread: 1.0,
            mean_floor: 0.0,
        }
    }
}

impl QualityPolicy {
    /// The default policy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `min_acceptable`.
    #[must_use]
    pub fn with_min_acceptable(mut self, value: f64) -> Self {
        self.min_acceptable = value;
        self
    }

    /// Set `max_acceptable`.
    #[must_use]
    pub fn with_max_acceptable(mut self, value: f64) -> Self {
        self.max_acceptable = value;
        self
    }

    /// Set `max_spread`.
    #[must_use]
    pub fn with_max_spread(mut self, value: f64) -> Self {
        self.max_spread = value;
        self
    }

    /// Set `mean_floor`.
    #[must_use]
    pub fn with_mean_floor(mut self, value: f64) -> Self {
        self.mean_floor = value;
        self
    }
}

/// A summary of a quality sample set.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct QualityEnvelope {
    /// Number of samples.
    pub count: usize,
    /// Minimum sample.
    pub min: f64,
    /// Median sample.
    pub median: f64,
    /// Arithmetic mean.
    pub mean: f64,
    /// Maximum sample.
    pub max: f64,
    /// Degradation flags raised by [`QualityEnvelope::flags`].
    pub flags: Vec<DegradationFlag>,
}

impl QualityEnvelope {
    /// Summarize `samples` under `policy`.
    ///
    /// An empty sample yields an all-zero envelope with a [`DegradationFlag::WeakCeiling`]
    /// flag (a ceiling of 0 is below any positive `max_acceptable`).
    #[must_use]
    pub fn from_samples(samples: &[f64], policy: &QualityPolicy) -> Self {
        if samples.is_empty() {
            return Self {
                flags: vec![DegradationFlag::WeakCeiling],
                ..Self::default()
            };
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let count = sorted.len();
        let min = sorted[0];
        let max = sorted[count - 1];
        let mean = sorted.iter().sum::<f64>() / count as f64;
        let median = if count % 2 == 1 {
            sorted[count / 2]
        } else {
            f64::midpoint(sorted[count / 2 - 1], sorted[count / 2])
        };
        let mut envelope = Self {
            count,
            min,
            median,
            mean,
            max,
            flags: Vec::new(),
        };
        envelope.flags = envelope.flags(policy);
        envelope
    }

    /// The degradation flags this envelope raises under `policy`.
    #[must_use]
    pub fn flags(&self, policy: &QualityPolicy) -> Vec<DegradationFlag> {
        let mut flags = Vec::new();
        if self.count > 0 {
            if self.min < policy.min_acceptable {
                flags.push(DegradationFlag::LowMinimum);
            }
            if self.max < policy.max_acceptable {
                flags.push(DegradationFlag::WeakCeiling);
            }
            if (self.max - self.min) > policy.max_spread {
                flags.push(DegradationFlag::HighSpread);
            }
            if self.mean < policy.mean_floor {
                flags.push(DegradationFlag::LowMean);
            }
        } else {
            flags.push(DegradationFlag::WeakCeiling);
        }
        flags
    }

    /// Whether the envelope raises any degradation flag.
    #[must_use]
    pub fn is_degraded(&self) -> bool {
        !self.flags.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_min_median_mean_max() {
        let envelope = QualityEnvelope::from_samples(&[1.0, 2.0, 3.0, 4.0], &QualityPolicy::new());
        assert_eq!(envelope.count, 4);
        assert!((envelope.min - 1.0).abs() < 1e-12);
        assert!((envelope.max - 4.0).abs() < 1e-12);
        assert!((envelope.median - 2.5).abs() < 1e-12);
        assert!((envelope.mean - 2.5).abs() < 1e-12);
    }

    #[test]
    fn odd_sample_median_is_the_middle_value() {
        let envelope = QualityEnvelope::from_samples(&[3.0, 1.0, 2.0], &QualityPolicy::new());
        assert!((envelope.median - 2.0).abs() < 1e-12);
    }

    #[test]
    fn flags_fire_on_low_min_and_high_spread() {
        let policy = QualityPolicy::new()
            .with_min_acceptable(0.5)
            .with_max_spread(2.0);
        let envelope = QualityEnvelope::from_samples(&[0.1, 9.0], &policy);
        assert!(envelope.flags.contains(&DegradationFlag::LowMinimum));
        assert!(envelope.flags.contains(&DegradationFlag::HighSpread));
        assert!(envelope.is_degraded());
    }

    #[test]
    fn healthy_envelope_has_no_flags() {
        let policy = QualityPolicy::new()
            .with_min_acceptable(0.5)
            .with_max_acceptable(0.8)
            .with_max_spread(1.0)
            .with_mean_floor(0.5);
        let envelope = QualityEnvelope::from_samples(&[0.9, 0.95, 1.0], &policy);
        assert!(envelope.flags.is_empty(), "flags: {:?}", envelope.flags);
        assert!(!envelope.is_degraded());
    }

    #[test]
    fn empty_sample_is_flagged() {
        let envelope = QualityEnvelope::from_samples(&[], &QualityPolicy::new());
        assert_eq!(envelope.count, 0);
        assert_eq!(envelope.flags, vec![DegradationFlag::WeakCeiling]);
    }
}
