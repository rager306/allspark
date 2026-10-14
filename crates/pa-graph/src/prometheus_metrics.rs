//! R14 — Prometheus metrics with health/readiness, hand-rolled text format.
//!
//! A process needs to answer two different questions: *what is it doing*
//! (metrics) and *can it serve* (health/readiness). [`MetricsRegistry`] covers
//! both, and it deliberately depends on nothing: the Prometheus exposition
//! text format is simple enough to emit directly, and pulling in a metrics
//! crate for counters, gauges, and histograms would be a heavy dependency for a
//! few hundred lines of formatting.
//!
//! Three metric kinds are supported, matching the Prometheus model:
//!
//! - **Counter** — monotonically increasing; `inc`/`inc_by`.
//! - **Gauge** — goes up or down; `set`/`inc`/`dec`.
//! - **Histogram** — observation buckets plus `_sum` and `_count`.
//!
//! [`MetricsRegistry::render`] emits the text format, including `# HELP` and
//! `# TYPE` lines. [`MetricsRegistry::health`] and [`MetricsRegistry::readiness`]
//! report liveness and whether every registered subsystem is ready.
//!
//! ```no_run
//! use pa_graph::MetricsRegistry;
//!
//! let metrics = MetricsRegistry::new();
//! metrics.counter("requests_total", "Requests served").inc();
//! metrics.gauge("in_flight", "Requests in flight").set(3.0);
//! metrics.histogram("latency_seconds", "Latency", &[0.01, 0.1, 1.0]).observe(0.05);
//! metrics.set_ready("store", true);
//! assert!(metrics.readiness().ready);
//! # let _ = metrics.render();
//! ```

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// Default histogram buckets (seconds), matching the usual latency layout.
pub const DEFAULT_BUCKETS: &[f64] = &[
    0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0,
];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A monotonically increasing counter.
#[derive(Debug, Default)]
pub struct Counter {
    value: Mutex<f64>,
}

impl Counter {
    /// Increment by one.
    pub fn inc(&self) {
        self.inc_by(1.0);
    }

    /// Increment by `amount` (negative amounts are ignored).
    pub fn inc_by(&self, amount: f64) {
        if amount <= 0.0 {
            return;
        }
        *lock(&self.value) += amount;
    }

    /// The current value.
    #[must_use]
    pub fn get(&self) -> f64 {
        *lock(&self.value)
    }
}

/// A gauge that can move both ways.
#[derive(Debug, Default)]
pub struct Gauge {
    value: Mutex<f64>,
}

impl Gauge {
    /// Set the gauge to `value`.
    pub fn set(&self, value: f64) {
        *lock(&self.value) = value;
    }

    /// Add `amount` to the gauge.
    pub fn inc_by(&self, amount: f64) {
        *lock(&self.value) += amount;
    }

    /// Subtract `amount` from the gauge.
    pub fn dec_by(&self, amount: f64) {
        *lock(&self.value) -= amount;
    }

    /// The current value.
    #[must_use]
    pub fn get(&self) -> f64 {
        *lock(&self.value)
    }
}

/// A histogram: cumulative bucket counts plus a sum and a count.
#[derive(Debug)]
pub struct Histogram {
    bounds: Vec<f64>,
    inner: Mutex<HistogramState>,
}

#[derive(Debug, Default)]
struct HistogramState {
    /// Cumulative counts, one per bound, plus a trailing `+Inf` bucket.
    cumulative: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Histogram {
    /// A histogram with the given upper bounds (sorted and de-duplicated).
    #[must_use]
    pub fn new(bounds: &[f64]) -> Self {
        let mut bounds: Vec<f64> = bounds.iter().copied().filter(|b| b.is_finite()).collect();
        bounds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        bounds.dedup();
        let mut cumulative = vec![0u64; bounds.len() + 1];
        cumulative.shrink_to_fit();
        Self {
            bounds,
            inner: Mutex::new(HistogramState {
                cumulative,
                sum: 0.0,
                count: 0,
            }),
        }
    }

    /// Observe one value.
    pub fn observe(&self, value: f64) {
        let mut state = lock(&self.inner);
        state.sum += value;
        state.count += 1;
        let bucket = self
            .bounds
            .iter()
            .position(|bound| value <= *bound)
            .unwrap_or(self.bounds.len());
        for slot in state.cumulative.iter_mut().skip(bucket) {
            *slot += 1;
        }
    }

    /// The bucket upper bounds.
    #[must_use]
    pub fn bounds(&self) -> &[f64] {
        &self.bounds
    }

    /// Total observations.
    #[must_use]
    pub fn count(&self) -> u64 {
        lock(&self.inner).count
    }

    /// Sum of all observed values.
    #[must_use]
    pub fn sum(&self) -> f64 {
        lock(&self.inner).sum
    }

    /// Cumulative counts per bucket (including the trailing `+Inf`).
    #[must_use]
    pub fn cumulative_counts(&self) -> Vec<u64> {
        lock(&self.inner).cumulative.clone()
    }
}

/// What kind of metric a registered series is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Counter.
    Counter,
    /// Gauge.
    Gauge,
    /// Histogram.
    Histogram,
}

