//! `/heartbeat` job management: create/pause/resume/clear lifecycle plus
//! the heartbeat management actions (pause/resume/stop).

use uuid::Uuid;

use super::state::normalize_optional_label;
use super::{
    iso_from_millis, now_millis, AgentCronJobStore, CreateAgentCronJobInput, CronJobsState,
    HeartbeatManagementAction,
};
use crate::cron::{
    is_heartbeat_cron_job, next_run_at_for_schedule, parse_agent_cron_schedule, parse_iso_millis,
    AgentCronJob, JobStatus, ScheduleKind, DEFAULT_HEARTBEAT_DELIVERY_MODE,
};

/// The id of the heartbeat row that is CURRENT for `active_session_id`, in one state.
///
/// The one predicate `get_heartbeat` uses, expressed over a state the caller already
/// holds under lock. Management calls must select with THIS, not with a pre-lock read:
/// a create landing between the read and the lock supersedes the row, and a
/// pause/resume/clear of the superseded row acks an action on a heartbeat that no longer
/// exists while the live one keeps firing (review #2).
fn current_heartbeat_id(state: &CronJobsState, active_session_id: &str) -> Option<String> {
    state
        .jobs
        .iter()
        .filter(|job| {
            job.active_session_id == active_session_id
                && job.source.as_deref() == Some("heartbeat")
                && matches!(job.status, JobStatus::Active | JobStatus::Paused)
        })
        .max_by_key(|job| parse_iso_millis(&job.updated_at).unwrap_or(0))
        .map(|job| job.id.clone())
}

impl AgentCronJobStore {
    pub fn get_heartbeat(&self, active_session_id: &str) -> Option<AgentCronJob> {
        self.read_jobs()
            .into_iter()
            .filter(|job| {
                job.active_session_id == active_session_id
                    && job.source.as_deref() == Some("heartbeat")
                    && matches!(job.status, JobStatus::Active | JobStatus::Paused)
            })
            .max_by_key(|job| parse_iso_millis(&job.updated_at).unwrap_or(0))
    }

    pub fn get_latest_heartbeat(&self, active_session_id: &str) -> Option<AgentCronJob> {
        self.read_jobs()
            .into_iter()
            .filter(|job| {
                job.active_session_id == active_session_id
                    && job.source.as_deref() == Some("heartbeat")
            })
            .max_by_key(|job| parse_iso_millis(&job.updated_at).unwrap_or(0))
    }

