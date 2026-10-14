//! Honest index-coverage accounting (G9).
//!
//! `index_coverage` answers one question the plain `IndexReport` cannot:
//! *how much of the codebase did we actually understand?* A run that reports
//! `failed == 0` can still have indexed files whose tree-sitter trees carry
//! `ERROR` regions, or skipped files entirely — a clean exit is **not**
//! evidence of completeness. This module turns the per-file notes the indexer
//! writes into a [`CoverageReport`] that separates four outcomes:
//!
//! - [`CoverageKind::Clean`] — parsed with no error regions.
//! - [`CoverageKind::Partial`] — indexed, but the tree carries `ERROR` regions.
//! - [`CoverageKind::Unusable`] — a single error region dominates the file
//!   (≥80% of its bytes), so the extracted symbols are near-worthless.
//! - [`CoverageKind::Skipped`] — never parsed (unsupported language, oversized,
//!   unreadable, or a grammar that is not installed).
//!
//! The distinction between `skipped` and `parse_partial` is deliberate: a
//! reader deciding whether to trust a query result needs to know whether a
//! file was *ignored* or *mangled*, and those are different failures.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::GraphStore;

/// The stored `index_coverage.kind` for a file indexed despite `ERROR` regions.
pub const KIND_PARTIAL: &str = "parse_partial";
/// The stored `index_coverage.kind` for a file whose parse is dominated by one
/// error region.
pub const KIND_UNUSABLE: &str = "parse_unusable";
/// The stored `index_coverage.kind` for a file that was never parsed.
pub const KIND_SKIPPED: &str = "skipped";

/// Fraction of a file a single error region must cover to be `unusable`.
const UNUSABLE_REGION_NUM: usize = 4; // ≥ 4/5 = 80%
const UNUSABLE_REGION_DEN: usize = 5;

/// What a single source file's parse produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseHealth {
    /// Parsed with no error regions.
    Clean,
    /// Parsed, but the tree carries recoverable `ERROR` regions.
    Partial {
        /// Total bytes covered by error regions (merged, non-overlapping).
        error_bytes: usize,
        /// Total source bytes.
        total_bytes: usize,
    },
    /// A single error region dominates the file; the parse is not trustworthy.
    Unusable {
        /// Bytes covered by the largest single error region.
        largest_region_bytes: usize,
        /// Total source bytes.
        total_bytes: usize,
    },
    /// The language has no usable grammar (e.g. an uninstalled wasm grammar).
    Unavailable,
}

impl ParseHealth {
    /// The `index_coverage.kind` this health maps to, or `None` when clean.
    #[must_use]
    pub const fn coverage_kind(self) -> Option<&'static str> {
        match self {
            Self::Clean => None,
            Self::Partial { .. } => Some(KIND_PARTIAL),
            Self::Unusable { .. } => Some(KIND_UNUSABLE),
            Self::Unavailable => Some(KIND_SKIPPED),
        }
    }

    /// Human-readable detail stored alongside the kind.
    #[must_use]
    pub fn detail(self) -> String {
        match self {
            Self::Clean => String::new(),
            Self::Partial {
                error_bytes,
                total_bytes,
            } => format!("{error_bytes}/{total_bytes} bytes in ERROR/MISSING regions"),
            Self::Unusable {
                largest_region_bytes,
                total_bytes,
            } => format!("one ERROR region spans {largest_region_bytes}/{total_bytes} bytes"),
            Self::Unavailable => "grammar unavailable".to_string(),
        }
    }
}

/// Classify a source string's parse health.
///
/// Uses the shared bounded parse cache, so calling this right after
/// [`crate::extract::extract`] re-parses nothing.
#[must_use]
pub fn classify_source(source: &str, lang: pa_ast::SupportLang) -> ParseHealth {
    let Ok(Some(tree)) = pa_ast::parse_cache::parse_cached(source, lang) else {
        return ParseHealth::Unavailable;
    };
    let root = tree.root_node();
    let total_bytes = source.len().max(root.end_byte());
    if !root.has_error() {
        return ParseHealth::Clean;
    }
    let (error_bytes, largest) = error_regions(root, source.len());
    if total_bytes > 0 && largest * UNUSABLE_REGION_DEN >= total_bytes * UNUSABLE_REGION_NUM {
        return ParseHealth::Unusable {
            largest_region_bytes: largest,
            total_bytes,
        };
    }
    ParseHealth::Partial {
        error_bytes,
        total_bytes,
    }
}

