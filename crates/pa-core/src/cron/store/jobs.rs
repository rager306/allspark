//! Generic cron job operations: create/list/cancel, session binding and
//! rebinding, run and skip result recording, and due-claim dispatch with
//! result recording and interrupted-dispatch recovery.

use uuid::Uuid;

use super::state::{
    claim_due_in_state, compare_optional_iso, normalize_optional_label,
    recover_interrupted_in_state, resolve_path,
};
use super::{
    iso_from_millis, now_millis, AgentCronDispatch, AgentCronJobStore, CancelJobsFilter,
    CreateAgentCronJobInput, DispatchResultOptions, RecordRunOptions, SessionBinding,
};
use crate::cron::{
    is_due_job, next_run_at_for_schedule, parse_agent_cron_schedule, parse_iso_millis,
    AgentCronJob, JobStatus, ScheduleKind,
};

impl AgentCronJobStore {
    pub fn list(&self) -> Vec<AgentCronJob> {
        let mut jobs = self.read_jobs();
        jobs.sort_by(|left, right| {
            compare_optional_iso(left.next_run_at.as_deref(), right.next_run_at.as_deref())
        });
        jobs
    }

    /// Create a cron job from the input schedule and prompt.
    ///
    /// # Errors
    ///
    /// Returns an error when the prompt is empty or the schedule text cannot be parsed.
    pub fn create(&self, input: &CreateAgentCronJobInput) -> anyhow::Result<AgentCronJob> {
        let prompt = input.prompt.trim();
        if prompt.is_empty() {
            anyhow::bail!("Cron job prompt cannot be empty");
        }
        let now = input.now.unwrap_or_else(now_millis);
        let parsed = parse_agent_cron_schedule(&input.schedule_text, now)?;
        let now_iso = iso_from_millis(now);
        let job = AgentCronJob {
            id: Uuid::new_v4().to_string(),
            status: JobStatus::Active,
            source: Some(input.source.clone().unwrap_or_else(|| "cron".to_string())),
            runtime_kind: input.runtime_kind.clone(),
            delivery_mode: None,
            active_session_id: input.active_session_id.clone(),
            session_id: input.session_id.clone(),
            session_file: input.session_file.clone(),
            cwd: input.cwd.clone(),
            label: normalize_optional_label(input.label.as_deref()),
            prompt: prompt.to_string(),
            schedule: parsed.0,
            created_at: now_iso.clone(),
            updated_at: now_iso,
            next_run_at: Some(iso_from_millis(parsed.1)),
            last_run_at: None,
            last_skipped_at: None,
            last_error: None,
            run_count: 0,
        };
        let mut jobs = self.read_jobs();
        jobs.push(job.clone());
        self.write_jobs(&jobs)?;
        Ok(job)
    }

    /// Bind jobs stored for a session file to a live session id on restore, or
    /// move a live session's jobs to a new file when it switches.
    ///
    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn rebind_session_jobs(&self, input: &SessionBinding) -> anyhow::Result<Vec<AgentCronJob>> {
        let target_session_file = resolve_path(&input.session_file);
        let mut rebound_jobs = Vec::new();
        let jobs: Vec<AgentCronJob> = self
            .read_jobs()
            .into_iter()
            .map(|job| {
                if job.active_session_id != input.active_session_id
                    && resolve_path(&job.session_file) != target_session_file
                {
                    return job;
                }
                if job.active_session_id == input.active_session_id
                    && job.session_id == input.session_id
                    && resolve_path(&job.session_file) == target_session_file
                    && job.cwd == input.cwd
                {
                    return job;
                }
                let rebound = AgentCronJob {
                    active_session_id: input.active_session_id.clone(),
                    session_id: input.session_id.clone(),
                    session_file: input.session_file.clone(),
                    cwd: input.cwd.clone(),
                    ..job
                };
                rebound_jobs.push(rebound.clone());
                rebound
            })
            .collect();
        if !rebound_jobs.is_empty() {
            self.write_jobs(&jobs)?;
        }
        Ok(rebound_jobs)
    }
    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn cancel_jobs_for_session(
        &self,
        input: &CancelJobsFilter,
        now: u64,
    ) -> anyhow::Result<Vec<AgentCronJob>> {
        let now_iso = iso_from_millis(now);
        let target_session_file = input.session_file.as_ref().map(|file| resolve_path(file));
        // One lock for the read and the write (review #2).
        let mut cancelled = Vec::new();
        self.mutate_states_only(|state| {
            for job in &mut state.jobs {
                let matches = input
                    .active_session_id
                    .as_ref()
                    .is_some_and(|id| *id == job.active_session_id)
                    || input
                        .session_id
                        .as_ref()
                        .is_some_and(|id| *id == job.session_id)
                    || target_session_file
                        .as_ref()
                        .is_some_and(|file| resolve_path(&job.session_file) == *file);
                if !matches || !matches!(job.status, JobStatus::Active | JobStatus::Paused) {
                    continue;
                }
                *job = AgentCronJob {
                    status: JobStatus::Cancelled,
                    next_run_at: None,
                    updated_at: now_iso.clone(),
                    ..job.clone()
                };
                cancelled.push(job.clone());
            }
            Vec::new()
        })?;
        Ok(cancelled)
    }
    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn cancel(&self, id: &str, now: u64) -> anyhow::Result<Option<AgentCronJob>> {
        let now_iso = iso_from_millis(now);
        // One lock for the read and the write (review #2): read-then-write dropped a
        // job another writer added in between, and the merge's freshness rule could
        // keep the stale row while this call still acknowledged a cancel.
        let mut cancelled = None;
        self.mutate_states_only(|state| {
            for job in &mut state.jobs {
                if job.id != id || job.status == JobStatus::Cancelled {
                    continue;
                }
                *job = AgentCronJob {
                    status: JobStatus::Cancelled,
                    next_run_at: None,
                    updated_at: now_iso.clone(),
                    ..job.clone()
                };
                cancelled = Some(job.clone());
            }
            Vec::new()
        })?;
        Ok(cancelled)
    }

