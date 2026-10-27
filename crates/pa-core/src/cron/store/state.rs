//! State save/load for the cron job store: cross-process file locking
//! (lockfile with stale takeover), state read/write/merge helpers, and the
//! in-state due-claim / interrupted-dispatch recovery transitions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{
    iso_from_millis, AgentCronDispatch, AgentCronDispatchRecord, AgentCronJob, AgentCronJobStore,
    CronJobsState,
};
use crate::cron::{
    is_due_job, next_run_at_for_schedule, parse_iso_millis, JobStatus, ScheduleKind,
};

const LOCK_STALE_MS: u64 = 30_000;

impl AgentCronJobStore {
    pub(crate) fn read_jobs(&self) -> Vec<AgentCronJob> {
        self.read_states()
            .into_iter()
            .flat_map(|state| state.jobs)
            .collect()
    }

    pub(crate) fn read_states(&self) -> Vec<CronJobsState> {
        if self.session_artifact_mode {
            let files = self
                .session_artifact_files
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            return files.values().map(|path| read_jobs_state(path)).collect();
        }
        vec![read_jobs_state(&self.require_file_path())]
    }

    pub(crate) fn mutate_states(
        &self,
        mut mutator: impl FnMut(&mut CronJobsState) -> Vec<AgentCronDispatch>,
    ) -> anyhow::Result<Vec<AgentCronDispatch>> {
        let paths: Vec<PathBuf> = if self.session_artifact_mode {
            self.session_artifact_files
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .cloned()
                .collect()
        } else {
            vec![self.require_file_path()]
        };
        let previous_heartbeats = Self::heartbeat_catalog_signature(&self.read_jobs());
        let mut changed = false;
        let dispatches = with_state_locks(&paths, || {
            let mut dispatches = Vec::new();
            for path in &paths {
                let mut state = read_jobs_state(path);
                let before = serde_json::to_string(&state).unwrap_or_default();
                dispatches.extend(mutator(&mut state));
                if serde_json::to_string(&state).unwrap_or_default() != before {
                    write_jobs_state(path, &state)?;
                    changed = true;
                }
            }
            Ok::<_, anyhow::Error>(dispatches)
        })
        .ok_or_else(|| anyhow::anyhow!("cron jobs state lock not acquired; skipped"))??;
        if changed && Self::heartbeat_catalog_signature(&self.read_jobs()) != previous_heartbeats {
            self.notify_heartbeat_change();
        }
        Ok(dispatches)
    }

    pub(crate) fn write_jobs(&self, jobs: &[AgentCronJob]) -> anyhow::Result<()> {
        let previous_heartbeats = Self::heartbeat_catalog_signature(&self.read_jobs());
        if self.session_artifact_mode {
            self.write_jobs_session_artifacts(jobs)?;
        } else {
            let path = self.require_file_path();
            with_state_locks(std::slice::from_ref(&path), || {
                write_jobs_file(&path, jobs, true)
            })
            .ok_or_else(|| anyhow::anyhow!("cron jobs state lock not acquired; skipped"))??;
        }
        if Self::heartbeat_catalog_signature(&self.read_jobs()) != previous_heartbeats {
            self.notify_heartbeat_change();
        }
        Ok(())
    }

    pub(crate) fn require_file_path(&self) -> PathBuf {
        self.file_path
            .clone()
            .expect("Cron job store requires a file path")
    }
}

pub(crate) fn resolve_path(path: &str) -> String {
    let clean = std::path::Path::new(path);
    let mut components: Vec<std::ffi::OsString> = Vec::new();
    for component in clean.components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop();
            }
            other => components.push(other.as_os_str().to_os_string()),
        }
    }
    let mut resolved = PathBuf::from("/");
    for component in components {
        resolved.push(component);
    }
    resolved.to_string_lossy().to_string()
}

pub(crate) fn compare_optional_iso(left: Option<&str>, right: Option<&str>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (left, right) {
        (left, right) if left == right => Ordering::Equal,
        (None, _) => Ordering::Greater,
        (_, None) => Ordering::Less,
        (Some(left), Some(right)) => parse_iso_millis(left)
            .unwrap_or(0)
            .cmp(&parse_iso_millis(right).unwrap_or(0)),
    }
}

