//! Position-free symbol addressing: a Rust port of Serena's `NamePathMatcher`.
//!
//! FORK-ONLY (allspark). oh-my-pi addresses symbols by line number; Serena by a
//! `Class/method` *name path* that survives edits. This module keeps the name-path
//! form so the model can say `find_symbol("LspManager/ensure_client")` without a
//! line, while still accepting a line when the caller has one.
//!
//! Pattern forms (Serena parity):
//! - `method` — a bare name; matches any symbol with that name.
//! - `class/method` — a *relative* name path; matches as a suffix of a symbol's path.
//! - `/class/method` — an *absolute* name path; requires the exact full path.
//! - `MyClass/my_method[1]` — an overload index selects one of several same-named
//!   symbols.
//!
//! The matcher walks a symbol's path from the leaf outward (reversed), which is
//! what makes `class/method` a suffix match without materializing every path.

use lsp_types::DocumentSymbol;

/// Separator between name-path components (Serena's `NAME_PATH_SEP`).
pub const NAME_PATH_SEP: char = '/';

/// One component of a name path: a name plus an optional overload index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamePathComponent {
    /// The component's name.
    pub name: String,
    /// The overload index when several siblings share the name (`name[idx]`).
    pub overload_idx: Option<usize>,
}

impl NamePathComponent {
    /// Parse `name` or `name[idx]`; a non-numeric bracket stays part of the name.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        if raw.ends_with(']') {
            if let Some(open) = raw.rfind('[') {
                let index_part = &raw[open + 1..raw.len() - 1];
                if !index_part.is_empty() && index_part.bytes().all(|b| b.is_ascii_digit()) {
                    return Self {
                        name: raw[..open].to_string(),
                        overload_idx: index_part.parse().ok(),
                    };
                }
            }
        }
        Self {
            name: raw.to_string(),
            overload_idx: None,
        }
    }

    /// Whether this pattern component matches a symbol component.
    #[must_use]
    pub fn matches(&self, other: &NamePathComponent, substring: bool) -> bool {
        let name_ok = if substring {
            other.name.contains(&self.name)
        } else {
            self.name == other.name
        };
        if !name_ok {
            return false;
        }
        // An unspecified overload index matches any; a specified one must agree.
        self.overload_idx
            .is_none_or(|idx| Some(idx) == other.overload_idx)
    }
}

/// A compiled name-path pattern (Serena's `NamePathMatcher`).
#[derive(Debug, Clone)]
pub struct NamePathMatcher {
    expr: String,
    components: Vec<NamePathComponent>,
    absolute: bool,
    substring_matching: bool,
}

impl NamePathMatcher {
    /// Compile a pattern. `substring_matching` relaxes the *last* (leaf) segment.
    ///
    /// # Errors
    /// Fails when the pattern is empty after trimming separators.
    pub fn new(pattern: &str, substring_matching: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(!pattern.is_empty(), "name_path must not be empty");
        let absolute = pattern.starts_with(NAME_PATH_SEP);
        let trimmed = pattern.trim_matches(NAME_PATH_SEP);
        let components: Vec<NamePathComponent> = trimmed
            .split(NAME_PATH_SEP)
            .map(NamePathComponent::parse)
            .collect();
        anyhow::ensure!(
            !components.is_empty(),
            "name_path must have at least one component"
        );
        Ok(Self {
            expr: pattern.to_string(),
            components,
            absolute,
            substring_matching,
        })
    }

    /// The original expression, for error text.
    #[must_use]
    pub fn expr(&self) -> &str {
        &self.expr
    }

    /// Match against a symbol's path given leaf-first components.
    ///
    /// The reversed iterator yields the symbol itself, then its parent, and so
    /// on; the pattern is compared leaf-first as well, so `class/method` reads
    /// as "leaf is `method`, its parent is `class`".
    #[must_use]
    pub fn matches_reversed<'a, I>(&self, mut components_reversed: I) -> bool
    where
        I: Iterator<Item = &'a NamePathComponent>,
    {
        for (i, pattern_component) in self.components.iter().rev().enumerate() {
            let Some(symbol_component) = components_reversed.next() else {
                return false;
            };
            let use_substring = self.substring_matching && i == 0;
            if !pattern_component.matches(symbol_component, use_substring) {
                return false;
            }
        }
        // An absolute pattern must consume the whole path; a relative one is a suffix.
        if self.absolute {
            components_reversed.next().is_none()
        } else {
            true
        }
    }
}

/// A symbol flattened from the `documentSymbol` tree, carrying its own path.
#[derive(Debug, Clone)]
pub struct FlatSymbol {
    /// The symbol's own name (the leaf of its path).
    pub name: String,
    /// The protocol symbol kind (function, struct, ...).
    pub kind: lsp_types::SymbolKind,
    /// The full path from the file root, e.g. `["MyClass", "my_method"]`.
    pub path: Vec<String>,
    /// The overload index when several siblings share `name` (0-based).
    pub overload_idx: Option<usize>,
    /// The name's own range (what a rename/position request should point at).
    pub selection_range: lsp_types::Range,
    /// The full body range (for `replace_symbol_body`-style edits later).
    pub range: lsp_types::Range,
}