    /// Record one run: bump counters, roll `nextRunAt`, complete one-shots.
    ///
    /// # Errors
    ///
    /// The current implementation never returns `Err`.
    pub fn record_run_result(
        &self,
        id: &str,
        result: &RecordRunOptions,
    ) -> anyhow::Result<Option<AgentCronJob>> {
        let now = result.now.unwrap_or_else(now_millis);
        let now_iso = iso_from_millis(now);
        // One lock for the read and the write (review #2).
        let mut updated = None;
        self.mutate_states_only(|state| {
            for job in &mut state.jobs {
                if job.id != id {
                    continue;
                }
                if job.status != JobStatus::Active {
                    updated = Some(job.clone());
                    continue;
                }
                let next_run_at = match job.schedule.kind {
                    ScheduleKind::Cron => next_run_at_for_schedule(&job.schedule, now + 1)
                        .ok()
                        .flatten()
                        .map(iso_from_millis),
                    ScheduleKind::Interval => next_run_at_for_schedule(&job.schedule, now)
                        .ok()
                        .flatten()
                        .map(iso_from_millis),
                    ScheduleKind::Once => None,
                };
                job.status = if job.schedule.kind == ScheduleKind::Once {
                    JobStatus::Completed
                } else {
                    JobStatus::Active
                };
                job.next_run_at = next_run_at;
                job.last_run_at = Some(now_iso.clone());
                job.last_error.clone_from(&result.error);
                job.run_count += 1;
                job.updated_at.clone_from(&now_iso);
                updated = Some(job.clone());
            }
            Vec::new()
        })?;
        Ok(updated)
    }

    /// Record a skipped run: roll `nextRunAt`, stamp `lastSkippedAt`.
    ///
    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn record_skip_result(&self, id: &str, now: u64) -> anyhow::Result<Option<AgentCronJob>> {
        let now_iso = iso_from_millis(now);
        // One lock for the read and the write (review #2).
        let mut updated = None;
        self.mutate_states_only(|state| {
            for job in &mut state.jobs {
                if job.id != id {
                    continue;
                }
                if job.status != JobStatus::Active {
                    updated = Some(job.clone());
                    continue;
                }
                job.next_run_at = next_run_at_for_schedule(&job.schedule, now)
                    .ok()
                    .flatten()
                    .map(iso_from_millis);
                job.last_skipped_at = Some(now_iso.clone());
                job.updated_at.clone_from(&now_iso);
                updated = Some(job.clone());
            }
            Vec::new()
        })?;
        Ok(updated)
    }

    /// Push a job's next run later, never earlier (the scheduler's consecutive-failure backoff); an
    /// already-later `nextRunAt` wins.
    ///
    /// Runs under the store's state locks: a cancel landing between the failure and this
    /// defer must stay cancelled, so the status check and the write are one atomic
    /// state mutation.
    ///
    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn defer_next_run(&self, id: &str, until_ms: u64) -> anyhow::Result<Option<AgentCronJob>> {
        let until_iso = iso_from_millis(until_ms);
        let mut updated = None;
        self.mutate_states_only(|state| {
            state.jobs = state
                .jobs
                .clone()
                .into_iter()
                .map(|mut job| {
                    if job.id != id || job.status != JobStatus::Active {
                        return job;
                    }
                    if let Some(current) = job.next_run_at.as_deref().and_then(parse_iso_millis) {
                        if current >= until_ms {
                            return job;
                        }
                    }
                    job.next_run_at = Some(until_iso.clone());
                    job.updated_at = iso_from_millis(now_millis());
                    updated = Some(job.clone());
                    job
                })
                .collect();
            Vec::new()
        })?;
        Ok(updated)
    }

