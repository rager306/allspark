//! M2 — temporal decay: exponential down-weighting by age
//! (ruvector `temporal-coherence`).
//!
//! A memory that was true six months ago is less useful than one from
//! yesterday. [`decay`] applies the standard half-life model:
//!
//! ```text
//! decayed = weight · 0.5^(age / half_life)
//! ```
//!
//! so the weight halves every `half_life` units of age. [`TemporalDecay`]
//! wraps a half-life and can rerank a batch of weighted items.

/// Decay `weight` by `age` under a `half_life`.
///
/// A non-positive `half_life` (or non-positive `age`) leaves the weight
/// unchanged, so the function is a no-op on fresh items.
#[must_use]
pub fn decay(weight: f64, age: f64, half_life: f64) -> f64 {
    if half_life <= 0.0 || age <= 0.0 {
        return weight;
    }
    weight * 0.5_f64.powf(age / half_life)
}

/// A weighted item to be decayed and ranked.
#[derive(Debug, Clone, PartialEq)]
pub struct Weighted {
    /// Item id.
    pub id: String,
    /// Raw weight before decay.
    pub weight: f64,
    /// Age of the item (same units as the half-life).
    pub age: f64,
}

impl Weighted {
    /// A new weighted item.
    #[must_use]
    pub fn new(id: impl Into<String>, weight: f64, age: f64) -> Self {
        Self {
            id: id.into(),
            weight,
            age,
        }
    }
}

/// A decay model with a fixed half-life.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemporalDecay {
    /// Age units per halving of the weight.
    pub half_life: f64,
}

impl TemporalDecay {
    /// A model with the given half-life.
    #[must_use]
    pub fn new(half_life: f64) -> Self {
        Self { half_life }
    }

    /// Apply the model to one `(weight, age)` pair.
    #[must_use]
    pub fn apply(&self, weight: f64, age: f64) -> f64 {
        decay(weight, age, self.half_life)
    }

    /// Rank `items` by decayed weight, descending, ties broken by id.
    #[must_use]
    pub fn rank(&self, items: &[Weighted]) -> Vec<(String, f64)> {
        let mut scored: Vec<(String, f64)> = items
            .iter()
            .map(|item| (item.id.clone(), self.apply(item.weight, item.age)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_half_life_halves_the_weight() {
        assert!((decay(1.0, 10.0, 10.0) - 0.5).abs() < 1e-12);
        assert!((decay(2.0, 20.0, 10.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn fresh_items_are_undecayed() {
        assert!((decay(3.0, 0.0, 10.0) - 3.0).abs() < 1e-12);
        assert!((decay(3.0, 5.0, 0.0) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn older_items_decay_faster() {
        let fresh = decay(1.0, 1.0, 10.0);
        let old = decay(1.0, 100.0, 10.0);
        assert!(fresh > old);
    }

    #[test]
    fn rank_orders_by_decayed_weight() {
        let model = TemporalDecay::new(10.0);
        let items = vec![
            Weighted::new("old-heavy", 10.0, 100.0),
            Weighted::new("new-light", 1.0, 0.0),
        ];
        let ranked = model.rank(&items);
        assert_eq!(ranked[0].0, "new-light");
        assert_eq!(ranked[1].0, "old-heavy");
    }

    #[test]
    fn rank_is_deterministic_on_ties() {
        let model = TemporalDecay::new(10.0);
        let items = vec![Weighted::new("b", 1.0, 5.0), Weighted::new("a", 1.0, 5.0)];
        let ranked = model.rank(&items);
        assert_eq!(ranked[0].0, "a");
        assert_eq!(ranked[1].0, "b");
    }
}