/// A handle to a registered counter.
#[derive(Debug, Clone)]
pub struct CounterHandle {
    inner: Arc<Counter>,
}

impl CounterHandle {
    /// Increment by one.
    pub fn inc(&self) {
        self.inner.inc();
    }

    /// Increment by `amount`.
    pub fn inc_by(&self, amount: f64) {
        self.inner.inc_by(amount);
    }

    /// The current value.
    #[must_use]
    pub fn get(&self) -> f64 {
        self.inner.get()
    }
}

/// A handle to a registered gauge.
#[derive(Debug, Clone)]
pub struct GaugeHandle {
    inner: Arc<Gauge>,
}

impl GaugeHandle {
    /// Set the gauge.
    pub fn set(&self, value: f64) {
        self.inner.set(value);
    }

    /// Add to the gauge.
    pub fn inc_by(&self, amount: f64) {
        self.inner.inc_by(amount);
    }

    /// Subtract from the gauge.
    pub fn dec_by(&self, amount: f64) {
        self.inner.dec_by(amount);
    }

    /// The current value.
    #[must_use]
    pub fn get(&self) -> f64 {
        self.inner.get()
    }
}

/// A handle to a registered histogram.
#[derive(Debug, Clone)]
pub struct HistogramHandle {
    inner: Arc<Histogram>,
}

impl HistogramHandle {
    /// Observe one value.
    pub fn observe(&self, value: f64) {
        self.inner.observe(value);
    }

    /// Total observations.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.inner.count()
    }

    /// Sum of observed values.
    #[must_use]
    pub fn sum(&self) -> f64 {
        self.inner.sum()
    }
}

#[derive(Debug)]
enum Series {
    Counter { help: String, value: Arc<Counter> },
    Gauge { help: String, value: Arc<Gauge> },
    Histogram { help: String, value: Arc<Histogram> },
}

/// Overall health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Serving normally.
    Healthy,
    /// Serving, but at least one subsystem is not ready.
    Degraded,
    /// Not serving.
    Unhealthy,
}

impl HealthStatus {
    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
        }
    }
}

/// Liveness: the process is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    /// The status.
    pub status: HealthStatus,
    /// Process version.
    pub version: String,
    /// Seconds since the registry was created.
    pub uptime_seconds: u64,
}

/// Readiness: every registered subsystem is ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    /// The status.
    pub status: HealthStatus,
    /// Whether every subsystem reports ready.
    pub ready: bool,
    /// Per-subsystem readiness, sorted by name.
    pub subsystems: Vec<(String, bool)>,
}

/// The metric registry: named counters, gauges, and histograms plus readiness.
#[derive(Debug)]
pub struct MetricsRegistry {
    series: Mutex<BTreeMap<String, Series>>,
    readiness: Mutex<BTreeMap<String, bool>>,
    start: Instant,
    version: String,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    /// A registry with the crate version and a fresh uptime clock.
    #[must_use]
    pub fn new() -> Self {
        Self::with_version(env!("CARGO_PKG_VERSION").to_string())
    }

    /// A registry reporting `version`.
    #[must_use]
    pub fn with_version(version: String) -> Self {
        Self {
            series: Mutex::new(BTreeMap::new()),
            readiness: Mutex::new(BTreeMap::new()),
            start: Instant::now(),
            version,
        }
    }

    /// Register (or fetch) a counter named `name`.
    pub fn counter(&self, name: &str, help: &str) -> CounterHandle {
        let mut series = lock(&self.series);
        if let Some(Series::Counter { value, .. }) = series.get(name) {
            return CounterHandle {
                inner: Arc::clone(value),
            };
        }
        let value = Arc::new(Counter::default());
        series.insert(
            name.to_string(),
            Series::Counter {
                help: help.to_string(),
                value: Arc::clone(&value),
            },
        );
        CounterHandle { inner: value }
    }

    /// Register (or fetch) a gauge named `name`.
    pub fn gauge(&self, name: &str, help: &str) -> GaugeHandle {
        let mut series = lock(&self.series);
        if let Some(Series::Gauge { value, .. }) = series.get(name) {
            return GaugeHandle {
                inner: Arc::clone(value),
            };
        }
        let value = Arc::new(Gauge::default());
        series.insert(
            name.to_string(),
            Series::Gauge {
                help: help.to_string(),
                value: Arc::clone(&value),
            },
        );
        GaugeHandle { inner: value }
    }

