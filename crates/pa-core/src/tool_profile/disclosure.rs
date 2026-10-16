//! S6: progressive disclosure of a REPL/API surface (facade -> method ->
//! type), with session-scoped de-duplication.
//!
//! Serena's `s.info()` expands in three levels: no argument lists the
//! facades, `s.info("lsp")` describes a facade, `s.info("lsp.find_symbol")`
//! a method, `s.info("LanguageServerSymbol")` a type. Types already described
//! earlier in the session are replaced by a short back-reference, so the same
//! documentation is never paid for twice.

use std::collections::{BTreeMap, BTreeSet};

/// How much of an item to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DisclosureLevel {
    /// One line per item (name + first summary line).
    Overview,
    /// Full signature + docstring.
    Full,
    /// Full body plus transitively referenced types.
    Deep,
}

/// A documented API item: a facade, a method, or a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureItem {
    /// Dotted path (`lsp`, `lsp.find_symbol`, `LanguageServerSymbol`).
    pub path: String,
    /// The kind of item.
    pub kind: DisclosureKind,
    /// One-line summary (used at [`DisclosureLevel::Overview`]).
    pub summary: String,
    /// Full documentation (used at [`DisclosureLevel::Full`] and deeper).
    pub detail: String,
    /// Paths this item transitively references (BFS at `Deep`).
    pub referenced: Vec<String>,
    /// Niche methods are rendered as one line plus a `s.info(...)` pointer.
    pub niche: bool,
}

/// The kind of a disclosure item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisclosureKind {
    /// A facade (a namespace of methods, e.g. `lsp`).
    Facade,
    /// A facade method (e.g. `lsp.find_symbol`).
    Method,
    /// A referenced type (e.g. `LanguageServerSymbol`).
    Type,
}

/// S6: a session-scoped progressive-disclosure index.
#[derive(Debug, Clone, Default)]
pub struct Disclosure {
    items: BTreeMap<String, DisclosureItem>,
    described: BTreeSet<String>,
}

impl Disclosure {
    /// An empty disclosure index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a documented item.
    pub fn register(&mut self, item: DisclosureItem) {
        self.items.insert(item.path.clone(), item);
    }

    /// The facades in registration order.
    #[must_use]
    pub fn facades(&self) -> Vec<&str> {
        self.items
            .values()
            .filter(|i| i.kind == DisclosureKind::Facade)
            .map(|i| i.path.as_str())
            .collect()
    }

    /// The methods of a facade, in registration order.
    #[must_use]
    pub fn methods_of(&self, facade: &str) -> Vec<&str> {
        let prefix = format!("{facade}.");
        self.items
            .values()
            .filter(|i| i.kind == DisclosureKind::Method && i.path.starts_with(&prefix))
            .map(|i| i.path.as_str())
            .collect()
    }

    /// The names described so far this session.
    #[must_use]
    pub fn described_names(&self) -> Vec<&str> {
        self.described.iter().map(String::as_str).collect()
    }

    /// Expand a path (or the facade overview when `path` is empty) at a level.
    ///
    /// Re-describing an item already described this session yields the short
    /// back-reference instead of the full text (Serena
    /// `session.described_type_names`).
    #[must_use]
    pub fn expand(&mut self, path: &str, level: DisclosureLevel) -> String {
        if path.is_empty() {
            return self.overview();
        }
        let Some(item) = self.items.get(path).cloned() else {
            return format!("Unknown API item: {path}");
        };
        if !self.described.insert(item.path.clone()) {
            return format!(
                "{} ({}) — documented earlier in this session.",
                item.path, item.summary
            );
        }
        let mut out = match level {
            DisclosureLevel::Overview => render_overview_line(&item),
            DisclosureLevel::Full | DisclosureLevel::Deep => render_full(&item),
        };
        if level == DisclosureLevel::Deep {
            for reference in &item.referenced {
                out.push_str("\n\n");
                out.push_str(&self.expand(reference, DisclosureLevel::Full));
            }
        }
        out
    }

    fn overview(&self) -> String {
        let mut out = String::from("Available facades:\n");
        for facade in self
            .items
            .values()
            .filter(|i| i.kind == DisclosureKind::Facade)
        {
            out.push_str(&render_overview_line(facade));
            out.push('\n');
            for method in self.methods_of(&facade.path) {
                out.push_str("  ");
                out.push_str(method);
                out.push('\n');
            }
        }
        out
    }
}

fn render_overview_line(item: &DisclosureItem) -> String {
    if item.niche {
        return format!(
            "{} — {} (call s.info(\"{}\"))",
            item.path, item.summary, item.path
        );
    }
    format!("{} — {}", item.path, item.summary)
}

fn render_full(item: &DisclosureItem) -> String {
    let mut out = format!("{}\n{}", item.path, item.detail);
    for reference in &item.referenced {
        out.push_str("\n  references: ");
        out.push_str(reference);
    }
    out
}
