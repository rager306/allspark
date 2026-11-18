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

/// The `custom` entry tag that carries the plan across a compaction.
///
/// A compacted window can drop the assistant `update_plan` call that created
/// the plan, so the transcript replay alone loses the obligation. The entry is
/// the durable copy; [`PlanState::from_entries`] accepts whichever of the two
/// it meets first in the branch.
pub const PLAN_STATE_CUSTOM_TYPE: &str = "session_plan_state";

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
    /// What would show this item is done (a command, a test, an observation).
    /// Optional so plans written before the field existed stay readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    /// The evidence recorded when the item settled: the command and its
    /// result, a review receipt id, or the observed state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// The commit the evidence was recorded against. The harness stamps it
    /// from the session's git context; the model never sets it. A later tree
    /// therefore reports the evidence as stale instead of silently passing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_revision: Option<String>,
}

impl PlanItemStatus {
    /// The wire spelling (`pending`, `in_progress`, `completed`, `blocked`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
        }
    }
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

    /// Whether the item settled without recording how it was shown to be done.
    /// An item that declared a `verify` step is the one the harness insists on.
    #[must_use]
    pub fn is_completed_without_evidence(&self) -> bool {
        self.status == PlanItemStatus::Completed
            && self.verify.is_some()
            && self
                .evidence
                .as_deref()
                .is_none_or(|evidence| evidence.trim().is_empty())
    }

    /// Whether the item's evidence was recorded against a different commit
    /// than `current`. Evidence without a recorded revision is not stale: the
    /// harness stamps it, and a session outside a repository has none.
    #[must_use]
    pub fn is_evidence_stale(&self, current: Option<&str>) -> bool {
        let (Some(current), Some(recorded)) = (current, self.evidence_revision.as_deref()) else {
            return false;
        };
        self.status == PlanItemStatus::Completed && self.evidence.is_some() && current != recorded
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

    /// The ids of completed items whose evidence was recorded against another
    /// commit, in plan order. Advisory: the TUI and the continuation prompt
    /// surface it; it never refuses completion on its own.
    #[must_use]
    pub fn stale_evidence_item_ids(&self, current: Option<&str>) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| item.is_evidence_stale(current))
            .map(|item| item.id.clone())
            .collect()
    }

    /// Stamp `revision` on every completed item that carries evidence but no
    /// recorded revision. The harness calls this with the session's commit.
    pub fn stamp_evidence_revision(&mut self, revision: Option<&str>) {
        let Some(revision) = revision else {
            return;
        };
        for item in &mut self.items {
            if item.status == PlanItemStatus::Completed
                && item.evidence.is_some()
                && item.evidence_revision.is_none()
            {
                item.evidence_revision = Some(revision.to_string());
            }
        }
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
    /// Returns an error naming the unfinished ids, or the item that claims
    /// completion without the evidence its own `verify` step promised.
    pub fn ensure_completable(&self) -> anyhow::Result<()> {
        let unfinished = self.unfinished_item_ids();
        if !unfinished.is_empty() {
            anyhow::bail!(
                "cannot complete the goal while plan items remain unfinished: {}; finish them or mark them blocked with update_plan",
                unfinished.join(", ")
            )
        }
        let unproven: Vec<&str> = self
            .items
            .iter()
            .filter(|item| item.is_completed_without_evidence())
            .map(|item| item.id.as_str())
            .collect();
        if !unproven.is_empty() {
            anyhow::bail!(
                "plan items claim completion without evidence: {}; record what showed the item done (the command and its result) in `evidence`",
                unproven.join(", ")
            );
        }
        Ok(())
    }

    /// Rebuild the plan from a session branch.
    ///
    /// The newest of two durable records wins: the `session_plan_state` custom
    /// entry (written with every update, and retained through compaction) or an
    /// assistant `update_plan` tool call still present in the branch. The
    /// tool-call arm keeps sessions written before the entry existed readable.
    #[must_use]
    pub fn from_entries(entries: &[FileEntry]) -> Self {
        for entry in entries.iter().rev() {
            match entry {
                FileEntry::Custom { payload, .. }
                    if payload.custom_type == PLAN_STATE_CUSTOM_TYPE =>
                {
                    if let Some(state) = payload.data.as_ref().and_then(Self::from_persisted_value)
                    {
                        return state;
                    }
                }
                FileEntry::Message {
                    message: AgentMessage::Assistant(message),
                    ..
                } => {
                    for block in message.content.iter().rev() {
                        let AssistantContentBlock::ToolCall(call) = block else {
                            continue;
                        };
                        if call.name != "update_plan" {
                            continue;
                        }
                        if let Some(state) = Self::from_update_plan_arguments(
                            &serde_json::Value::Object(call.arguments.clone()),
                        ) {
                            return state;
                        }
                    }
                }
                _ => {}
            }
        }
        Self::default()
    }

    /// The value written to the `session_plan_state` custom entry.
    #[must_use]
    pub fn to_persisted_value(&self) -> serde_json::Value {
        serde_json::json!({ "items": self.items })
    }

    fn from_persisted_value(data: &serde_json::Value) -> Option<Self> {
        let items: Vec<PlanItem> = serde_json::from_value(data.get("items")?.clone()).ok()?;
        Self::validate_items(&items).ok()?;
        Some(Self { items })
    }

    fn from_update_plan_arguments(arguments: &serde_json::Value) -> Option<Self> {
        let items: Vec<PlanItem> = serde_json::from_value(arguments.get("items")?.clone()).ok()?;
        Self::validate_items(&items).ok()?;
        Some(Self { items })
    }
}

