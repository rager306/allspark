//! Reference checking: whether a stored cross-database key still names
//! something.
//!
//! A reference is a claim about another database that nothing verifies at
//! write time — the target may be reindexed, renamed, or never exist. The
//! checker resolves each key through a caller-supplied [`RefResolver`], which
//! keeps this crate independent of any particular target database: the project
//! crate never links the graph.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::model::{PendingRef, ReadModel};

/// The status of a reference after a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefStatus {
    /// The resolver found the target.
    Resolved,
    /// The resolver looked and found nothing: the key is a dangling claim.
    Dangling,
    /// No resolver handles this reference's target database, so no claim is
    /// made either way.
    Pending,
}

impl RefStatus {
    /// The stored spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Dangling => "dangling",
            Self::Pending => "pending",
        }
    }
}

/// Resolves a reference's `target_key` against its target database.
///
/// Implementations return `Ok(None)` only when they looked and the target is
/// absent. Returning `Err` means the lookup itself failed, and the checker
/// leaves the reference `pending` rather than recording a guess: a database
/// that could not be read is not evidence of a dangling link.
pub trait RefResolver {
    /// The `target_db` values this resolver answers.
    fn databases(&self) -> &[&str];

    /// Whether `target_type`/`target_key` names a live target.
    ///
    /// # Errors
    ///
    /// Returns an error when the lookup cannot be performed.
    fn resolve(&self, target_type: &str, target_key: &str) -> Result<Option<String>>;
}

/// What one check pass changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LinkReport {
    /// Reference ids that moved status, with their new status.
    pub changed: Vec<(String, RefStatus)>,
    /// References left `pending` because a lookup failed, with the reason.
    pub unresolved: Vec<(String, String)>,
}

impl LinkReport {
    /// Whether the pass changed anything.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.changed.is_empty() && self.unresolved.is_empty()
    }

    /// Counts by status, for an operator summary.
    #[must_use]
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for (_, status) in &self.changed {
            *counts.entry(status.as_str()).or_insert(0) += 1;
        }
        counts
    }
}

/// Re-resolve every reference the model holds and store the outcomes.
///
/// The pass is idempotent: a reference that resolves to the same status writes
/// nothing, so a repeated check is free and a stable model stays stable.
///
/// # Errors
///
/// Returns an error when the model cannot be read or written.
pub fn check_links(model: &ReadModel, resolvers: &[&dyn RefResolver]) -> Result<LinkReport> {
    let mut report = LinkReport::default();
    for reference in model.pending_refs(None)? {
        let Some(status) = resolve_one(resolvers, &reference, &mut report) else {
            continue;
        };
        if status.as_str() != reference.status {
            model.set_ref_status(&reference.id, status.as_str())?;
            report.changed.push((reference.id, status));
        }
    }
    Ok(report)
}

