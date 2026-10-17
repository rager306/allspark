//! Progressive shortening of large LSP results (L17).
//!
//! FORK-ONLY (allspark). Ports Serena's renderer cascade (`lsp_api.py:118-318`):
//! when a result overflows a line budget, the renderer *degrades* it in stages
//! rather than truncating blindly. The stages, richest first:
//!
//! - symbols overview: `full tree` → `depth-0 (top level only)` → `counts by kind`
//! - references: `locations` → `counts per file` → `"Found N references"`
//!
//! Every function here is pure (no LSP, no I/O), so the cascade is unit-testable
//! without a server. The caller picks a budget; the renderer walks the stages
//! until the output fits.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::symbol_layer::{kind_name, SymbolNode};

/// A rendered block plus which shortening stage produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The rendered text.
    pub text: String,
    /// The stage that fit the budget (`0` = richest).
    pub stage: usize,
    /// A short label for the stage, for output/debugging.
    pub label: &'static str,
}

/// One reference location, flattened for rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceLine {
    /// The file the reference is in.
    pub file: String,
    /// The 1-based line.
    pub line: u32,
    /// The 1-based column.
    pub column: u32,
}

/// Render a symbols-overview tree with progressive shortening.
///
/// Stage 0 renders the tree to `max_depth`; if that exceeds `budget` lines,
/// stage 1 drops to depth 0 (top-level symbols only); if that still overflows,
/// stage 2 emits counts by kind.
#[must_use]
pub fn render_overview(roots: &[SymbolNode], max_depth: u32, budget: usize) -> Rendered {
    let full = render_tree(roots, max_depth);
    if line_count(&full) <= budget {
        return Rendered {
            text: full,
            stage: 0,
            label: "tree",
        };
    }
    let depth0 = render_tree(roots, 0);
    if line_count(&depth0) <= budget {
        return Rendered {
            text: depth0,
            stage: 1,
            label: "depth-0",
        };
    }
    Rendered {
        text: render_kind_counts(roots),
        stage: 2,
        label: "counts-by-kind",
    }
}

/// Render references with progressive shortening.
///
/// Stage 0 lists every location; if that overflows `budget`, stage 1 emits a
/// per-file count; if there are still too many files, stage 2 emits the total.
#[must_use]
pub fn render_references(references: &[ReferenceLine], budget: usize) -> Rendered {
    let full = render_reference_locations(references);
    if line_count(&full) <= budget {
        return Rendered {
            text: full,
            stage: 0,
            label: "locations",
        };
    }
    let per_file = render_reference_counts(references);
    if line_count(&per_file) <= budget {
        return Rendered {
            text: per_file,
            stage: 1,
            label: "counts-per-file",
        };
    }
    Rendered {
        text: format!("Found {} reference(s).\n", references.len()),
        stage: 2,
        label: "total",
    }
}

/// The full tree render up to `max_depth` (children beyond it are omitted).
#[must_use]
pub fn render_tree(roots: &[SymbolNode], max_depth: u32) -> String {
    let mut out = String::new();
    for root in roots {
        render_node(root, 0, max_depth, &mut out);
    }
    out
}

fn render_node(node: &SymbolNode, indent: usize, max_depth: u32, out: &mut String) {
    let _ = writeln!(
        out,
        "{:indent$}- {} ({})",
        "",
        node.name_path(),
        kind_name(node.kind),
        indent = indent * 2
    );
    if u32::try_from(indent).unwrap_or(u32::MAX) >= max_depth {
        return;
    }
    for child in &node.children {
        render_node(child, indent + 1, max_depth, out);
    }
}

/// Counts by kind across the whole tree, deepest included.
#[must_use]
pub fn render_kind_counts(roots: &[SymbolNode]) -> String {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for root in roots {
        count_kinds(root, &mut counts);
    }
    let total: usize = counts.values().sum();
    let mut out = format!("{total} symbol(s) by kind:\n");
    for (kind, count) in counts {
        let _ = writeln!(out, "  {kind}: {count}");
    }
    out
}

fn count_kinds(node: &SymbolNode, counts: &mut BTreeMap<&'static str, usize>) {
    *counts.entry(kind_name(node.kind)).or_insert(0) += 1;
    for child in &node.children {
        count_kinds(child, counts);
    }
}

