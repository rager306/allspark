//! The scheduling surface: the worker arms for the cron/heartbeat catalog,
//! the per-session artifact store, and the scheduler that fires due jobs
//! into the session queue. Deviation: TS `promptHeartbeat` steers a running
//! turn mid-stream; this port's lanes deliver at the next turn boundary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};

use pa_core::cron::scheduler::{AgentCronScheduler, AgentCronSchedulerHooks};
use pa_core::cron::store::{
    AgentCronJobStore, CancelJobsFilter, CreateAgentCronJobInput, HeartbeatManagementAction,
    SessionBinding,
};
use pa_core::cron::{
    is_heartbeat_cron_job, normalize_heartbeat_delivery_mode, normalize_heartbeat_schedule,
    should_defer_heartbeat_cron_job, AgentCronJob, DeliveryMode, HeartbeatSessionActivity,
    JobStatus,
};

use crate::protocol::{response_failure, response_success, DaemonResponse};
use crate::worker::{QueuedItem, SessionCore, Worker};

/// How long a fire waits for its turn to settle before answering the scheduler with a skip (a stuck
/// turn must not pin the dispatch lane forever).
const FIRE_SETTLE_TIMEOUT_MS: u64 = 15 * 60 * 1000;

/// The session-artifact directory for one session file: `<sessions>/../session-artifacts/<id>`.
pub(crate) fn session_artifact_dir(session_file: &Path, session_id: &str) -> Option<PathBuf> {
    session_file
        .parent()?
        .parent()
        .map(|root| root.join("session-artifacts").join(session_id))
}

/// Is `status` one of the fold's END statuses?
///
/// The listing fold normalizes `hidden` and `sleep` to `archived` and keeps anything else
/// verbatim, so the end set is those four spellings. This is the fold's mapping restated where
/// the destructive decision can see it; `is_recorded_end_agrees_with_the_fold` fails if the two
/// ever disagree, which is the only way this copy can go stale.
fn is_recorded_end(status: &str) -> bool {
    matches!(status, "archived" | "crash" | "hidden" | "sleep")
}

/// What the file's newest `session_state` row says, read directly rather than through the
/// listing fold.
enum NewestStateRow {
    /// The newest state row carries a readable status (the fold's `state` value).
    Recorded(String),
    /// A state row at or after the newest readable one cannot be classified by this build.
    Unreadable,
    /// No state row at all (the fold read one, so the file changed under it).
    Missing,
}

/// Classify the session file's newest `session_state` row.
///
/// `None` when the file cannot be read at all. Lines that do not parse as JSON at all are
/// ignored exactly as the fold ignores them: an unrelated corrupt row is not evidence about
/// the lifecycle, and treating it as one would leave a dead session's schedule uncancellable
/// forever. A line that NAMES a state row and does not parse (a torn append, another build's
/// shape) is unclassifiable, not absent.
fn newest_state_row(path: &Path) -> Option<NewestStateRow> {
    const STATE_TYPE: &str = "session_state";
    let file = std::fs::File::open(path).ok()?;
    let mut status: Option<String> = None;
    let mut unreadable_after: Option<bool> = None;
    for line in std::io::BufRead::lines(std::io::BufReader::new(file)) {
        let Ok(line) = line else {
            return None;
        };
        if !line.contains(STATE_TYPE) {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(value) => {
                if value.get("type").and_then(Value::as_str) != Some(STATE_TYPE) {
                    continue;
                }
                // The fold's own test: a string `state.status`, whatever its value.
                match value
                    .get("state")
                    .and_then(|state| state.get("status"))
                    .and_then(Value::as_str)
                {
                    Some(found) => {
                        status = Some(found.to_string());
                        unreadable_after = Some(false);
                    }
                    None => unreadable_after = Some(true),
                }
            }
            // The bytes name a state row and do not parse: a torn append or another build's
            // shape. Unclassifiable, not absent — treating it as absent is exactly how a
            // replaced `archived` row read as a recorded end.
            Err(_) => unreadable_after = Some(true),
        }
    }
    Some(if unreadable_after == Some(true) {
        NewestStateRow::Unreadable
    } else if let Some(status) = status {
        NewestStateRow::Recorded(status)
    } else {
        NewestStateRow::Missing
    })
}

