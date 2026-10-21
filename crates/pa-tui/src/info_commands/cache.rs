//! FORK-ONLY `/cache`: the per-model cache statistics readout. The data is
//! the daemon's `get_cache_stats` payload (`{ "models": [ { provider, id,
//! input, cacheRead, cacheWrite, totalTokens, cost { input, cacheRead,
//! cacheWrite, total } } ] }`), with the same fields also read from a nested
//! `ownUsage` block when present (the context-tree fold's shape).

use serde_json::Value;

use super::{dim, js_to_fixed, raw_span, ClientLine};

/// One model's cache row, parsed from either the flat `get_cache_stats` shape
/// or a nested `ownUsage` block.
struct ModelCache {
    provider: String,
    id: String,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    total_tokens: u64,
    /// `cost.input` / `cost.cacheRead`: the uncached-input and cache-read
    /// prices, used for the savings estimate.
    cost_input: f64,
    cost_cache_read: f64,
}

impl ModelCache {
    fn parse(entry: &Value) -> Option<ModelCache> {
        // The flat fields win; a nested `ownUsage` block is the fallback (the
        // context-tree fold's shape).
        let usage = entry.get("ownUsage").unwrap_or(entry);
        let id = entry.get("id").and_then(Value::as_str)?.to_string();
        let provider = entry
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let u64_field = |field: &str| usage.get(field).and_then(Value::as_u64).unwrap_or_default();
        let cost = |field: &str| {
            usage
                .get("cost")
                .and_then(|cost| cost.get(field))
                .and_then(Value::as_f64)
                .unwrap_or_default()
        };
        Some(ModelCache {
            provider,
            id,
            input: u64_field("input"),
            cache_read: u64_field("cacheRead"),
            cache_write: u64_field("cacheWrite"),
            total_tokens: u64_field("totalTokens"),
            cost_input: cost("input"),
            cost_cache_read: cost("cacheRead"),
        })
    }

    /// The `provider/id` key (a bare `id` when no provider is recorded).
    fn key(&self) -> String {
        if self.provider.is_empty() {
            self.id.clone()
        } else {
            format!("{}/{}", self.provider, self.id)
        }
    }

    /// `cacheRead / (input + cacheRead) * 100`, rounded; `None` when the
    /// denominator is zero. The shared [`pa_types::usage::cache_hit_percent`]
    /// keeps this panel and the tray's `cache N%` segment on one formula.
    fn hit_percent(&self) -> Option<u64> {
        pa_types::usage::cache_hit_percent(self.input, self.cache_read)
    }

    /// The estimated dollars saved by the cache: the cache-read tokens priced
    /// at the uncached-input rate minus what was actually paid for them.
    /// `None` when there is nothing to estimate.
    fn saved_dollars(&self) -> Option<f64> {
        if self.input == 0 || self.cache_read == 0 {
            return None;
        }
        let uncached_rate = self.cost_input / self.input as f64;
        Some(self.cache_read as f64 * uncached_rate - self.cost_cache_read)
    }
}

/// The session's cache totals as the `/cache` panel reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheTotals {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
    /// The shared hit ratio; `None` when nothing was sent, so the tray hides the
    /// segment instead of printing a meaningless `0%`.
    pub hit_percent: Option<u64>,
}

/// Whether `current_model` names this model: an exact `provider/id` match, a
/// suffix match on the bare id, or a prefix `provider/` match.
fn is_current(model: &ModelCache, current_model: &str) -> bool {
    let key = model.key();
    key == current_model
        || key.ends_with(&format!("/{current_model}"))
        || current_model.ends_with(&format!("/{}", model.id))
        || current_model == model.id
}

/// `formatCost`: `$<toFixed(2)>`.
fn format_cost(cost: f64) -> String {
    format!("${}", js_to_fixed(cost, 2))
}

/// One model's summary line: `provider/id  input=N  cached=R (hit%)  write=W  total=T`.
fn model_line(model: &ModelCache) -> String {
    let hit = model
        .hit_percent()
        .map_or_else(|| "-".to_string(), |percent| format!("{percent}%"));
    format!(
        "{}  input={}  cached={} ({})  write={}  total={}",
        model.key(),
        model.input,
        model.cache_read,
        hit,
        model.cache_write,
        model.total_tokens,
    )
}

/// The grand total across the rows `/cache` renders, plus the same ratio the panel
/// prints: the TUI tray's `cache N%` segment reads THIS function over the very
/// `get_cache_stats` payload `/cache` renders, so the two surfaces share one fold
/// and one formula by construction. `hit_percent` is `None` when no model logged
/// traffic, which is what hides the tray segment.
#[must_use]
pub fn cache_totals(tree: &Value) -> CacheTotals {
    let mut totals = CacheTotals::default();
    let models: Vec<ModelCache> = tree
        .get("models")
        .and_then(Value::as_array)
        .map(|models| models.iter().filter_map(ModelCache::parse).collect())
        .unwrap_or_default();
    for model in models
        .iter()
        .filter(|model| model.input + model.cache_read > 0)
    {
        totals.input = totals.input.saturating_add(model.input);
        totals.cache_read = totals.cache_read.saturating_add(model.cache_read);
        totals.cache_write = totals.cache_write.saturating_add(model.cache_write);
        totals.total_tokens = totals.total_tokens.saturating_add(model.total_tokens);
    }
    totals.hit_percent = pa_types::usage::cache_hit_percent(totals.input, totals.cache_read);
    totals
}

/// The `/cache` rows: one summary line per model with the cache-hit rate, the
/// current model flagged, the grand total, and the estimated savings. The
/// `tree` is the `get_cache_stats` payload; models without any input or
/// cache-read traffic are skipped.
#[must_use]
pub fn cache_rows(tree: &Value, current_model: Option<&str>) -> Vec<ClientLine> {
    let models: Vec<ModelCache> = tree
        .get("models")
        .and_then(Value::as_array)
        .map(|models| models.iter().filter_map(ModelCache::parse).collect())
        .unwrap_or_default();

    let mut rows = vec![vec![raw_span("Cache Stats")], vec![]];
    let active: Vec<&ModelCache> = models
        .iter()
        .filter(|model| model.input + model.cache_read > 0)
        .collect();
    if active.is_empty() {
        rows.push(vec![dim("No cache activity recorded for this session.")]);
        return rows;
    }
    for model in &active {
        let mut spans = vec![raw_span(model_line(model))];
        if current_model.is_some_and(|current| is_current(model, current)) {
            spans.push(dim(" \u{2190} current"));
        }
        rows.push(spans);
    }
    rows.push(vec![]);

    // The grand total reads the one shared fold the tray segment also reads, so a
    // session can never show one hit rate here and another in the status line.
    let totals = cache_totals(tree);
    let hit = totals
        .hit_percent
        .map_or_else(|| "-".to_string(), |percent| format!("{percent}%"));
    rows.push(vec![
        dim("Total:"),
        raw_span(format!(
            " input={}  cached={} ({hit})  write={}  total={}",
            totals.input, totals.cache_read, totals.cache_write, totals.total_tokens
        )),
    ]);

    // The savings estimate: summed over the model rows that carry a rate.
    let savings: f64 = active
        .iter()
        .filter_map(|model| model.saved_dollars())
        .sum();
    if savings > 0.0 {
        rows.push(vec![
            dim("Saved:"),
            raw_span(format!(" {} (cache hit pricing)", format_cost(savings))),
        ]);
    }
    rows
}
