//! The scheduled-jobs unit battery: the queue hooks, the fire/defer decision,
//! and the nine protocol arms' wire shapes.
use super::*;
use crate::worker::Worker;
use std::sync::Arc;

fn persisted_worker_config(dir: &std::path::Path) -> crate::worker::WorkerConfig {
    crate::worker::WorkerConfig {
        socket_path: dir.join("worker.sock"),
        supervisor_socket_path: std::path::PathBuf::new(),
        token: "token".to_string(),
        worker_instance_id: String::new(),
        active_session_id: "hb-fire-session".to_string(),
        agent_dir: dir.join("agent"),
        recovery_journal_path: dir.join("recovery.jsonl"),
        telemetry_disabled: None,
        script: Some(serde_json::json!({ "responses": ["ack", "ack", "ack", "ack"] })),
    }
}

/// The fire-chain e2e behind the dogfood P0 (a kernel-created heartbeat never
/// fired): the mutation hook re-arms the scheduler, fires the job, and records the run.
fn heartbeat_job(
    id: &str,
    prompt: &str,
    delivery_mode: DeliveryMode,
    session: &(String, std::path::PathBuf),
) -> AgentCronJob {
    AgentCronJob {
        id: id.to_string(),
        status: JobStatus::Active,
        source: Some("rlm_heartbeat".to_string()),
        runtime_kind: None,
        delivery_mode: Some(delivery_mode),
        active_session_id: session.0.clone(),
        session_id: session.0.clone(),
        session_file: session.1.to_string_lossy().to_string(),
        cwd: "/w".to_string(),
        label: None,
        prompt: prompt.to_string(),
        schedule: pa_core::cron::AgentCronSchedule {
            kind: pa_core::cron::ScheduleKind::Interval,
            expression: "every 10s".to_string(),
            interval_ms: Some(10_000),
        },
        created_at: "2026-09-22T00:00:00.000Z".to_string(),
        updated_at: "2026-09-22T00:00:00.000Z".to_string(),
        next_run_at: None,
        last_run_at: None,
        last_skipped_at: None,
        last_error: None,
        run_count: 0,
    }
}

/// A persisted, `active`-state session file the fire's target
/// verification (TS `isPersistedCronJobRunnable`) reads.
fn write_active_session(dir: &std::path::Path) -> (String, std::path::PathBuf) {
    let mut session = crate::session_store::SessionFile::create("/w", None, 0);
    session.append_message(&serde_json::json!({
        "role": "user", "content": "hi", "timestamp": 1u64
    }));
    let path = dir.join(crate::session_store::session_file_name(
        session.session_id(),
    ));
    session.set_path(path.clone());
    let _ = session.append_session_state("active");
    session.rewrite().unwrap();
    (session.session_id().to_string(), path)
}