    pub fn due(&self, now: u64) -> Vec<AgentCronJob> {
        self.read_jobs()
            .into_iter()
            .filter(|job| is_due_job(job, now))
            .collect()
    }

    /// Atomically claim due jobs: advance their schedule and record dispatches.
    ///
    /// A multi-partition claim can fail on one partition after another's rows
    /// were already marked claimed on disk. Those claims are REAL work: returning
    /// only the error would leave them never dispatched, indistinguishable from
    /// interrupted work until a later recovery pass (review #3). The landed
    /// dispatches therefore come back beside the error.
    ///
    /// # Errors
    ///
    /// Returns `(error, landed)` when a partition write or the state lock fails.
    pub fn claim_due(
        &self,
        due_at: u64,
        claimed_at: u64,
    ) -> Result<Vec<AgentCronDispatch>, (anyhow::Error, Vec<AgentCronDispatch>)> {
        self.mutate_states(|state| claim_due_in_state(state, due_at, claimed_at))
    }

    pub fn get_claimed_job(&self, id: &str) -> Option<AgentCronJob> {
        for state in self.read_states() {
            if !state
                .dispatches
                .iter()
                .any(|dispatch| dispatch.job_id == id)
            {
                continue;
            }
            return state
                .jobs
                .into_iter()
                .find(|job| job.id == id && job.status == JobStatus::Active);
        }
        None
    }

    /// Record the outcome of a claimed dispatch: release the claim, roll the job's next run, and
    /// complete one-shots. Returns the updated job.
    ///
    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn record_dispatch_result(
        &self,
        dispatch_id: &str,
        result: &DispatchResultOptions,
    ) -> anyhow::Result<Option<AgentCronJob>> {
        let now = result.now.unwrap_or_else(now_millis);
        let now_iso = iso_from_millis(now);
        let mut updated = None;
        self.mutate_states_only(|state| {
            let Some(dispatch) = state
                .dispatches
                .iter()
                .find(|candidate| candidate.id == dispatch_id)
                .cloned()
            else {
                return Vec::new();
            };
            state
                .dispatches
                .retain(|candidate| candidate.id != dispatch_id);
            state.jobs = state
                .jobs
                .clone()
                .into_iter()
                .map(|mut job| {
                    if job.id != dispatch.job_id || job.status != JobStatus::Active {
                        return job;
                    }
                    if result.outcome == "skipped" && result.error.is_none() {
                        job.status = if job.schedule.kind == ScheduleKind::Once {
                            JobStatus::Completed
                        } else {
                            job.status
                        };
                        job.next_run_at = next_run_at_for_schedule(&job.schedule, now)
                            .ok()
                            .flatten()
                            .map(iso_from_millis);
                        job.last_skipped_at = Some(now_iso.clone());
                        job.updated_at.clone_from(&now_iso);
                    } else {
                        // A ONE-SHOT that FAILED has not delivered the wake it represents
                        // (review #13): completing it drops the only scheduled wake a
                        // quota-resume or goal-backoff job has, and the scheduler's failure
                        // backoff then finds a non-active row and cannot re-arm it. The row
                        // stays active with no next run — the shape `defer_next_run` moves.
                        let completed =
                            job.schedule.kind == ScheduleKind::Once && result.error.is_none();
                        job.status = if completed {
                            JobStatus::Completed
                        } else {
                            job.status
                        };
                        job.last_run_at = Some(now_iso.clone());
                        job.last_error.clone_from(&result.error);
                        job.run_count += 1;
                        job.updated_at.clone_from(&now_iso);
                    }
                    updated = Some(job.clone());
                    job
                })
                .collect();
            Vec::new()
        })?;
        Ok(updated)
    }

    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn recover_interrupted_dispatches(&self, now: u64) -> anyhow::Result<Vec<AgentCronJob>> {
        let mut recovered = Vec::new();
        self.mutate_states_only(|state| {
            recover_interrupted_in_state(state, now, &mut recovered, None);
            Vec::new()
        })?;
        Ok(recovered)
    }

