//! Process-wide metrics for the native tool surface.
//!
//! One registry shared by every session in the process, so "how expensive is `graph`
//! `search`" has an answer that does not require replaying a session. It is filled at
//! the telemetry boundary (`AgentEvent::ToolExecutionEnd`), which is the single place a
//! finished tool call is observed with its duration — instrumenting each tool would
//! mean four copies of the same measurement and a fifth one the day a tool is added.
//!
//! The registry renders Prometheus text (`pa_graph::MetricsRegistry::render`), so a
//! `metrics` scrape endpoint is a transport question, not a measurement one.

use std::sync::{Arc, LazyLock};

use pa_graph::MetricsRegistry;

static REGISTRY: LazyLock<Arc<MetricsRegistry>> = LazyLock::new(|| {
    Arc::new(MetricsRegistry::with_version(
        env!("CARGO_PKG_VERSION").to_string(),
    ))
});

/// The process-wide registry. Cheap to call: one `LazyLock` read plus a clone of the
/// `Arc`, so a hot path can ask per event without caching it.
#[must_use]
pub fn registry() -> Arc<MetricsRegistry> {
    Arc::clone(&REGISTRY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_registry_is_shared() {
        let first = registry();
        let second = registry();
        assert!(Arc::ptr_eq(&first, &second), "one registry per process");
    }

    /// The labelled-series mechanics, on a registry of this test's own.
    ///
    /// It deliberately does not use the process registry: that one is written by
    /// concurrent tests, so an exact count there is a flake. The process-wide behaviour is
    /// asserted in `telemetry::tests` by the fact that a finished tool call appears.
    #[test]
    fn labelled_series_of_the_same_metric_stay_distinct() {
        let first = MetricsRegistry::new();
        first
            .labelled_counter(
                "tool_calls_total",
                "calls",
                &[("tool", "graph"), ("outcome", "ok")],
            )
            .inc();
        first
            .labelled_counter(
                "tool_calls_total",
                "calls",
                &[("tool", "edit"), ("outcome", "error")],
            )
            .inc();
        first
            .labelled_histogram(
                "tool_duration_seconds",
                "time",
                &[0.01, 1.0],
                &[("tool", "graph")],
            )
            .observe(0.5);

        let rendered = first.render();
        // Both series survive: an unlabelled registry would have kept only the last.
        assert!(
            rendered.contains("tool_calls_total{tool=\"graph\",outcome=\"ok\"} 1"),
            "{rendered}"
        );
        assert!(
            rendered.contains("tool_calls_total{tool=\"edit\",outcome=\"error\"} 1"),
            "{rendered}"
        );
        assert!(
            rendered.contains("tool_duration_seconds_bucket{tool=\"graph\",le=\"0.01\"} 0"),
            "a labelled histogram keeps `le` inside the label set: {rendered}"
        );
        assert!(
            rendered.contains("tool_duration_seconds_bucket{tool=\"graph\",le=\"+Inf\"} 1"),
            "{rendered}"
        );
    }
}
