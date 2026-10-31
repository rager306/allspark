//! The agent-facing `schedule` tool: durable cron jobs the user can see
//! (fork-only; gated by `schedule.enabled`, default OFF).
//!
//! The CLI has had `schedule <list|add|cancel>` since the TS port
//! (`crates/pa-cli/src/command_registry.rs`), while the agent's only
//! scheduling primitive was the session-scoped heartbeat: it dies with the
//! session, and it never fires in a daemonless run. So an agent asked to
//! watch something overnight had to substitute in-session polling that
//! vanished on exit. This tool exposes the same durable store the command
//! does (`crate::cron::store::AgentCronJobStore`), over the same worker-side
//! path (`cron_add`/`cron_list`/`cron_cancel`), so an agent's schedule and a
//! user's schedule are one catalog and one lifecycle.
//!
//! Three actions over one vocabulary:
//!
//! - `add`: create a job from a schedule plus a prompt. The prompt is what the
//!   session is asked when the job fires; the schedule is the same free-form
//!   text the CLI accepts (`0 9 * * 1-5`, `every 30m`, `in 2h`, `at <ISO>`).
//! - `list`: this session's jobs, active and paused, with their next run.
//! - `cancel`: cancel one of this session's jobs by id.
//!
//! Scope is the session, not the daemon: `list`/`cancel` see only jobs this
//! session created, so a subagent cannot cancel its parent's work and a
//! resumed session still owns what it scheduled (jobs rebind by session file).
//! A job outlives the turn, the worker and the daemon restart; it does not
//! outlive a killed session, because the worker cancels the killed session's
//! job set (a fire must never revive a stopped session).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback, ToolResultContent};

use crate::cron::store::{AgentCronJobStore, CreateAgentCronJobInput};
use crate::cron::{format_agent_cron_job, AgentCronJob, JobStatus};

/// The job `source` this tool stamps: the store's filter for "an agent asked
/// for this at the tool surface", distinct from the user's `cron` jobs and the
/// heartbeat families (`heartbeat`, `rlm_heartbeat`).
pub const SCHEDULE_TOOL_SOURCE: &str = "schedule_tool";

/// Where a tool-created job binds: the live session id the daemon routes by,
/// plus the durable identity the store partitions and rebinds by.
#[derive(Debug, Clone)]
pub struct ScheduleBinding {
    pub active_session_id: String,
    pub session_id: String,
    pub session_file: String,
    pub cwd: String,
    /// The session's artifact directory (`<sessions>/../session-artifacts/<id>`).
    ///
    /// The daemon's store is the PARTITIONED one (`for_session_artifacts`), and
    /// its write path refuses a job whose session has no registered partition
    /// ("cron job session artifact not registered"). The worker's `cron_add`
    /// arm registers the partition before it writes; a tool call arrives
    /// through a different door and must do the same, or the first schedule of
    /// a session that has not read its catalog yet fails. Registration is
    /// idempotent, so doing it on every call is the designed pattern.
    pub artifact_dir: Option<std::path::PathBuf>,
}

/// The embedding's post-mutation seam: the second argument is `drop_queued`,
/// true when a queued fire of that job must be withdrawn (a cancel), false
/// when the job only needs the scheduler re-armed (a create).
///
/// Without it a job created after the session's scheduler bound its timer
/// never fires: the timer task parks on a notify and re-evaluates on wake
/// only (`AgentCronScheduler::schedule_next`). The daemon's own `cron_add`
/// arm wakes it for exactly that reason, and a tool-created job must reach
/// the same arm.
pub type ScheduleWake =
    Arc<dyn Fn(AgentCronJob, bool) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Everything the tool needs from the session it belongs to: the shared
/// store, the identity it binds jobs to, and the scheduler wake.
#[derive(Clone)]
pub struct ScheduleTarget {
    pub store: Arc<AgentCronJobStore>,
    pub binding: ScheduleBinding,
    pub wake: Option<ScheduleWake>,
}

/// What one call did, for the result text and the post-mutation wake.
#[derive(Debug)]
struct Outcome {
    text: String,
    mutation: Option<(AgentCronJob, bool)>,
}

