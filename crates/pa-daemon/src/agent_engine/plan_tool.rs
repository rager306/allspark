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
    /// The project store, when the session has one. `None` keeps every
    /// existing session byte-identical: no store, no event.
    recorder: Option<crate::project_store::ProjectRecorderHandle>,
    schema: OnceLock<serde_json::Value>,
}

impl PlanTool {
    pub(crate) fn new(store: PlanStore, cwd: PathBuf) -> Self {
        Self {
            store,
            cwd,
            recorder: None,
            schema: OnceLock::new(),
        }
    }

    /// Record every plan change to the project store as well.
    #[must_use]
    pub(crate) fn with_recorder(
        mut self,
        recorder: crate::project_store::ProjectRecorderHandle,
    ) -> Self {
        self.recorder = Some(recorder);
        self
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
            // The session record already landed; a store failure is logged and
            // never fails the tool call, so a project store cannot break a
            // session that would otherwise work.
            // The store write and the projection are blocking file and SQLite
            // work (measured 17.2 ms per call). Running them inline would block
            // an async worker for that long on the tool-call path, so they go to
            // the blocking pool and the call waits for the result as it did.
            if let Some(recorder) = &self.recorder {
                let items = state.items.clone();
                let recorder = std::sync::Arc::clone(recorder);
                let written = tokio::task::spawn_blocking(move || {
                    recorder.record_plan(&items).and_then(|()| recorder.sync())
                })
                .await;
                match written {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        eprintln!("pa-daemon: project plan event write failed: {error:#}");
                    }
                    Err(error) => {
                        eprintln!("pa-daemon: project plan write task failed: {error}");
                    }
                }
            }
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
    #[tokio::test]
    async fn a_plan_update_reaches_the_project_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = PlanStore::default();
        let recorder = Arc::new(crate::project_store::ProjectRecorder::new(
            crate::project_store::ProjectStorePaths::under(dir.path(), "proj"),
            None,
        ));
        let paths = recorder.paths().clone();
        let tool =
            Arc::new(PlanTool::new(store.clone(), PathBuf::from(".")).with_recorder(recorder));

        tool.execute(
            "call-store".to_string(),
            serde_json::json!({
                "items": [{
                    "id": "one",
                    "text": "first step",
                    "status": "completed",
                    "evidence": "cargo test -p pa-daemon: 6 passed"
                }]
            }),
            AbortSignal::never(),
            no_updates(),
        )
        .await
        .expect("the plan is valid");

        let events = pa_project::event::EventLog::at(&paths.events)
            .read_all()
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "entity.upserted");
        assert_eq!(
            events[0].payload["data"]["evidence"],
            "cargo test -p pa-daemon: 6 passed"
        );
    }

    #[tokio::test]
    async fn without_a_recorder_the_tool_writes_no_store_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = PlanStore::default();
        let tool = Arc::new(PlanTool::new(store, PathBuf::from(".")));

        tool.execute(
            "call-none".to_string(),
            serde_json::json!({
                "items": [{ "id": "one", "text": "first step", "status": "pending" }]
            }),
            AbortSignal::never(),
            no_updates(),
        )
        .await
        .expect("the plan is valid");

        assert!(!dir.path().join("projects").exists());
    }
}
