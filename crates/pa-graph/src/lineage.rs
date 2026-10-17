//! M5 — record lineage (`CAMA`-style provenance tracking).
//!
//! Every derived record answers "where did I come from?": which edit, index
//! pass, or observation produced it. [`Lineage`] stores child → parents edges
//! and walks them to expose the full set of transitive ancestors, so a
//! consumer can trace a fact back to its roots.

use std::collections::{BTreeMap, BTreeSet};

/// A directed acyclic provenance graph: record id → ids it derives from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lineage {
    parents: BTreeMap<String, Vec<String>>,
}

impl Lineage {
    /// An empty lineage graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `id` derives from `parents` (idempotent, deduped).
    pub fn record(&mut self, id: impl Into<String>, parents: &[&str]) {
        let entry = self.parents.entry(id.into()).or_default();
        for parent in parents {
            let owned = (*parent).to_string();
            if !entry.contains(&owned) {
                entry.push(owned);
            }
        }
        entry.sort();
    }

    /// Whether `id` is tracked.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.parents.contains_key(id)
    }

    /// Number of tracked ids.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parents.len()
    }

    /// Whether nothing is tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }

    /// Direct parents of `id`, sorted.
    #[must_use]
    pub fn parents(&self, id: &str) -> Vec<String> {
        self.parents.get(id).cloned().unwrap_or_default()
    }

    /// Every transitive ancestor of `id`, sorted and deduped.
    ///
    /// Cycle-safe: a repeated node is visited once.
    #[must_use]
    pub fn ancestors(&self, id: &str) -> Vec<String> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut stack: Vec<String> = self.parents.get(id).cloned().unwrap_or_default();
        while let Some(current) = stack.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            if let Some(grandparents) = self.parents.get(&current) {
                stack.extend(grandparents.iter().cloned());
            }
        }
        seen.into_iter().collect()
    }

    /// Ancestors of `id` that themselves have no parents (the roots).
    #[must_use]
    pub fn roots(&self, id: &str) -> Vec<String> {
        self.ancestors(id)
            .into_iter()
            .filter(|ancestor| self.parents.get(ancestor).is_none_or(Vec::is_empty))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_are_transitive() {
        let mut lineage = Lineage::new();
        lineage.record("edit", &["file"]);
        lineage.record("symbol", &["edit"]);
        lineage.record("fact", &["symbol"]);
        let ancestors = lineage.ancestors("fact");
        assert_eq!(ancestors, vec!["edit", "file", "symbol"]);
    }

    #[test]
    fn roots_are_ancestors_without_parents() {
        let mut lineage = Lineage::new();
        lineage.record("edit", &["file"]);
        lineage.record("fact", &["edit"]);
        assert_eq!(lineage.roots("fact"), vec!["file"]);
    }

    #[test]
    fn cycles_do_not_hang() {
        let mut lineage = Lineage::new();
        lineage.record("a", &["b"]);
        lineage.record("b", &["a"]);
        assert_eq!(lineage.ancestors("a"), vec!["a", "b"]);
    }

    #[test]
    fn unknown_id_has_no_ancestors() {
        let lineage = Lineage::new();
        assert!(
            lineage.ancestors("nope").is_empty(),
            "unknown id has no ancestors"
        );
        assert!(!lineage.contains("nope"));
    }

    #[test]
    fn parents_are_deduped() {
        let mut lineage = Lineage::new();
        lineage.record("x", &["a", "a", "b"]);
        assert_eq!(lineage.parents("x"), vec!["a", "b"]);
    }
}
