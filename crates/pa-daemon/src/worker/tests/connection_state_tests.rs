//! The attach snapshot's context-usage fill (TS `createAgentConnectionState`
//! parity): the state block carries `contextUsage` off the same store walk
//! `get_session_stats` serves, so the client's first frame reads the tray's
//! usage off the snapshot instead of blocking on a stats round-trip.
use super::*;
use crate::engine::{PromptRequest, SessionEngine};
use crate::worker::WorkerConfig;

/// The scripted harness engine with a resolved model context window: the
/// tray-usage fill's one input beyond the store.
struct WindowedEngine;
impl SessionEngine for WindowedEngine {
    fn run_prompt(
        &self,
        _: usize,
        _: PromptRequest,
        _: &dyn Fn() -> bool,
        _: &mut dyn FnMut(crate::engine::EngineEvent) -> bool,
    ) {
    }
    fn run_side_question(
        &self,
        request: crate::engine::SideQuestionRequest,
        signal: &pa_agent::abort::AbortSignal,
        sink: &pa_core::session_engine::side_question::SideQuestionSink,
    ) -> crate::engine::SideQuestionOutcome {
        crate::engine::ScriptedEngine::default().run_side_question(request, signal, sink)
    }
    fn run_compaction(
        &self,
        request: crate::engine::CompactionRequest,
        signal: &pa_agent::abort::AbortSignal,
    ) -> crate::engine::CompactionOutcome {
        crate::engine::ScriptedEngine::default().run_compaction(request, signal)
    }
    fn run_branch_summary(
        &self,
        request: crate::engine::BranchSummaryRequest,
        signal: &pa_agent::abort::AbortSignal,
    ) -> crate::engine::BranchSummaryOutcome {
        crate::engine::ScriptedEngine::default().run_branch_summary(request, signal)
    }
    fn rebuild_session_context(
        &self,
        _: Vec<pa_types::session::FileEntry>,
        _: pa_core::session_engine::goal_driver::GoalBranchReload,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn model_context_window(&self) -> Option<u64> {
        Some(128_000)
    }
}

/// The model resolver may finish with an old selection while a command
/// waits for core. The lock-side selection check retries the resolve instead
/// of pairing that old model with the post-switch core state.
struct SwitchingEngine {
    selection: std::sync::Mutex<String>,
    pause: std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}

impl SessionEngine for SwitchingEngine {
    fn run_prompt(
        &self,
        _: usize,
        _: PromptRequest,
        _: &dyn Fn() -> bool,
        _: &mut dyn FnMut(crate::engine::EngineEvent) -> bool,
    ) {
    }
    fn run_side_question(
        &self,
        request: crate::engine::SideQuestionRequest,
        signal: &pa_agent::abort::AbortSignal,
        sink: &pa_core::session_engine::side_question::SideQuestionSink,
    ) -> crate::engine::SideQuestionOutcome {
        WindowedEngine.run_side_question(request, signal, sink)
    }
    fn run_compaction(
        &self,
        request: crate::engine::CompactionRequest,
        signal: &pa_agent::abort::AbortSignal,
    ) -> crate::engine::CompactionOutcome {
        WindowedEngine.run_compaction(request, signal)
    }
    fn run_branch_summary(
        &self,
        request: crate::engine::BranchSummaryRequest,
        signal: &pa_agent::abort::AbortSignal,
    ) -> crate::engine::BranchSummaryOutcome {
        WindowedEngine.run_branch_summary(request, signal)
    }
    fn rebuild_session_context(
        &self,
        entries: Vec<pa_types::session::FileEntry>,
        reload: pa_core::session_engine::goal_driver::GoalBranchReload,
    ) -> anyhow::Result<()> {
        WindowedEngine.rebuild_session_context(entries, reload)
    }
    fn model_identity(&self) -> (Option<String>, Option<String>) {
        (
            Some("test".to_string()),
            Some(self.selection.lock().unwrap().clone()),
        )
    }
    fn model_metadata(&self) -> Option<Value> {
        let model = self.selection.lock().unwrap().clone();
        if let Some((entered, resume)) = self.pause.lock().unwrap().take() {
            entered.send(()).unwrap();
            resume.recv().unwrap();
        }
        Some(json!({ "provider": "test", "id": model }))
    }
    fn effective_thinking_level(&self) -> Option<String> {
        Some(self.selection.lock().unwrap().clone())
    }
    fn supported_thinking_levels(&self) -> Option<Vec<String>> {
        Some(vec![self.selection.lock().unwrap().clone()])
    }
    fn model_context_window(&self) -> Option<u64> {
        Some(if *self.selection.lock().unwrap() == "new" {
            200
        } else {
            100
        })
    }
}

#[tokio::test]
async fn summary_retry_resolves_the_selection_that_owns_the_locked_core() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let engine = std::sync::Arc::new(SwitchingEngine {
        selection: std::sync::Mutex::new("old".to_string()),
        pause: std::sync::Mutex::new(Some((entered_tx, resume_rx))),
    });
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::new(
        WorkerConfig {
            socket_path: dir.path().join("worker.sock"),
            supervisor_socket_path: PathBuf::new(),
            token: "switch-test".into(),
            worker_instance_id: String::new(),
            active_session_id: "switch-test".into(),
            agent_dir: dir.path().join("agent"),
            recovery_journal_path: dir.path().join("recovery.jsonl"),
            telemetry_disabled: None,
            script: Some(json!({ "responses": [] })),
        },
        None,
    );
    worker.engine = engine.clone();
    let worker = std::sync::Arc::new(worker);
    let core = worker.core.lock().unwrap();
    let reader = {
        let worker = std::sync::Arc::clone(&worker);
        std::thread::spawn(move || {
            let (core, inputs) = worker.summary_inputs();
            worker.summary_locked(&core, inputs)
        })
    };
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    *engine.selection.lock().unwrap() = "new".to_string();
    resume_tx.send(()).unwrap();
    drop(core);
    let summary = reader.join().unwrap();
    assert_eq!(
        summary.model,
        Some(json!({ "provider": "test", "id": "new" }))
    );
    assert_eq!(summary.thinking_level.as_deref(), Some("new"));
    let (core, inputs) = worker.connection_state_inputs();
    let state = Worker::connection_state_locked(&core, inputs);
    assert_eq!(
        state.model,
        Some(json!({ "provider": "test", "id": "new" }))
    );
    assert_eq!(state.available_thinking_levels, vec!["new".to_string()]);
    assert_eq!(state.context_usage, None);
}