/// Resolve one reference, recording a failed lookup in `report`.
///
/// Returns `None` when the reference's database has no resolver: the stored
/// status is then left alone, because a missing resolver is not a verdict.
fn resolve_one(
    resolvers: &[&dyn RefResolver],
    reference: &PendingRef,
    report: &mut LinkReport,
) -> Option<RefStatus> {
    let resolver = resolvers
        .iter()
        .find(|resolver| resolver.databases().contains(&reference.target_db.as_str()))?;
    match resolver.resolve(&reference.target_type, &reference.target_key) {
        Ok(Some(_)) => Some(RefStatus::Resolved),
        // No resolver claims the database, so nothing is known: a missing
        // resolver is not a verdict.
        Ok(None) => Some(RefStatus::Dangling),
        Err(error) => {
            report
                .unresolved
                .push((reference.id.clone(), format!("{error:#}")));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::*;
    use crate::event::{Actor, Event, SCHEMA_VERSION};
    use crate::project::project;

    const PROJECT: &str = "proj";

    /// A resolver over a fixed set of keys, one database name.
    struct Fixed {
        database: &'static str,
        keys: BTreeSet<String>,
        failure: Option<&'static str>,
    }

    impl Fixed {
        fn new(database: &'static str, keys: &[&str]) -> Self {
            Self {
                database,
                keys: keys.iter().map(|key| (*key).to_string()).collect(),
                failure: None,
            }
        }

        fn failing(mut self, reason: &'static str) -> Self {
            self.failure = Some(reason);
            self
        }
    }

    impl RefResolver for Fixed {
        fn databases(&self) -> &[&str] {
            std::slice::from_ref(&self.database)
        }

        fn resolve(&self, _target_type: &str, target_key: &str) -> Result<Option<String>> {
            if let Some(reason) = self.failure {
                anyhow::bail!("{reason}");
            }
            Ok(self
                .keys
                .contains(target_key)
                .then(|| target_key.to_string()))
        }
    }

    fn model_with_refs(keys: &[(&str, &str, &str)]) -> (tempfile::TempDir, ReadModel) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.db");
        let mut model = ReadModel::open(&db).unwrap();
        let events: Vec<Event> = keys
            .iter()
            .enumerate()
            .map(|(index, (target_db, target_key, status))| Event {
                event_id: format!("e-{index}"),
                event_type: "ref.added".to_string(),
                schema_version: SCHEMA_VERSION,
                project_id: PROJECT.to_string(),
                session_id: None,
                occurred_at: "2026-11-18T12:00:00Z".to_string(),
                actor: Actor {
                    kind: "agent".to_string(),
                    id: None,
                },
                payload: json!({
                    "id": format!("ref-{index}"),
                    "from": "P1",
                    "predicate": "evidence_about_code",
                    "targetDb": target_db,
                    "targetType": "code_ref",
                    "targetKey": target_key,
                    "resolver": "code_ref",
                    "status": status
                }),
            })
            .collect();
        project(&mut model, PROJECT, &events).unwrap();
        (dir, model)
    }

    #[test]
    fn a_present_target_resolves_and_a_missing_one_is_dangling() {
        let (_dir, model) = model_with_refs(&[
            ("graph", "src/lib.rs#foo", "pending"),
            ("graph", "src/gone.rs#foo", "pending"),
        ]);
        let resolver = Fixed::new("graph", &["src/lib.rs#foo"]);

        let report = check_links(&model, &[&resolver]).unwrap();

        assert_eq!(report.changed.len(), 2);
        assert_eq!(report.unresolved, Vec::new());
        assert_eq!(report.counts()["resolved"], 1);
        assert_eq!(report.counts()["dangling"], 1);
        assert_eq!(model.refs_from("P1").unwrap().len(), 2);
    }

    #[test]
    fn a_repeated_check_writes_nothing() {
        let (_dir, model) = model_with_refs(&[("graph", "src/lib.rs#foo", "pending")]);
        let resolver = Fixed::new("graph", &["src/lib.rs#foo"]);

        check_links(&model, &[&resolver]).unwrap();
        let second = check_links(&model, &[&resolver]).unwrap();

        assert!(second.is_clean());
        assert_eq!(second.changed, Vec::new());
    }

    #[test]
    fn a_failed_lookup_leaves_the_reference_pending() {
        let (_dir, model) = model_with_refs(&[("graph", "src/lib.rs#foo", "pending")]);
        let resolver = Fixed::new("graph", &["src/lib.rs#foo"]).failing("index is locked");

        let report = check_links(&model, &[&resolver]).unwrap();

        assert_eq!(report.changed, Vec::new());
        assert_eq!(report.unresolved.len(), 1);
        assert!(report.unresolved[0].1.contains("index is locked"));
        // Not evidence of a dangling link: the unread database says nothing.
        assert_eq!(model.refs_from("P1").unwrap()[0].1, "pending");
    }

    #[test]
    fn a_database_with_no_resolver_keeps_its_stored_status() {
        let (_dir, model) = model_with_refs(&[("knowledge", "note:42", "pending")]);
        let resolver = Fixed::new("graph", &["src/lib.rs#foo"]);

        let report = check_links(&model, &[&resolver]).unwrap();

        assert!(report.is_clean());
        assert_eq!(model.refs_from("P1").unwrap()[0].1, "pending");
    }

    #[test]
    fn a_reference_that_disappears_becomes_dangling() {
        let (_dir, model) = model_with_refs(&[("graph", "src/lib.rs#foo", "resolved")]);
        let resolver = Fixed::new("graph", &[]);

        let report = check_links(&model, &[&resolver]).unwrap();

        assert_eq!(
            report.changed,
            vec![("ref-0".to_string(), RefStatus::Dangling)]
        );
        assert_eq!(model.refs_from("P1").unwrap()[0].1, "dangling");
    }
}
