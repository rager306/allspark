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
    // build recorded it, or the log was edited by hand. Refusing the whole plan
    // over one bad item throws away work the session still needs, so the
    // violations are repaired instead — and only the ones the rules can repair
    // without inventing a decision.
    let repaired = repair_stored_items(items);
    // Repair can drop every item (all blank ids or texts). An empty plan is not
    // a restored plan: the caller installs it and the session then believes its
    // obligation is gone.
    if repaired.is_empty() {
        return None;
    }
    Some(PlanState { items: repaired })
}

/// Bring a stored plan back inside the invariants `update_plan` enforces.
///
/// Two rules are repairable without a decision:
/// - a repeated id is a duplicate ROW, so the first occurrence wins and later
///   ones are dropped (same id, same step: keeping both would fail every lookup);
/// - only the first `in_progress` item stays in progress; the rest become
///   `pending`, because two active items is an ordering claim nobody made.
///
/// An item with empty text or an empty id is dropped: it names no work, and a
/// blank obligation in front of the model is worse than a shorter plan. The
/// result is then valid by construction; a caller that must not proceed on a
/// repaired plan cannot tell it apart, which is why the repair is documented
/// here rather than silent.
fn repair_stored_items(items: Vec<PlanItem>) -> Vec<PlanItem> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut kept_in_progress = false;
    items
        .into_iter()
        .filter_map(|mut item| {
            if item.id.trim().is_empty() || item.text.trim().is_empty() {
                return None;
            }
            if !seen.insert(item.id.clone()) {
                return None;
            }
            if item.status == PlanItemStatus::InProgress {
                if kept_in_progress {
                    item.status = PlanItemStatus::Pending;
                } else {
                    kept_in_progress = true;
                }
            }
            Some(item)
        })
        .collect()
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
    fn a_second_in_progress_item_comes_back_as_pending() {
        let mut first = stored("P1", 0, PlanItemStatus::InProgress);
        first.text = "one".to_string();
        let mut second = stored("P2", 1, PlanItemStatus::InProgress);
        second.text = "two".to_string();

        // Two `in_progress` items is what `update_plan` refuses. Refusing the
        // whole plan over it would throw away work the session needs, so the
        // later one becomes pending — the ordering claim nobody made.
        let plan = plan_for_start(&PlanState::default(), Some(&[first, second]))
            .expect("a repairable plan is still restored");
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);
        assert_eq!(plan.items[1].status, PlanItemStatus::Pending);
        PlanState::validate_items(&plan.items).expect("the repair is valid");
    }

    #[test]
    fn a_duplicate_id_keeps_the_first_row_and_drops_the_rest() {
        let mut first = stored("P1", 0, PlanItemStatus::Pending);
        first.text = "first".to_string();
        let mut duplicate = stored("P1", 1, PlanItemStatus::Pending);
        duplicate.text = "second".to_string();

        let plan = plan_for_start(&PlanState::default(), Some(&[first, duplicate]))
            .expect("a repairable plan is still restored");
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].text, "first");
    }

    #[test]
    fn an_item_with_empty_text_is_dropped_and_the_rest_survives() {
        let mut blank = stored("P1", 0, PlanItemStatus::Pending);
        blank.text = "   ".to_string();
        let mut real = stored("P2", 1, PlanItemStatus::Pending);
        real.text = "real step".to_string();

        let plan = plan_for_start(&PlanState::default(), Some(&[blank, real]))
            .expect("the valid item is kept");
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].text, "real step");
    }

    /// Repair must not invent an obligation: a stored plan whose every item is
    /// blank yields nothing rather than an empty plan that looks restored.
    #[test]
    fn a_plan_with_nothing_repairable_is_not_adopted() {
        let mut blank = stored("P1", 0, PlanItemStatus::Pending);
        blank.text = String::new();
        assert!(plan_for_start(&PlanState::default(), Some(&[blank])).is_none());
    }
}
