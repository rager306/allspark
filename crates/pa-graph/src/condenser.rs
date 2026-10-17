//! R12 — streaming condenser: lazy re-condensation behind a dirty flag.
//!
//! "Condensation" is any expensive summary derived from a stream of items
//! (cluster centroids, a merged adjacency, a statistics digest). When items
//! keep arriving, re-condensing on every push is wasteful; a consumer usually
//! only needs a *fresh* condensation on demand.
//!
//! [`StreamingCondenser`] keeps the raw items plus a cached condensation and a
//! `dirty` flag. [`push`](StreamingCondenser::push) marks the cache dirty but
//! does no work; [`condense`](StreamingCondenser::condense) rebuilds only when
//! dirty (otherwise it returns the cached summary in O(1)). This is the "lazy
//! re-condense" pattern from ruvector's graph-condense crate.

use std::collections::BTreeMap;

/// A condensation of a stream of numeric items: count, sum, mean, min, max,
/// and a small fixed-width histogram.
#[derive(Debug, Clone, PartialEq)]
pub struct Condensation {
    /// Number of items condensed.
    pub count: usize,
    /// Sum of the items.
    pub sum: f64,
    /// Arithmetic mean.
    pub mean: f64,
    /// Minimum item.
    pub min: f64,
    /// Maximum item.
    pub max: f64,
    /// Fixed-width histogram bucket counts (10 buckets over `[min, max]`).
    pub histogram: Vec<u32>,
    /// Number of items that arrived since the previous condensation.
    pub delta_count: usize,
}

/// Number of histogram buckets.
pub const HIST_BUCKETS: usize = 10;

/// A stream condenser that rebuilds its summary lazily.
#[derive(Debug, Clone)]
pub struct StreamingCondenser {
    items: Vec<f64>,
    cached: Option<Condensation>,
    dirty: bool,
    /// Items appended since the last condensation (for `delta_count`).
    since: usize,
}

impl Default for StreamingCondenser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingCondenser {
    /// A new, empty condenser (initially clean: the empty summary is valid).
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            cached: None,
            dirty: true,
            since: 0,
        }
    }

    /// Whether the cached condensation is stale.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Number of raw items held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no items have been pushed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Append an item. Marks the condenser dirty; does no condensation work.
    pub fn push(&mut self, value: f64) {
        self.items.push(value);
        self.dirty = true;
        self.since += 1;
    }

    /// Append many items in one call.
    pub fn extend(&mut self, values: impl IntoIterator<Item = f64>) {
        for v in values {
            self.push(v);
        }
    }

    /// Return the condensation, rebuilding only if dirty.
    ///
    /// The returned reference is valid until the next `push`/`extend`.
    ///
    /// # Panics
    /// Never panics in practice: the cache is populated on the same line it is
    /// read, but the `expect` is kept as a defensive invariant check.
    pub fn condense(&mut self) -> &Condensation {
        if self.dirty || self.cached.is_none() {
            self.cached = Some(self.rebuild());
            self.dirty = false;
            self.since = 0;
        }
        self.cached.as_ref().expect("condensation was just built")
    }

    /// Force a rebuild even if clean (useful for tests/instrumentation).
    ///
    /// # Panics
    /// Never panics in practice; see [`StreamingCondenser::condense`].
    pub fn condense_forced(&mut self) -> &Condensation {
        self.cached = Some(self.rebuild());
        self.dirty = false;
        self.since = 0;
        self.cached.as_ref().expect("condensation was just built")
    }

    /// Drop all items and reset to the empty state.
    pub fn clear(&mut self) {
        self.items.clear();
        self.cached = None;
        self.dirty = true;
        self.since = 0;
    }

    fn rebuild(&self) -> Condensation {
        if self.items.is_empty() {
            return Condensation {
                count: 0,
                sum: 0.0,
                mean: 0.0,
                min: 0.0,
                max: 0.0,
                histogram: vec![0; HIST_BUCKETS],
                delta_count: self.since,
            };
        }
        let mut sum = 0.0f64;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for &v in &self.items {
            sum += v;
            min = min.min(v);
            max = max.max(v);
        }
        let count = self.items.len();
        let mean = sum / count as f64;
        let span = max - min;
        let mut histogram = vec![0u32; HIST_BUCKETS];
        for &v in &self.items {
            let bucket = if span <= f64::EPSILON {
                0
            } else {
                (((v - min) / span) * (HIST_BUCKETS as f64 - 1.0)).round() as usize
            };
            histogram[bucket.min(HIST_BUCKETS - 1)] += 1;
        }
        Condensation {
            count,
            sum,
            mean,
            min,
            max,
            histogram,
            delta_count: self.since,
        }
    }
}