/// FORK-ONLY: the attach state's `cacheUsage` is byte-identical to
/// `get_session_stats`'s, so the first frame's tray row and the post-turn refresh
/// can never disagree — and both read the same per-model fold `/cache` renders.
#[tokio::test]
async fn the_attach_state_carries_the_stats_cache_totals() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();
    let session_file = session_dir.join("cache-facts.jsonl");
    let lines = [
        json!({"type": "session", "version": 3, "id": "tray-cache",
               "timestamp": "2026-09-24T00:00:00.000Z", "cwd": "/tmp"}),
        // The per-model fold resolves every usage row to a model; without a model
        // timeline the summarizer's own usage has no bucket and the breakdown (and
        // with it the tray's cache rate) degrades to `None`.
        json!({"type": "model_change", "id": "mc1", "parentId": null,
               "timestamp": "2026-09-24T00:00:01.000Z",
               "provider": "anthropic", "modelId": "claude-opus-4-6"}),
        json!({"type": "message", "id": "u1", "parentId": "mc1",
               "timestamp": "2026-09-24T00:00:02.000Z",
               "message": {"role": "user", "content": "one"}}),
        json!({"type": "message", "id": "a1", "parentId": "u1",
               "timestamp": "2026-09-24T00:00:03.000Z",
               "message": {"role": "assistant", "provider": "anthropic",
                           "model": "claude-opus-4-6",
                           "content": [{"type": "text", "text": "hi"}],
                           "stopReason": "stop",
                           "usage": {"input": 1_000, "output": 200,
                                     "cacheRead": 9_000, "cacheWrite": 0,
                                     "totalTokens": 10_200,
                                     "cost": {"input": 0.005, "output": 0.0025,
                                              "cacheRead": 0.0045, "cacheWrite": 0.0,
                                              "total": 0.012}}}}),
    ];
    std::fs::write(
        &session_file,
        lines
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let worker = Worker::new(
        WorkerConfig {
            socket_path: dir.path().join("worker.sock"),
            supervisor_socket_path: PathBuf::new(),
            token: "tray-cache".into(),
            worker_instance_id: String::new(),
            active_session_id: "tray-cache-live".into(),
            agent_dir: dir.path().join("agent"),
            recovery_journal_path: dir.path().join("recovery.jsonl"),
            telemetry_disabled: None,
            script: Some(json!({"responses": []})),
        },
        None,
    );
    let created = worker
        .dispatch(
            "create",
            &json!({
                "sessionPath": session_file.display().to_string(),
                "cwd": "/tmp",
                "name": "tray cache",
            }),
        )
        .await;
    assert!(created.success, "create failed: {created:?}");
    let worker = std::sync::Arc::new(worker);
    let snapshot_state = {
        let (core, inputs) = worker.connection_state_inputs();
        let sealed = Worker::connection_state_locked(&core, inputs);
        serde_json::to_value(&sealed).unwrap()
    };
    assert_eq!(
        snapshot_state.get("cacheUsage"),
        Some(&json!({
            "input": 1_000, "cacheRead": 9_000, "cacheWrite": 0,
            "totalTokens": 10_200, "hitPercent": 90,
        })),
        "the attach state carries the cache totals: {snapshot_state}"
    );
    let stats = worker.dispatch("get_session_stats", &json!({})).await;
    assert!(stats.success, "{stats:?}");
    assert_eq!(
        stats.data.as_ref().unwrap().get("cacheUsage"),
        snapshot_state.get("cacheUsage"),
        "the snapshot fill equals the stats response"
    );
}

