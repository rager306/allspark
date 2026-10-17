//! Symbol info retrieval with a time budget (L16).
//!
//! FORK-ONLY (allspark). Ports Serena's `request_info_for_symbol_batch`
//! (`symbol.py:634`): hover (docstring/signature) is fetched for many symbols
//! with a wall-clock budget (default 10s). Requests are grouped by file so one
//! file's symbols share a pass, and once the budget is exhausted the remaining
//! symbols come back as `timed_out` with no info — a *partial* result rather
//! than a hang. On a large C++/clangd project a full batch can genuinely take
//! longer than the budget, and Serena returns partials there too.
//!
//! The fetch itself is a caller-supplied async closure, so the grouping and
//! budget logic is hermetic-testable without a language server.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use lsp_types::Position;
use tokio::time::Instant;

/// Default wall-clock budget for a batch of symbol-info (hover) requests.
pub const DEFAULT_SYMBOL_INFO_BUDGET: Duration = Duration::from_secs(10);

/// One symbol to fetch info for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoTarget {
    /// The symbol's name path, echoed back in the result.
    pub symbol: String,
    /// The file the symbol lives in.
    pub file: PathBuf,
    /// The position to hover at (the symbol's name).
    pub position: Position,
}

/// The outcome for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoResult {
    /// The symbol's name path.
    pub symbol: String,
    /// The file the symbol lives in.
    pub file: PathBuf,
    /// The hover text, when one was fetched in time.
    pub info: Option<String>,
    /// Whether the budget ran out before this target was attempted.
    pub timed_out: bool,
}

/// Group targets by file, preserving first-seen file order and target order
/// within each file (Serena groups by file to reuse one client pass).
#[must_use]
pub fn group_by_file(targets: &[InfoTarget]) -> Vec<(PathBuf, Vec<&InfoTarget>)> {
    let mut order: Vec<PathBuf> = Vec::new();
    let mut groups: BTreeMap<PathBuf, Vec<&InfoTarget>> = BTreeMap::new();
    for target in targets {
        match groups.entry(target.file.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                order.push(target.file.clone());
                entry.insert(vec![target]);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().push(target);
            }
        }
    }
    order
        .into_iter()
        .filter_map(|file| groups.remove(&file).map(|items| (file, items)))
        .collect()
}

/// Fetch hover info for every target under a wall-clock `budget`.
///
/// `fetch` is called once per target, in file-grouped order. When the elapsed
/// time reaches `budget`, the remaining targets are returned as
/// [`InfoResult::timed_out`] without calling `fetch` — the batch degrades to a
/// partial result instead of stalling.
pub async fn batch_symbol_info<F, Fut>(
    targets: &[InfoTarget],
    budget: Duration,
    fetch: F,
) -> Vec<InfoResult>
where
    F: Fn(&InfoTarget) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<String>>>,
{
    let deadline = Instant::now() + budget;
    let mut out = Vec::with_capacity(targets.len());
    for (_, group) in group_by_file(targets) {
        for target in group {
            if Instant::now() >= deadline {
                out.push(InfoResult {
                    symbol: target.symbol.clone(),
                    file: target.file.clone(),
                    info: None,
                    timed_out: true,
                });
                continue;
            }
            let info = fetch(target).await.unwrap_or(None);
            out.push(InfoResult {
                symbol: target.symbol.clone(),
                file: target.file.clone(),
                info,
                timed_out: false,
            });
        }
    }
    out
}

/// Render a batch result: one line per symbol with its (possibly absent) info.
#[must_use]
pub fn render_info_results(results: &[InfoResult]) -> String {
    let mut text = format!("{} symbol(s):\n", results.len());
    for result in results {
        let body = match (&result.info, result.timed_out) {
            (_, true) => "(timed out)".to_string(),
            (Some(info), false) => info.replace('\n', " "),
            (None, false) => "(no info)".to_string(),
        };
        let _ = writeln!(text, "  - {}: {body}", result.symbol);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn target(symbol: &str, file: &str) -> InfoTarget {
        InfoTarget {
            symbol: symbol.to_string(),
            file: PathBuf::from(file),
            position: Position::new(0, 0),
        }
    }

    #[test]
    fn groups_targets_by_file_in_first_seen_order() {
        let targets = vec![
            target("a", "b.rs"),
            target("b", "a.rs"),
            target("c", "b.rs"),
        ];
        let groups = group_by_file(&targets);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, PathBuf::from("b.rs"));
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, PathBuf::from("a.rs"));
        assert_eq!(groups[1].1.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn batch_returns_info_when_within_budget() {
        let targets = vec![target("a", "a.rs"), target("b", "b.rs")];
        let results = batch_symbol_info(&targets, Duration::from_secs(10), |t| {
            let symbol = t.symbol.clone();
            async move { Ok(Some(format!("doc for {symbol}"))) }
        })
        .await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| !r.timed_out));
        assert_eq!(results[0].info.as_deref(), Some("doc for a"));
    }

    #[tokio::test(start_paused = true)]
    async fn batch_degrades_to_partial_when_budget_runs_out() {
        let targets = vec![
            target("a", "a.rs"),
            target("b", "b.rs"),
            target("c", "c.rs"),
        ];
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = Arc::clone(&calls);
        let results = batch_symbol_info(&targets, Duration::from_secs(1), move |t| {
            let symbol = t.symbol.clone();
            let calls = Arc::clone(&calls_in);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                // Burn past the 1s budget on the first call.
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok(Some(format!("doc for {symbol}")))
            }
        })
        .await;
        assert_eq!(results.len(), 3);
        assert!(!results[0].timed_out);
        assert!(results[1].timed_out);
        assert!(results[2].timed_out);
        // The timed-out targets never invoked the fetcher.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_fetch_yields_no_info_but_is_not_a_timeout() {
        let targets = vec![target("a", "a.rs")];
        let results = batch_symbol_info(&targets, Duration::from_secs(10), |_| async {
            anyhow::bail!("server exploded")
        })
        .await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].timed_out);
        assert!(results[0].info.is_none());
    }

    #[test]
    fn renders_missing_and_timed_out_entries() {
        let results = vec![
            InfoResult {
                symbol: "a".into(),
                file: "a.rs".into(),
                info: Some("hi".into()),
                timed_out: false,
            },
            InfoResult {
                symbol: "b".into(),
                file: "b.rs".into(),
                info: None,
                timed_out: false,
            },
            InfoResult {
                symbol: "c".into(),
                file: "c.rs".into(),
                info: None,
                timed_out: true,
            },
        ];
        let text = render_info_results(&results);
        assert!(text.contains("- a: hi"));
        assert!(text.contains("- b: (no info)"));
        assert!(text.contains("- c: (timed out)"));
    }
}