/// The scheduler hooks: how a claimed job reaches this session.
pub(crate) struct QueueHooks {
    core: Arc<Mutex<SessionCore>>,
    work_notify: Arc<Notify>,
    user_bash: Arc<crate::user_bash::UserBash>,
    store: Arc<AgentCronJobStore>,
    /// The worker recovery journal (the fire checkpoint's busy evidence).
    recovery: Arc<Mutex<Option<crate::journal::WorkerRecoveryJournal>>>,
    /// The dispatch each claimed job is serving, handed over by `begin_dispatch`: job id ->
    /// dispatch id. The queue key's unique half comes from here, so it is DURABLE (the dispatch
    /// record is on disk) and a respawned worker cannot mint a key a previous life already
    /// used — that collision evicted a restored fire (review #6).
    dispatch_ids: Arc<Mutex<HashMap<String, String>>>,
}

impl QueueHooks {
    /// Is the INTERRUPTED dispatch's fire still parked in either lane?
    ///
    /// A restored journal row keeps the key the previous worker life minted, so this is the
    /// only evidence that a claim being retried after a crash already has its prompt in the
    /// queue (review #6: reconciliation by dispatch identity, not by a fresh enqueue).
    ///
    /// The identity is the INTERRUPTED dispatch's, read from the recovery marker — not the
    /// one this claim is serving. The current claim was minted by `claim_due` seconds ago,
    /// after recovery released the old one, so no lane can hold a row keyed by it; matching
    /// it answered "not delivered" for every real recovery and let the duplicate through
    /// (review #6, residual).
    ///
    /// With no id in the marker there is no identity to compare, and the answer is NOT to
    /// skip: a same-job prefix match would answer "already delivered" for ANY earlier
    /// dispatch of the job — the exact opposite of the guard's purpose — while a duplicate
    /// wake is recoverable and a lost one is not. Older builds wrote the bare marker, so that
    /// shape reaches this path too.
    fn fire_is_already_queued(&self, job: &AgentCronJob) -> bool {
        let Some(dispatch_id) = job
            .last_error
            .as_deref()
            .and_then(pa_core::cron::store::interrupted_fire_dispatch)
        else {
            return false;
        };
        let key = format!("cron:{}:{dispatch_id}", job.id);
        // The lanes are the whole evidence: create restores the previous life's journal
        // snapshot onto them (`restore_queue_snapshot`), so a survived handoff is a parked
        // row here. Reading the journal as well would need a second lock order for no fact
        // the lanes do not already hold.
        let core = self
            .core
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // EXACT key equality: the marker names the dispatch, so only that dispatch's row is
        // evidence. A prefix (`cron:<job>:`) would answer "already delivered" for any earlier
        // dispatch of the same job — the defect this guard exists to prevent.
        core.steering
            .iter()
            .chain(core.follow_up.iter())
            .any(|item| item.queue_key.as_deref() == Some(key.as_str()))
    }

    /// The session's activity snapshot: busy flags off the core plus the bash slot.
    fn activity(&self) -> HeartbeatSessionActivity {
        let core = self
            .core
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        HeartbeatSessionActivity {
            is_streaming: core.busy,
            is_compacting: core.compacting,
            // The abort flag the retry lane reads: the closest live signal
            // for an in-flight retry.
            is_retrying: core.retry_abort_requested,
            is_bash_running: self.user_bash.is_running(),
            has_pending_session_work: !core.pending_next_turn.is_empty(),
            unfinished_action_count: core.steering.len() + core.follow_up.len(),
        }
    }
}

/// What a fire can learn about its session file before it runs.
///
/// The three answers are deliberately not two: `read_session_info` reports `None` for a
/// deleted file AND for one that could not be read or parsed, and the fire path PERSISTS a
/// cancellation for "gone". Collapsing an I/O failure into "gone" therefore cancelled every
/// scheduled job of a live session because one read raced a rewrite (review #13).
#[derive(Debug, PartialEq, Eq)]
enum TargetState {
    /// The file is there, it is this job's session, and the session is `active`.
    Live,
    /// The session is confirmed deleted or archived: a fire can never revive it.
    Gone,
    /// The file exists but could not be read or understood: the fire must not decide.
    Indeterminate(String),
}

