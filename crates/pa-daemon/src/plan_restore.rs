//! Reading a project's stored plan back into a session.
//!
//! The project store keeps a durable plan outside the session. This module is
//! the translation edge: it turns stored rows into [`PlanItem`]s, and decides
//! which plan a starting session should have. Nothing here writes.

use pa_core::session_engine::plan::{PlanItem, PlanItemStatus, PlanState};
use serde_json::Value;

/// One plan item as the read model holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPlanItem {
    /// The item's stable id.
    pub id: String,
    /// Position in the plan's list; the model's rows have no inherent order.
    pub position: usize,
    /// The item's text.
    pub text: String,
    /// Its status.
    pub status: PlanItemStatus,
    /// What would show the item is done.
    pub verify: Option<String>,
    /// What settled it.
    pub evidence: Option<String>,
    /// The revision the evidence was recorded against.
    pub evidence_revision: Option<String>,
}

impl StoredPlanItem {
    /// Read one stored row, or `None` when it cannot describe an item.
    ///
    /// `id` is the entity's own id column, returned beside `data_json` rather
    /// than duplicated inside it. A row with no text is skipped rather than
    /// defaulted: an item with no text is not a task, and inventing one would
    /// put a blank obligation in front of the model.
    #[must_use]
    pub fn from_row(id: &str, value: &Value) -> Option<Self> {
        let status = match value.get("status").and_then(Value::as_str)? {
            "pending" => PlanItemStatus::Pending,
            "in_progress" => PlanItemStatus::InProgress,
            "completed" => PlanItemStatus::Completed,
            "blocked" => PlanItemStatus::Blocked,
            _ => return None,
        };
        Some(Self {
            id: id.to_string(),
            position: value
                .get("position")
                .and_then(Value::as_u64)
                .and_then(|position| usize::try_from(position).ok())
                .unwrap_or(usize::MAX),
            text: value.get("text").and_then(Value::as_str)?.to_string(),
            status,
            verify: value
                .get("verify")
                .and_then(Value::as_str)
                .map(str::to_string),
            evidence: value
                .get("evidence")
                .and_then(Value::as_str)
                .map(str::to_string),
            evidence_revision: value
                .get("evidenceRevision")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    fn into_item(self) -> PlanItem {
        PlanItem {
            id: self.id,
            text: self.text,
            status: self.status,
            verify: self.verify,
            evidence: self.evidence,
            evidence_revision: self.evidence_revision,
        }
    }
}

/// The plan a starting session should carry.
///
/// The session's own record wins: a session that has a plan in its transcript
/// is resuming that session, and the project's stored plan belongs to whatever
/// session wrote it last. The project plan therefore only fills an empty
/// session, which is the case it exists for — a fresh session in a project that
/// already has unfinished work.
#[must_use]
pub fn plan_for_start(current: &PlanState, stored: Option<&[StoredPlanItem]>) -> Option<PlanState> {
    if !current.items.is_empty() {
        return None;
    }
    let stored = stored?;
    if stored.is_empty() {
        return None;
    }
    let items: Vec<PlanItem> = stored
        .iter()
        .cloned()
        .map(StoredPlanItem::into_item)
        .collect();
    // A stored plan can violate the invariants the tool enforces: an older
    // build recorded it, or the log was edited by hand. Installing it would put
    // the session under obligations no plan may hold (two `in_progress` items,
    // duplicate ids), so the invalid plan is not adopted. Restoring nothing is
    // honest; the session simply starts empty.
    if PlanState::validate_items(&items).is_err() {
        return None;
    }
    Some(PlanState { items })
}

#[cfg(test)]
mod tests {
    use pa_core::session_engine::plan::{PlanItem, PlanItemStatus, PlanState};
    use serde_json::json;

    use super::*;

    fn stored(id: &str, position: usize, status: PlanItemStatus) -> StoredPlanItem {
        StoredPlanItem {
            id: id.to_string(),
            position,
            text: format!("do {id}"),
            status,
            verify: None,
            evidence: None,
            evidence_revision: None,
        }
    }

    #[test]
    fn a_stored_row_becomes_an_item_with_its_evidence() {
        let row = json!({
            "position": 2,
            "text": "run the suite",
            "status": "completed",
            "verify": "cargo test",
            "evidence": "cargo test: 37 passed",
            "evidenceRevision": "abc123"
        });

        let item = StoredPlanItem::from_row("P1", &row).unwrap();

        assert_eq!(item.id, "P1");
        assert_eq!(item.position, 2);
        assert_eq!(item.status, PlanItemStatus::Completed);
        assert_eq!(item.verify.as_deref(), Some("cargo test"));
        assert_eq!(item.evidence.as_deref(), Some("cargo test: 37 passed"));
        assert_eq!(item.evidence_revision.as_deref(), Some("abc123"));
    }

    #[test]
    fn a_row_without_text_is_skipped_rather_than_defaulted() {
        assert!(StoredPlanItem::from_row("P1", &json!({ "status": "pending" })).is_none());
        // An unknown status is a row a later build wrote; guessing `pending`
        // would invent an obligation.
        assert!(
            StoredPlanItem::from_row("P1", &json!({ "text": "x", "status": "abandoned" }))
                .is_none()
        );
    }

    #[test]
    fn an_empty_session_adopts_the_projects_plan_in_order() {
        let mut second = stored("P2", 1, PlanItemStatus::Pending);
        second.text = "second".to_string();
        let mut first = stored("P1", 0, PlanItemStatus::InProgress);
        first.text = "first".to_string();

        let plan = plan_for_start(&PlanState::default(), Some(&[second, first])).unwrap();

        assert_eq!(
            plan.items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "first"]
        );
    }

    #[test]
    fn a_session_with_its_own_plan_keeps_it() {
        let current = PlanState {
            items: vec![PlanItem {
                id: "mine".to_string(),
                text: "the session's own item".to_string(),
                status: PlanItemStatus::Pending,
                verify: None,
                evidence: None,
                evidence_revision: None,
            }],
        };

        assert!(
            plan_for_start(&current, Some(&[stored("P1", 0, PlanItemStatus::Pending)])).is_none()
        );
    }

    #[test]
    fn nothing_stored_adopts_nothing() {
        assert!(plan_for_start(&PlanState::default(), None).is_none());
        assert!(plan_for_start(&PlanState::default(), Some(&[])).is_none());
    }
    #[test]
    fn a_stored_plan_that_breaks_the_invariants_is_not_adopted() {
        let mut first = stored("P1", 0, PlanItemStatus::InProgress);
        first.text = "one".to_string();
        let mut second = stored("P2", 1, PlanItemStatus::InProgress);
        second.text = "two".to_string();

        // Two `in_progress` items is what `update_plan` refuses; a restored
        // plan must not smuggle it past the tool.
        assert!(plan_for_start(&PlanState::default(), Some(&[first, second])).is_none());
    }
}
