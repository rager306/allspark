//! The session plan tool: the model writes the explicit multi-step plan; the
//! daemon continuation consult reads it.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback, ToolExecutionMode};
use pa_core::session::manager::capture_git_context;
use pa_core::session_engine::plan::{PlanItem, PlanState, PlanStore};

pub(crate) const PLAN_TOOL_NAME: &str = "update_plan";

const DESCRIPTION: &str = "Replace the session plan for authorized multi-step work. Pass the complete items list with unique ids; at most one item may be in_progress. Give an item a `verify` step when one exists, and put what showed it done (the command and its result) in `evidence` before marking it completed. Update statuses alongside a real action rather than as a separate turn. Do not use this for plan-only or one-step requests.";

/// The plan tool backed by one session-scoped store.
pub(crate) struct PlanTool {
    store: PlanStore,
    /// The session working directory: the plan tool stamps the commit the
    /// evidence was recorded against, so a later tree reports it as stale.
    cwd: PathBuf,
    schema: OnceLock<serde_json::Value>,
}

impl PlanTool {
    pub(crate) fn new(store: PlanStore, cwd: PathBuf) -> Self {
        Self {
            store,
            cwd,
            schema: OnceLock::new(),
        }
    }

    fn parameters_schema(&self) -> &serde_json::Value {
        self.schema.get_or_init(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "text": { "type": "string" },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "blocked"]
                                },
                                "verify": { "type": "string" },
                                "evidence": { "type": "string" }
                            },
                            "required": ["id", "text", "status"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            })
        })
    }
}

impl AgentTool for PlanTool {
    fn name(&self) -> &str {
        PLAN_TOOL_NAME
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &serde_json::Value {
        self.parameters_schema()
    }

    fn execute(
        self: Arc<Self>,
        _tool_call_id: String,
        params: serde_json::Value,
        _signal: pa_agent::abort::AbortSignal,
        _on_update: AgentToolUpdateCallback,
    ) -> pa_agent::BoxFut<'static, anyhow::Result<AgentToolResult>> {
        Box::pin(async move {
            let items_value = params
                .get("items")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("update_plan requires an items array"))?;
            let mut items: Vec<PlanItem> = serde_json::from_value(items_value)
                .map_err(|error| anyhow::anyhow!("invalid plan items: {error}"))?;
            PlanState::validate_items(&items)?;
            // The revision is the harness's, never the model's: a plan update
            // that set its own revision could launder stale evidence.
            let cwd = self.cwd.clone();
            let revision = tokio::task::spawn_blocking(move || {
                capture_git_context(&cwd).and_then(|git| git.commit)
            })
            .await
            .ok()
            .flatten();
            let mut probe = PlanState { items };
            probe.stamp_evidence_revision(revision.as_deref());
            items = probe.items;
            let state = self.store.replace_from_tool(items);
            Ok(AgentToolResult {
                content: vec![pa_agent::types::ToolResultContent::text(
                    serde_json::to_string_pretty(&state)
                        .unwrap_or_else(|_| "plan updated".to_string()),
                )],
                details: serde_json::to_value(&state).unwrap_or(serde_json::Value::Null),
                terminate: None,
                is_error: false,
            })
        })
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use std::path::PathBuf;

    use pa_agent::abort::AbortSignal;
    use pa_agent::types::{AgentTool, AgentToolUpdateCallback};
    use pa_core::session_engine::plan::{PlanItem, PlanItemStatus, PlanStore};

    use super::PlanTool;

    fn no_updates() -> AgentToolUpdateCallback {
        Arc::new(|_| {})
    }

    #[tokio::test]
    async fn the_tool_replaces_the_plan_and_returns_the_normalized_state() {
        let store = PlanStore::default();
        let tool = Arc::new(PlanTool::new(store.clone(), PathBuf::from(".")));
        let result = tool
            .execute(
                "call-1".to_string(),
                serde_json::json!({
                    "items": [
                        { "id": "one", "text": "first step", "status": "pending" },
                        { "id": "two", "text": "second step", "status": "in_progress" }
                    ]
                }),
                AbortSignal::never(),
                no_updates(),
            )
            .await
            .expect("the plan is valid");

        assert!(!result.is_error);
        let state = store.snapshot();
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[1].status, PlanItemStatus::InProgress);
        assert_eq!(result.details["items"][0]["text"], "first step");
    }

    #[tokio::test]
    async fn the_tool_keeps_verify_and_evidence_on_an_item() {
        let store = PlanStore::default();
        let tool = Arc::new(PlanTool::new(store.clone(), PathBuf::from(".")));
        tool.execute(
            "call-evidence".to_string(),
            serde_json::json!({
                "items": [{
                    "id": "one",
                    "text": "first step",
                    "status": "completed",
                    "verify": "cargo test -p pa-core",
                    "evidence": "cargo test -p pa-core: 10 passed"
                }]
            }),
            AbortSignal::never(),
            no_updates(),
        )
        .await
        .expect("the plan is valid");

        let state = store.snapshot();
        assert_eq!(
            state.items[0].verify.as_deref(),
            Some("cargo test -p pa-core")
        );
        assert_eq!(
            state.items[0].evidence.as_deref(),
            Some("cargo test -p pa-core: 10 passed")
        );
        state.ensure_completable().unwrap();
    }

    #[tokio::test]
    async fn duplicate_ids_are_rejected_before_the_plan_is_replaced() {
        let store = PlanStore::default();
        let tool = Arc::new(PlanTool::new(store.clone(), PathBuf::from(".")));
        let error = tool
            .execute(
                "call-dup".to_string(),
                serde_json::json!({
                    "items": [
                        { "id": "one", "text": "first", "status": "pending" },
                        { "id": "one", "text": "second", "status": "pending" }
                    ]
                }),
                AbortSignal::never(),
                no_updates(),
            )
            .await
            .expect_err("duplicate ids are invalid");

        assert!(error.to_string().contains("unique"), "{error}");
        assert_eq!(store.snapshot().items, Vec::<PlanItem>::new());
    }

    #[tokio::test]
    async fn more_than_one_in_progress_item_is_rejected() {
        let store = PlanStore::default();
        let tool = Arc::new(PlanTool::new(store.clone(), PathBuf::from(".")));
        let error = tool
            .execute(
                "call-two-active".to_string(),
                serde_json::json!({
                    "items": [
                        { "id": "one", "text": "first", "status": "in_progress" },
                        { "id": "two", "text": "second", "status": "in_progress" }
                    ]
                }),
                AbortSignal::never(),
                no_updates(),
            )
            .await
            .expect_err("only one item may be in progress");

        assert!(error.to_string().contains("at most one"), "{error}");
        assert_eq!(store.snapshot().items, Vec::<PlanItem>::new());
    }

    #[tokio::test]
    async fn empty_ids_are_rejected_before_the_plan_is_replaced() {
        let store = PlanStore::default();
        let tool = Arc::new(PlanTool::new(store.clone(), PathBuf::from(".")));
        let error = tool
            .execute(
                "call-2".to_string(),
                serde_json::json!({
                    "items": [{ "id": " ", "text": "step", "status": "pending" }]
                }),
                AbortSignal::never(),
                no_updates(),
            )
            .await
            .expect_err("an empty id is invalid");

        assert!(error.to_string().contains("non-empty"), "{error}");
        assert_eq!(store.snapshot().items, Vec::<PlanItem>::new());
    }
}