/// The attach snapshot's state carries the tray's context usage exactly as
/// `get_session_stats` serves it (TS `createAgentConnectionState`'s
/// `contextUsage: session.getContextUsage()`): the same walk, the same
/// store, so the snapshot-fed tray row is byte-identical to the row a
/// blocking stats fetch would have painted.
#[tokio::test]
async fn the_attach_state_carries_the_stats_context_usage() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();
    let mut worker = Worker::new(
        WorkerConfig {
            socket_path: dir.path().join("worker.sock"),
            supervisor_socket_path: PathBuf::new(),
            token: "context-usage-test".into(),
            worker_instance_id: String::new(),
            active_session_id: "ctx-usage".into(),
            agent_dir: dir.path().join("agent"),
            recovery_journal_path: dir.path().join("recovery.jsonl"),
            telemetry_disabled: None,
            script: Some(json!({"responses":[]})),
        },
        None,
    );
    worker.engine = std::sync::Arc::new(WindowedEngine);
    let created = worker
        .dispatch(
            "create",
            &json!({"cwd": "/tmp", "sessionDir": session_dir, "name": "usage"}),
        )
        .await;
    assert!(created.success, "create failed: {created:?}");

    let snapshot_state = {
        let (core, inputs) = worker.connection_state_inputs();
        let sealed = Worker::connection_state_locked(&core, inputs);
        serde_json::to_value(&sealed).unwrap()
    };
    assert_eq!(
        snapshot_state.get("contextUsage"),
        Some(&json!({"tokens": 0, "contextWindow": 128_000, "percent": 0.0})),
        "the attach state carries the tray usage: {snapshot_state}"
    );

    // The equivalence the byte-identity oracle rests on: the stats
    // response serves the very same usage object.
    let stats = worker.dispatch("get_session_stats", &json!({})).await;
    assert!(stats.success, "{stats:?}");
    assert_eq!(
        stats.data.as_ref().unwrap().get("contextUsage"),
        snapshot_state.get("contextUsage"),
        "the snapshot fill equals the stats response"
    );

    // A model-less engine keeps the field off the wire exactly like a TS
    // session without a model (the scripted default).
    let worker = Worker::new(
        WorkerConfig {
            socket_path: dir.path().join("worker2.sock"),
            supervisor_socket_path: PathBuf::new(),
            token: "context-usage-test-2".into(),
            worker_instance_id: String::new(),
            active_session_id: "ctx-usage-2".into(),
            agent_dir: dir.path().join("agent"),
            recovery_journal_path: dir.path().join("recovery2.jsonl"),
            telemetry_disabled: None,
            script: Some(json!({"responses":[]})),
        },
        None,
    );
    worker
        .dispatch(
            "create",
            &json!({"cwd": "/tmp", "sessionDir": session_dir, "name": "usage-2"}),
        )
        .await;
    let modelless = {
        let (core, inputs) = worker.connection_state_inputs();
        let sealed = Worker::connection_state_locked(&core, inputs);
        serde_json::to_value(&sealed).unwrap()
    };
    assert!(
        modelless.get("contextUsage").is_none(),
        "a model-less session omits the field: {modelless}"
    );
}