#[tokio::test]
async fn a_fire_at_a_killed_session_cancels_and_skips() {
    let dir = std::env::temp_dir().join(format!("pa-sched-dead-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (session_id, session_file) = write_active_session(&dir);
    let store = AgentCronJobStore::for_session_artifacts();
    let artifact_dir = session_artifact_dir(&session_file, &session_id).unwrap();
    std::fs::create_dir_all(&artifact_dir).unwrap();
    store.register_session_artifact(&session_id, &artifact_dir);
    let job = store
        .create(&CreateAgentCronJobInput {
            active_session_id: session_id.clone(),
            session_id: session_id.clone(),
            session_file: session_file.to_string_lossy().to_string(),
            cwd: "/w".to_string(),
            prompt: "lane-liveness ping".to_string(),
            schedule_text: "every 10s".to_string(),
            now: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(job.status, JobStatus::Active);

    // The session is killed: the close appended the `archived` state.
    let mut session = crate::session_store::SessionFile::open(&session_file).unwrap();
    let _ = session.append_session_state("archived");
    session.rewrite().unwrap();

    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let hooks = QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::new(std::sync::Mutex::new(None)),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    // The dead-target cancel registers the artifact partition itself (a fresh store knows
    // nothing of the session yet).
    hooks
        .store
        .register_session_artifact(&session_id, &artifact_dir);

    let verdict = AgentCronSchedulerHooks::run_job(&hooks, &job)
        .await
        .unwrap();
    assert_eq!(verdict, Some("skipped"));
    let stored = hooks.store.list();
    let cancelled = stored
        .iter()
        .find(|candidate| candidate.id == job.id)
        .expect("the job stays in the store");
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert_eq!(cancelled.next_run_at, None);
    let core = core.lock().unwrap();
    assert!(
        core.steering.is_empty() && core.follow_up.is_empty(),
        "no fire parks at a killed session"
    );
}

/// A single unreadable read must not cancel the session's schedule (review #13).
///
/// `read_session_info` answers `None` for a deleted file AND for a file it could not open,
/// scan, or parse. The fire path treats `None` as "the target is gone" and PERSISTS a
/// cancellation of every job for the session — so one read that raced a rewrite deleted a
/// live schedule. The three answers must stay distinct: gone cancels, unreadable fails the
/// fire (the scheduler backs off and re-reads), live fires.
#[tokio::test]
async fn an_unreadable_session_file_does_not_cancel_the_schedule() {
    let dir = std::env::temp_dir().join(format!("pa-sched-torn-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (session_id, session_file) = write_active_session(&dir);
    let store = AgentCronJobStore::for_session_artifacts();
    let artifact_dir = session_artifact_dir(&session_file, &session_id).unwrap();
    std::fs::create_dir_all(&artifact_dir).unwrap();
    store.register_session_artifact(&session_id, &artifact_dir);
    let job = store
        .create(&CreateAgentCronJobInput {
            active_session_id: session_id.clone(),
            session_id: session_id.clone(),
            session_file: session_file.to_string_lossy().to_string(),
            cwd: "/w".to_string(),
            prompt: "reader-hiccup ping".to_string(),
            schedule_text: "every 10s".to_string(),
            now: Some(1),
            ..Default::default()
        })
        .unwrap();

    // The file still EXISTS but its first line no longer parses: exactly what a read racing a
    // rewrite (or a torn append) looks like, and what `read_session_info` cannot tell apart
    // from a deleted session.
    std::fs::write(&session_file, "{not a session file}\n").unwrap();

    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let hooks = QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::new(std::sync::Mutex::new(None)),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    hooks
        .store
        .register_session_artifact(&session_id, &artifact_dir);

    let verdict = AgentCronSchedulerHooks::run_job(&hooks, &job).await;
    assert!(
        verdict.is_err(),
        "an unreadable target is an indeterminate read, not a dead session: {verdict:?}"
    );
    let stored = hooks.store.list();
    let row = stored
        .iter()
        .find(|candidate| candidate.id == job.id)
        .expect("the job stays in the store");
    assert_eq!(
        row.status,
        JobStatus::Active,
        "a failed read must leave the schedule armed for the next fire"
    );
    let core = core.lock().unwrap();
    assert!(
        core.steering.is_empty() && core.follow_up.is_empty(),
        "nothing was fired at a target it never read"
    );
}

/// A session that records NO lifecycle state is not a dead session (review #13,
/// residual).
///
/// `read_session_info` answers `state: None` for a session file written before the
/// `session_state` entry existed — and for a state row this build cannot read. The fire
/// path collapsed every readable-but-not-`active` row into `Gone` and PERSISTED a
/// cancellation of the session's whole schedule, so the first fire of an older session's
/// schedule deleted it for good. Absence is not deletion: the three answers stay
/// distinct, and only a recorded end (`archived`/`crash`) may cancel.
#[tokio::test]
async fn a_session_without_a_recorded_state_does_not_cancel_the_schedule() {
    let dir = std::env::temp_dir().join(format!("pa-sched-nostate-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    // A pre-state-row session: the header and a message, and no `session_state` entry.
    let mut session = crate::session_store::SessionFile::create("/w", None, 0);
    session.append_message(&serde_json::json!({
        "role": "user", "content": "hi", "timestamp": 1u64
    }));
    let session_file = dir.join(crate::session_store::session_file_name(
        session.session_id(),
    ));
    session.set_path(session_file.clone());
    session.rewrite().unwrap();
    let session_id = session.session_id().to_string();
    let info = crate::session_store::read_session_info(&session_file).expect("the file folds");
    assert_eq!(info.state, None, "the fixture records no state");

    let store = AgentCronJobStore::for_session_artifacts();
    let artifact_dir = session_artifact_dir(&session_file, &session_id).unwrap();
    std::fs::create_dir_all(&artifact_dir).unwrap();
    store.register_session_artifact(&session_id, &artifact_dir);
    let job = store
        .create(&CreateAgentCronJobInput {
            active_session_id: session_id.clone(),
            session_id: session_id.clone(),
            session_file: session_file.to_string_lossy().to_string(),
            cwd: "/w".to_string(),
            prompt: "pre-state ping".to_string(),
            schedule_text: "every 10s".to_string(),
            now: Some(1),
            ..Default::default()
        })
        .unwrap();

    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let hooks = QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::new(std::sync::Mutex::new(None)),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    hooks
        .store
        .register_session_artifact(&session_id, &artifact_dir);

    let verdict = AgentCronSchedulerHooks::run_job(&hooks, &job).await;
    assert!(
        verdict.is_err(),
        "a missing state is not a confirmed end: {verdict:?}"
    );
    let stored = hooks.store.list();
    let row = stored
        .iter()
        .find(|candidate| candidate.id == job.id)
        .expect("the job stays in the store");
    assert_eq!(
        row.status,
        JobStatus::Active,
        "a session that never recorded an end must keep its schedule"
    );
    let core = core.lock().unwrap();
    assert!(
        core.steering.is_empty() && core.follow_up.is_empty(),
        "nothing was fired at a target the fire could not classify"
    );
}

/// A session that recorded its own end still cancels (the other side of the
/// three-valued decision).
#[tokio::test]
async fn a_session_that_recorded_its_end_cancels_the_schedule() {
    let dir = std::env::temp_dir().join(format!("pa-sched-crash-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (session_id, session_file) = write_active_session(&dir);
    let store = AgentCronJobStore::for_session_artifacts();
    let artifact_dir = session_artifact_dir(&session_file, &session_id).unwrap();
    std::fs::create_dir_all(&artifact_dir).unwrap();
    store.register_session_artifact(&session_id, &artifact_dir);
    let job = store
        .create(&CreateAgentCronJobInput {
            active_session_id: session_id.clone(),
            session_id: session_id.clone(),
            session_file: session_file.to_string_lossy().to_string(),
            cwd: "/w".to_string(),
            prompt: "post-crash ping".to_string(),
            schedule_text: "every 10s".to_string(),
            now: Some(1),
            ..Default::default()
        })
        .unwrap();

    // The supervisor's crash record, not a kill: the second confirmed end.
    let mut session = crate::session_store::SessionFile::open(&session_file).unwrap();
    let _ = session.append_session_state("crash");
    session.rewrite().unwrap();

    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let hooks = QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::new(std::sync::Mutex::new(None)),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    hooks
        .store
        .register_session_artifact(&session_id, &artifact_dir);
    assert_eq!(
        AgentCronSchedulerHooks::run_job(&hooks, &job)
            .await
            .unwrap(),
        Some("skipped")
    );
    let stored = hooks.store.list();
    let row = stored
        .iter()
        .find(|candidate| candidate.id == job.id)
        .expect("the job stays in the store");
    assert_eq!(row.status, JobStatus::Cancelled);
}

/// A state row this build cannot read is "unknown", not a recorded end (review #13,
/// residual).
///
/// Another build's `session_state` row — or a truncated one — folds to `state: None`
/// (`fold_scan_entry` keeps the last VALID status, and there is none). Treating that as
/// `Gone` persisted a cancellation of the session's whole schedule.
#[tokio::test]
async fn an_unclassifiable_state_row_does_not_cancel_the_schedule() {
    let dir = std::env::temp_dir().join(format!("pa-sched-badstate-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = crate::session_store::SessionFile::create("/w", None, 0);
    session.append_message(&serde_json::json!({
        "role": "user", "content": "hi", "timestamp": 1u64
    }));
    let session_file = dir.join(crate::session_store::session_file_name(
        session.session_id(),
    ));
    session.set_path(session_file.clone());
    session.rewrite().unwrap();
    // A state row from a build whose status is not a string: valid JSON, unreadable status.
    let mut raw = std::fs::read_to_string(&session_file).unwrap();
    raw.push_str("{\"type\":\"session_state\",\"id\":\"bad\",\"timestamp\":\"t\",\"state\":{\"status\":42}}\n");
    std::fs::write(&session_file, raw).unwrap();
    let session_id = session.session_id().to_string();
    let info = crate::session_store::read_session_info(&session_file).expect("the file folds");
    assert_eq!(info.state, None, "the fixture's only state is unreadable");

    let store = AgentCronJobStore::for_session_artifacts();
    let artifact_dir = session_artifact_dir(&session_file, &session_id).unwrap();
    std::fs::create_dir_all(&artifact_dir).unwrap();
    store.register_session_artifact(&session_id, &artifact_dir);
    let job = store
        .create(&CreateAgentCronJobInput {
            active_session_id: session_id.clone(),
            session_id: session_id.clone(),
            session_file: session_file.to_string_lossy().to_string(),
            cwd: "/w".to_string(),
            prompt: "bad-state ping".to_string(),
            schedule_text: "every 10s".to_string(),
            now: Some(1),
            ..Default::default()
        })
        .unwrap();

    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let hooks = QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::new(std::sync::Mutex::new(None)),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    hooks
        .store
        .register_session_artifact(&session_id, &artifact_dir);

    assert!(
        AgentCronSchedulerHooks::run_job(&hooks, &job)
            .await
            .is_err(),
        "an unreadable state row is an indeterminate read"
    );
    let stored = hooks.store.list();
    let row = stored
        .iter()
        .find(|candidate| candidate.id == job.id)
        .expect("the job stays in the store");
    assert_eq!(row.status, JobStatus::Active);
}

/// A plain cron fire's queue key is its DISPATCH id, not a process counter (review #6).
///
/// The counter restarted at zero in every worker life, so a respawned worker minted a key an
/// earlier life had already used; admission replaces on a matching key, so that life's
/// restored fire was removed by the new one. The dispatch record is durable, so its id is the
/// only half of the key that cannot collide across lives: two independent hook instances —
/// the closest thing to two worker lives here — produce the SAME key for the same claim.
#[tokio::test]
async fn a_plain_fire_is_keyed_by_its_dispatch_identity() {
    let dir = std::env::temp_dir().join(format!("pa-sched-key-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let session = write_active_session(&dir);
    let job = AgentCronJob {
        source: Some("cron".to_string()),
        ..heartbeat_job("cron-key", "nightly sweep", DeliveryMode::Steer, &session)
    };

    // Park one fire through each of two independent hook instances, both serving the same
    // claimed dispatch.
    let mut keys = Vec::new();
    for _ in 0..2 {
        let core = Arc::new(std::sync::Mutex::new(
            crate::worker::SessionCore::test_core(None, "/w".to_string()),
        ));
        let hooks = Arc::new(QueueHooks {
            core: Arc::clone(&core),
            work_notify: Arc::new(Notify::new()),
            user_bash: Arc::new(crate::user_bash::UserBash::new()),
            store: Arc::new(AgentCronJobStore::for_session_artifacts()),
            recovery: Arc::new(std::sync::Mutex::new(None)),
            dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        });
        let dispatch = pa_core::cron::store::AgentCronDispatch {
            id: "dispatch-durable-identity".to_string(),
            job: job.clone(),
        };
        let _end = AgentCronSchedulerHooks::begin_dispatch(&*hooks, &dispatch);
        let spawned_job = job.clone();
        let run = tokio::spawn(async move {
            pa_core::cron::scheduler::AgentCronSchedulerHooks::run_job(&*hooks, &spawned_job).await
        });
        let park_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let parked = {
                let mut core = core.lock().unwrap();
                core.steering
                    .pop_front()
                    .or_else(|| core.follow_up.pop_front())
            };
            if let Some(item) = parked {
                keys.push(item.queue_key.expect("a plain fire carries a key"));
                break;
            }
            assert!(std::time::Instant::now() < park_deadline, "never parked");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // Releasing the settle so the spawned fire does not outlive the test.
        let _ = run.await;
    }
    assert_eq!(
        keys[0], keys[1],
        "the key must come from the durable dispatch, not a per-process counter"
    );
    assert_eq!(
        keys[0], "cron:cron-key:dispatch-durable-identity",
        "{keys:?}"
    );
}

/// A claim re-armed after a crash does not enqueue a second copy of a fire that survived
/// (review #6).
///
/// `recover_interrupted_in_state` re-arms the claim, and the journal also restores the lane
/// the previous life had durably queued. Firing again without looking is exactly the
/// duplicate the finding names: the same wake runs twice. The job's interruption marker plus
/// a queued `cron:<job>:` row is the evidence that the handoff already happened. A PLAIN cron
/// job on purpose: a heartbeat has its own deferral rule and would pass this test by
/// accident.
#[tokio::test]
async fn a_rearmed_claim_whose_fire_survived_is_not_enqueued_twice() {
    let dir = std::env::temp_dir().join(format!("pa-sched-replay-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let session = write_active_session(&dir);
    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let hooks = QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::new(std::sync::Mutex::new(None)),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    let mut job = AgentCronJob {
        source: Some("cron".to_string()),
        ..heartbeat_job(
            "cron-replay",
            "resume the goal",
            DeliveryMode::Steer,
            &session,
        )
    };
    // The producer's own constant, not a copied literal: a rename on either side would
    // otherwise turn the replay guard off silently (review #6, residual).
    job.last_error = Some(pa_core::cron::store::INTERRUPTED_FIRE_MARKER.to_string());
    // The row the previous worker life durably queued, restored onto the lane.
    {
        let mut core = core.lock().unwrap();
        core.follow_up.push_back(crate::worker::QueuedItem {
            priority: crate::worker::QueuePriority::Background,
            message: "resume the goal".to_string(),
            preview: None,
            custom_message: None,
            agent_message: None,
            admission_id: None,
            images: Vec::new(),
            queue_key: Some("cron:cron-replay:dispatch-from-the-previous-life".to_string()),
            done: None,
            queue_visible: true,
            policy: crate::worker::TurnPolicy::Injected,
            forced_batch: false,
        });
    }

    let verdict = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        AgentCronSchedulerHooks::run_job(&hooks, &job),
    )
    .await
    .expect("a replay must answer at once, not park a second copy and wait for its settle")
    .expect("the skip is not an error");
    assert_eq!(
        verdict,
        Some("skipped"),
        "the surviving fire is the delivery"
    );
    let core = core.lock().unwrap();
    assert_eq!(
        core.follow_up.len(),
        1,
        "the restored fire must stay the only copy"
    );
}

/// A settle timeout must not report live work as skipped (review #12).
///
/// The window is 15 minutes, so the decision — not the wait — is what the test drives. A fire
/// still PARKED never ran: it is withdrawn (it must not execute later behind the scheduler's
/// back) and reported as a skip. A fire the runner already picked up is executing: reporting a
/// skip would have `record_dispatch_result` complete a running one-shot and stamp a recurring
/// job's `lastSkippedAt`. A running ONE-SHOT records a clean run (its wake is delivered); a
/// running RECURRING job records an unknown outcome and backs off.
#[tokio::test]
async fn a_settle_timeout_never_reports_live_work_as_skipped() {
    let dir = std::env::temp_dir().join(format!("pa-sched-timeout-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let session = write_active_session(&dir);
    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let hooks = QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::new(std::sync::Mutex::new(None)),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    let job = heartbeat_job("hb-timeout", "still going", DeliveryMode::Steer, &session);
    let key = format!("heartbeat:{}", job.id);

    // The runner already gathered the fire: nothing is parked under its key. This is a
    // RECURRING job, so the schedule must not record a clean run for work whose end this
    // path cannot see (review #12, residual).
    let error = hooks
        .on_settle_timeout(&job, Some(key.as_str()))
        .expect_err("a running recurring fire has an UNKNOWN outcome, not a clean run");
    assert!(
        error.to_string().contains("outcome is unknown"),
        "the recorded error must say what is unknown: {error}"
    );

    // A running ONE-SHOT is different: its wake is durably queued, so completing it is
    // correct and the turn owns the outcome.
    let mut once = job.clone();
    once.schedule.kind = pa_core::cron::ScheduleKind::Once;
    assert!(
        hooks
            .on_settle_timeout(&once, Some(key.as_str()))
            .expect("a delivered one-shot wake is not an error")
            .is_none(),
        "a running one-shot is a delivered wake, not a skip and not a failure"
    );

    // The fire is still parked: it never ran.
    {
        let mut core = core.lock().unwrap();
        let mut item = heartbeat_job("hb-timeout", "still going", DeliveryMode::Steer, &session);
        item.id = job.id.clone();
        core.follow_up.push_back(crate::worker::QueuedItem {
            priority: crate::worker::QueuePriority::Background,
            message: "still going".to_string(),
            preview: None,
            custom_message: None,
            agent_message: None,
            admission_id: None,
            images: Vec::new(),
            queue_key: Some(key.clone()),
            done: None,
            queue_visible: true,
            policy: crate::worker::TurnPolicy::Injected,
            forced_batch: false,
        });
    }
    assert_eq!(
        hooks
            .on_settle_timeout(&job, Some(key.as_str()))
            .expect("no error"),
        Some("skipped"),
        "a fire that never left the lane is a skip"
    );
    let core = core.lock().unwrap();
    assert!(
        core.steering.is_empty() && core.follow_up.is_empty(),
        "the withdrawn fire must not execute later behind the scheduler's back"
    );
}

/// A withdrawn fire is checkpointed, not only removed from memory (review #12, residual).
///
/// The parked row is part of the journal's queue snapshot, so a crash between the withdrawal
/// and the next checkpoint restored it on respawn: the fire the scheduler had already
/// recorded as skipped (completing its one-shot) would then execute behind its back.
#[tokio::test]
async fn withdrawing_a_parked_fire_checkpoints_the_removal() {
    let dir = std::env::temp_dir().join(format!("pa-sched-wd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let session = write_active_session(&dir);
    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let active_session_id = core.lock().unwrap().active_session_id.clone();
    let journal = {
        let mut journal =
            crate::journal::WorkerRecoveryJournal::open(&dir.join("recovery.jsonl")).unwrap();
        // The snapshot BEFORE the withdrawal: this is what a respawn would restore.
        journal
            .record_queue_snapshot(
                &active_session_id,
                &[crate::journal::WorkerQueueItemRecord {
                    message: "still going".to_string(),
                    priority: None,
                    preview: None,
                    custom_message: None,
                    queue_key: Some("heartbeat:hb-wd".to_string()),
                    queue_visible: true,
                    policy: "injected".to_string(),
                    forced_batch: false,
                    images: Vec::new(),
                }],
                &[],
                false,
            )
            .unwrap();
        journal
    };
    let journal = Arc::new(std::sync::Mutex::new(Some(journal)));
    let hooks = QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::clone(&journal),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    let job = heartbeat_job("hb-wd", "still going", DeliveryMode::Steer, &session);
    let key = format!("heartbeat:{}", job.id);
    {
        let mut core = core.lock().unwrap();
        core.follow_up.push_back(crate::worker::QueuedItem {
            priority: crate::worker::QueuePriority::Background,
            message: "still going".to_string(),
            preview: None,
            custom_message: None,
            agent_message: None,
            admission_id: None,
            images: Vec::new(),
            queue_key: Some(key.clone()),
            done: None,
            queue_visible: true,
            policy: crate::worker::TurnPolicy::Injected,
            forced_batch: false,
        });
    }

    assert_eq!(
        hooks
            .on_settle_timeout(&job, Some(key.as_str()))
            .expect("no error"),
        Some("skipped")
    );
    let after = journal
        .lock()
        .unwrap()
        .as_ref()
        .expect("the journal stays open")
        .latest_queue_snapshot(&active_session_id)
        .expect("the checkpoint wrote a snapshot");
    let keys: Vec<&str> = after
        .0
        .iter()
        .chain(after.1.iter())
        .filter_map(|item| item.queue_key.as_deref())
        .collect();
    assert!(
        keys.is_empty(),
        "a respawn would restore the withdrawn fire from the journal: {keys:?}"
    );
}

/// `run_job`'s settle-timeout arm must delegate that decision, not answer a skip itself.
///
/// The classifier above is testable; whether the fire path calls it is a separate fact, and
/// the finding is precisely that the arm answered `skipped` unconditionally. The wait is 15
/// minutes, so the call site is pinned rather than driven — the same mechanism pin the
/// transport owner checks use.
#[test]
fn the_settle_timeout_arm_consults_the_queue_position() {
    let source = include_str!("../scheduled_jobs.rs");
    let start = source
        .find("pub(crate) struct QueueHooks")
        .expect("the hooks exist");
    let end = source[start..]
        .find("/// The worker's schedule catalog")
        .map(|at| start + at)
        .expect("the hooks end");
    let body = &source[start..end];
    let arm = &body[body
        .find("Err(_) =>")
        .expect("the settle-timeout arm exists")..];
    let arm = &arm[..arm.find('\n').unwrap_or(arm.len())];
    assert!(
        arm.contains("on_settle_timeout"),
        "a settle timeout must ask where the fire is before calling it a skip: {arm}"
    );
}

/// A heartbeat parks as the injected `heartbeat_prompt` row with the TS preview (the
/// queue strip reads `Heartbeat prompt: <content>`, the turn text keeps the raw content).
#[tokio::test]
async fn heartbeat_fire_parks_the_labeled_preview_on_its_lane() {
    let dir = std::env::temp_dir().join(format!("pa-sched-fire-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let session = write_active_session(&dir);
    let core = Arc::new(std::sync::Mutex::new(
        crate::worker::SessionCore::test_core(None, "/w".to_string()),
    ));
    let hooks = Arc::new(QueueHooks {
        core: Arc::clone(&core),
        work_notify: Arc::new(Notify::new()),
        user_bash: Arc::new(crate::user_bash::UserBash::new()),
        store: Arc::new(AgentCronJobStore::for_session_artifacts()),
        recovery: Arc::new(std::sync::Mutex::new(None)),
        dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    });
    let steer_heartbeat = heartbeat_job("hb-1", "steer the mission", DeliveryMode::Steer, &session);
    let follow_up_heartbeat = heartbeat_job(
        "hb-2",
        "wrap the mission up",
        DeliveryMode::FollowUp,
        &session,
    );
    let plain_cron = AgentCronJob {
        source: Some("cron".to_string()),
        ..heartbeat_job("cron-1", "nightly sweep", DeliveryMode::Steer, &session)
    };
    for job in [&steer_heartbeat, &follow_up_heartbeat, &plain_cron] {
        let hooks = Arc::clone(&hooks);
        let spawned_job = job.clone();
        let run = tokio::spawn(async move {
            pa_core::cron::scheduler::AgentCronSchedulerHooks::run_job(&*hooks, &spawned_job).await
        });
        // The spawned fire parks its item before its settle wait; the runner is absent,
        // so the item stays parked until this test pops it (releasing the settle).
        let park_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (lane, item) = loop {
            let popped = {
                let mut core = core.lock().unwrap();
                core.steering
                    .pop_front()
                    .map(|item| ("steering", item))
                    .or_else(|| core.follow_up.pop_front().map(|item| ("follow_up", item)))
            };
            if let Some(popped) = popped {
                break popped;
            }
            assert!(
                std::time::Instant::now() < park_deadline,
                "the fire for {} never parked",
                job.id
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let content = item.message.clone();
        if is_heartbeat_cron_job(job) {
            assert_eq!(
                content,
                format!("[heartbeat: every 10s run#0]\n\n{}", job.prompt)
            );
            assert_eq!(
                item.preview.as_deref(),
                Some(
                    format!(
                        "{}: {content}",
                        pa_core::session_engine::messages::HEARTBEAT_PROMPT_PREVIEW_LABEL
                    )
                    .as_str()
                ),
                "the parked row must carry the labeled preview"
            );
            assert_eq!(
                item.queue_key.as_deref(),
                Some(format!("heartbeat:{}", job.id).as_str())
            );
            assert_eq!(
                item.custom_message
                    .as_ref()
                    .and_then(|row| row.get("customType"))
                    .and_then(Value::as_str),
                Some(pa_core::session_engine::messages::HEARTBEAT_PROMPT_CUSTOM_TYPE)
            );
            assert_eq!(
                lane,
                if job.delivery_mode == Some(DeliveryMode::Steer) {
                    "steering"
                } else {
                    "follow_up"
                }
            );
        } else {
            assert_eq!(content, "nightly sweep");
            assert_eq!(item.preview, None);
            assert_eq!(item.custom_message, None);
            // A plain cron fire carries a key unique to THIS dispatch: it identifies the fire
            // for the settle timeout without replacing a queued fire the way a shared key
            // would (successive plain fires stack, as they always did).
            let key = item
                .queue_key
                .clone()
                .expect("a plain cron fire carries a key");
            assert!(key.starts_with("cron:cron-1:"), "{key}");
            assert_eq!(lane, "follow_up");
        }
        drop(item);
        let outcome = run.await.unwrap().expect("run_job");
        assert_eq!(outcome, None);
    }
}

/// The failure propagation behind the backoff (dogfood incident: a failing
/// heartbeat re-fired ~120x): a real error settles as a run, abort/withdrawal
/// as a clean run and a skip.
#[tokio::test]
async fn settles_classify_ran_failed_or_skipped() {
    // Park one fire, settle it with `settle`, and return the verdict.
    async fn settle_one(
        session: &(String, std::path::PathBuf),
        settle: crate::worker::TurnSettle,
    ) -> anyhow::Result<Option<&'static str>> {
        let core = Arc::new(std::sync::Mutex::new(
            crate::worker::SessionCore::test_core(None, "/w".to_string()),
        ));
        let hooks = Arc::new(QueueHooks {
            core: Arc::clone(&core),
            work_notify: Arc::new(Notify::new()),
            user_bash: Arc::new(crate::user_bash::UserBash::new()),
            store: Arc::new(AgentCronJobStore::for_session_artifacts()),
            recovery: Arc::new(std::sync::Mutex::new(None)),
            dispatch_ids: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        });
        let job = heartbeat_job("hb-1", "steer the mission", DeliveryMode::Steer, session);
        let hooks_for_run = Arc::clone(&hooks);
        let spawned_job = job.clone();
        let run = tokio::spawn(async move {
            pa_core::cron::scheduler::AgentCronSchedulerHooks::run_job(
                &*hooks_for_run,
                &spawned_job,
            )
            .await
        });
        let park_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let popped = {
                let mut core = core.lock().unwrap();
                core.steering
                    .pop_front()
                    .or_else(|| core.follow_up.pop_front())
            };
            if let Some(item) = popped {
                let done = item.done.expect("a fire settles through done");
                let _ = done.send(settle.clone());
                break;
            }
            assert!(std::time::Instant::now() < park_deadline, "never parked");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        run.await.expect("run task")
    }

    let dir = std::env::temp_dir().join(format!("pa-sched-fail-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let session = write_active_session(&dir);

    // A provider failure surfaces: the scheduler records it and backs off (the incident's
    // 404).
    let failure = "404 No endpoints found that support tool use.".to_string();
    let error = settle_one(&session, crate::worker::TurnSettle::Failed(failure.clone()))
        .await
        .expect_err("the failed settle surfaces");
    assert_eq!(error.to_string(), failure);
    // A provider failure whose text happens to equal the abort wire text still fails
    // (the typed settle never reads the text).
    let sneaky = settle_one(
        &session,
        crate::worker::TurnSettle::Failed(crate::worker::ABORTED_TURN_SETTLE_ERROR.to_string()),
    )
    .await
    .expect_err("provider text cannot masquerade as an abort");
    assert_eq!(sneaky.to_string(), crate::worker::ABORTED_TURN_SETTLE_ERROR);
    // An aborted turn is a clean run.
    assert_eq!(
        settle_one(&session, crate::worker::TurnSettle::Aborted)
            .await
            .expect("aborted turn"),
        None
    );
    // A withdrawn fire (queue edit delete, abort cancel) skips.
    assert_eq!(
        settle_one(
            &session,
            crate::worker::TurnSettle::Withdrawn(crate::worker::QUEUED_PROMPT_DELETED.to_string()),
        )
        .await
        .expect("withdrawn fire"),
        Some("skipped")
    );
    assert_eq!(
        settle_one(
            &session,
            crate::worker::TurnSettle::Withdrawn(
                crate::worker::PROMPT_ABORTED_BEFORE_DELIVERY.to_string(),
            ),
        )
        .await
        .expect("abort-cancelled fire"),
        Some("skipped")
    );
}

#[tokio::test]
async fn rlm_heartbeat_mutation_hook_fires_into_the_session_queue() {
    let dir = std::env::temp_dir().join(format!("pa-hb-fire-{}", uuid::Uuid::new_v4()));
    let sessions_dir = dir.join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let worker = Arc::new(Worker::new(persisted_worker_config(&dir), None));
    let created = worker
        .dispatch(
            "create",
            &json!({
                "cwd": dir.to_string_lossy(),
                "sessionDir": sessions_dir.to_string_lossy(),
            }),
        )
        .await;
    assert!(created.success, "create failed: {created:?}");

    // The kernel host handler's store mutation, the same live binding the
    // engine's kernel cron wiring binds: `rlm_heartbeat.create`.
    let job = {
        let core = worker
            .core
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (binding, _) = live_binding(&core).expect("the created session is persisted");
        worker
            .scheduled
            .store()
            .create_rlm_heartbeat(&CreateAgentCronJobInput {
                active_session_id: binding.active_session_id,
                session_id: binding.session_id,
                session_file: binding.session_file,
                cwd: binding.cwd,
                source: Some("rlm_heartbeat".to_string()),
                prompt: "print hello world".to_string(),
                schedule_text: "every 10s".to_string(),
                delivery_mode: Some(DeliveryMode::Steer),
                ..Default::default()
            })
            .expect("rlm heartbeat create")
    };
    assert_eq!(job.status, JobStatus::Active);

    // The mutation hook the worker installs: withdraws dropped queued
    // fires, then re-arms the scheduler.
    let hook = worker.scheduled.mutation_hook();
    hook(
        pa_core::session_engine::host_requests::RlmHeartbeatMutation {
            job: job.clone(),
            drop_queued: false,
        },
    )
    .await;

    // The re-armed timer fires within the interval: the job's prompt lands on the steer
    // lane, the turn runs, and the store records the run (`runCount` + `lastRunAt`).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    loop {
        let recorded = worker
            .scheduled
            .store()
            .list()
            .into_iter()
            .find(|listed| listed.id == job.id);
        let Some(recorded) = recorded else {
            panic!("the created heartbeat vanished from the store");
        };
        if recorded.run_count >= 1 {
            assert!(recorded.last_run_at.is_some());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the heartbeat never fired: {recorded:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // The fired prompt ran as the session's turn and persisted as the
    // injected `heartbeat_prompt` custom row, never a plain user message.
    let fired_row = {
        let core = worker
            .core
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(store) = core.store.as_ref() else {
            panic!("the session store vanished");
        };
        store
            .entries()
            .iter()
            .find(|entry| {
                entry.fields.get("customType").and_then(Value::as_str)
                    == Some(pa_core::session_engine::messages::HEARTBEAT_PROMPT_CUSTOM_TYPE)
            })
            .map(|entry| entry.fields.clone())
    };
    let fired_row = fired_row.expect("the heartbeat fire never persisted its prompt row");
    let content = fired_row.get("content").and_then(Value::as_str).unwrap();
    // The claimed job snapshot carries the pre-increment run count.
    assert_eq!(content, "[heartbeat: every 10s run#0]\n\nprint hello world");
    let details = fired_row.get("details").cloned().unwrap_or(Value::Null);
    assert_eq!(details["jobId"], job.id);
    assert_eq!(details["schedule"], "every 10s");
    assert_eq!(details["status"], "active");
    assert_eq!(details["runCount"], 0);
}