    /// Register (or fetch) a histogram named `name` with `bounds`.
    pub fn histogram(&self, name: &str, help: &str, bounds: &[f64]) -> HistogramHandle {
        let mut series = lock(&self.series);
        if let Some(Series::Histogram { value, .. }) = series.get(name) {
            return HistogramHandle {
                inner: Arc::clone(value),
            };
        }
        let value = Arc::new(Histogram::new(bounds));
        series.insert(
            name.to_string(),
            Series::Histogram {
                help: help.to_string(),
                value: Arc::clone(&value),
            },
        );
        HistogramHandle { inner: value }
    }

    /// The kind of the metric registered under `name`, if any.
    #[must_use]
    pub fn kind(&self, name: &str) -> Option<MetricKind> {
        lock(&self.series).get(name).map(|series| match series {
            Series::Counter { .. } => MetricKind::Counter,
            Series::Gauge { .. } => MetricKind::Gauge,
            Series::Histogram { .. } => MetricKind::Histogram,
        })
    }

    /// Number of registered series.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.series).len()
    }

    /// Whether no series is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        lock(&self.series).is_empty()
    }

    /// Mark a named subsystem ready or not.
    pub fn set_ready(&self, subsystem: &str, ready: bool) {
        lock(&self.readiness).insert(subsystem.to_string(), ready);
    }

    /// Liveness: this registry is always up, so [`HealthStatus::Healthy`].
    #[must_use]
    pub fn health(&self) -> Health {
        Health {
            status: HealthStatus::Healthy,
            version: self.version.clone(),
            uptime_seconds: self.start.elapsed().as_secs(),
        }
    }

    /// Readiness: healthy only when every registered subsystem is ready.
    ///
    /// With no subsystems registered the process is [`HealthStatus::Degraded`]
    /// — nothing has *asserted* it can serve, so claiming healthy would be a
    /// guess.
    #[must_use]
    pub fn readiness(&self) -> Readiness {
        let subsystems: Vec<(String, bool)> = lock(&self.readiness)
            .iter()
            .map(|(name, ready)| (name.clone(), *ready))
            .collect();
        let ready = !subsystems.is_empty() && subsystems.iter().all(|(_, ready)| *ready);
        let status = if ready {
            HealthStatus::Healthy
        } else if subsystems.iter().any(|(_, ready)| *ready) {
            HealthStatus::Degraded
        } else {
            HealthStatus::Unhealthy
        };
        Readiness {
            status,
            ready,
            subsystems,
        }
    }

    /// Render every metric in the Prometheus text exposition format.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let series = lock(&self.series);
        let mut out = String::new();
        for (name, metric) in series.iter() {
            let (kind, help) = match metric {
                Series::Counter { help, .. } => ("counter", help),
                Series::Gauge { help, .. } => ("gauge", help),
                Series::Histogram { help, .. } => ("histogram", help),
            };
            let _ = writeln!(out, "# HELP {name} {}", escape_help(help));
            let _ = writeln!(out, "# TYPE {name} {kind}");
            match metric {
                Series::Counter { value, .. } => {
                    let _ = writeln!(out, "{name} {}", format_float(value.get()));
                }
                Series::Gauge { value, .. } => {
                    let _ = writeln!(out, "{name} {}", format_float(value.get()));
                }
                Series::Histogram { value, .. } => {
                    let bounds = value.bounds();
                    let cumulative = value.cumulative_counts();
                    for (i, bound) in bounds.iter().enumerate() {
                        let _ = writeln!(
                            out,
                            "{name}_bucket{{le=\"{}\"}} {}",
                            format_float(*bound),
                            cumulative[i]
                        );
                    }
                    let _ = writeln!(
                        out,
                        "{name}_bucket{{le=\"+Inf\"}} {}",
                        cumulative.last().copied().unwrap_or(0)
                    );
                    let _ = writeln!(out, "{name}_sum {}", format_float(value.sum()));
                    let _ = writeln!(out, "{name}_count {}", value.count());
                }
            }
        }
        out
    }
}

/// Prometheus help text: backslashes and newlines escaped.
fn escape_help(help: &str) -> String {
    help.replace('\\', "\\\\").replace('\n', "\\n")
}