impl QueueHooks {
    /// A persisted job may only fire at a session that still exists — file
    /// present, still the job's session, still `active`.
    fn persisted_target_state(job: &AgentCronJob) -> TargetState {
        if job.session_file.is_empty() {
            // No file named at all: a malformed job, not a read failure.
            return TargetState::Gone;
        }
        let path = Path::new(&job.session_file);
        match std::fs::metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return TargetState::Gone,
            Err(error) => {
                return TargetState::Indeterminate(format!(
                    "{} is unreadable: {error}",
                    path.display()
                ))
            }
            Ok(_) => {}
        }
        match crate::session_store::read_session_info(path) {
            Some(info) if info.id == job.session_id => match info.state.as_deref() {
                Some("active") => TargetState::Live,
                // A RECORDED end is the only evidence that may cancel the session's whole
                // schedule; `hidden`/`sleep` normalize to `archived` upstream. The fold keeps
                // the last status it could READ, so this branch is CONFIRMED against the
                // newest state row before it cancels anything (review #13, residual).
                Some("archived" | "crash") => Self::confirmed_end(path),
                // A session file with no `session_state` row at all (every session
                // written before the row existed) or a state this build cannot read.
                // Both are "unknown", not "gone": the cancel is persisted, so guessing
                // here deleted an older session's schedule on its first fire
                // (review #13, residual).
                Some(other) => TargetState::Indeterminate(format!(
                    "{} records the state {other:?}, which this build cannot classify",
                    path.display()
                )),
                None => TargetState::Indeterminate(format!(
                    "{} records no lifecycle state",
                    path.display()
                )),
            },
            Some(_) => TargetState::Gone,
            // Present but unreadable or not a session this build understands: a transient
            // failure (a rewrite in flight, a lock that was momentarily held) looks exactly
            // like this, so it is never "gone".
            None => TargetState::Indeterminate(format!(
                "{} is present but could not be read as a session",
                path.display()
            )),
        }
    }

    /// The fold's recorded end, confirmed against the newest `session_state` row.
    ///
    /// The fold keeps the last status it could READ, which is the right answer for a listing
    /// and the wrong one for a destructive decision: an `archived` row followed by one this
    /// build cannot parse still reads as `archived`, and `Gone` PERSISTS a cancellation of the
    /// session's whole schedule on evidence the session may already have replaced (review
    /// #13, residual). Cancelling is not reversible, so this branch — and only this branch —
    /// pays one sequential read to ask the file itself.
    fn confirmed_end(path: &Path) -> TargetState {
        match newest_state_row(path) {
            Some(NewestStateRow::Recorded(status)) if is_recorded_end(&status) => TargetState::Gone,
            Some(NewestStateRow::Recorded(status)) => TargetState::Indeterminate(format!(
                "{} records the state {status:?} while the listing fold reported an end",
                path.display()
            )),
            Some(NewestStateRow::Unreadable) => TargetState::Indeterminate(format!(
                "{} records a lifecycle row this build cannot read after the end the fold \
                 reported; it may already have been replaced",
                path.display()
            )),
            Some(NewestStateRow::Missing) => TargetState::Indeterminate(format!(
                "{} no longer records the lifecycle row the fold read",
                path.display()
            )),
            None => TargetState::Indeterminate(format!(
                "{} could not be re-read to confirm its recorded end",
                path.display()
            )),
        }
    }

    /// The failed-runnable cancel: the store cancels the dead session's whole
    /// job set by file, so the artifact never re-fires.
    fn cancel_jobs_for_dead_target(&self, job: &AgentCronJob) -> anyhow::Result<()> {
        self.store.cancel_jobs_for_session(
            &CancelJobsFilter {
                active_session_id: None,
                session_id: None,
                session_file: Some(job.session_file.clone()),
            },
            crate::util::now_ms(),
        )?;
        Ok(())
    }
}