/// The tool over the session's store and identity.
pub struct ScheduleTool {
    store: Arc<AgentCronJobStore>,
    binding: ScheduleBinding,
    wake: Option<ScheduleWake>,
    schema: serde_json::Value,
}

impl ScheduleTool {
    /// Build the tool from the session's resolved target.
    #[must_use]
    pub fn new(target: ScheduleTarget) -> Self {
        ScheduleTool {
            store: target.store,
            binding: target.binding,
            wake: target.wake,
            schema: params_schema(),
        }
    }

    fn dispatch(&self, params: &serde_json::Value) -> anyhow::Result<Outcome> {
        // Bind this session's partition first: the store's write path rejects a
        // job whose session it does not read (see `ScheduleBinding::artifact_dir`).
        if let Some(dir) = self.binding.artifact_dir.as_deref() {
            self.store
                .register_session_artifact(&self.binding.session_id, dir);
        }
        let action = params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .map_or("", str::trim)
            .to_ascii_lowercase();
        match action.as_str() {
            "add" => self.add(params),
            "list" => Ok(Outcome {
                text: self.list(),
                // A catalog read mutates nothing.
                mutation: None,
            }),
            "cancel" => self.cancel(params),
            other => anyhow::bail!(
                "unknown schedule action {other:?}: expected one of add, list, cancel"
            ),
        }
    }

    fn add(&self, params: &serde_json::Value) -> anyhow::Result<Outcome> {
        let schedule = required_str(params, "schedule")?;
        let prompt = required_str(params, "prompt")?;
        let job = self.store.create(&CreateAgentCronJobInput {
            active_session_id: self.binding.active_session_id.clone(),
            session_id: self.binding.session_id.clone(),
            session_file: self.binding.session_file.clone(),
            cwd: self.binding.cwd.clone(),
            source: Some(SCHEDULE_TOOL_SOURCE.to_string()),
            label: optional_str(params, "label"),
            prompt: prompt.to_string(),
            schedule_text: schedule.to_string(),
            ..Default::default()
        })?;
        Ok(Outcome {
            text: format!("Scheduled {}.\n{}", job.id, format_agent_cron_job(&job)),
            mutation: Some((job, false)),
        })
    }

    fn list(&self) -> String {
        let mut jobs: Vec<_> = self
            .store
            .list()
            .into_iter()
            .filter(|job| {
                job.active_session_id == self.binding.active_session_id
                    && matches!(job.status, JobStatus::Active | JobStatus::Paused)
            })
            .collect();
        if jobs.is_empty() {
            return "No schedules for this session.".to_string();
        }
        jobs.sort_by(|left, right| left.next_run_at.cmp(&right.next_run_at));
        let mut text = String::new();
        for job in jobs {
            text.push_str(&format_agent_cron_job(&job));
            text.push('\n');
        }
        text
    }

    fn cancel(&self, params: &serde_json::Value) -> anyhow::Result<Outcome> {
        let id = required_str(params, "id")?;
        // The ownership check is the point: an id belonging to another session
        // must read the same as an id that does not exist, so a cancel cannot
        // be used to probe or to reach across sessions.
        let owned = self
            .store
            .list()
            .into_iter()
            .any(|job| job.id == id && job.active_session_id == self.binding.active_session_id);
        if !owned {
            anyhow::bail!(
                "no schedule {id:?} belongs to this session (list them with action \"list\")"
            );
        }
        let cancelled = self.store.cancel(id, crate::cron::store::now_millis())?;
        match cancelled {
            Some(job) => Ok(Outcome {
                text: format!("Cancelled {}.\n{}", job.id, format_agent_cron_job(&job)),
                mutation: Some((job, true)),
            }),
            None => anyhow::bail!("schedule {id:?} was already cancelled"),
        }
    }

    /// Hand the mutation to the embedding's scheduler wake. Awaited, not
    /// spawned: the caller's next fire must not race a job the scheduler has
    /// not armed yet.
    async fn wake(&self, mutation: Option<(AgentCronJob, bool)>) {
        let (Some(wake), Some((job, drop_queued))) = (self.wake.as_ref(), mutation) else {
            return;
        };
        wake(job, drop_queued).await;
    }
}