/// Writes one plan change to the session's durable record. The embedding
/// installs it after the session exists; [`PlanStore::replace`] never takes a
/// session lock itself, and the hook is expected to hand the write to a task.
pub type PlanPersistHook = Arc<dyn Fn(&PlanState) + Send + Sync>;

struct PlanInner {
    state: PlanState,
    last_offered: Option<String>,
    repeat_count: u32,
    persist: Option<PlanPersistHook>,
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
            persist: None,
        })))
    }
}

impl PlanStore {
    /// Install the durable-record hook. Called once by the embedding after the
    /// session exists; a later call replaces it.
    pub fn set_persist_hook(&self, hook: PlanPersistHook) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .persist = Some(hook);
    }

    /// Replace the complete plan without writing a durable record. Any item
    /// change resets the reminder guard.
    pub fn replace(&self, items: Vec<PlanItem>) {
        self.replace_inner(items, false);
    }

    /// Replace the plan from a model tool call and write the durable record,
    /// so a compacted window still restores the obligation.
    #[must_use]
    pub fn replace_from_tool(&self, items: Vec<PlanItem>) -> PlanState {
        self.replace_inner(items, true)
    }

    fn replace_inner(&self, items: Vec<PlanItem>, persist: bool) -> PlanState {
        let (state, hook) = {
            let mut inner = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.state = PlanState { items };
            inner.last_offered = None;
            inner.repeat_count = 0;
            (inner.state.clone(), inner.persist.clone())
        };
        if persist {
            if let Some(hook) = hook {
                hook(&state);
            }
        }
        state
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
        self.replace(state.items);
        self.snapshot()
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
    use super::{PlanItem, PlanItemStatus, PlanState, PlanStore, PLAN_STATE_CUSTOM_TYPE};

    fn item(id: &str, status: PlanItemStatus) -> PlanItem {
        PlanItem {
            id: id.to_string(),
            text: format!("finish {id}"),
            status,
            verify: None,
            evidence: None,
            evidence_revision: None,
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
    fn the_persisted_entry_restores_a_plan_a_compaction_dropped() {
        // The compacted branch holds only the summary: the `update_plan` call
        // that created the plan is gone. The custom entry is the record that
        // survives, so the obligation (and the goal guard) still holds.
        let entry: pa_types::session::FileEntry = serde_json::from_value(serde_json::json!({
            "type": "custom",
            "customType": PLAN_STATE_CUSTOM_TYPE,
            "data": { "items": [
                { "id": "one", "text": "step one", "status": "completed" },
                { "id": "two", "text": "step two", "status": "in_progress" }
            ] },
            "id": "entry",
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00.000Z"
        }))
        .expect("the custom entry parses");

        let state = PlanState::from_entries(&[entry]);
        assert_eq!(state.unfinished_item_ids(), vec!["two".to_string()]);
        assert!(state.ensure_completable().is_err());
    }

    #[test]
    fn a_newer_update_plan_call_still_wins_over_an_older_entry() {
        let entry: pa_types::session::FileEntry = serde_json::from_value(serde_json::json!({
            "type": "custom",
            "customType": PLAN_STATE_CUSTOM_TYPE,
            "data": { "items": [
                { "id": "stale", "text": "stale step", "status": "pending" }
            ] },
            "id": "older",
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00.000Z"
        }))
        .expect("the custom entry parses");
        let call: pa_types::session::FileEntry = serde_json::from_value(serde_json::json!({
            "type": "message",
            "id": "newer",
            "parentId": "older",
            "timestamp": "2026-01-01T00:00:01.000Z",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "id": "call",
                    "name": "update_plan",
                    "arguments": { "items": [
                        { "id": "fresh", "text": "fresh step", "status": "pending" }
                    ] }
                }],
                "api": "faux",
                "provider": "faux",
                "model": "faux-1",
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
                },
                "stopReason": "toolUse",
                "timestamp": 0
            }
        }))
        .expect("the tool-call entry parses");

        let state = PlanState::from_entries(&[entry, call]);
        assert_eq!(state.unfinished_item_ids(), vec!["fresh".to_string()]);
    }

    #[test]
    fn an_invalid_persisted_entry_is_ignored_rather_than_trusted() {
        let entry: pa_types::session::FileEntry = serde_json::from_value(serde_json::json!({
            "type": "custom",
            "customType": PLAN_STATE_CUSTOM_TYPE,
            "data": { "items": [
                { "id": "dup", "text": "one", "status": "pending" },
                { "id": "dup", "text": "two", "status": "pending" }
            ] },
            "id": "entry",
            "parentId": null,
            "timestamp": "2026-01-01T00:00:00.000Z"
        }))
        .expect("the custom entry parses");

        assert_eq!(PlanState::from_entries(&[entry]).items, Vec::new());
    }

    #[test]
    fn a_completed_item_that_promised_a_check_needs_evidence() {
        let store = PlanStore::default();
        store.replace(vec![PlanItem {
            id: "one".to_string(),
            text: "finish one".to_string(),
            status: PlanItemStatus::Completed,
            verify: Some("cargo test -p pa-core".to_string()),
            evidence: None,
            evidence_revision: None,
        }]);
        let error = store.snapshot().ensure_completable().unwrap_err();
        assert!(error.to_string().contains("one"), "{error}");

        store.replace(vec![PlanItem {
            id: "one".to_string(),
            text: "finish one".to_string(),
            status: PlanItemStatus::Completed,
            verify: Some("cargo test -p pa-core".to_string()),
            evidence: Some("cargo test -p pa-core: 1831 passed".to_string()),
            evidence_revision: None,
        }]);
        store.snapshot().ensure_completable().unwrap();
    }

    #[test]
    fn evidence_recorded_against_another_commit_is_reported_stale() {
        let state = PlanState {
            items: vec![PlanItem {
                id: "one".to_string(),
                text: "finish one".to_string(),
                status: PlanItemStatus::Completed,
                verify: Some("cargo test -p pa-core".to_string()),
                evidence: Some("cargo test -p pa-core: 12 passed".to_string()),
                evidence_revision: Some("aaaa".to_string()),
            }],
        };

        assert_eq!(state.stale_evidence_item_ids(Some("bbbb")), vec!["one"]);
        assert_eq!(
            state.stale_evidence_item_ids(Some("aaaa")),
            Vec::<String>::new()
        );
        // Outside a repository there is no revision to compare against.
        assert_eq!(state.stale_evidence_item_ids(None), Vec::<String>::new());

        // Staleness is advisory: the item still counts as settled.
        state.ensure_completable().unwrap();
    }

    #[test]
    fn the_first_revision_sticks_and_a_later_one_does_not_rewrite_it() {
        let mut state = PlanState {
            items: vec![PlanItem {
                id: "one".to_string(),
                text: "finish one".to_string(),
                status: PlanItemStatus::Completed,
                verify: None,
                evidence: Some("observed on screen".to_string()),
                evidence_revision: None,
            }],
        };

        state.stamp_evidence_revision(Some("aaaa"));
        assert_eq!(state.items[0].evidence_revision.as_deref(), Some("aaaa"));
        state.stamp_evidence_revision(Some("bbbb"));
        assert_eq!(
            state.items[0].evidence_revision.as_deref(),
            Some("aaaa"),
            "re-stamping would launder stale evidence"
        );

        // An item with no evidence is never stamped.
        state.items[0].evidence = None;
        state.items[0].evidence_revision = None;
        state.stamp_evidence_revision(Some("cccc"));
        assert_eq!(state.items[0].evidence_revision, None);
    }

    #[test]
    fn a_plan_without_a_verify_step_stays_readable_and_completable() {
        // Plans written before the field existed declare no check; they must
        // not be blocked by a rule they never opted into.
        let store = PlanStore::default();
        store.replace(vec![item("one", PlanItemStatus::Completed)]);
        store.snapshot().ensure_completable().unwrap();
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