impl AgentCronSchedulerHooks for QueueHooks {
    /// Remember which dispatch a claimed job is serving: the queue key derives from it, so the
    /// fire's identity is the DURABLE dispatch id rather than a per-process counter (review #6).
    fn begin_dispatch(
        &self,
        dispatch: &pa_core::cron::store::AgentCronDispatch,
    ) -> Option<Box<dyn FnOnce() + Send>> {
        self.dispatch_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(dispatch.job.id.clone(), dispatch.id.clone());
        let ids = Arc::clone(&self.dispatch_ids);
        let job_id = dispatch.job.id.clone();
        Some(Box::new(move || {
            ids.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&job_id);
        }))
    }

    async fn run_job(&self, job: &AgentCronJob) -> anyhow::Result<Option<&'static str>> {
        // A persisted job whose target is no longer live (killed — `archived` — or deleted)
        // cancels the session's jobs and skips: a fire can never revive a stopped session.
        // A target that cannot be READ is not gone: the cancel is persisted, so guessing here
        // would cancel a live session's whole schedule over one failed read (review #13).
        match Self::persisted_target_state(job) {
            TargetState::Live => {}
            TargetState::Gone => {
                self.cancel_jobs_for_dead_target(job)?;
                return Ok(Some("skipped"));
            }
            TargetState::Indeterminate(reason) => {
                // No cancel and no fire: the scheduler records a run ERROR (not a skip), so the
                // schedule survives with its failure backoff and the next fire re-reads the
                // file. A skip would complete a one-shot job on one bad read.
                anyhow::bail!("scheduled job target could not be read: {reason}");
            }
        }
        let activity = self.activity();
        if should_defer_heartbeat_cron_job(job, &activity) {
            return Ok(Some("skipped"));
        }
        let (done_tx, done_rx) = oneshot::channel();
        let heartbeat = is_heartbeat_cron_job(job);
        // Every fire carries a queue identity, and for the settle timeout below it is the only
        // way to tell a fire still PARKED in the lane from one the runner already picked up
        // (review #12). A heartbeat keeps its `heartbeat:<id>` key, whose replace-on-enqueue
        // dedupe is the TS behaviour; a plain cron fire gets a key unique to THIS dispatch, so
        // identifying it does not also change how successive fires stack.
        //
        // The unique half is the DURABLE dispatch id, not a process counter (review #6): a
        // respawned worker restarts a counter at zero, so its first fire minted a key an earlier
        // worker life had already used, and the admission's replace-on-key removed the row that
        // life had restored from the journal — a durable, not-yet-run prompt silently dropped.
        // A dispatch id is minted once and recorded on disk, so two lives cannot collide.
        let dispatch_id = self
            .dispatch_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&job.id)
            .cloned();
        let queue_key = Some(if heartbeat {
            format!("heartbeat:{}", job.id)
        } else {
            format!(
                "cron:{}:{}",
                job.id,
                dispatch_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
            )
        });
        // A heartbeat rides its delivery-mode lane; a plain cron job queues on the follow-up lane.
        let rides_steering =
            heartbeat && !matches!(job.delivery_mode, Some(DeliveryMode::FollowUp));
        // The key survives the admission (the item owns it): the settle timeout reads it back to
        // ask whether the fire is still parked.
        let fire_key = queue_key.clone();
        // A claim RE-ARMED from an interrupted dispatch may already be durably queued (review
        // #6). Recovery restores the interrupted life's lane from the journal, so a fire for a
        // job whose recovery marker names a dispatch already parked under this job's key is a
        // REPLAY of a prompt that survived: enqueueing a second copy would run the same wake
        // twice. The skip is the right record — a one-shot completes because its wake is
        // delivered, and a recurring job keeps its schedule. Read BEFORE the lanes are locked
        // below (two locks, never nested).
        if self.fire_is_already_queued(job) {
            return Ok(Some("skipped"));
        }
        {
            let mut core = self
                .core
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !core.created || core.shutdown_requested || job.status != JobStatus::Active {
                return Ok(Some("skipped"));
            }
            // TS cron fires resume the suspension before admission (`resumeIfIdle: true`): a
            // fire on a post-abort/post-compact session is a resume site.
            core.queued_input_suspended = false;
            // The TS `heartbeat:<id>` queue key: a later fire replaces the queued one instead of
            // stacking.
            if let Some(key) = &queue_key {
                core.steering
                    .retain(|item| item.queue_key.as_deref() != Some(key.as_str()));
                core.follow_up
                    .retain(|item| item.queue_key.as_deref() != Some(key.as_str()));
            }
            let lane = if rides_steering {
                &mut core.steering
            } else {
                &mut core.follow_up
            };
            // A heartbeat fire delivers through `promptHeartbeat`: the turn IS
            // the injected `heartbeat_prompt` custom row, a plain cron job a regular prompt.
            let (message, preview, custom_message) = if heartbeat {
                let row = pa_core::session_engine::messages::create_heartbeat_prompt_message(
                    job,
                    crate::util::now_ms(),
                );
                let content = row.content.text();
                // The parked row reads `Heartbeat prompt: <content>`, while the
                // active-action label keeps the raw content.
                let preview = format!(
                    "{}: {content}",
                    pa_core::session_engine::messages::HEARTBEAT_PROMPT_PREVIEW_LABEL
                );
                (
                    content,
                    Some(preview),
                    Some(crate::session_commands::custom_message_value(&row)),
                )
            } else {
                (job.prompt.clone(), None, None)
            };
            crate::worker::enqueue_priority(
                lane,
                QueuedItem {
                    priority: crate::worker::QueuePriority::Background,
                    message,
                    preview,
                    custom_message,
                    agent_message: None,
                    admission_id: None,
                    images: Vec::new(),
                    queue_key,
                    done: Some(done_tx),
                    queue_visible: true,
                    policy: crate::worker::TurnPolicy::Injected,
                    forced_batch: false,
                },
            );
        }
        // The fire checkpoint (busy=true): a scheduled prompt runs unattended —
        // a crash mid-fire must revive the worker.
        crate::worker::checkpoint_queue_recovery(
            &self.recovery,
            &self.core,
            crate::worker::QueueCheckpoint::Admitted {
                operation: if rides_steering {
                    "steer_queued"
                } else {
                    "follow_up_queued"
                },
            },
        );
        self.work_notify.notify_one();
        match tokio::time::timeout(
            std::time::Duration::from_millis(FIRE_SETTLE_TIMEOUT_MS),
            done_rx,
        )
        .await
        {
            // The typed settle classifies the fire (never the error text): a
            // settled or failed turn counts as a run (the failure backoff
            // stretches the next fire; deviation: TS re-fires per schedule);
            // an aborted turn is a clean run; a withdrawn fire skips, no backoff.
            Ok(Ok(settle)) => match settle {
                crate::worker::TurnSettle::Completed | crate::worker::TurnSettle::Aborted => {
                    Ok(None)
                }
                crate::worker::TurnSettle::Withdrawn(_) => Ok(Some("skipped")),
                crate::worker::TurnSettle::Failed(error) => Err(anyhow::anyhow!(error)),
            },
            // The queued item was consumed without a settle handshake (its waiter dropped):
            // the fire ran as far as the queue could deliver it.
            Ok(Err(_)) => Ok(None),
            // The settle window expired. What that means depends on ONE fact the timeout
            // alone cannot see (review #12): a fire still PARKED in its lane never ran, while
            // a fire the runner picked up is executing right now.
            Err(_) => self.on_settle_timeout(job, fire_key.as_deref()),
        }
    }
}

