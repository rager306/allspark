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

    /// Run one mutator against every partition under one lock.
    ///
    /// A partition whose write fails stops the loop but does NOT discard the
    /// claims earlier partitions already committed: those rows are marked claimed
    /// on disk, so returning nothing would leave them never dispatched —
    /// indistinguishable from interrupted work until a later recovery pass
    /// (review #3). The landed claims come back beside the error instead.
    ///
    /// # Errors
    ///
    /// Returns `(error, landed)` when a partition write fails, and a plain error
    /// when the cross-process lock cannot be taken.
    pub(crate) fn mutate_states(
        &self,
        mut mutator: impl FnMut(&mut CronJobsState) -> Vec<AgentCronDispatch>,
    ) -> Result<Vec<AgentCronDispatch>, (anyhow::Error, Vec<AgentCronDispatch>)> {
        let paths: Vec<PathBuf> = if self.session_artifact_mode {
            // Sorted, not `HashMap` order: the loop below stops at the first
            // failing partition, so the set of claims that landed must not depend
            // on hash iteration order (review #3 — a nondeterministic prefix makes
            // the failure unlearnable and the tests flaky).
            let mut paths: Vec<PathBuf> = self
                .session_artifact_files
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .cloned()
                .collect();
            paths.sort();
            paths
        } else {
            vec![self.require_file_path()]
        };
        let previous_heartbeats = Self::heartbeat_catalog_signature(&self.read_jobs());
        let mut changed = false;
        let Some(outcome) = with_state_locks(&paths, || {
            let mut dispatches = Vec::new();
            let mut failure: Option<anyhow::Error> = None;
            for path in &paths {
                // A read this build cannot interpret is an ERROR, not an empty catalog: the
                // writer below replaces the file, so a lenient read here would destroy the
                // rows it dropped (review #7).
                let mut state = match read_jobs_state_checked(path) {
                    Ok(state) => state,
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                };
                let before = serde_json::to_string(&state).unwrap_or_default();
                let claimed = mutator(&mut state);
                if serde_json::to_string(&state).unwrap_or_default() != before {
                    if let Err(error) = write_jobs_state(path, &state) {
                        failure = Some(error);
                        break;
                    }
                    changed = true;
                }
                dispatches.extend(claimed);
            }
            match failure {
                Some(error) => Err((error, dispatches)),
                None => Ok(dispatches),
            }
        }) else {
            return Err((
                anyhow::anyhow!("cron jobs state lock not acquired; skipped"),
                Vec::new(),
            ));
        };
        if changed && Self::heartbeat_catalog_signature(&self.read_jobs()) != previous_heartbeats {
            self.notify_heartbeat_change();
        }
        outcome
    }

    /// [`Self::mutate_states`] for callers that produce no dispatches.
    ///
    /// Management calls only mutate rows, so a partially applied partition is
    /// just an error for them; the landed-claims channel exists for
    /// [`Self::claim_due`]'s callers, which must run work already marked claimed.
    ///
    /// # Errors
    ///
    /// Propagates the partition write or lock error.
    pub(crate) fn mutate_states_only(
        &self,
        mut mutator: impl FnMut(&mut CronJobsState) -> Vec<AgentCronDispatch>,
    ) -> anyhow::Result<()> {
        self.mutate_states(|state| mutator(state))
            .map(|_| ())
            .map_err(|(error, _landed)| error)
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

/// Settle claims left behind by a process that died before recording their outcome.
///
/// A recurring job keeps its schedule (the next run is already rolled), but a ONE-SHOT claim is
/// the user's only wake — a quota-resume or goal-backoff fire — and it is re-armed here rather
/// than completed (review #6). Completion would claim the prompt ran, and the claim is written
/// BEFORE the prompt reaches the session queue, so a crash in that window loses the wake
/// outright. Re-arming is at-least-once: a crash after the prompt was durably queued replays it,
/// which is the direction a durability record must err in.
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
    // The window each claim covered: a re-armed one-shot returns to the moment it was due.
    let scheduled_for: HashMap<String, String> = interrupted
        .iter()
        .filter(|dispatch| !dispatch.scheduled_for.is_empty())
        .map(|dispatch| (dispatch.job_id.clone(), dispatch.scheduled_for.clone()))
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
            if job.schedule.kind == ScheduleKind::Once {
                // Re-arm, never complete: nothing recorded that this prompt was handed to the
                // session, and completing it destroys the only wake the one-shot represents.
                job.next_run_at = Some(
                    scheduled_for
                        .get(&job.id)
                        .cloned()
                        .unwrap_or_else(|| iso_from_millis(now)),
                );
            }
            job.last_error = Some(super::INTERRUPTED_FIRE_MARKER.to_string());
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

/// The state a READ-MODIFY-WRITE path may build on (review #7): absent reads as empty,
/// anything this build cannot understand is an error.
///
/// [`read_jobs_state`] is deliberately lenient — a listing must render even for a file a
/// newer build wrote — but the same leniency is destructive on a write path. A file with one
/// unparsable row used to read as "the rest of the catalog", so the next create appended to
/// that partial view and replaced the artifact: the rows the read had silently dropped lost
/// their schedules, and a top-level parse failure turned "one corrupted file" into "the
/// catalog is empty" followed by an overwrite with only the new job.
///
/// So: `NotFound` is genuinely empty, every other read error, an invalid document, and any
/// invalid row or dispatch is refused BEFORE the writer replaces the file. The original
/// bytes stay for repair, and the caller reports the failure instead of acknowledging a
/// change it destroyed the evidence for.
pub(crate) fn read_jobs_state_checked(path: &Path) -> anyhow::Result<CronJobsState> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CronJobsState::default())
        }
        Err(error) => {
            return Err(anyhow::anyhow!(
                "scheduled jobs state {} is unreadable: {error}",
                path.display()
            ))
        }
    };
    let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
        anyhow::anyhow!(
            "scheduled jobs state {} is not valid JSON ({error}); refusing to overwrite it",
            path.display()
        )
    })?;
    // The object check comes BEFORE any field lookup: `get` on a non-object answers
    // `None`, which the per-field rule below reads as "no jobs", so a top-level `[]`
    // or `null` became an empty catalog a writer then replaced (review #7).
    if !parsed.is_object() {
        anyhow::bail!(
            "scheduled jobs state {} is not a JSON object ({parsed}); refusing to overwrite it",
            path.display()
        );
    }
    let rows = |key: &str| -> anyhow::Result<&Vec<serde_json::Value>> {
        match parsed.get(key) {
            None | Some(serde_json::Value::Null) => Ok(&EMPTY_ROWS),
            Some(serde_json::Value::Array(rows)) => Ok(rows),
            Some(other) => Err(anyhow::anyhow!(
                "scheduled jobs state {} has a {key} field that is not a list ({other}); \
                 refusing to overwrite it",
                path.display()
            )),
        }
    };
    let jobs = rows("jobs")?
        .iter()
        .enumerate()
        .map(|(index, row)| {
            serde_json::from_value::<AgentCronJob>(row.clone()).map_err(|error| {
                anyhow::anyhow!(
                    "scheduled jobs state {} has an unreadable job at [{index}] ({error}); \
                     refusing to overwrite it",
                    path.display()
                )
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let dispatches = rows("dispatches")?
        .iter()
        .enumerate()
        .map(|(index, row)| {
            serde_json::from_value::<AgentCronDispatchRecord>(row.clone()).map_err(|error| {
                anyhow::anyhow!(
                    "scheduled jobs state {} has an unreadable dispatch at [{index}] ({error}); \
                     refusing to overwrite it",
                    path.display()
                )
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(CronJobsState { jobs, dispatches })
}

/// The empty row list a document without (or with a null) `jobs`/`dispatches` reads as.
static EMPTY_ROWS: Vec<serde_json::Value> = Vec::new();

fn write_jobs_file(path: &Path, jobs: &[AgentCronJob], merge_current: bool) -> anyhow::Result<()> {
    let current = read_jobs_state_checked(path)?;
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

    /// A catalog this build cannot read is REFUSED before a write path replaces it
    /// (review #7).
    ///
    /// `read_jobs_state` is lenient on purpose — a listing must render for a file a newer
    /// build wrote — but `create` reads the catalog and then REPLACES it. The lenient read
    /// turned one corrupt file into "the catalog is empty", so the create wrote the artifact
    /// holding only the new job and the original schedule was gone before anyone could
    /// repair it. Both shapes below are the finding: the document never parsed, and the
    /// document parsed with one row that did not.
    #[test]
    fn an_unreadable_catalog_is_refused_instead_of_overwritten() {
        let now = 1_700_000_000_000;
        let cases = [
            ("not JSON at all", "this is not json"),
            (
                "one unreadable row beside a good one",
                r#"{"jobs":[{"id":"job-a","status":"active","activeSessionId":"s","sessionId":"s","sessionFile":"/s.jsonl","cwd":"/w","prompt":"keep me","schedule":{"kind":"interval","expression":"every 10m","intervalMs":600000},"createdAt":"2023-11-14T22:13:20.000Z","updatedAt":"2023-11-14T22:13:20.000Z"},{"id":42}]}"#,
            ),
        ];
        for (label, corrupt) in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("jobs.json");
            std::fs::write(&path, corrupt).unwrap();
            let store = AgentCronJobStore::new(path.clone());
            let error = store
                .create(&crate::cron::store::input("tick", "every 10m", now))
                .expect_err("a create must not overwrite a catalog it cannot read");
            assert!(
                error.to_string().contains("refusing to overwrite"),
                "{label}: the refusal must say why the file was kept: {error}"
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                corrupt,
                "{label}: the original bytes stay for repair"
            );
        }
    }

    /// A top-level document that is valid JSON but not an object is not an empty catalog
    /// (review #7, residual).
    ///
    /// The object check used to happen per FIELD: `parsed.get("jobs")` answers `None` for a
    /// value that has no fields at all, so `[]`, `null`, `42` and `"text"` all read as "no
    /// jobs" and the next create replaced them with a one-row catalog. Valid JSON never
    /// reached the parse-error arm, so nothing refused the overwrite.
    #[test]
    fn a_catalog_that_is_not_a_json_object_is_refused() {
        let now = 1_700_000_000_000;
        for corrupt in ["[]", "null", "42", "\"text\"", "true"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("jobs.json");
            std::fs::write(&path, corrupt).unwrap();
            let store = AgentCronJobStore::new(path.clone());
            let error = store
                .create(&crate::cron::store::input("tick", "every 10m", now))
                .expect_err("a catalog that is not an object must not be overwritten");
            assert!(
                error.to_string().contains("refusing to overwrite"),
                "{corrupt}: the refusal must say why the file was kept: {error}"
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                corrupt,
                "{corrupt}: the original bytes stay for repair"
            );
        }
    }

    /// The claim path must refuse the same file: `claim_due` rewrites every partition, so a
    /// lenient read here would mark the rows it could parse as claimed and drop the rest
    /// (review #7 — a lost schedule is worse than a refused pass, which retries).
    #[test]
    fn a_claim_refuses_a_partition_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let corrupt = r#"{"jobs":[{"id":"job-a","status":"active"}]}"#;
        std::fs::write(&path, corrupt).unwrap();
        let store = AgentCronJobStore::new(path.clone());
        let error = store
            .claim_due(1_700_000_000_000, 1_700_000_000_000)
            .expect_err("a claim must not rewrite a catalog it cannot read")
            .0;
        assert!(
            error.to_string().contains("refusing to overwrite"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "the original bytes stay for repair"
        );
    }

    /// An interrupted one-shot is RE-ARMED, never completed (review #6).
    ///
    /// The claim is written before the prompt reaches the session queue, so a crash in that
    /// window leaves no record that anything ran. Completing the job destroyed the only wake a
    /// one-shot represents — a quota-resume or goal-backoff fire simply stopped existing —
    /// while a recurring job kept its schedule because its next run was already rolled.
    #[test]
    fn an_interrupted_one_shot_is_rearmed_at_its_due_time() {
        let now = 1_700_000_000_000_u64;
        let once: AgentCronJob = serde_json::from_value(serde_json::json!({
            "id": "wake",
            "activeSessionId": "s-1",
            "sessionId": "s-1",
            "sessionFile": "/tmp/s-1.jsonl",
            "cwd": "/tmp",
            "prompt": "resume the parked goal",
            "schedule": { "kind": "once", "expression": "in 5m" },
            "status": "active",
            "nextRunAt": "2023-11-14T22:18:20.000Z",
            "createdAt": "2023-11-14T22:13:20.000Z",
            "updatedAt": "2023-11-14T22:13:20.000Z"
        }))
        .expect("the one-shot fixture parses");
        let due_iso = once.next_run_at.clone().expect("the fixture is due");
        let mut state = CronJobsState {
            jobs: vec![once],
            dispatches: Vec::new(),
        };
        let claimed = claim_due_in_state(&mut state, now + 300_000, now + 300_001);
        assert_eq!(claimed.len(), 1, "the one-shot is claimed");
        let dispatch_id = claimed[0].id.clone();
        assert_eq!(
            state.jobs[0].next_run_at, None,
            "a claimed one-shot carries no next run"
        );

        let mut recovered = Vec::new();
        recover_interrupted_in_state(&mut state, now + 309_000, &mut recovered, None);
        assert_eq!(recovered.len(), 1, "the interrupted claim is reported");
        assert_eq!(
            state.jobs[0].status,
            JobStatus::Active,
            "the one-shot must not be completed: nothing recorded that it ran"
        );
        assert_eq!(
            state.jobs[0].next_run_at.as_deref(),
            Some(due_iso.as_str()),
            "it returns to the moment it was due, so the next pass fires it"
        );
        assert!(
            state.jobs[0].last_error.is_some(),
            "the recovery stamps why the wake is late"
        );
        assert!(
            state.dispatches.is_empty(),
            "the claim is released, so the next pass can take it again: {dispatch_id}"
        );
    }

    /// The checked read: absent is empty, everything unreadable is an error (review #7).
    #[test]
    fn the_checked_read_defaults_only_on_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        assert_eq!(
            read_jobs_state_checked(&path).expect("an absent catalog is empty"),
            CronJobsState::default()
        );
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(
            read_jobs_state_checked(&path).expect("an empty object is an empty catalog"),
            CronJobsState::default()
        );
        std::fs::write(&path, r#"{"jobs":"nope"}"#).unwrap();
        let error = read_jobs_state_checked(&path).expect_err("a non-list field is refused");
        assert!(error.to_string().contains("not a list"), "{error}");
        // The lenient read is unchanged: listings still render a partial catalog.
        assert_eq!(read_jobs_state(&path), CronJobsState::default());
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

    /// A partition write failure still returns the claims that landed (#3).
    ///
    /// `mutate_states` writes each session-artifact partition in a loop. Before the
    /// fix it propagated a later partition's write error with `?`, discarding the
    /// dispatches of the partitions whose rows were ALREADY marked claimed on disk —
    /// never dispatched, and indistinguishable from interrupted work to the next
    /// recovery pass, which cannot tell "claimed but never run" from "claimed and
    /// crashed".
    ///
    /// The failure is injected below `read_jobs_state`: a directory at the file's
    /// ATOMIC TEMP path (`<path>.tmp<pid>`) makes the write fail while the read
    /// still succeeds. A directory at the partition path itself does not work — the
    /// read returns the default, the state never changes, and the write is never
    /// attempted.
    #[test]
    fn a_partition_write_failure_still_returns_the_landed_claims() {
        let root = tempfile::tempdir().unwrap();
        let good = root.path().join("a");
        let bad = root.path().join("b");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::create_dir_all(&bad).unwrap();
        let store = AgentCronJobStore::for_session_artifacts();
        assert!(store.register_session_artifact("a", &good));
        assert!(store.register_session_artifact("b", &bad));

        let seed = |session: &str| CronJobsState {
            jobs: vec![serde_json::from_value(serde_json::json!({
                "id": format!("job-{session}"),
                "activeSessionId": session,
                "sessionId": session,
                "sessionFile": format!("/tmp/{session}.jsonl"),
                "cwd": "/tmp",
                "prompt": "ping",
                "schedule": { "kind": "interval", "expression": "each 30s", "intervalMs": 30000 },
                "status": "active",
                "nextRunAt": "2023-11-14T22:20:00.000Z",
                "createdAt": "2023-11-14T22:13:20.000Z",
                "updatedAt": "2023-11-14T22:13:20.000Z"
            }))
            .expect("job json")],
            dispatches: Vec::new(),
        };
        write_jobs_state(&good.join("scheduled-jobs.json"), &seed("a")).expect("seed a");
        let bad_path = bad.join("scheduled-jobs.json");
        write_jobs_state(&bad_path, &seed("b")).expect("seed b");
        // The bad partition still READS, but cannot WRITE: its atomic temp path is a
        // directory.
        let temp = PathBuf::from(format!("{}.tmp{}", bad_path.display(), std::process::id()));
        std::fs::create_dir(&temp).expect("block the atomic temp path");

        // A mutator that claims everything it sees, so both partitions "land" work.
        let outcome = store.mutate_states(|state| {
            let mut claimed = Vec::new();
            for job in &mut state.jobs {
                if job.status == JobStatus::Active {
                    job.status = JobStatus::Paused;
                    claimed.push(AgentCronDispatch {
                        id: format!("dispatch-{}", job.id),
                        job: job.clone(),
                    });
                }
            }
            claimed
        });

        let landed = match outcome {
            Ok(dispatches) => {
                panic!("the blocked partition must fail the write, got {dispatches:?}")
            }
            Err((_error, landed)) => landed,
        };
        assert_eq!(
            landed.len(),
            1,
            "the claim the good partition committed must come back beside the error, not vanish"
        );
        assert_eq!(landed[0].job.id, "job-a");
    }
}
