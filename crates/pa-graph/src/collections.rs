//! R3 — multi-collections with retargetable aliases.
//!
//! A *collection* is a named set of document ids. An *alias* is an alternate
//! name that resolves to a collection; retargeting an alias atomically
//! switches every lookup through that alias to a new collection. This mirrors
//! the "alias → index" pattern of search backends (e.g. Elasticsearch), where
//! a zero-downtime reindex is a single `retarget`.
//!
//! Dropping a collection that is still the target of a live alias is an
//! **error** ([`CollectionsError::Aliased`]) rather than a cascade: silently
//! removing an alias would break callers that still route through it, so the
//! caller must retarget or drop the alias first. This is the documented
//! policy; [`Collections::drop_cascade`] offers the explicit cascade variant.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

/// Error type for [`Collections`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CollectionsError {
    /// The named collection already exists.
    #[error("collection already exists: {0}")]
    Exists(String),
    /// The named collection does not exist.
    #[error("collection not found: {0}")]
    NotFound(String),
    /// The named alias does not exist.
    #[error("alias not found: {0}")]
    AliasNotFound(String),
    /// The alias name collides with an existing collection.
    #[error("alias name collides with a collection: {0}")]
    AliasNameTaken(String),
    /// The alias name is already used by another alias.
    #[error("alias already exists: {0}")]
    AliasExists(String),
    /// The target collection is still referenced by a live alias.
    #[error("collection {0} is still referenced by alias(es): {1}")]
    Aliased(String, String),
}

/// Per-collection statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CollectionStats {
    /// Number of document ids in the collection.
    pub count: usize,
    /// Logical timestamp of the last mutation (insert / retarget / create).
    pub last_updated: u64,
}

#[derive(Debug, Clone, Default)]
struct Collection {
    ids: BTreeSet<u64>,
    last_updated: u64,
}

/// A registry of named collections plus retargetable aliases.
#[derive(Debug, Clone, Default)]
pub struct Collections {
    sets: BTreeMap<String, Collection>,
    aliases: BTreeMap<String, String>,
    clock: u64,
}

impl Collections {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Create a new, empty collection.
    ///
    /// # Errors
    /// Returns [`CollectionsError::Exists`] if the name is taken, or
    /// [`CollectionsError::AliasNameTaken`] if an alias already uses the name.
    pub fn create(&mut self, name: &str) -> Result<(), CollectionsError> {
        if self.sets.contains_key(name) {
            return Err(CollectionsError::Exists(name.to_string()));
        }
        if self.aliases.contains_key(name) {
            return Err(CollectionsError::AliasNameTaken(name.to_string()));
        }
        let now = self.tick();
        self.sets.insert(
            name.to_string(),
            Collection {
                ids: BTreeSet::new(),
                last_updated: now,
            },
        );
        Ok(())
    }