pub(crate) fn normalize_optional_label(label: Option<&str>) -> Option<String> {
    let trimmed = label.map(str::trim).filter(|label| !label.is_empty());
    trimmed.map(std::string::ToString::to_string)
}

pub(crate) fn merge_fresh_jobs(
    current_jobs: Vec<AgentCronJob>,
    next_jobs: Vec<AgentCronJob>,
) -> Vec<AgentCronJob> {
    let mut merged: HashMap<String, AgentCronJob> = HashMap::new();
    for job in current_jobs {
        merged.insert(job.id.clone(), job);
    }
    for job in next_jobs {
        let is_fresh = merged
            .get(&job.id)
            .is_none_or(|current| is_at_least_as_fresh(&job, current));
        if is_fresh {
            merged.insert(job.id.clone(), job);
        }
    }
    merged.into_values().collect()
}

fn is_at_least_as_fresh(candidate: &AgentCronJob, current: &AgentCronJob) -> bool {
    let Some(current_time) = parse_iso_millis(&current.updated_at) else {
        return true;
    };
    let Some(candidate_time) = parse_iso_millis(&candidate.updated_at) else {
        return false;
    };
    candidate_time >= current_time
}

pub(crate) fn claim_due_in_state(
    state: &mut CronJobsState,
    due_at: u64,
    claimed_at: u64,
) -> Vec<AgentCronDispatch> {
    let claimed_iso = iso_from_millis(claimed_at);
    let claimed_job_ids: std::collections::HashSet<String> = state
        .dispatches
        .iter()
        .map(|dispatch| dispatch.job_id.clone())
        .collect();
    let mut dispatches = Vec::new();
    let jobs = std::mem::take(&mut state.jobs);
    let mut new_jobs = Vec::with_capacity(jobs.len());
    for mut job in jobs {
        if !is_due_job(&job, due_at) {
            new_jobs.push(job);
            continue;
        }
        let scheduled_for = job.next_run_at.clone().unwrap_or_default();
        let next_run_at = next_run_at_for_schedule(&job.schedule, claimed_at)
            .ok()
            .flatten()
            .map(iso_from_millis);
        job.next_run_at = next_run_at;
        job.updated_at.clone_from(&claimed_iso);
        if claimed_job_ids.contains(&job.id) {
            job.last_skipped_at = Some(claimed_iso.clone());
            new_jobs.push(job);
            continue;
        }
        let dispatch = AgentCronDispatchRecord {
            id: Uuid::new_v4().to_string(),
            job_id: job.id.clone(),
            claimed_at: claimed_iso.clone(),
            scheduled_for,
        };
        state.dispatches.push(dispatch.clone());
        dispatches.push(AgentCronDispatch {
            id: dispatch.id,
            job: job.clone(),
        });
        new_jobs.push(job);
    }
    state.jobs = new_jobs;
    dispatches
}

pub(crate) fn recover_interrupted_in_state(
    state: &mut CronJobsState,
    now: u64,
    recovered: &mut Vec<AgentCronJob>,
    dispatch_ids: Option<&std::collections::HashSet<String>>,
) {
    let interrupted: Vec<AgentCronDispatchRecord> = match dispatch_ids {
        Some(ids) => state
            .dispatches
            .iter()
            .filter(|dispatch| ids.contains(&dispatch.id))
            .cloned()
            .collect(),
        None => state.dispatches.clone(),
    };
    if interrupted.is_empty() {
        return;
    }
    let interrupted_ids: std::collections::HashSet<String> = interrupted
        .iter()
        .map(|dispatch| dispatch.job_id.clone())
        .collect();
    match dispatch_ids {
        Some(ids) => state
            .dispatches
            .retain(|dispatch| !ids.contains(&dispatch.id)),
        None => state.dispatches.clear(),
    }
    state.jobs = state
        .jobs
        .clone()
        .into_iter()
        .map(|mut job| {
            if !interrupted_ids.contains(&job.id) || job.status != JobStatus::Active {
                return job;
            }
            job.status = if job.schedule.kind == ScheduleKind::Once {
                JobStatus::Completed
            } else {
                job.status
            };
            job.last_error = Some("Interrupted before scheduled operation completion".to_string());
            job.updated_at = iso_from_millis(now);
            recovered.push(job.clone());
            job
        })
        .collect();
}

