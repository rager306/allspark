//! Session-scoped plan state for explicit multi-step work.
//!
//! The model-facing `update_plan` tool writes the plan, and the continuation
//! policy reads it. The state is deliberately small and in-memory: it is a
//! continuation input, not a second session store. Persistence and UI
//! projection can be added without changing this contract.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use pa_types::ai::AssistantContentBlock;
use pa_types::session::{AgentMessage, FileEntry};
use serde::{Deserialize, Serialize};

/// The state of one plan item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Blocked,
}

/// One explicit unit of work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub text: String,
    pub status: PlanItemStatus,
}

impl PlanItem {
    /// Whether the item still asks for work: `pending` or `in_progress`.
    #[must_use]
    pub fn is_actionable(&self) -> bool {
        matches!(
            self.status,
            PlanItemStatus::Pending | PlanItemStatus::InProgress
        )
    }
}

/// The plan as last replaced by the model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanState {
    pub items: Vec<PlanItem>,
}

impl PlanState {
    /// Validate the invariants the model-facing plan must keep: non-empty
    /// fields, unique ids, and at most one `in_progress` item.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first violated invariant.
    pub fn validate_items(items: &[PlanItem]) -> anyhow::Result<()> {
        let mut ids = HashSet::with_capacity(items.len());
        let mut in_progress = Vec::new();
        for item in items {
            if item.id.trim().is_empty() || item.text.trim().is_empty() {
                anyhow::bail!("plan item ids and text must be non-empty");
            }
            if !ids.insert(item.id.as_str()) {
                anyhow::bail!("plan item ids must be unique: {}", item.id);
            }
            if item.status == PlanItemStatus::InProgress {
                in_progress.push(item.id.as_str());
            }
        }
        if in_progress.len() > 1 {
            anyhow::bail!(
                "plan may have at most one in_progress item; found {}: {}",
                in_progress.len(),
                in_progress.join(", ")
            );
        }
        Ok(())
    }

    /// The ids of items that still ask for work, in plan order.
    #[must_use]
    pub fn unfinished_item_ids(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| item.is_actionable())
            .map(|item| item.id.clone())
            .collect()
    }

    /// Refuse completion while an item still asks for work. Completed and
    /// blocked items are settled: `blocked` is the model's explicit report
    /// that the remaining step is not actionable in this run.
    ///
    /// # Errors
    ///
    /// Returns an error naming the unfinished ids.
    pub fn ensure_completable(&self) -> anyhow::Result<()> {
        let unfinished = self.unfinished_item_ids();
        if unfinished.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(
                "cannot complete the goal while plan items remain unfinished: {}; finish them or mark them blocked with update_plan",
                unfinished.join(", ")
            )
        }
    }

    /// Rebuild the plan from the latest `update_plan` tool call in a session
    /// branch. The transcript is the durable record, so a rebuilt worker
    /// restores the same obligation without a second store.
    #[must_use]
    pub fn from_entries(entries: &[FileEntry]) -> Self {
        for entry in entries.iter().rev() {
            let FileEntry::Message {
                message: AgentMessage::Assistant(message),
                ..
            } = entry
            else {
                continue;
            };
            for block in message.content.iter().rev() {
                let AssistantContentBlock::ToolCall(call) = block else {
                    continue;
                };
                if call.name != "update_plan" {
                    continue;
                }
                if let Some(state) = Self::from_update_plan_arguments(&serde_json::Value::Object(
                    call.arguments.clone(),
                )) {
                    return state;
                }
            }
        }
        Self::default()
    }

    fn from_update_plan_arguments(arguments: &serde_json::Value) -> Option<Self> {
        let items: Vec<PlanItem> = serde_json::from_value(arguments.get("items")?.clone()).ok()?;
        Self::validate_items(&items).ok()?;
        Some(Self { items })
    }
}

struct PlanInner {
    state: PlanState,
    last_offered: Option<String>,
    repeat_count: u32,
}

/// The session's plan cell: one cheap lock shared by the tool and the
/// continuation consult.
#[derive(Clone)]
pub struct PlanStore(Arc<Mutex<PlanInner>>);

impl Default for PlanStore {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(PlanInner {
            state: PlanState::default(),
            last_offered: None,
            repeat_count: 0,
        })))
    }
}

impl PlanStore {
    /// Replace the complete plan. Any item change resets the reminder guard.
    pub fn replace(&self, items: Vec<PlanItem>) -> PlanState {
        let mut inner = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.state = PlanState { items };
        inner.last_offered = None;
        inner.repeat_count = 0;
        inner.state.clone()
    }

    /// Read the current plan.
    pub fn snapshot(&self) -> PlanState {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
            .clone()
    }

    /// Restore the plan from a session branch's latest `update_plan` call.
    /// Resets the reminder guard, exactly like a fresh tool write.
    #[must_use]
    pub fn restore_from_entries(&self, entries: &[FileEntry]) -> PlanState {
        let state = PlanState::from_entries(entries);
        self.replace(state.items)
    }

    /// The next unblocked item, with a bounded reminder guard.
    ///
    /// The same item is offered at most twice without a plan update. This
    /// prevents a model that ends every turn with text from looping on one
    /// item; a real status change resets the guard.
    pub fn next_item(&self) -> Option<PlanItem> {
        let mut inner = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let candidates: Vec<PlanItem> = inner
            .state
            .items
            .iter()
            .filter(|item| item.is_actionable())
            .cloned()
            .collect();
        for item in candidates {
            if inner.last_offered.as_deref() == Some(item.id.as_str()) {
                inner.repeat_count = inner.repeat_count.saturating_add(1);
                if inner.repeat_count >= 2 {
                    continue;
                }
            } else {
                inner.last_offered = Some(item.id.clone());
                inner.repeat_count = 0;
            }
            return Some(item);
        }
        None
    }