    /// Create the heartbeat job for a session, cancelling its existing active/paused
    /// heartbeat first.
    ///
    /// # Errors
    ///
    /// Returns an error when the schedule is invalid or not recurring.
    pub fn create_heartbeat(
        &self,
        input: &CreateAgentCronJobInput,
    ) -> anyhow::Result<AgentCronJob> {
        let now = input.now.unwrap_or_else(now_millis);
        let parsed = parse_agent_cron_schedule(&input.schedule_text, now)?;
        if parsed.0.kind == ScheduleKind::Once {
            anyhow::bail!("Heartbeat schedule must be recurring");
        }
        let prompt = input.prompt.trim();
        if prompt.is_empty() {
            anyhow::bail!("Heartbeat instruction cannot be empty");
        }
        let now_iso = iso_from_millis(now);
        let cancel_stamp = now_iso.clone();
        let job = AgentCronJob {
            id: Uuid::new_v4().to_string(),
            status: JobStatus::Active,
            source: Some("heartbeat".to_string()),
            runtime_kind: input.runtime_kind.clone(),
            delivery_mode: Some(
                input
                    .delivery_mode
                    .unwrap_or(DEFAULT_HEARTBEAT_DELIVERY_MODE),
            ),
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
        // Cancel-and-add inside ONE lock, against the state as it is on disk:
        // read-then-write dropped a job another writer added in between, and a
        // timestamp tie kept the stale row (review #2).
        let created = job.clone();
        self.mutate_states_only(|state| {
            for existing in &mut state.jobs {
                if existing.active_session_id == input.active_session_id
                    && existing.source.as_deref() == Some("heartbeat")
                    && matches!(existing.status, JobStatus::Active | JobStatus::Paused)
                {
                    existing.status = JobStatus::Cancelled;
                    existing.next_run_at = None;
                    existing.updated_at.clone_from(&cancel_stamp);
                }
            }
            state.jobs.push(created.clone());
            Vec::new()
        })?;
        Ok(job)
    }
    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn pause_heartbeat(
        &self,
        active_session_id: &str,
        now: u64,
    ) -> anyhow::Result<Option<AgentCronJob>> {
        let now_iso = iso_from_millis(now);
        // The SELECTION and the write happen under ONE lock. Writing under the lock
        // was not enough: the row was chosen by `get_heartbeat` BEFORE the lock was
        // taken, so a create_heartbeat landing in the window left this call editing a
        // row that had just been superseded — it acknowledged a pause of a heartbeat
        // that no longer existed while the new one kept firing (review #2, the half
        // that survived the first fix). The same predicate `get_heartbeat` uses is
        // applied in-loop, so "the row I edited" and "the row that was current" cannot
        // disagree.
        let mut paused = None;
        self.mutate_states_only(|state| {
            let Some(current_id) = current_heartbeat_id(state, active_session_id) else {
                return Vec::new();
            };
            for job in &mut state.jobs {
                if job.id != current_id {
                    continue;
                }
                let paused_job = AgentCronJob {
                    status: JobStatus::Paused,
                    next_run_at: None,
                    updated_at: now_iso.clone(),
                    ..job.clone()
                };
                *job = paused_job.clone();
                paused = Some(paused_job);
                break;
            }
            Vec::new()
        })?;
        Ok(paused)
    }

    /// Resume the heartbeat for a session, recomputing its next run time. `Ok(None)`
    /// when the session has no heartbeat.
    ///
    /// # Errors
    ///
    /// Returns an error when the stored schedule is invalid or not recurring.
    pub fn resume_heartbeat(
        &self,
        active_session_id: &str,
        now: u64,
    ) -> anyhow::Result<Option<AgentCronJob>> {
        let now_iso = iso_from_millis(now);
        // Selection inside the lock, as in `pause_heartbeat`. The schedule is read from
        // the row being edited, not from a copy taken before the lock: a heartbeat
        // re-created with a different schedule in the window would otherwise resume to
        // the OLD schedule's next run.
        let mut resumed = None;
        let mut failure: Option<anyhow::Error> = None;
        self.mutate_states_only(|state| {
            let Some(current_id) = current_heartbeat_id(state, active_session_id) else {
                return Vec::new();
            };
            for job in &mut state.jobs {
                if job.id != current_id {
                    continue;
                }
                match next_run_at_for_schedule(&job.schedule, now) {
                    Ok(Some(next_run_at)) => {
                        let resumed_job = AgentCronJob {
                            status: JobStatus::Active,
                            next_run_at: Some(iso_from_millis(next_run_at)),
                            updated_at: now_iso.clone(),
                            ..job.clone()
                        };
                        *job = resumed_job.clone();
                        resumed = Some(resumed_job);
                    }
                    Ok(None) => {
                        failure = Some(anyhow::anyhow!("Heartbeat schedule must be recurring"));
                    }
                    Err(error) => failure = Some(error),
                }
                break;
            }
            Vec::new()
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(resumed)
    }

    /// # Errors
    ///
    /// Returns an error when the state lock or write fails.
    pub fn clear_heartbeat(
        &self,
        active_session_id: &str,
        now: u64,
    ) -> anyhow::Result<Option<AgentCronJob>> {
        let now_iso = iso_from_millis(now);
        // Selection inside the lock (review #2): see `pause_heartbeat`.
        let mut cleared = None;
        self.mutate_states_only(|state| {
            let Some(current_id) = current_heartbeat_id(state, active_session_id) else {
                return Vec::new();
            };
            for job in &mut state.jobs {
                if job.id != current_id {
                    continue;
                }
                let cleared_job = AgentCronJob {
                    status: JobStatus::Cancelled,
                    next_run_at: None,
                    updated_at: now_iso.clone(),
                    ..job.clone()
                };
                *job = cleared_job.clone();
                cleared = Some(cleared_job);
                break;
            }
            Vec::new()
        })?;
        Ok(cleared)
    }

    /// Apply a pause, stop, or resume management action to a heartbeat job. `Ok(None)`
    /// when no matching job was found (cancelled and completed jobs are left untouched).
    ///
    /// # Errors
    ///
    /// Returns an error when resuming a job whose stored schedule is invalid.
    pub fn manage_heartbeat(
        &self,
        active_session_id: &str,
        id: &str,
        action: HeartbeatManagementAction,
        now: u64,
    ) -> anyhow::Result<Option<AgentCronJob>> {
        let now_iso = iso_from_millis(now);
        // One lock for the read and the write (review #2). The resume arm used to
        // write twice — once for the status change, once for the recomputed
        // `next_run_at` — so a concurrent writer's job could be dropped by either.
        let mut updated = None;
        let mut failure: Option<anyhow::Error> = None;
        self.mutate_states_only(|state| {
            for job in &mut state.jobs {
                if job.id != id
                    || job.active_session_id != active_session_id
                    || !is_heartbeat_cron_job(job)
                {
                    continue;
                }
                if matches!(job.status, JobStatus::Cancelled | JobStatus::Completed) {
                    continue;
                }
                let mut next_job = match action {
                    HeartbeatManagementAction::Pause => AgentCronJob {
                        status: JobStatus::Paused,
                        next_run_at: None,
                        updated_at: now_iso.clone(),
                        ..job.clone()
                    },
                    HeartbeatManagementAction::Stop => AgentCronJob {
                        status: JobStatus::Cancelled,
                        next_run_at: None,
                        updated_at: now_iso.clone(),
                        ..job.clone()
                    },
                    HeartbeatManagementAction::Resume => AgentCronJob {
                        status: JobStatus::Active,
                        updated_at: now_iso.clone(),
                        ..job.clone()
                    },
                };
                if action == HeartbeatManagementAction::Resume {
                    match next_run_at_for_schedule(&next_job.schedule, now) {
                        Ok(Some(next_run_at)) => {
                            next_job.next_run_at = Some(iso_from_millis(next_run_at));
                        }
                        Ok(None) => {
                            failure = Some(anyhow::anyhow!("Heartbeat schedule must be recurring"));
                            break;
                        }
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
                *job = next_job.clone();
                updated = Some(next_job);
                break;
            }
            Vec::new()
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(updated)
    }
}

#[cfg(test)]
impl AgentCronJobStore {
    /// The whole state as stored, for a test that must land a competing write between
    /// another call's selection and its lock.
    fn read_state_for_test(&self) -> CronJobsState {
        crate::cron::store::state::read_jobs_state(&self.require_file_path())
    }

    fn write_state_for_test(&self, state: &CronJobsState) {
        crate::cron::store::state::write_jobs_state(&self.require_file_path(), state)
            .expect("write the competing state");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::store::input;

    #[test]
    fn pause_does_not_ack_when_state_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let store = AgentCronJobStore::new(path.clone());
        let now = 1_700_000_000_000;
        let heartbeat = store
            .create_heartbeat(&input("continue", "every 5m", now))
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let held =
            crate::platform::lock_dir::LockDir::acquire(&path, std::time::Duration::from_secs(30))
                .unwrap();
        assert!(store.pause_heartbeat("live-1", now + 1).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(held);
        assert_eq!(store.get_heartbeat("live-1"), Some(heartbeat));
    }

    #[test]
    fn heartbeat_lifecycle() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000;
        let heartbeat = store
            .create_heartbeat(&input("continue the mission", "every 5m", now))
            .unwrap();
        assert_eq!(store.get_heartbeat("live-1").unwrap().id, heartbeat.id);
        let paused = store.pause_heartbeat("live-1", now + 1).unwrap().unwrap();
        assert_eq!(paused.status, JobStatus::Paused);
        assert_eq!(paused.next_run_at, None);
        let resumed = store.resume_heartbeat("live-1", now + 2).unwrap().unwrap();
        assert_eq!(resumed.status, JobStatus::Active);
        assert!(resumed.next_run_at.is_some());
        let second = store
            .create_heartbeat(&input("new instruction", "every 2m", now + 3))
            .unwrap();
        assert_eq!(store.get_heartbeat("live-1").unwrap().id, second.id);
        assert_eq!(store.list().len(), 2);
        let jobs = store.list();
        let cancelled_first = jobs.iter().find(|job| job.id != second.id).unwrap();
        assert_eq!(cancelled_first.status, JobStatus::Cancelled);
        let cleared = store.clear_heartbeat("live-1", now + 4).unwrap().unwrap();
        assert_eq!(cleared.status, JobStatus::Cancelled);
        assert!(store
            .create_heartbeat(&input("nope", "in 10m", now))
            .is_err());
    }

    /// The predicate the three management calls select with: newest live heartbeat.
    ///
    /// One owner for "which row is current", so `get_heartbeat` (a read, outside any
    /// lock) and the in-lock selection of pause/resume/clear cannot drift apart. The
    /// three filters are the ones the finding turns on: a heartbeat source, a live
    /// status, and the newest stamp — a cancelled row is not current however recent.
    #[test]
    fn the_current_heartbeat_is_the_newest_live_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentCronJobStore::new(dir.path().join("jobs.json"));
        let now = 1_700_000_000_000_u64;
        let first = store
            .create_heartbeat(&input("first", "every 5m", now))
            .unwrap();
        let second = store
            .create_heartbeat(&input("second", "every 5m", now + 1000))
            .unwrap();

        let state = store.read_state_for_test();
        assert_eq!(
            current_heartbeat_id(&state, "live-1").as_deref(),
            Some(second.id.as_str()),
            "the newest live heartbeat is current; the one it replaced is cancelled"
        );
        assert_ne!(first.id, second.id);

        // A session with no heartbeat has no current row.
        assert!(current_heartbeat_id(&state, "other-session").is_none());
    }

    /// The selection happens INSIDE the state lock, not before it.
    ///
    /// This is the mechanism the finding is about, and it is not observable from a
    /// single-threaded call: writing under the lock while choosing the row before it
    /// looks identical from outside until a competing writer lands in the window. So the
    /// check is on the mechanism — the three management calls must not select with
    /// `get_heartbeat`, which takes no lock. Red on the pre-fix code, where each of the
    /// three began with `let Some(current) = self.get_heartbeat(..)`.
    #[test]
    fn management_calls_select_under_the_lock_not_before_it() {
        let source = include_str!("heartbeat.rs");
        let body_start = source
            .find("pub fn pause_heartbeat(")
            .expect("pause exists");
        let body_end = source
            .find("pub fn manage_heartbeat(")
            .expect("manage exists");
        let management = &source[body_start..body_end];
        assert!(
            !management.contains("get_heartbeat("),
            "pause/resume/clear still select their row with the UNLOCKED read: \n{management}"
        );
        assert!(
            management.contains("current_heartbeat_id(state"),
            "the selection must run on the state held under the lock"
        );
    }

    /// With the lock held by another writer, a pause still pauses the LIVE heartbeat.
    ///
    /// The competing write lands while this thread holds the state lock, so the pause
    /// cannot read state until it is released — and then it must act on what it finds,
    /// not on a copy taken beforehand. No fixed sleep: the lock itself orders the two.
    #[test]
    fn a_pause_after_a_competing_write_pauses_the_live_row() {
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let now = 1_700_000_000_000_u64;
        let store = AgentCronJobStore::new(path.clone());
        let first = store
            .create_heartbeat(&input("continue", "every 5m", now))
            .unwrap();

        let held =
            crate::platform::lock_dir::LockDir::acquire(&path, Duration::from_secs(30)).unwrap();
        let pauser = std::thread::spawn(move || {
            AgentCronJobStore::new(path)
                .pause_heartbeat("live-1", now + 2)
                .unwrap()
        });

        // The replacement, written while the pauser cannot hold the lock.
        let mut state = store.read_state_for_test();
        for job in &mut state.jobs {
            if job.id == first.id {
                job.status = JobStatus::Cancelled;
                job.next_run_at = None;
                job.updated_at = iso_from_millis(now + 1);
            }
        }
        let mut replacement = first;
        replacement.id = "replacement".to_string();
        replacement.status = JobStatus::Active;
        replacement.updated_at = iso_from_millis(now + 1);
        replacement.next_run_at = Some(iso_from_millis(now + 300_000));
        state.jobs.push(replacement);
        store.write_state_for_test(&state);
        drop(held);

        pauser.join().unwrap();
        let live = store
            .read_jobs()
            .into_iter()
            .find(|job| job.id == "replacement")
            .expect("the replacement exists");
        assert_eq!(
            live.status,
            JobStatus::Paused,
            "the pause must act on the live heartbeat, not on a pre-lock copy"
        );
    }

    /// A management call stamped BEHIND the row it edits is refused, not acked.
    ///
    /// The merge accepts a candidate only when it is at least as fresh, so a pause
    /// stamped before the row's own `updated_at` is discarded — while the old
    /// read-modify-write returned `Some(paused)` as if it had persisted. Callers got an
    /// acknowledgement for a change the store threw away.
    #[test]
    fn a_management_write_older_than_the_row_is_not_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let now = 1_700_000_000_000_u64;

        let store = AgentCronJobStore::new(path);
        let job = store
            .create_heartbeat(&input("continue", "every 5m", now))
            .unwrap();
        // The row is stamped later by a resume.
        store
            .resume_heartbeat("live-1", now + 60_000)
            .unwrap()
            .expect("resumed");

        // A pause stamped BEFORE that instant cannot win the merge.
        let paused = store.pause_heartbeat("live-1", now).unwrap();
        let on_disk = store
            .read_jobs()
            .into_iter()
            .find(|row| row.id == job.id)
            .expect("the job survives");

        assert!(
            !(paused.is_some() && on_disk.status != JobStatus::Paused),
            "the store acknowledged a pause it discarded: ack={paused:?} on_disk={on_disk:?}"
        );
    }
}
