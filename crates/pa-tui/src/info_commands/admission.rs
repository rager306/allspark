//! FORK-ONLY `/admission`: the provider-admission plane's observability
//! readout (ADR `ratelimit-admission`, M5). The data is the daemon's
//! `provider_admission` `snapshot` payload (`{ "providers": [ ... ],
//! "queuedTotal": N }`), one row per `(provider, model)` the registry has
//! seen: the AIMD limit against its ceiling, the live in-flight and queue
//! depths, the 429 streak, and the provider breaker with its retry-budget
//! standing. The view is a demand snapshot: fetched when the command runs,
//! never polled, no state kept between opens.

use serde_json::Value;

use super::{dim, raw_span, ClientLine, ClientSpan};
use crate::theme::ThemeColor;

/// One `(provider, model)` row of the snapshot wire.
struct ProviderRow {
    provider: String,
    model: String,
    limit: u64,
    in_flight: u64,
    ceiling: u64,
    streak_429: u64,
    queue_depth: u64,
    breaker_state: String,
    budget_failures: u64,
    budget_samples: u64,
}

impl ProviderRow {
    /// Parse one `providers` entry; fields the wire omits render as zero.
    /// A row without a `provider` string is dropped (the key is the row).
    fn parse(entry: &Value) -> Option<ProviderRow> {
        let provider = entry.get("provider").and_then(Value::as_str)?.to_string();
        let field = |name: &str| entry.get(name).and_then(Value::as_u64).unwrap_or_default();
        Some(ProviderRow {
            provider,
            model: entry
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            limit: field("currentLimit"),
            in_flight: field("inFlight"),
            ceiling: field("ceiling"),
            streak_429: field("streak429"),
            queue_depth: field("queueDepth"),
            breaker_state: entry
                .get("breakerState")
                .and_then(Value::as_str)
                .unwrap_or("closed")
                .to_string(),
            budget_failures: field("budgetFailures"),
            budget_samples: field("budgetSamples"),
        })
    }

    /// The `provider/model` selector (a bare `provider` when no model is
    /// recorded — the registry keys spawn gates that way).
    fn selector(&self) -> String {
        if self.model.is_empty() {
            self.provider.clone()
        } else {
            format!("{}/{}", self.provider, self.model)
        }
    }

    /// One provider's row: the selector, then the counters, the breaker
    /// colored by its standing, and the retry budget as `failures/samples`.
    fn line(&self) -> ClientLine {
        vec![
            raw_span(format!(
                "{}  limit={}  in_flight={}  ceiling={}  429_streak={}  queue_depth={}  breaker=",
                self.selector(),
                self.limit,
                self.in_flight,
                self.ceiling,
                self.streak_429,
                self.queue_depth,
            )),
            ClientSpan {
                text: self.breaker_state.clone(),
                color: Some(breaker_color(&self.breaker_state)),
            },
            raw_span(format!(
                "  budget={}/{}",
                self.budget_failures, self.budget_samples,
            )),
        ]
    }
}

/// The breaker's standing color: `open` fails fast (error), `half_open`
/// holds its one probe (warning), everything else is normal traffic.
fn breaker_color(state: &str) -> ThemeColor {
    match state {
        "open" => ThemeColor::Error,
        "half_open" => ThemeColor::Warning,
        _ => ThemeColor::Success,
    }
}

/// The `/admission` rows: one line per `(provider, model)` the registry
/// has seen, the cross-provider queue total, and the refresh hint (the
/// panel is a demand snapshot — it never polls). The `snapshot` is the
/// daemon's `provider_admission` response data.
#[must_use]
pub fn admission_rows(snapshot: &Value) -> Vec<ClientLine> {
    let providers: Vec<ProviderRow> = snapshot
        .get("providers")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(ProviderRow::parse).collect())
        .unwrap_or_default();
    let mut rows = vec![vec![raw_span("Provider Admission")], vec![]];
    if providers.is_empty() {
        rows.push(vec![dim(
            "No provider admission state yet; rows appear once streams take slots.",
        )]);
        return rows;
    }
    for provider in &providers {
        rows.push(provider.line());
    }
    rows.push(vec![]);
    let queued_total = snapshot
        .get("queuedTotal")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    rows.push(vec![
        dim("Queued:"),
        raw_span(format!(" {queued_total} waiting across providers")),
    ]);
    rows.push(vec![dim(
        "Demand snapshot \u{2014} run /admission again to refresh.",
    )]);
    rows
}