impl QueueHooks {
    /// Classify a fire whose settle window expired, by where its queue item is.
    ///
    /// - Still PARKED in a lane: it never ran, so it is withdrawn (it must not execute later
    ///   behind the scheduler's back), the withdrawal is checkpointed so a respawn cannot
    ///   restore it, and it is reported as a skip — no run happened.
    /// - Gone from the lanes, i.e. the runner picked it up: never report a skip.
    ///   `record_dispatch_result` completes a one-shot on a skip, which would close a prompt
    ///   that is executing right now and stamp a recurring job's `lastSkippedAt` for work in
    ///   flight.
    ///   - A ONE-SHOT records a clean run (`Ok(None)`): its wake is durably queued, so
    ///     completing it is correct and the turn owns the outcome.
    ///   - A RECURRING job records an unknown outcome (`Err`): the scheduler stores the error
    ///     and follows with the failure backoff, which stretches the next fire. Recording a
    ///     clean run for a fire whose end this path cannot see was the residual.
    fn on_settle_timeout(
        &self,
        job: &AgentCronJob,
        fire_key: Option<&str>,
    ) -> anyhow::Result<Option<&'static str>> {
        let _was_parked = Self::withdraw_parked_fire(&self.core, fire_key);
        crate::worker::checkpoint_queue_recovery(
            &self.recovery,
            &self.core,
            crate::worker::QueueCheckpoint::Settle {
                operation: "cron_fire_withdrawn",
            },
        );
        if _was_parked {
            // The fire NEVER RAN. For a recurring job that is a skip: the run did not happen
            // and the schedule keeps its cadence. For a ONE-SHOT it is not: a skip COMPLETES
            // the row (`record_dispatch_result`), and completion claims the wake was
            // delivered — a quota-resume or goal-backoff prompt would be gone without ever
            // having been queued. Report the undelivered fire instead, so the scheduler
            // records the failure and its backoff re-arms the only wake this job represents
            // (review #6's at-least-once rule).
            if job.schedule.kind == pa_core::cron::ScheduleKind::Once {
                tracing::warn!(
                    job = %job.id,
                    "a one-shot fire sat in its lane past the settle window; the wake was \
                     never delivered, so the schedule is re-armed rather than completed"
                );
                return Err(anyhow::anyhow!(
                    "one-shot fire waited {FIRE_SETTLE_TIMEOUT_MS} ms in its lane without being \
                     delivered; the wake is re-armed"
                ));
            }
            return Ok(Some("skipped"));
        }
        // The fire left the lanes: the runner is executing it, so nothing here may call it a
        // skip. What the SCHEDULE may record still differs by schedule kind (review #12,
        // residual).
        if job.schedule.kind == pa_core::cron::ScheduleKind::Once {
            // A one-shot's wake HAS been delivered: its prompt is durably queued and running.
            // Completing it is right, and re-arming it would mint a second dispatch and run the
            // same wake twice. The turn owns its own outcome.
            tracing::warn!(
                job = %job.id,
                "a one-shot fire outlived its settle window while running; the wake is \
                 delivered and the turn owns the outcome"
            );
            return Ok(None);
        }
        // A recurring job has no such cover: `Ok(None)` records a clean run — `run_count`,
        // `last_run_at`, and a cleared failure streak — for work whose outcome this fire cannot
        // see. Report the unknown outcome instead: the scheduler stores it as `last_error` and
        // follows with the failure backoff, which STRETCHES the next fire rather than dropping
        // it, so nothing is lost and nothing is claimed.
        tracing::warn!(
            job = %job.id,
            "a scheduled fire outlived its settle window while running; recording an unknown \
             outcome and backing the schedule off"
        );
        Err(anyhow::anyhow!(
            "scheduled fire outlived its settle window ({FIRE_SETTLE_TIMEOUT_MS} ms) while \
             running; its outcome is unknown"
        ))
    }

    /// Remove a still-parked fire from its lane, returning whether it was there.
    ///
    /// The lane is where a fire waits; once the runner gathers it the row is gone from both
    /// lanes, which is exactly the evidence [`Self::on_settle_timeout`] needs.
    fn withdraw_parked_fire(core: &Mutex<SessionCore>, queue_key: Option<&str>) -> bool {
        let Some(queue_key) = queue_key else {
            return false;
        };
        let mut core = core
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = core.steering.len() + core.follow_up.len();
        core.steering
            .retain(|item| item.queue_key.as_deref() != Some(queue_key));
        core.follow_up
            .retain(|item| item.queue_key.as_deref() != Some(queue_key));
        before != core.steering.len() + core.follow_up.len()
    }
}

