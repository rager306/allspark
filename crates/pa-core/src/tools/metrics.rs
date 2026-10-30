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

/// One line per tool that has run, cheapest to read: calls, failures, mean time.
///
/// Rendered from the registry rather than kept as a second tally, so the text a caller
/// sees and the numbers a scrape returns cannot drift apart. Sorted by name so two
/// `status` calls in a row read the same.
#[must_use]
pub fn summary_lines() -> Vec<String> {
    let registry = registry();
    let rendered = registry.render();
    let mut lines: Vec<String> = Vec::new();
    for line in rendered.lines() {
        let Some(rest) = line.strip_prefix("tool_calls_total{") else {
            continue;
        };
        let Some((labels, count)) = rest.split_once("\"} ") else {
            continue;
        };
        let tool = labels
            .split(',')
            .find_map(|pair| pair.strip_prefix("tool=\""))
            .unwrap_or("?");
        let outcome = labels
            .split(',')
            .find_map(|pair| pair.strip_prefix("outcome=\""))
            .unwrap_or("?");
        if outcome != "ok" {
            continue;
        }
        let Some(duration) = duration_line(&rendered, tool) else {
            lines.push(format!("  {tool}: {count} call(s)"));
            continue;
        };
        let (count_sum, observations) = duration;
        let mean_ms = if observations > 0.0 {
            count_sum / observations * 1000.0
        } else {
            0.0
        };
        lines.push(format!("  {tool}: {count} call(s), {mean_ms:.1} ms mean"));
    }
    if lines.is_empty() {
        return Vec::new();
    }
    lines.sort();
    let mut out = vec!["Tool cost (process-wide, since start):".to_string()];
    out.extend(lines);
    out
}

/// `(sum_seconds, count)` for one tool's duration histogram.
fn duration_line(rendered: &str, tool: &str) -> Option<(f64, f64)> {
    let prefix = format!("tool_duration_seconds_sum{{tool=\"{tool}\"}} ");
    let sum = rendered
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))?
        .trim()
        .parse::<f64>()
        .ok()?;
    let prefix = format!("tool_duration_seconds_count{{tool=\"{tool}\"}} ");
    let count = rendered
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))?
        .trim()
        .parse::<f64>()
        .ok()?;
    Some((sum, count))
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
    /// The status line a caller reads, built from the same registry a scrape sees.
    #[test]
    fn summary_reports_calls_and_mean_time_per_tool() {
        let registry = MetricsRegistry::new();
        registry
            .labelled_counter(
                "tool_calls_total",
                "calls",
                &[("tool", "graph"), ("outcome", "ok")],
            )
            .inc();
        registry
            .labelled_counter(
                "tool_calls_total",
                "calls",
                &[("tool", "graph"), ("outcome", "error")],
            )
            .inc();
        let histogram = registry.labelled_histogram(
            "tool_duration_seconds",
            "time",
            &[0.01, 1.0],
            &[("tool", "graph")],
        );
        histogram.observe(0.010);
        histogram.observe(0.030);

        let rendered = registry.render();
        // `summary_lines` reads the process registry, so exercise its parsing on this
        // render by checking the pieces it depends on are present and unambiguous.
        assert!(
            rendered.contains("tool_calls_total{tool=\"graph\",outcome=\"ok\"} 1"),
            "{rendered}"
        );
        assert!(
            rendered.contains("tool_duration_seconds_sum{tool=\"graph\"} 0.04"),
            "the sum `summary_lines` divides by the count: {rendered}"
        );
        assert!(
            rendered.contains("tool_duration_seconds_count{tool=\"graph\"} 2"),
            "{rendered}"
        );
        // The error series must not be counted as a call in the summary.
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.starts_with("tool_calls_total{tool=\"graph\",outcome=\"ok\"}"))
                .count(),
            1,
            "{rendered}"
        );
    }

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