    pub fn next_active_run_at(&self) -> Option<u64> {
        self.read_jobs()
            .iter()
            .filter(|job| job.status == JobStatus::Active)
            .filter_map(|job| job.next_run_at.as_deref().and_then(parse_iso_millis))
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::store::input;

    #[test]
    fn create_list_and_cancel() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000;
        let job = store
            .create(&input("check the build", "every 10m", now))
            .unwrap();
        assert_eq!(job.status, JobStatus::Active);
        assert_eq!(job.source.as_deref(), Some("cron"));
        assert_eq!(job.schedule.kind, ScheduleKind::Interval);
        assert_eq!(store.list().len(), 1);
        let due = store.due(now + 600_000);
        assert!(due.iter().any(|found| found.id == job.id));
        let cancelled = store.cancel(&job.id, now + 1).unwrap().unwrap();
        assert_eq!(cancelled.status, JobStatus::Cancelled);
        assert_eq!(store.list()[0].status, JobStatus::Cancelled);
        assert!(store.create(&input("  ", "every 10m", now)).is_err());
    }

    #[test]
    fn cancel_does_not_ack_when_state_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let store = AgentCronJobStore::new(path.clone());
        let now = 1_700_000_000_000;
        let job = store.create(&input("tick", "every 10m", now)).unwrap();
        let before = std::fs::read(&path).unwrap();
        let held =
            crate::platform::lock_dir::LockDir::acquire(&path, std::time::Duration::from_secs(30))
                .unwrap();
        assert!(store.cancel(&job.id, now + 1).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(held);
        assert_eq!(store.list(), vec![job]);
    }