/// The worker's schedule catalog: the shared artifact store plus the scheduler (started when the
/// first session binds).
pub(crate) struct ScheduledJobs {
    store: Arc<AgentCronJobStore>,
    hooks: Arc<QueueHooks>,
    scheduler: tokio::sync::Mutex<Option<Arc<AgentCronScheduler<QueueHooks>>>>,
}

impl ScheduledJobs {
    pub(crate) fn new(
        core: Arc<Mutex<SessionCore>>,
        work_notify: Arc<Notify>,
        user_bash: Arc<crate::user_bash::UserBash>,
        events: Arc<crate::worker::EventPump>,
        recovery: Arc<Mutex<Option<crate::journal::WorkerRecoveryJournal>>>,
    ) -> Self {
        let mut store = AgentCronJobStore::for_session_artifacts();
        // `cronStore.onHeartbeatChange` broadcasts `{ type: "heartbeats_changed" }`
        // to the clients; the supervisor re-broadcasts daemon-wide.
        store.on_heartbeat_change(Box::new(move || {
            events.send(crate::worker::OutboundFrame::heartbeats_changed());
        }));
        let store = Arc::new(store);
        ScheduledJobs {
            hooks: Arc::new(QueueHooks {
                core,
                work_notify,
                user_bash,
                store: Arc::clone(&store),
                recovery,
                dispatch_ids: Arc::new(Mutex::new(HashMap::new())),
            }),
            store,
            scheduler: tokio::sync::Mutex::new(None),
        }
    }