/// Cross-process state locks: lockfile with stale takeover, sorted by path.
pub(crate) fn with_state_locks<T>(paths: &[PathBuf], action: impl FnOnce() -> T) -> Option<T> {
    let mut unique: Vec<PathBuf> = paths.to_vec();
    unique.sort();
    unique.dedup();
    let mut guards: Vec<crate::platform::lock_dir::LockDir> = Vec::new();
    for path in &unique {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // TS `withCronJobsStateLocks`: proper-lockfile, 100 attempts x
        // 10ms, 30s staleness takeover.
        let stale = std::time::Duration::from_millis(LOCK_STALE_MS);
        match crate::platform::lock_dir::LockDir::acquire_retrying(
            path,
            stale,
            100,
            std::time::Duration::from_millis(10),
        ) {
            Ok(guard) => guards.push(guard),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    path = %path.display(),
                    "cron jobs state lock not acquired; skipped"
                );
                return None;
            }
        }
    }
    let result = action();
    drop(guards);
    Some(result)
}

pub(crate) fn read_jobs_state(path: &Path) -> CronJobsState {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return CronJobsState::default();
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return CronJobsState::default();
    };
    CronJobsState {
        jobs: parsed
            .get("jobs")
            .and_then(|jobs| jobs.as_array())
            .map(|jobs| {
                jobs.iter()
                    .filter_map(|job| serde_json::from_value::<AgentCronJob>(job.clone()).ok())
                    .collect()
            })
            .unwrap_or_default(),
        dispatches: parsed
            .get("dispatches")
            .and_then(|dispatches| dispatches.as_array())
            .map(|dispatches| {
                dispatches
                    .iter()
                    .filter_map(|dispatch| {
                        serde_json::from_value::<AgentCronDispatchRecord>(dispatch.clone()).ok()
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn write_jobs_file(path: &Path, jobs: &[AgentCronJob], merge_current: bool) -> anyhow::Result<()> {
    let current = read_jobs_state(path);
    let jobs = if merge_current {
        merge_fresh_jobs(current.jobs, jobs.to_vec())
    } else {
        jobs.to_vec()
    };
    write_jobs_state(
        path,
        &CronJobsState {
            jobs,
            dispatches: current.dispatches,
        },
    )
}

pub(crate) fn write_jobs_state(path: &Path, state: &CronJobsState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(state)?;
    // Cron state opts into the shared durable write (TS `writeJobsState`
    // passes `{ mode: 0o600, fsync: true }`).
    crate::settings::storage::atomic_write_with(
        path,
        &format!("{serialized}\n"),
        crate::settings::storage::AtomicWriteOptions { fsync: true },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_state_write_propagates_physical_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        std::fs::create_dir(&path).unwrap();
        assert!(write_jobs_state(&path, &CronJobsState::default()).is_err());
    }

    /// The cron state write opts in (the TS test pins `{ fsync: true, mode: 0o600 }`)
    /// `to_string_pretty(state) + "\n"` bytes.
    #[test]
    fn jobs_state_write_keeps_exactly_one_fsync() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let state = CronJobsState::default();
        let expected = format!("{}\n", serde_json::to_string_pretty(&state).unwrap());
        let before = crate::settings::storage::opt_in_fsync_calls();
        write_jobs_state(&path, &state).unwrap();
        assert_eq!(
            crate::settings::storage::opt_in_fsync_calls(),
            before + 1,
            "the cron state write must keep its one opt-in fsync"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        #[cfg(unix)]
        assert_eq!(
            crate::platform::perms::file_mode(&path),
            Some(0o600),
            "the durable write keeps the private mode"
        );
    }
}

/// Deterministic simulation of the cron CLAIM path — the `pa-daemon` scheduler's
/// queueing invariant, which no test asserted before (issue #14 AC2).
///
/// The property: two dispatch passes racing on one store must never claim the
/// SAME job twice. `claim_due_in_state` is the whole mechanism (it reads the
/// outstanding dispatch ids, then rolls each due job), and it is a pure
/// state mutation — so a scheduler can explore its interleavings without a
/// daemon, a clock, or a file.
///
/// The interleaving that matters is the one a normal test cannot force: pass A
/// reads `state.dispatches` and is preempted before it appends its claim, while
/// pass B reads the same (still empty) set. Both then append, and the job fires
/// twice in one window.
#[cfg(test)]
mod shuttle_claim_tests {
    use super::*;
    use crate::cron::AgentCronJob;

    fn state_with_one_due_job() -> CronJobsState {
        let job: AgentCronJob = serde_json::from_value(serde_json::json!({
            "id": "job-1",
            "activeSessionId": "s-1",
            "sessionId": "s-1",
            "sessionFile": "/tmp/s-1.jsonl",
            "cwd": "/tmp",
            "prompt": "ping",
            "schedule": { "kind": "interval", "expression": "each 30s", "intervalMs": 30000 },
            "status": "active",
            "nextRunAt": "2023-11-14T22:20:00.000Z",
            "createdAt": "2023-11-14T22:13:20.000Z",
            "updatedAt": "2023-11-14T22:13:20.000Z"
        }))
        .expect("the job fixture parses");
        CronJobsState {
            jobs: vec![job],
            dispatches: Vec::new(),
        }
    }

    /// The REAL guard, without concurrency: given a state that already carries
    /// an outstanding claim for a job, a later pass must not dispatch it again.
    ///
    /// This is the invariant the `Mutex` above merely serializes access to —
    /// `claim_due_in_state` reads the outstanding dispatch ids itself and
    /// records `last_skipped_at` instead of a second claim. Tested here because
    /// a lock proves nothing about the function under it.
    #[test]
    fn a_second_pass_skips_a_job_whose_claim_is_outstanding() {
        let mut state = state_with_one_due_job();
        let now = u64::MAX / 2;

        let first = claim_due_in_state(&mut state, now, now);
        assert_eq!(first.len(), 1, "the first pass claims the due job");
        assert_eq!(state.dispatches.len(), 1);

        // Roll the job due again, simulating the clock moving past its interval
        // while the first dispatch is still outstanding.
        state.jobs[0].next_run_at = Some("2023-11-14T22:20:00.000Z".to_string());

        let second = claim_due_in_state(&mut state, now, now);
        assert!(
            second.is_empty(),
            "a job with an outstanding claim must not be dispatched again: {second:?}"
        );
        assert_eq!(state.dispatches.len(), 1, "no second claim was recorded");
        assert!(
            state.jobs[0].last_skipped_at.is_some(),
            "the skipped window is recorded, not silently dropped"
        );
    }

    /// One schedule per run, 2 threads, 2 passes each. The assertion is the
    /// invariant, not the count: a job may be skipped for a window (the store
    /// records `last_skipped_at`), but it must never be CLAIMED twice while a
    /// claim for it is outstanding.
    #[test]
    fn shuttle_two_passes_never_claim_one_job_twice() {
        shuttle::check_random(
            || {
                use shuttle::sync::Mutex;

                let state = std::sync::Arc::new(Mutex::new(state_with_one_due_job()));
                let passes = 2;
                let threads: Vec<_> = (0..2)
                    .map(|_| {
                        let state = std::sync::Arc::clone(&state);
                        shuttle::thread::spawn(move || {
                            for _ in 0..passes {
                                let mut guard = state.lock().unwrap();
                                let _ = claim_due_in_state(&mut guard, u64::MAX / 2, u64::MAX / 2);
                            }
                        })
                    })
                    .collect();
                for thread in threads {
                    thread.join().expect("pass");
                }
                let guard = state.lock().unwrap();
                // The invariant: at most ONE outstanding dispatch per job id.
                let mut seen = std::collections::HashSet::new();
                for dispatch in &guard.dispatches {
                    assert!(
                        seen.insert(dispatch.job_id.clone()),
                        "the job {} was claimed twice while a claim was outstanding: {:?}",
                        dispatch.job_id,
                        guard.dispatches
                    );
                }
            },
            50,
        );
    }
}
