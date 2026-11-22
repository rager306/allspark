//! The plan tool's durable record: a `update_plan` call must land a
//! `session_plan_state` row in the worker's session JSONL, not only in the
//! engine's in-memory session manager. A rebuilt worker replays the row to
//! restore the plan.
use super::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::session_store::SessionFile;

fn plan_worker(dir: &std::path::Path) -> std::sync::Arc<Worker> {
    std::sync::Arc::new(Worker::new(
        WorkerConfig {
            socket_path: dir.join("worker.sock"),
            supervisor_socket_path: std::path::PathBuf::new(),
            token: "token".to_string(),
            worker_instance_id: String::new(),
            active_session_id: "plan-durability-session".to_string(),
            agent_dir: dir.join("agent"),
            recovery_journal_path: dir.join("recovery.jsonl"),
            telemetry_disabled: None,
            script: Some(json!({
                "engine": "faux",
                "repeatLastResponse": true,
                "responses": [
                    {
                        "content": [{
                            "type": "toolCall",
                            "name": "update_plan",
                            "arguments": {
                                "items": [{
                                    "id": "one",
                                    "text": "durable step",
                                    "status": "in_progress"
                                }]
                            }
                        }]
                    },
                    "done"
                ]
            })),
        },
        None,
    ))
}

fn durable_plan(path: &std::path::Path) -> Option<Value> {
    let store = SessionFile::open(path).ok()?;
    store.entries().iter().rev().find_map(|entry| {
        let custom_type = entry.fields.get("customType").and_then(Value::as_str)?;
        (entry.type_ == "custom"
            && custom_type == pa_core::session_engine::plan::PLAN_STATE_CUSTOM_TYPE)
            .then(|| entry.fields.get("data").cloned())
            .flatten()
    })
}

#[cfg(unix)]
#[allow(clippy::await_holding_lock)] // the faux registry is process-global
#[tokio::test]
async fn a_plan_update_reaches_the_durable_session_file() {
    let _faux = crate::agent_engine::tests::FAUX_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = std::env::temp_dir().join(format!("pa-plan-durable-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let worker = plan_worker(&dir);
    let created = worker
        .dispatch(
            "create",
            &json!({
                "cwd": dir.to_string_lossy(),
                "sessionDir": dir.join("sessions").to_string_lossy(),
            }),
        )
        .await;
    assert!(created.success, "create failed: {created:?}");
    let session_file = PathBuf::from(
        created.data.expect("summary")["sessionFile"]
            .as_str()
            .expect("session file"),
    );
    let prompted = worker
        .dispatch(
            "prompt_and_wait",
            &json!({
                "activeSessionId": "plan-durability-session",
                "message": "record the plan",
            }),
        )
        .await;
    assert!(prompted.success, "the prompt failed: {prompted:?}");

    let deadline = Instant::now() + Duration::from_secs(5);
    let plan = loop {
        if let Some(plan) = durable_plan(&session_file) {
            break plan;
        }
        assert!(
            Instant::now() < deadline,
            "no durable session_plan_state row in {}",
            session_file.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(plan["items"][0]["id"], "one");
    assert_eq!(plan["items"][0]["text"], "durable step");
    assert_eq!(plan["items"][0]["status"], "in_progress");
}