    pub(crate) fn store(&self) -> &Arc<AgentCronJobStore> {
        &self.store
    }

    /// Bind the live session (TS `rebindCronJobsToState`): register the session's artifact
    /// partition, move its stored jobs onto the live ids, and start (or wake) the scheduler.
    pub(crate) async fn bind_session(
        &self,
        binding: SessionBinding,
        artifact_dir: Option<PathBuf>,
    ) -> anyhow::Result<()> {
        if let Some(dir) = artifact_dir {
            std::fs::create_dir_all(&dir)?;
            self.store
                .register_session_artifact(&binding.session_id, &dir);
        }
        if !binding.session_file.is_empty() {
            self.store.rebind_session_jobs(&binding)?;
        }
        let mut guard = self.scheduler.lock().await;
        if let Some(scheduler) = guard.as_ref() {
            scheduler.wake().await;
            return Ok(());
        }
        let scheduler = Arc::new(AgentCronScheduler::new(
            Arc::clone(&self.store),
            Arc::clone(&self.hooks),
        ));
        scheduler.start().await;
        *guard = Some(scheduler);
        Ok(())
    }

    /// Re-arm the timer after a catalog mutation (TS `cronScheduler.wake`).
    pub(crate) async fn wake(&self) {
        let guard = self.scheduler.lock().await;
        if let Some(scheduler) = guard.as_ref() {
            scheduler.wake().await;
        }
    }

    /// The kernel rlm heartbeat mutation hook (after every create/update/delete):
    /// without the wake, a heartbeat created after the bind never fires.
    pub(crate) fn mutation_hook(
        self: &std::sync::Arc<Self>,
    ) -> pa_core::session_engine::host_requests::RlmHeartbeatMutationHook {
        let scheduled = std::sync::Arc::clone(self);
        std::sync::Arc::new(move |mutation| {
            let scheduled = std::sync::Arc::clone(&scheduled);
            Box::pin(async move {
                if mutation.drop_queued {
                    scheduled.remove_queued_heartbeat_follow_up(&mutation.job);
                }
                scheduled.wake().await;
            })
        })
    }

    /// Drop the queued fire of a heartbeat job from the session's queue.
    pub(crate) fn remove_queued_heartbeat_follow_up(&self, job: &AgentCronJob) {
        if !is_heartbeat_cron_job(job) {
            return;
        }
        let key = format!("heartbeat:{}", job.id);
        {
            let mut core = self
                .hooks
                .core
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            core.steering
                .retain(|item| item.queue_key.as_deref() != Some(key.as_str()));
            core.follow_up
                .retain(|item| item.queue_key.as_deref() != Some(key.as_str()));
        }
        // Same settle as the other withdrawals: the verdict and snapshot must not keep the
        // withdrawn fire's busy=true (a revive would replay the deleted heartbeat's prompt).
        crate::worker::checkpoint_queue_recovery(
            &self.hooks.recovery,
            &self.hooks.core,
            crate::worker::QueueCheckpoint::Settle {
                operation: "queue_purged",
            },
        );
    }
}