/// A per-key streaming condenser: condense each key's sub-stream lazily.
#[derive(Debug, Clone, Default)]
pub struct KeyedCondenser {
    streams: BTreeMap<String, StreamingCondenser>,
}

impl KeyedCondenser {
    /// A new keyed condenser.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a value under `key`.
    pub fn push(&mut self, key: impl Into<String>, value: f64) {
        self.streams.entry(key.into()).or_default().push(value);
    }

    /// Whether `key`'s stream is dirty.
    #[must_use]
    pub fn is_dirty(&self, key: &str) -> bool {
        self.streams
            .get(key)
            .is_some_and(StreamingCondenser::is_dirty)
    }

    /// Condense one key's stream.
    pub fn condense(&mut self, key: &str) -> Option<&Condensation> {
        self.streams.get_mut(key).map(StreamingCondenser::condense)
    }

    /// Keys currently tracked.
    #[must_use]
    pub fn keys(&self) -> Vec<&str> {
        self.streams.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_marks_dirty_and_condense_clears_it() {
        let mut c = StreamingCondenser::new();
        assert!(c.is_dirty(), "a fresh condenser starts dirty");
        c.push(1.0);
        assert!(c.is_dirty());
        let _ = c.condense();
        assert!(!c.is_dirty(), "condensing clears the dirty flag");
    }

    #[test]
    fn condense_is_cached_until_next_push() {
        let mut c = StreamingCondenser::new();
        c.push(1.0);
        c.push(2.0);
        let first = c.condense().clone();
        assert_eq!(first.count, 2);
        assert_eq!(
            first.delta_count, 2,
            "both items are new to the first summary"
        );
        // A second condense with no push returns the same summary (no rebuild).
        let second = c.condense().clone();
        assert_eq!(first, second, "clean condense is cached verbatim");

        c.push(3.0);
        assert!(c.is_dirty());
        let third = c.condense().clone();
        assert_eq!(third.count, 3);
        assert_eq!(third.delta_count, 1, "one item arrived since last condense");
    }

    #[test]
    fn statistics_are_correct() {
        let mut c = StreamingCondenser::new();
        c.extend([2.0, 4.0, 6.0]);
        let cond = c.condense().clone();
        assert_eq!(cond.count, 3);
        assert_eq!(cond.sum, 12.0);
        assert_eq!(cond.mean, 4.0);
        assert_eq!(cond.min, 2.0);
        assert_eq!(cond.max, 6.0);
        let hist_total: u32 = cond.histogram.iter().sum();
        assert_eq!(hist_total, 3);
    }

    #[test]
    fn histogram_buckets_span_the_range() {
        let mut c = StreamingCondenser::new();
        c.extend([0.0, 10.0]);
        let cond = c.condense().clone();
        assert_eq!(cond.histogram.len(), HIST_BUCKETS);
        assert_eq!(cond.histogram[0], 1, "min lands in the first bucket");
        assert_eq!(cond.histogram[HIST_BUCKETS - 1], 1, "max lands in the last");
    }

    #[test]
    fn empty_condensation_is_well_formed() {
        let mut c = StreamingCondenser::new();
        let cond = c.condense().clone();
        assert_eq!(cond.count, 0);
        assert_eq!(cond.histogram.len(), HIST_BUCKETS);
        assert!(c.is_empty());
    }

    #[test]
    fn clear_resets_state() {
        let mut c = StreamingCondenser::new();
        c.push(1.0);
        let _ = c.condense();
        c.clear();
        assert!(c.is_dirty());
        assert!(c.is_empty());
        assert_eq!(c.condense().count, 0);
    }

    #[test]
    fn keyed_condenser_tracks_dirty_per_key() {
        let mut kc = KeyedCondenser::new();
        kc.push("a", 1.0);
        kc.push("a", 2.0);
        kc.push("b", 5.0);
        assert!(kc.is_dirty("a"));
        assert!(kc.is_dirty("b"));
        let a = kc.condense("a").expect("a").clone();
        assert_eq!(a.count, 2);
        assert!(!kc.is_dirty("a"), "a is clean after condensing");
        assert!(kc.is_dirty("b"), "b is still dirty");
        assert_eq!(kc.keys(), vec!["a", "b"]);
    }

    #[test]
    fn condense_forced_rebuilds_even_when_clean() {
        let mut c = StreamingCondenser::new();
        c.push(1.0);
        let _ = c.condense();
        assert!(!c.is_dirty());
        let forced = c.condense_forced().clone();
        assert_eq!(forced.count, 1);
        assert!(!c.is_dirty());
    }
}