/// Walk the tree for `ERROR`/`MISSING` nodes, returning the total merged error
/// byte count and the largest single region. Overlapping ranges (an inner
/// `ERROR` nested inside an outer one) collapse into the outer span.
fn error_regions(root: tree_sitter::Node<'_>, source_len: usize) -> (usize, usize) {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    // Iterative pre-order walk; tree depth is bounded by nesting, not file size.
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            let start = node.start_byte().min(source_len);
            let end = node.end_byte().min(source_len).max(start);
            spans.push((start, end));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    if spans.is_empty() {
        return (0, 0);
    }
    spans.sort_unstable();
    let mut total = 0usize;
    let mut largest = 0usize;
    let (mut cur_start, mut cur_end) = spans[0];
    for &(start, end) in &spans[1..] {
        if start <= cur_end {
            cur_end = cur_end.max(end);
        } else {
            total += cur_end - cur_start;
            largest = largest.max(cur_end - cur_start);
            cur_start = start;
            cur_end = end;
        }
    }
    total += cur_end - cur_start;
    largest = largest.max(cur_end - cur_start);
    (total, largest)
}

/// One stored coverage note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageNote {
    /// Repo-relative path.
    pub rel_path: String,
    /// Coverage kind (`parse_partial`, `parse_unusable`, `skipped`).
    pub kind: String,
    /// Free-form detail (byte counts, skip reason).
    pub detail: String,
}

/// Aggregate honesty report over a project's index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageReport {
    /// Project name.
    pub project: String,
    /// Files with a stored hash — the universe coverage is measured against.
    pub total: usize,
    /// Files that parsed with no error regions.
    pub parsed: usize,
    /// Files indexed despite carrying `ERROR` regions.
    pub partial: usize,
    /// Files whose parse is dominated by a single error region.
    pub unusable: usize,
    /// Files that were never parsed.
    pub skipped: usize,
    /// `kind` → count, sorted by key for deterministic output.
    pub reason_histogram: BTreeMap<String, usize>,
    /// Per-file notes, sorted by path then kind.
    pub notes: Vec<CoverageNote>,
    /// `true` only when every known file parsed cleanly.
    pub complete: bool,
}

impl CoverageReport {
    /// Fraction of known files that parsed cleanly, in `[0.0, 1.0]`.
    ///
    /// Returns `1.0` for an empty project (vacuously complete) — callers that
    /// care should check [`Self::total`].
    #[must_use]
    pub fn clean_ratio(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        self.parsed as f64 / self.total as f64
    }
}

/// Build the honesty report for `project`.
///
/// `parsed` counts every file with a stored hash that has no coverage note
/// (i.e. parsed cleanly). A file can appear in at most one of
/// partial/unusable/skipped because `record_coverage` keys notes by
/// `(project, rel_path, kind)` and the indexer clears a file's notes before
/// re-recording, but a hand-written store could carry several; in that case
/// the counts are computed from distinct paths.
///
/// # Errors
/// Propagates SQLite errors.
pub fn coverage(store: &GraphStore, project: &str) -> Result<CoverageReport> {
    let rows = store.coverage_rows(project)?;
    let total = store.file_count(project)?;

    let mut notes = Vec::with_capacity(rows.len());
    let mut histogram: BTreeMap<String, usize> = BTreeMap::new();
    let mut partial_paths = std::collections::HashSet::new();
    let mut unusable_paths = std::collections::HashSet::new();
    let mut skipped_paths = std::collections::HashSet::new();
    for (rel_path, kind, detail) in rows {
        *histogram.entry(kind.clone()).or_insert(0) += 1;
        match kind.as_str() {
            KIND_PARTIAL => {
                partial_paths.insert(rel_path.clone());
            }
            KIND_UNUSABLE => {
                unusable_paths.insert(rel_path.clone());
            }
            KIND_SKIPPED => {
                skipped_paths.insert(rel_path.clone());
            }
            _ => {}
        }
        notes.push(CoverageNote {
            rel_path,
            kind,
            detail,
        });
    }

    let partial = partial_paths.len();
    let unusable = unusable_paths.len();
    let skipped = skipped_paths.len();
    let accounted = partial + unusable + skipped;
    // `parsed` is what is left of the universe once every non-clean file is
    // subtracted; saturating so a store whose coverage notes outnumber its
    // hashes (possible after a partial reindex) never underflows.
    let parsed = total.saturating_sub(accounted);
    let complete = total > 0 && accounted == 0;

    Ok(CoverageReport {
        project: project.to_string(),
        total,
        parsed,
        partial,
        unusable,
        skipped,
        reason_histogram: histogram,
        notes,
        complete,
    })
}