/// Format a float the way the exposition format expects: integers without a
/// fractional part, and `+Inf`/`NaN` spelled out.
fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Inf".to_string()
        } else {
            "+Inf".to_string()
        };
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_increments_monotonically() {
        let metrics = MetricsRegistry::new();
        let counter = metrics.counter("requests_total", "Requests served");
        counter.inc();
        counter.inc_by(4.0);
        assert_eq!(counter.get(), 5.0);
        // Negative increments are ignored: a counter never goes down.
        counter.inc_by(-3.0);
        assert_eq!(counter.get(), 5.0);
    }

    #[test]
    fn gauge_moves_both_ways() {
        let metrics = MetricsRegistry::new();
        let gauge = metrics.gauge("in_flight", "In flight");
        gauge.set(3.0);
        gauge.inc_by(2.0);
        gauge.dec_by(1.0);
        assert_eq!(gauge.get(), 4.0);
    }

    #[test]
    fn histogram_buckets_are_cumulative() {
        let hist = Histogram::new(&[0.1, 1.0, 10.0]);
        hist.observe(0.05); // le=0.1
        hist.observe(0.5); // le=1.0
        hist.observe(5.0); // le=10.0
        hist.observe(50.0); // +Inf
        assert_eq!(hist.count(), 4);
        assert_eq!(hist.cumulative_counts(), vec![1, 2, 3, 4]);
        assert!((hist.sum() - 55.55).abs() < 1e-9);
    }

    #[test]
    fn histogram_bounds_are_sorted_and_deduplicated() {
        let hist = Histogram::new(&[1.0, 0.1, 1.0, 0.1]);
        assert_eq!(hist.bounds(), &[0.1, 1.0]);
    }

    #[test]
    fn render_emits_help_type_and_samples() {
        let metrics = MetricsRegistry::new();
        metrics.counter("requests_total", "Requests served").inc_by(7.0);
        metrics.gauge("in_flight", "In flight").set(2.0);
        metrics.histogram("latency_seconds", "Latency", &[0.1, 1.0]).observe(0.5);
        let text = metrics.render();

        assert!(text.contains("# HELP requests_total Requests served"));
        assert!(text.contains("# TYPE requests_total counter"));
        assert!(text.contains("requests_total 7"));
        assert!(text.contains("# TYPE in_flight gauge"));
        assert!(text.contains("in_flight 2"));
        assert!(text.contains("# TYPE latency_seconds histogram"));
        assert!(text.contains("latency_seconds_bucket{le=\"0.1\"} 0"));
        assert!(text.contains("latency_seconds_bucket{le=\"1\"} 1"));
        assert!(text.contains("latency_seconds_bucket{le=\"+Inf\"} 1"));
        assert!(text.contains("latency_seconds_sum 0.5"));
        assert!(text.contains("latency_seconds_count 1"));
    }

    #[test]
    fn render_is_sorted_by_metric_name() {
        let metrics = MetricsRegistry::new();
        metrics.counter("z_total", "z");
        metrics.counter("a_total", "a");
        let text = metrics.render();
        let a = text.find("a_total").expect("a");
        let z = text.find("z_total").expect("z");
        assert!(a < z, "metrics render in name order");
    }

    #[test]
    fn health_is_always_healthy() {
        let metrics = MetricsRegistry::new();
        let health = metrics.health();
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn readiness_requires_every_subsystem() {
        let metrics = MetricsRegistry::new();
        // Nothing registered yet: not ready (nothing asserted it can serve).
        assert!(!metrics.readiness().ready);
        assert_eq!(metrics.readiness().status, HealthStatus::Unhealthy);

        metrics.set_ready("store", true);
        metrics.set_ready("index", true);
        assert!(metrics.readiness().ready);
        assert_eq!(metrics.readiness().status, HealthStatus::Healthy);

        metrics.set_ready("index", false);
        let readiness = metrics.readiness();
        assert!(!readiness.ready);
        assert_eq!(readiness.status, HealthStatus::Degraded);
        assert_eq!(readiness.subsystems.len(), 2);
    }

    #[test]
    fn registering_the_same_name_twice_shares_the_series() {
        let metrics = MetricsRegistry::new();
        let a = metrics.counter("c", "help");
        let b = metrics.counter("c", "help");
        a.inc();
        assert_eq!(b.get(), 1.0);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics.kind("c"), Some(MetricKind::Counter));
    }

    #[test]
    fn float_formatting_is_prometheus_correct() {
        assert_eq!(format_float(7.0), "7");
        assert_eq!(format_float(0.5), "0.5");
        assert_eq!(format_float(f64::INFINITY), "+Inf");
        assert_eq!(format_float(f64::NEG_INFINITY), "-Inf");
        assert_eq!(format_float(f64::NAN), "NaN");
    }

    #[test]
    fn empty_registry_renders_nothing() {
        let metrics = MetricsRegistry::new();
        assert!(metrics.is_empty());
        assert_eq!(metrics.render(), "");
    }
}