/// Every reference as `file:line:column`.
#[must_use]
pub fn render_reference_locations(references: &[ReferenceLine]) -> String {
    let mut out = format!("{} reference(s):\n", references.len());
    for reference in references {
        let _ = writeln!(
            out,
            "  - {}:{}:{}",
            reference.file, reference.line, reference.column
        );
    }
    out
}

/// References rolled up to a per-file count.
#[must_use]
pub fn render_reference_counts(references: &[ReferenceLine]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for reference in references {
        *counts.entry(reference.file.as_str()).or_insert(0) += 1;
    }
    let mut out = format!(
        "{} reference(s) across {} file(s):\n",
        references.len(),
        counts.len()
    );
    for (file, count) in counts {
        let _ = writeln!(out, "  - {file}: {count}");
    }
    out
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range, SymbolKind};

    fn node(path: &[&str], kind: SymbolKind, children: Vec<SymbolNode>) -> SymbolNode {
        let name = path.last().copied().unwrap_or_default().to_string();
        SymbolNode {
            name,
            kind,
            path: path.iter().map(|s| (*s).to_string()).collect(),
            overload_idx: None,
            selection_range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            range: Range::new(Position::new(0, 0), Position::new(1, 0)),
            depth: path.len() - 1,
            children,
        }
    }

    fn tree() -> Vec<SymbolNode> {
        vec![
            node(
                &["Foo"],
                SymbolKind::CLASS,
                vec![
                    node(&["Foo", "a"], SymbolKind::METHOD, vec![]),
                    node(&["Foo", "b"], SymbolKind::METHOD, vec![]),
                    node(&["Foo", "c"], SymbolKind::METHOD, vec![]),
                ],
            ),
            node(&["top"], SymbolKind::FUNCTION, vec![]),
        ]
    }

    #[test]
    fn overview_keeps_the_tree_when_it_fits() {
        let rendered = render_overview(&tree(), 5, 100);
        assert_eq!(rendered.stage, 0);
        assert!(rendered.text.contains("Foo/a"));
    }

    #[test]
    fn overview_falls_back_to_depth_zero() {
        // Budget of 3 lines: the 6-line tree overflows, depth-0 (3 lines) fits.
        let rendered = render_overview(&tree(), 5, 3);
        assert_eq!(rendered.stage, 1);
        assert!(rendered.text.contains("Foo"));
        assert!(!rendered.text.contains("Foo/a"));
    }

    #[test]
    fn overview_falls_back_to_kind_counts() {
        let rendered = render_overview(&tree(), 5, 1);
        assert_eq!(rendered.stage, 2);
        assert!(rendered.text.contains("5 symbol(s) by kind"));
        assert!(rendered.text.contains("method: 3"));
    }

    #[test]
    fn references_keep_locations_when_they_fit() {
        let refs = vec![
            ReferenceLine {
                file: "a.rs".into(),
                line: 1,
                column: 1,
            },
            ReferenceLine {
                file: "b.rs".into(),
                line: 2,
                column: 2,
            },
        ];
        let rendered = render_references(&refs, 100);
        assert_eq!(rendered.stage, 0);
        assert!(rendered.text.contains("a.rs:1:1"));
    }

    #[test]
    fn references_fall_back_to_per_file_counts() {
        let refs = vec![
            ReferenceLine {
                file: "a.rs".into(),
                line: 1,
                column: 1,
            },
            ReferenceLine {
                file: "a.rs".into(),
                line: 2,
                column: 2,
            },
            ReferenceLine {
                file: "b.rs".into(),
                line: 3,
                column: 3,
            },
        ];
        let rendered = render_references(&refs, 3);
        assert_eq!(rendered.stage, 1);
        assert!(rendered.text.contains("a.rs: 2"));
        assert!(rendered.text.contains("b.rs: 1"));
    }

    #[test]
    fn references_fall_back_to_a_total() {
        let refs = vec![
            ReferenceLine {
                file: "a.rs".into(),
                line: 1,
                column: 1,
            },
            ReferenceLine {
                file: "b.rs".into(),
                line: 2,
                column: 2,
            },
        ];
        let rendered = render_references(&refs, 1);
        assert_eq!(rendered.stage, 2);
        assert_eq!(rendered.text, "Found 2 reference(s).\n");
    }
}