fn required_str<'a>(params: &'a serde_json::Value, key: &str) -> anyhow::Result<&'a str> {
    params
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("schedule action needs a non-empty {key:?}"))
}

fn optional_str(params: &serde_json::Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::string::ToString::to_string)
}

impl AgentTool for ScheduleTool {
    fn name(&self) -> &'static str {
        "schedule"
    }

    fn description(&self) -> &'static str {
        "Create, list, and cancel DURABLE scheduled prompts for this session. A schedule \
         survives the turn, the worker, and a daemon restart — unlike a heartbeat, which \
         belongs to the running session. Use it when the user asks for work to happen \
         later or repeatedly (\"check the build every morning\", \"remind me in 2 hours\"); \
         the prompt is delivered as a new turn when the schedule fires. Actions: add \
         (schedule, prompt, optional label), list (this session's active and paused \
         schedules with their next run), cancel (id). Schedule text is the same form the \
         `schedule` command takes: a 5-field cron expression, `every 30m`, `in 2h`, or \
         `at <ISO 8601>`. The job fires into THIS session, so it stops when the session is \
         killed."
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.schema
    }

    fn label(&self) -> &'static str {
        "Schedule"
    }

    fn execute(
        self: Arc<Self>,
        _tool_call_id: String,
        params: serde_json::Value,
        signal: AbortSignal,
        _on_update: AgentToolUpdateCallback,
    ) -> pa_agent::BoxFut<'static, anyhow::Result<AgentToolResult>> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            // The store read/write is a file operation under a cross-process
            // lock: keep it off the async executor. The scheduler wake is
            // awaited after it (never inside), because a wake re-reads the
            // store's `next_run_at`.
            let outcome = tokio::select! {
                biased;
                () = signal.aborted() => Err(anyhow::anyhow!("schedule call aborted")),
                result = tokio::task::spawn_blocking({
                    let this = Arc::clone(&self);
                    let params = params.clone();
                    move || this.dispatch(&params)
                }) => {
                    result.map_err(|join| anyhow::anyhow!("schedule task join failed: {join}"))?
                }
            }?;
            self.wake(outcome.mutation).await;
            Ok(AgentToolResult {
                content: vec![ToolResultContent::text(outcome.text)],
                details: serde_json::Value::Null,
                terminate: None,
                is_error: false,
            }
            .with_duration_ms(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)))
        })
    }
}