impl FlatSymbol {
    /// The leaf-first components of this symbol's path, for [`NamePathMatcher`].
    #[must_use]
    pub fn reversed_components(&self) -> Vec<NamePathComponent> {
        let mut out: Vec<NamePathComponent> = Vec::with_capacity(self.path.len());
        // Walk parents from the leaf outward; only the leaf carries the index.
        for (i, name) in self.path.iter().enumerate().rev() {
            let is_leaf = i + 1 == self.path.len();
            out.push(NamePathComponent {
                name: name.clone(),
                overload_idx: if is_leaf { self.overload_idx } else { None },
            });
        }
        out
    }

    /// The `Class/method` name path.
    #[must_use]
    pub fn name_path(&self) -> String {
        let mut path = self.path.join(&NAME_PATH_SEP.to_string());
        if let Some(idx) = self.overload_idx {
            use std::fmt::Write as _;
            let _ = write!(path, "[{idx}]");
        }
        path
    }
}

/// Flatten a `DocumentSymbol` tree into leaf-carrying paths, assigning overload
/// indices to same-named siblings.
#[must_use]
pub fn flatten_symbols(symbols: &[DocumentSymbol]) -> Vec<FlatSymbol> {
    let mut out = Vec::new();
    flatten_into(symbols, &mut Vec::new(), &mut out);
    out
}

fn flatten_into(symbols: &[DocumentSymbol], prefix: &mut Vec<String>, out: &mut Vec<FlatSymbol>) {
    // An overload index is only meaningful when siblings share a name: a unique
    // sibling carries no index, so its name path stays `Class/method`.
    let mut totals: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for symbol in symbols {
        *totals.entry(symbol.name.as_str()).or_insert(0) += 1;
    }
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for symbol in symbols {
        let idx = seen.entry(symbol.name.as_str()).or_insert(0);
        let overload_idx = (totals[symbol.name.as_str()] > 1).then_some(*idx);
        *idx += 1;

        prefix.push(symbol.name.clone());
        out.push(FlatSymbol {
            name: symbol.name.clone(),
            kind: symbol.kind,
            path: prefix.clone(),
            overload_idx,
            selection_range: symbol.selection_range,
            range: symbol.range,
        });
        if let Some(children) = &symbol.children {
            flatten_into(children, prefix, out);
        }
        prefix.pop();
    }
}

/// Resolve a name-path pattern to the matching flattened symbols, in document
/// order. An empty pattern yields every symbol.
#[must_use]
pub fn resolve(flat: &[FlatSymbol], pattern: &str, substring_matching: bool) -> Vec<FlatSymbol> {
    let Ok(matcher) = NamePathMatcher::new(pattern, substring_matching) else {
        return Vec::new();
    };
    flat.iter()
        .filter(|symbol| matcher.matches_reversed(symbol.reversed_components().iter()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range, SymbolKind};

    fn sym(name: &str, children: Vec<DocumentSymbol>) -> DocumentSymbol {
        #[allow(deprecated)]
        DocumentSymbol {
            name: name.into(),
            detail: None,
            kind: SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            range: Range::new(Position::new(0, 0), Position::new(1, 0)),
            selection_range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            children: if children.is_empty() {
                None
            } else {
                Some(children)
            },
        }
    }

    fn tree() -> Vec<DocumentSymbol> {
        vec![
            sym(
                "MyClass",
                vec![sym("my_method", vec![]), sym("other", vec![])],
            ),
            sym("free_fn", vec![]),
        ]
    }

    #[test]
    fn bare_name_matches_by_suffix() {
        let flat = flatten_symbols(&tree());
        let hits = resolve(&flat, "my_method", false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name_path(), "MyClass/my_method");
    }

    #[test]
    fn relative_path_is_a_suffix_match() {
        let flat = flatten_symbols(&tree());
        assert_eq!(resolve(&flat, "MyClass/my_method", false).len(), 1);
        // A bare class name also matches its own node.
        assert_eq!(resolve(&flat, "MyClass", false).len(), 1);
    }

    #[test]
    fn absolute_path_requires_the_whole_path() {
        let flat = flatten_symbols(&tree());
        assert_eq!(resolve(&flat, "/MyClass/my_method", false).len(), 1);
        // `/my_method` is absolute but its full path is MyClass/my_method.
        assert_eq!(resolve(&flat, "/my_method", false).len(), 0);
    }

    #[test]
    fn substring_matching_relaxes_the_leaf() {
        let flat = flatten_symbols(&tree());
        assert_eq!(resolve(&flat, "method", true).len(), 1);
        assert_eq!(resolve(&flat, "method", false).len(), 0);
    }

    #[test]
    fn overload_index_disambiguates_siblings() {
        let flat = flatten_symbols(&[sym("f", vec![sym("dup", vec![]), sym("dup", vec![])])]);
        assert_eq!(resolve(&flat, "f/dup[0]", false).len(), 1);
        assert_eq!(resolve(&flat, "f/dup[1]", false).len(), 1);
        assert_eq!(resolve(&flat, "f/dup[0]", false)[0].overload_idx, Some(0));
        // No index matches both.
        assert_eq!(resolve(&flat, "f/dup", false).len(), 2);
    }
}