/// The bind inputs of one live session plus the session's artifact partition:
/// `None` for in-memory sessions.
pub(crate) fn live_binding(core: &SessionCore) -> Option<(SessionBinding, Option<PathBuf>)> {
    let store = core.store.as_ref()?;
    if store.path.as_os_str().is_empty() {
        return None;
    }
    Some((
        SessionBinding {
            active_session_id: core.active_session_id.clone(),
            session_id: store.session_id().to_string(),
            session_file: store.path.to_string_lossy().to_string(),
            cwd: core.cwd.clone(),
        },
        session_artifact_dir(&store.path, store.session_id()),
    ))
}

impl Worker {
    /// Register the live session's artifact partition on the store (idempotent) so catalog reads
    /// see this session's jobs.
    fn bind_store_artifact(&self, core: &SessionCore) {
        let Some(store) = core.store.as_ref() else {
            return;
        };
        if store.path.as_os_str().is_empty() {
            return;
        }
        if let Some(dir) = session_artifact_dir(&store.path, store.session_id()) {
            self.scheduled
                .store()
                .register_session_artifact(store.session_id(), &dir);
        }
    }

    /// The killed close's cancel: the session's whole job set cancels by any of
    /// its three identities — durably, so the session's own heartbeats can
    /// never revive it.
    pub(crate) async fn cancel_session_scheduled_jobs(&self) -> anyhow::Result<()> {
        let (active_session_id, session_id, session_file) = {
            let core = self
                .core
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bind_store_artifact(&core);
            let Some(store) = core.store.as_ref() else {
                return Ok(());
            };
            (
                core.active_session_id.clone(),
                store.session_id().to_string(),
                store.path.to_string_lossy().to_string(),
            )
        };
        let cancelled = self.scheduled.store().cancel_jobs_for_session(
            &pa_core::cron::store::CancelJobsFilter {
                active_session_id: Some(active_session_id),
                session_id: Some(session_id),
                session_file: Some(session_file),
            },
            crate::util::now_ms(),
        )?;
        for job in &cancelled {
            self.scheduled.remove_queued_heartbeat_follow_up(job);
        }
        if !cancelled.is_empty() {
            self.scheduled.wake().await;
        }
        Ok(())
    }

    /// Only a subagent's RLM heartbeat jobs cancel here (the plain cron jobs
    /// survive the replacement); a top-level session cancels nothing.
    pub(crate) async fn cancel_session_rlm_heartbeats(&self) -> anyhow::Result<()> {
        let (is_subagent, active_session_id) = {
            let core = self
                .core
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bind_store_artifact(&core);
            (
                core.runtime_kind == "subagent",
                core.active_session_id.clone(),
            )
        };
        if !is_subagent {
            return Ok(());
        }
        let cancelled = self
            .scheduled
            .store()
            .cancel_rlm_heartbeats_for_session(&active_session_id, crate::util::now_ms())?;
        for job in &cancelled {
            self.scheduled.remove_queued_heartbeat_follow_up(job);
        }
        if !cancelled.is_empty() {
            self.scheduled.wake().await;
        }
        Ok(())
    }

    /// The saved-session delete's cancel: cancel the deleted file's whole job
    /// set by file (its partition derives from the file's stem); best-effort —
    /// the deletion never fails on a store error.
    pub(crate) fn cancel_deleted_session_jobs(&self, session_file: &std::path::Path) {
        let Some(session_id) = session_file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
        else {
            return;
        };
        let Some(dir) = session_artifact_dir(session_file, session_id) else {
            return;
        };
        if !dir
            .join(pa_core::cron::store::SESSION_SCHEDULED_JOBS_FILENAME)
            .is_file()
        {
            return;
        }
        self.scheduled
            .store()
            .register_session_artifact(session_id, &dir);
        if let Err(error) = self.scheduled.store().cancel_jobs_for_session(
            &pa_core::cron::store::CancelJobsFilter {
                active_session_id: None,
                session_id: None,
                session_file: Some(session_file.to_string_lossy().to_string()),
            },
            crate::util::now_ms(),
        ) {
            eprintln!("failed to cancel deleted session jobs: {error}");
        }
    }
}

// The scheduling protocol arms live in scheduled_jobs::arms as the same
// inherent impl Worker block.
mod arms;

// The inline unit battery lives in scheduled_jobs::tests.
#[cfg(test)]
mod tests;