    /// The continuation text for the next item, or `None` when there is no
    /// actionable item or the reminder bound was reached.
    #[must_use]
    pub fn continuation_text(&self) -> Option<String> {
        let item = self.next_item()?;
        Some(format!(
            "[plan: continuation]\n\nContinue the active plan. The next unblocked item is `{}`: {}.\n\nTake the next concrete action for it now. If it is genuinely blocked, call update_plan and mark it blocked with the reason; if it is complete, update its status before stopping.",
            item.id, item.text
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{PlanItem, PlanItemStatus, PlanState, PlanStore};

    fn item(id: &str, status: PlanItemStatus) -> PlanItem {
        PlanItem {
            id: id.to_string(),
            text: format!("finish {id}"),
            status,
        }
    }

    #[test]
    fn next_item_skips_completed_and_blocked_items() {
        let store = PlanStore::default();
        store.replace(vec![
            item("one", PlanItemStatus::Completed),
            item("two", PlanItemStatus::Blocked),
            item("three", PlanItemStatus::Pending),
            item("four", PlanItemStatus::InProgress),
        ]);

        assert_eq!(store.next_item().unwrap().id, "three");
        assert!(store.continuation_text().unwrap().contains("finish three"));
    }

    #[test]
    fn the_same_item_is_offered_twice_then_the_reminder_stops() {
        let store = PlanStore::default();
        store.replace(vec![item("one", PlanItemStatus::Pending)]);

        assert!(store.next_item().is_some());
        assert!(store.next_item().is_some());
        assert!(store.next_item().is_none());
    }

    #[test]
    fn unfinished_ids_name_only_actionable_items() {
        let store = PlanStore::default();
        store.replace(vec![
            item("one", PlanItemStatus::Pending),
            item("two", PlanItemStatus::Completed),
            item("three", PlanItemStatus::InProgress),
            item("four", PlanItemStatus::Blocked),
        ]);

        assert_eq!(
            store.snapshot().unfinished_item_ids(),
            vec!["one".to_string(), "three".to_string()]
        );
    }

    #[test]
    fn the_latest_update_plan_call_restores_the_branch_plan() {
        let entry = |items: serde_json::Value| {
            serde_json::from_value::<pa_types::session::FileEntry>(serde_json::json!({
                "type": "message",
                "id": "entry",
                "parentId": null,
                "timestamp": "2026-01-01T00:00:00.000Z",
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "toolCall",
                        "id": "call",
                        "name": "update_plan",
                        "arguments": { "items": items }
                    }],
                    "api": "faux",
                    "provider": "faux",
                    "model": "faux-1",
                    "usage": {
                        "input": 0,
                        "output": 0,
                        "cacheRead": 0,
                        "cacheWrite": 0,
                        "totalTokens": 0,
                        "cost": {
                            "input": 0,
                            "output": 0,
                            "cacheRead": 0,
                            "cacheWrite": 0,
                            "total": 0
                        }
                    },
                    "stopReason": "toolUse",
                    "timestamp": 0
                }
            }))
            .expect("the fixture entry parses")
        };

        let entries = vec![
            entry(serde_json::json!([
                { "id": "old", "text": "old step", "status": "pending" }
            ])),
            entry(serde_json::json!([
                { "id": "one", "text": "step one", "status": "completed" },
                { "id": "two", "text": "step two", "status": "in_progress" }
            ])),
        ];
        let direct = PlanState::from_update_plan_arguments(&serde_json::json!({
            "items": [
                { "id": "one", "text": "step one", "status": "completed" },
                { "id": "two", "text": "step two", "status": "in_progress" }
            ]
        }))
        .expect("the direct parser accepts the plan");
        assert_eq!(direct.unfinished_item_ids(), vec!["two".to_string()]);

        let pa_types::session::FileEntry::Message {
            message: pa_types::session::AgentMessage::Assistant(message),
            ..
        } = &entries[1]
        else {
            panic!("the fixture is not an assistant message: {:?}", entries[1]);
        };
        assert_eq!(message.content.len(), 1, "fixture content: {message:?}");

        let state = PlanState::from_entries(&entries);
        assert_eq!(
            state.unfinished_item_ids(),
            vec!["two".to_string()],
            "the latest call must win"
        );
        assert_eq!(state.items[1].status, PlanItemStatus::InProgress);
    }

    #[test]
    fn ensure_completable_accepts_only_settled_items() {
        let store = PlanStore::default();
        store.replace(vec![
            item("one", PlanItemStatus::Completed),
            item("two", PlanItemStatus::Blocked),
        ]);
        store.snapshot().ensure_completable().unwrap();

        store.replace(vec![item("three", PlanItemStatus::InProgress)]);
        let error = store.snapshot().ensure_completable().unwrap_err();
        assert!(error.to_string().contains("three"), "{error}");
    }

    #[test]
    fn an_exhausted_item_yields_to_the_next_pending_item() {
        let store = PlanStore::default();
        store.replace(vec![
            item("one", PlanItemStatus::Pending),
            item("two", PlanItemStatus::Pending),
        ]);

        assert_eq!(store.next_item().unwrap().id, "one");
        assert_eq!(store.next_item().unwrap().id, "one");
        assert_eq!(store.next_item().unwrap().id, "two");
    }

    #[test]
    fn a_plan_update_resets_the_reminder_bound() {
        let store = PlanStore::default();
        store.replace(vec![item("one", PlanItemStatus::Pending)]);
        assert!(store.next_item().is_some());
        assert!(store.next_item().is_some());
        assert!(store.next_item().is_none());

        store.replace(vec![item("one", PlanItemStatus::InProgress)]);
        assert!(store.next_item().is_some());
    }
}