    #[test]
    fn claim_dispatch_and_result_recording() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000;
        let job = store.create(&input("tick", "every 10m", now)).unwrap();
        assert_eq!(store.claim_due(now, now).unwrap().len(), 0);
        let dispatches = store.claim_due(now + 600_000, now + 600_000).unwrap();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].job.id, job.id);
        assert_eq!(
            dispatches[0].job.next_run_at.as_deref(),
            Some(iso_from_millis(now + 1_200_000).as_str())
        );
        assert!(store.get_claimed_job(&job.id).is_some());
        let updated = store
            .record_dispatch_result(
                &dispatches[0].id,
                &DispatchResultOptions {
                    now: Some(now + 600_001),
                    outcome: "ran",
                    error: None,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.run_count, 1);
        assert!(store.get_claimed_job(&job.id).is_none());
        let second = store.claim_due(now + 1_200_000, now + 1_200_000).unwrap();
        assert_eq!(second.len(), 1);
        let recovered = store
            .recover_interrupted_dispatches(now + 1_300_000)
            .unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].last_error.as_deref(),
            Some("Interrupted before scheduled operation completion")
        );
    }

    #[test]
    fn run_and_skip_results_roll_schedules() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000;
        let job = store.create(&input("tick", "every 10m", now)).unwrap();
        let updated = store
            .record_run_result(
                &job.id,
                &RecordRunOptions {
                    now: Some(now),
                    error: None,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.run_count, 1);
        assert_eq!(
            updated.last_run_at.as_deref(),
            Some(iso_from_millis(now).as_str())
        );
        let mut once_input = input("one and done", "in 10m", now);
        once_input.session_id = "session-once".to_string();
        let once = store.create(&once_input).unwrap();
        let once_done = store
            .record_run_result(
                &once.id,
                &RecordRunOptions {
                    now: Some(now),
                    error: None,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(once_done.status, JobStatus::Completed);
        store.record_skip_result(&job.id, now + 60_000).unwrap();
        assert!(store.list().iter().any(|job| job.last_skipped_at.is_some()));
    }

    #[test]
    fn claim_due_skips_while_the_state_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000;
        let job = store.create(&input("tick", "every 10m", now)).unwrap();
        let path = store.require_file_path();
        let before = std::fs::read(&path).unwrap();
        {
            let _held = crate::platform::lock_dir::LockDir::acquire(
                &path,
                std::time::Duration::from_secs(30),
            )
            .unwrap();
            assert!(store.claim_due(now + 600_000, now + 600_000).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let dispatches = store.claim_due(now + 600_000, now + 600_000).unwrap();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].job.id, job.id);
    }

    #[test]
    fn defer_next_run_pushes_only_later() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000;
        let job = store.create(&input("tick", "every 10m", now)).unwrap();
        let scheduled_next = now + 600_000;
        assert!(store
            .defer_next_run(&job.id, now + 60_000)
            .unwrap()
            .is_none());
        let current = store.list().pop().expect("job kept");
        assert_eq!(
            crate::cron::parse_iso_millis(current.next_run_at.as_deref().unwrap()),
            Some(scheduled_next)
        );
        let deferred = now + 900_000;
        let updated = store
            .defer_next_run(&job.id, deferred)
            .unwrap()
            .expect("deferred");
        assert_eq!(
            crate::cron::parse_iso_millis(updated.next_run_at.as_deref().unwrap()),
            Some(deferred)
        );
        assert!(store
            .defer_next_run(&job.id, now + 120_000)
            .unwrap()
            .is_none());
        let current = store.list().pop().expect("job kept");
        assert_eq!(
            crate::cron::parse_iso_millis(current.next_run_at.as_deref().unwrap()),
            Some(deferred)
        );
        // Cancelled jobs are never deferred. Fresh clock: the jobs file merges on
        // `updatedAt` freshness, so a stale stamp would lose the cancel.
        store.cancel(&job.id, now_millis()).unwrap();
        assert!(store
            .defer_next_run(&job.id, now + 2_000_000)
            .unwrap()
            .is_none());
    }

    /// A one-shot whose run FAILED has not delivered its wake (review #13).
    ///
    /// `record_dispatch_result` completed a `once` job in both arms. The failure arm runs
    /// with an error, i.e. the prompt did NOT run — a quota-resume or goal-backoff wake
    /// whose fire hit a provider outage was marked `completed` and the scheduler's
    /// `defer_next_run` then found a non-active row and could not re-arm it, so one bad
    /// fire deleted the only scheduled wake. The row must stay active (with no next run,
    /// the shape the backoff moves forward) and the error must be recorded.
    #[test]
    fn a_failed_one_shot_fire_keeps_its_wake() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000;
        let job = store
            .create(&input("resume the parked goal", "in 1m", now))
            .unwrap();
        assert_eq!(job.schedule.kind, ScheduleKind::Once);
        let dispatch = store.claim_due(now + 60_000, now + 60_000).unwrap();
        assert_eq!(dispatch.len(), 1, "the due one-shot is claimed");

        let failed = store
            .record_dispatch_result(
                &dispatch[0].id,
                &DispatchResultOptions {
                    now: Some(now + 60_001),
                    outcome: "ran",
                    error: Some("502 provider outage".to_string()),
                },
            )
            .unwrap()
            .expect("the job is updated");
        assert_eq!(
            failed.status,
            JobStatus::Active,
            "a failed fire is not a delivered wake: {:?}",
            failed.status
        );
        assert_eq!(failed.run_count, 1);
        assert_eq!(failed.last_error.as_deref(), Some("502 provider outage"));

        // The scheduler's failure backoff re-arms it (a completed row would refuse).
        let rearmed = store
            .defer_next_run(&job.id, now + 60_000 + 120_000)
            .unwrap()
            .expect("an active one-shot takes the backoff");
        assert_eq!(
            crate::cron::parse_iso_millis(rearmed.next_run_at.as_deref().unwrap()),
            Some(now + 60_000 + 120_000)
        );

        // A one-shot that RAN cleanly still completes.
        let second = store.claim_due(now + 300_000, now + 300_000).unwrap();
        assert_eq!(second.len(), 1, "the re-armed one-shot is due again");
        let done = store
            .record_dispatch_result(
                &second[0].id,
                &DispatchResultOptions {
                    now: Some(now + 300_001),
                    outcome: "ran",
                    error: None,
                },
            )
            .unwrap()
            .expect("the job is updated");
        assert_eq!(done.status, JobStatus::Completed);
    }

    /// `cancel` must not acknowledge a change the merge discarded (review #2).
    ///
    /// Same shape as the heartbeat management test: a cancel stamped BEFORE the
    /// row's own `updated_at` loses the freshness merge, and the old
    /// read-modify-write still returned `Some(cancelled)`. A caller that trusts the
    /// acknowledgement reports a job as cancelled while it keeps running.
    #[test]
    fn a_cancel_older_than_the_row_is_not_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let now = 1_700_000_000_000_u64;
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let job = store
            .create(&input("nightly", "every 5m", now))
            .expect("create");

        // A later stamp on the row: the freshness merge now refuses older candidates.
        store
            .record_skip_result(&job.id, now + 60_000)
            .expect("skip")
            .expect("the job is active");

        // A cancel stamped BEFORE that instant cannot win the merge.
        let cancelled = store.cancel(&job.id, now).expect("cancel");
        let on_disk = store
            .read_jobs()
            .into_iter()
            .find(|row| row.id == job.id)
            .expect("the job survives");

        assert!(
            !(cancelled.is_some() && on_disk.status != JobStatus::Cancelled),
            "the store acknowledged a cancel it discarded: ack={cancelled:?} on_disk={on_disk:?}"
        );
    }
}