    /// Whether a collection exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.sets.contains_key(name)
    }

    /// Create an alias pointing at `target`.
    ///
    /// # Errors
    /// Fails if `target` is missing, if the alias name is taken by another
    /// alias or a collection.
    pub fn alias(&mut self, alias: &str, target: &str) -> Result<(), CollectionsError> {
        if !self.sets.contains_key(target) {
            return Err(CollectionsError::NotFound(target.to_string()));
        }
        if self.sets.contains_key(alias) {
            return Err(CollectionsError::AliasNameTaken(alias.to_string()));
        }
        if self.aliases.contains_key(alias) {
            return Err(CollectionsError::AliasExists(alias.to_string()));
        }
        self.aliases.insert(alias.to_string(), target.to_string());
        Ok(())
    }

    /// Atomically point `alias` at a different collection. The alias must
    /// already exist. All subsequent lookups through it resolve to `target`.
    ///
    /// # Errors
    /// Fails if the alias is unknown or the new target is missing.
    pub fn retarget(&mut self, alias: &str, target: &str) -> Result<(), CollectionsError> {
        if !self.aliases.contains_key(alias) {
            return Err(CollectionsError::AliasNotFound(alias.to_string()));
        }
        if !self.sets.contains_key(target) {
            return Err(CollectionsError::NotFound(target.to_string()));
        }
        self.aliases.insert(alias.to_string(), target.to_string());
        self.tick();
        Ok(())
    }

    /// Remove an alias.
    ///
    /// # Errors
    /// Fails if the alias is unknown.
    pub fn drop_alias(&mut self, alias: &str) -> Result<(), CollectionsError> {
        self.aliases
            .remove(alias)
            .map(|_| ())
            .ok_or_else(|| CollectionsError::AliasNotFound(alias.to_string()))
    }

    /// Drop a collection. Refuses (documented policy) when a live alias still
    /// points at it.
    ///
    /// # Errors
    /// Fails if the collection is missing or still aliased.
    pub fn drop(&mut self, name: &str) -> Result<(), CollectionsError> {
        if !self.sets.contains_key(name) {
            return Err(CollectionsError::NotFound(name.to_string()));
        }
        let holders: Vec<&String> = self
            .aliases
            .iter()
            .filter(|(_, target)| target.as_str() == name)
            .map(|(alias, _)| alias)
            .collect();
        if !holders.is_empty() {
            let list = holders
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(",");
            return Err(CollectionsError::Aliased(name.to_string(), list));
        }
        self.sets.remove(name);
        self.tick();
        Ok(())
    }

    /// Drop a collection *and* every alias pointing at it (explicit cascade).
    ///
    /// Returns the dropped alias names (sorted).
    ///
    /// # Errors
    /// Fails if the collection is missing.
    pub fn drop_cascade(&mut self, name: &str) -> Result<Vec<String>, CollectionsError> {
        if !self.sets.contains_key(name) {
            return Err(CollectionsError::NotFound(name.to_string()));
        }
        let dropped: Vec<String> = self
            .aliases
            .iter()
            .filter(|(_, target)| target.as_str() == name)
            .map(|(alias, _)| alias.clone())
            .collect();
        for alias in &dropped {
            self.aliases.remove(alias);
        }
        self.sets.remove(name);
        self.tick();
        Ok(dropped)
    }

    /// Insert a document id into a collection (idempotent).
    ///
    /// # Errors
    /// Fails if the collection is missing.
    pub fn insert(&mut self, name: &str, id: u64) -> Result<(), CollectionsError> {
        let now = self.tick();
        let collection = self
            .sets
            .get_mut(name)
            .ok_or_else(|| CollectionsError::NotFound(name.to_string()))?;
        collection.ids.insert(id);
        collection.last_updated = now;
        Ok(())
    }

    /// Resolve an alias to its target collection name.
    #[must_use]
    pub fn resolve_alias(&self, alias: &str) -> Option<&str> {
        self.aliases.get(alias).map(String::as_str)
    }

    /// Resolve a name that may be either a collection or an alias to the
    /// underlying collection name. The returned reference borrows from `self`.
    #[must_use]
    pub fn resolve<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        if self.sets.contains_key(name) {
            Some(name)
        } else {
            self.resolve_alias(name)
        }
    }

    /// List collection names (sorted).
    #[must_use]
    pub fn list(&self) -> Vec<String> {
        self.sets.keys().cloned().collect()
    }

    /// List alias names (sorted).
    #[must_use]
    pub fn list_aliases(&self) -> Vec<String> {
        self.aliases.keys().cloned().collect()
    }

    /// Statistics for a collection (by name, not alias).
    #[must_use]
    pub fn stats(&self, name: &str) -> Option<CollectionStats> {
        self.sets.get(name).map(|c| CollectionStats {
            count: c.ids.len(),
            last_updated: c.last_updated,
        })
    }

    /// The document ids of a collection (sorted).
    #[must_use]
    pub fn ids(&self, name: &str) -> Option<Vec<u64>> {
        self.sets.get(name).map(|c| c.ids.iter().copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retarget_switches_all_lookups() {
        let mut c = Collections::new();
        c.create("blue").unwrap();
        c.create("green").unwrap();
        c.insert("blue", 1).unwrap();
        c.insert("green", 2).unwrap();
        c.alias("live", "blue").unwrap();

        assert_eq!(c.resolve_alias("live"), Some("blue"));
        assert_eq!(c.ids(c.resolve("live").unwrap()), Some(vec![1]));

        // Retarget the alias; every lookup now sees green.
        c.retarget("live", "green").unwrap();
        assert_eq!(c.resolve_alias("live"), Some("green"));
        assert_eq!(c.ids(c.resolve("live").unwrap()), Some(vec![2]));

        // The original collection is untouched.
        assert_eq!(c.ids("blue"), Some(vec![1]));
    }

    #[test]
    fn drop_with_live_alias_errors_then_cascade_works() {
        let mut c = Collections::new();
        c.create("blue").unwrap();
        c.create("green").unwrap();
        c.alias("a", "blue").unwrap();
        c.alias("b", "blue").unwrap();

        // Documented policy: refuse the drop.
        assert_eq!(
            c.drop("blue"),
            Err(CollectionsError::Aliased("blue".into(), "a,b".into()))
        );
        assert!(c.contains("blue"));

        // Retarget one away, drop the other alias, then the drop succeeds.
        c.retarget("a", "green").unwrap();
        c.drop_alias("b").unwrap();
        c.drop("blue").unwrap();
        assert!(!c.contains("blue"));
        assert_eq!(c.resolve_alias("a"), Some("green"));

        // Explicit cascade removes the collection and its aliases.
        c.alias("c", "green").unwrap();
        c.alias("d", "green").unwrap();
        let dropped = c.drop_cascade("green").unwrap();
        assert_eq!(dropped, vec!["a".to_string(), "c".into(), "d".into()]);
        assert!(!c.contains("green"));
        assert!(c.list_aliases().is_empty());
    }

    #[test]
    fn stats_are_exact_and_last_updated_tracks_mutations() {
        let mut c = Collections::new();
        c.create("blue").unwrap();
        let created = c.stats("blue").unwrap();
        assert_eq!(created.count, 0);

        c.insert("blue", 10).unwrap();
        let after_first = c.stats("blue").unwrap();
        assert_eq!(after_first.count, 1);
        assert!(after_first.last_updated > created.last_updated);

        // Duplicate insert is idempotent: count stays, timestamp advances.
        c.insert("blue", 10).unwrap();
        let after_dup = c.stats("blue").unwrap();
        assert_eq!(after_dup.count, 1);
        assert!(after_dup.last_updated > after_first.last_updated);

        c.insert("blue", 11).unwrap();
        assert_eq!(c.stats("blue").unwrap().count, 2);
        assert_eq!(c.ids("blue"), Some(vec![10, 11]));
        assert_eq!(c.stats("missing"), None);
    }

    #[test]
    fn alias_errors() {
        let mut c = Collections::new();
        c.create("blue").unwrap();
        assert_eq!(
            c.alias("a", "missing"),
            Err(CollectionsError::NotFound("missing".into()))
        );
        c.alias("a", "blue").unwrap();
        assert_eq!(
            c.alias("a", "blue"),
            Err(CollectionsError::AliasExists("a".into()))
        );
        assert_eq!(
            c.alias("blue", "blue"),
            Err(CollectionsError::AliasNameTaken("blue".into()))
        );
        assert_eq!(
            c.retarget("nope", "blue"),
            Err(CollectionsError::AliasNotFound("nope".into()))
        );
        assert_eq!(
            c.create("blue"),
            Err(CollectionsError::Exists("blue".into()))
        );
        assert_eq!(c.list(), vec!["blue".to_string()]);
    }
}