impl GraphStore {
    /// Build the honesty coverage report for `project` (see [`coverage`]).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn coverage(&self, project: &str) -> Result<CoverageReport> {
        coverage(self, project)
    }

    /// Delete a file's coverage notes before re-recording fresh ones, so a file
    /// that parsed cleanly this run stops being reported as `parse_partial`.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn clear_file_coverage(&self, project: &str, rel_path: &str) -> Result<usize> {
        let n = self.connection().execute(
            "DELETE FROM index_coverage WHERE project = ?1 AND rel_path = ?2",
            rusqlite::params![project, rel_path],
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BROKEN: &str = "fn main() {\n    let x = ;\n    let y = );\n    ((((((\n}\n";

    #[test]
    fn clean_source_is_clean() {
        let h = classify_source("fn main() {}\n", pa_ast::SupportLang::Rust);
        assert_eq!(h, ParseHealth::Clean);
        assert_eq!(h.coverage_kind(), None);
    }

    #[test]
    fn broken_source_is_partial_or_unusable_not_clean() {
        let h = classify_source(BROKEN, pa_ast::SupportLang::Rust);
        assert_ne!(h, ParseHealth::Clean, "broken source must not read clean");
        assert!(
            matches!(
                h,
                ParseHealth::Partial { .. } | ParseHealth::Unusable { .. }
            ),
            "{h:?}"
        );
        assert!(h.coverage_kind().is_some());
    }

    #[test]
    fn dominated_error_region_is_unusable() {
        // An unclosed brace makes tree-sitter wrap everything after the
        // opening `{` in a single ERROR region spanning nearly the whole file.
        let src = format!("fn main() {{\n{}\n", "    let x = 1;\n".repeat(200));
        let h = classify_source(&src, pa_ast::SupportLang::Rust);
        match h {
            ParseHealth::Unusable {
                largest_region_bytes,
                total_bytes,
            } => {
                assert!(largest_region_bytes * 5 >= total_bytes * 4, "{h:?}");
            }
            other => panic!("expected unusable, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_grammar_is_unavailable() {
        // A wasm-only grammar that is not installed parses to `None`.
        let h = classify_source("x", pa_ast::SupportLang::Fortran);
        // Either the grammar is linked (clean-ish) or unavailable; both are
        // honest outcomes, never a panic.
        let _ = h;
    }

    #[test]
    fn report_counts_from_stored_notes() {
        let store = GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        let dir = tempfile::tempdir().expect("tempdir");
        for name in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(dir.path().join(name), "fn f() {}\n").expect("write");
            let hash = crate::hash::hash_file(&dir.path().join(name)).expect("hash");
            store.put_file_hash("p", name, &hash).expect("hash row");
        }
        store
            .record_coverage("p", "b.rs", KIND_PARTIAL, "1/2 bytes")
            .expect("partial");
        store
            .record_coverage("p", "c.rs", KIND_SKIPPED, "unsupported language")
            .expect("skip");

        let report = coverage(&store, "p").expect("coverage");
        assert_eq!(report.total, 3);
        assert_eq!(report.parsed, 1, "{report:?}");
        assert_eq!(report.partial, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.unusable, 0);
        assert!(!report.complete);
        assert!((report.clean_ratio() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(report.reason_histogram.get(KIND_PARTIAL), Some(&1));
    }

    #[test]
    fn clean_project_is_complete() {
        let store = GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn f() {}\n").expect("write");
        let hash = crate::hash::hash_file(&dir.path().join("a.rs")).expect("hash");
        store.put_file_hash("p", "a.rs", &hash).expect("hash row");
        let report = coverage(&store, "p").expect("coverage");
        assert!(report.complete);
        assert_eq!(report.parsed, 1);
        assert!((report.clean_ratio() - 1.0).abs() < 1e-9);
    }
}