/// The tool's JSON-Schema parameters.
#[must_use]
pub fn params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["add", "list", "cancel"],
                "description": "add (create a durable schedule), list (this session's schedules), cancel (by id)."
            },
            "schedule": {
                "type": "string",
                "description": "add: when it fires — a 5-field cron expression (`0 9 * * 1-5`), `every 30m`, `in 2h`, or `at <ISO 8601>`."
            },
            "prompt": {
                "type": "string",
                "description": "add: the text delivered as a new turn when the schedule fires. Write it as an instruction to yourself later: it has none of this turn's context."
            },
            "label": {
                "type": "string",
                "description": "add: an optional short name shown in the schedule list."
            },
            "id": {
                "type": "string",
                "description": "cancel: the job id, as printed by add or list."
            }
        },
        "required": ["action"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(dir: &std::path::Path, session: &str) -> ScheduleTarget {
        ScheduleTarget {
            store: Arc::new(AgentCronJobStore::new(dir.join("jobs.json"))),
            binding: ScheduleBinding {
                active_session_id: session.to_string(),
                session_id: session.to_string(),
                session_file: format!("/w/{session}.jsonl"),
                cwd: "/w".to_string(),
                artifact_dir: Some(dir.join("artifacts").join(session)),
            },
            wake: None,
        }
    }

    fn text(outcome: &Outcome) -> &str {
        &outcome.text
    }

    fn job_id(outcome: &Outcome) -> &str {
        let (job, _) = outcome.mutation.as_ref().expect("a mutation");
        &job.id
    }

    /// the bug this tool exists for: an agent had no way to create durable
    /// work, so the prompt's only scheduling primitive was a session-scoped
    /// heartbeat. The tool's `add` must land in the SAME store the user's
    /// `schedule list` reads, and its own `list` must see it.
    #[test]
    fn add_creates_a_job_that_list_and_the_store_report() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ScheduleTool::new(target(dir.path(), "live-1"));
        let added = tool
            .dispatch(&serde_json::json!({
                "action": "add",
                "schedule": "every 30m",
                "prompt": "check the build",
                "label": "build watch",
            }))
            .unwrap();
        assert!(text(&added).contains("check the build"), "{}", text(&added));
        let id = job_id(&added).to_string();
        // The user's own catalog (the daemon's `cron_list`) reads the same row.
        let stored = tool.store.list();
        assert_eq!(stored.len(), 1, "{stored:?}");
        assert_eq!(stored[0].id, id);
        assert_eq!(stored[0].source.as_deref(), Some(SCHEDULE_TOOL_SOURCE));
        assert_eq!(stored[0].active_session_id, "live-1");
        // And the tool's own list shows it.
        let listed = tool
            .dispatch(&serde_json::json!({ "action": "list" }))
            .unwrap();
        assert!(text(&listed).contains("build watch"), "{}", text(&listed));
        assert!(listed.mutation.is_none(), "a read mutates nothing");
    }

    /// A create must re-arm the scheduler; a read must not. Without the wake a
    /// job created after the timer armed never fires (the store timer parks on
    /// a notify), which is the regression the kernel path has a test for.
    #[tokio::test]
    async fn a_create_wakes_the_scheduler_and_a_read_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut target = target(dir.path(), "live-1");
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        target.wake = Some({
            let wakes = Arc::clone(&wakes);
            Arc::new(move |_job: AgentCronJob, drop_queued: bool| {
                let wakes = Arc::clone(&wakes);
                let flag = usize::from(drop_queued);
                Box::pin(async move {
                    wakes.fetch_add(1 + flag, std::sync::atomic::Ordering::SeqCst);
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            })
        });
        let tool = ScheduleTool::new(target);
        let added = tool
            .dispatch(&serde_json::json!({
                "action": "add",
                "schedule": "every 30m",
                "prompt": "check the build",
            }))
            .unwrap();
        let added_id = job_id(&added).to_string();
        tool.wake(added.mutation).await;
        assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 1);
        let listed = tool
            .dispatch(&serde_json::json!({ "action": "list" }))
            .unwrap();
        tool.wake(listed.mutation).await;
        assert_eq!(
            wakes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a catalog read must not re-arm the scheduler"
        );
        let cancelled = tool
            .dispatch(&serde_json::json!({
                "action": "cancel",
                "id": added_id,
            }))
            .unwrap();
        tool.wake(cancelled.mutation).await;
        // A cancel carries `drop_queued`, so the wake is weighted 1 + 1.
        assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    /// Scope is the session: another session's job must be invisible to `list`
    /// and unreachable by `cancel`, so a subagent cannot stop its parent's
    /// work and an id cannot be used to probe the catalog.
    #[test]
    fn another_sessions_job_is_not_listed_and_cannot_be_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let theirs = ScheduleTool::new(target(dir.path(), "live-2"));
        let added = theirs
            .dispatch(&serde_json::json!({
                "action": "add",
                "schedule": "every 30m",
                "prompt": "their work",
            }))
            .unwrap();
        let ours = ScheduleTool::new(ScheduleTarget {
            store: Arc::clone(&theirs.store),
            binding: ScheduleBinding {
                active_session_id: "live-1".to_string(),
                ..theirs.binding.clone()
            },
            wake: None,
        });
        let listed = ours
            .dispatch(&serde_json::json!({ "action": "list" }))
            .unwrap();
        assert_eq!(text(&listed), "No schedules for this session.");
        let error = ours
            .dispatch(&serde_json::json!({
                "action": "cancel",
                "id": job_id(&added),
            }))
            .unwrap_err()
            .to_string();
        assert!(error.contains("belongs to this session"), "{error}");
        // The refused cancel changed nothing.
        assert_eq!(theirs.store.list()[0].status, JobStatus::Active);
    }

    /// A cancelled job leaves the active list, and cancelling it twice is an
    /// error the model can act on rather than a silent success.
    #[test]
    fn cancel_is_idempotently_refused_after_the_first_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ScheduleTool::new(target(dir.path(), "live-1"));
        let added = tool
            .dispatch(&serde_json::json!({
                "action": "add",
                "schedule": "every 30m",
                "prompt": "check the build",
            }))
            .unwrap();
        let id = job_id(&added).to_string();
        let cancelled = tool
            .dispatch(&serde_json::json!({ "action": "cancel", "id": id }))
            .unwrap();
        assert!(
            text(&cancelled).contains("cancelled"),
            "{}",
            text(&cancelled)
        );
        let listed = tool
            .dispatch(&serde_json::json!({ "action": "list" }))
            .unwrap();
        assert_eq!(text(&listed), "No schedules for this session.");
        let again = tool
            .dispatch(&serde_json::json!({ "action": "cancel", "id": id }))
            .unwrap_err()
            .to_string();
        assert!(again.contains("already cancelled"), "{again}");
    }

    /// The daemon's store is partitioned per session artifact, and its write
    /// path refuses a job whose session partition is unregistered. A tool call
    /// reaches the store through a different door than the worker's `cron_add`
    /// arm, which registers the partition first — so without the tool's own
    /// registration, the FIRST schedule of a live session fails with
    /// "session artifact not registered" even though everything else is right.
    #[test]
    fn a_partitioned_store_accepts_a_job_after_the_tool_binds_its_session() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = dir.path().join("artifacts").join("live-1");
        std::fs::create_dir_all(&artifacts).unwrap();
        let tool = ScheduleTool::new(ScheduleTarget {
            store: Arc::new(AgentCronJobStore::for_session_artifacts()),
            binding: ScheduleBinding {
                active_session_id: "live-1".to_string(),
                session_id: "live-1".to_string(),
                session_file: dir.path().join("live-1.jsonl").display().to_string(),
                cwd: dir.path().display().to_string(),
                artifact_dir: Some(artifacts.clone()),
            },
            wake: None,
        });
        let added = tool
            .dispatch(&serde_json::json!({
                "action": "add",
                "schedule": "every 30m",
                "prompt": "check the build",
            }))
            .expect("a live session's first schedule");
        assert_eq!(job_id(&added).len(), 36, "a uuid");
        // The row landed in THIS session's partition, which is what the daemon
        // catalog and the supervisor's passive scan read.
        let artifact = crate::cron::store::read_scheduled_jobs_artifact(
            &artifacts.join(crate::cron::store::SESSION_SCHEDULED_JOBS_FILENAME),
        );
        assert_eq!(artifact.len(), 1, "{artifact:?}");
        assert_eq!(artifact[0].prompt, "check the build");
    }

    /// The schedule text is the CLI's own grammar: an expression the store
    /// rejects must reach the model as an error, not as an accepted job.
    #[test]
    fn an_unparseable_schedule_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ScheduleTool::new(target(dir.path(), "live-1"));
        let error = tool
            .dispatch(&serde_json::json!({
                "action": "add",
                "schedule": "not a schedule",
                "prompt": "check the build",
            }))
            .unwrap_err()
            .to_string();
        assert!(error.to_lowercase().contains("cron"), "{error}");
        assert_eq!(tool.store.list().len(), 0, "nothing was created");
        // A missing prompt is refused before the store is touched.
        let error = tool
            .dispatch(&serde_json::json!({
                "action": "add",
                "schedule": "every 30m",
                "prompt": "   ",
            }))
            .unwrap_err()
            .to_string();
        assert!(error.contains("prompt"), "{error}");
        assert_eq!(tool.store.list().len(), 0, "nothing was created");
    }

    /// An unknown action is named with the accepted set, so a misspelling is
    /// actionable rather than a silent no-op.
    #[test]
    fn an_unknown_action_names_the_accepted_set() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ScheduleTool::new(target(dir.path(), "live-1"));
        let error = tool
            .dispatch(&serde_json::json!({ "action": "delete" }))
            .unwrap_err()
            .to_string();
        assert!(error.contains("add, list, cancel"), "{error}");
    }
}
