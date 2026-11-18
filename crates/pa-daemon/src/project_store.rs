//! The project store writer: records plan and evidence events for a project.
//!
//! The event log is canonical and the SQLite read model is derived, so this
//! writer only appends. It is opt-in: a session without a configured project
//! store records nothing, which keeps every existing session byte-identical.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pa_project::event::{Actor, EventLog, SCHEMA_VERSION};
use serde_json::{json, Value};

/// Where a project's durable record lives.
#[derive(Debug, Clone)]
pub struct ProjectStorePaths {
    /// The project's event log.
    pub events: PathBuf,
    /// The derived read model; projected lazily, never on the write path.
    pub model: PathBuf,
    /// The project identity every event carries.
    pub project_id: String,
}

impl ProjectStorePaths {
    /// The project identity for `cwd`: the git remote when one exists, else
    /// the repository's common dir, else the canonical working directory.
    ///
    /// Not the working directory alone: worktrees of one repository must share
    /// a plan, and two different clones must not.
    #[must_use]
    pub fn project_id_for(cwd: &Path) -> String {
        let key = git_identity(cwd).unwrap_or_else(|| cwd.display().to_string());
        let digest = <sha2::Sha256 as sha2::Digest>::digest(key.as_bytes());
        // 32 hex characters is plenty for a directory name and keeps paths
        // readable; the full digest buys nothing.
        digest.iter().take(16).fold(String::new(), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        })
    }

    /// The store under `<agent_dir>/projects/<project_id>/`.
    ///
    /// `project_id` is the caller's stable identity (a remote or a git common
    /// dir digest), not the working directory: worktrees of one repository
    /// must share a plan, and two different clones must not.
    #[must_use]
    pub fn under(agent_dir: &Path, project_id: &str) -> Self {
        let root = agent_dir.join("projects").join(project_id);
        Self {
            events: root.join("events.jsonl"),
            model: root.join("project.db"),
            project_id: project_id.to_string(),
        }
    }
}

/// Appends plan and evidence events for one project.
#[derive(Debug)]
pub struct ProjectRecorder {
    paths: ProjectStorePaths,
    session_id: Option<String>,
    /// The plan item ids the log already holds. Loaded from the log on first
    /// use, then kept in step: rereading the whole log on every snapshot would
    /// cost a pass per update for a set this writer already knows.
    known: std::sync::Mutex<Option<Vec<String>>>,
}

impl ProjectRecorder {
    /// A recorder over `paths`, tagging events with `session_id`.
    #[must_use]
    pub fn new(paths: ProjectStorePaths, session_id: Option<String>) -> Self {
        Self {
            paths,
            session_id,
            known: std::sync::Mutex::new(None),
        }
    }

    /// The paths this recorder writes to.
    #[must_use]
    pub fn paths(&self) -> &ProjectStorePaths {
        &self.paths
    }

    /// Record one full plan snapshot as `entity.upserted` events.
    ///
    /// A snapshot is what the model writes (`update_plan` replaces the whole
    /// list), so an item that disappeared from the list is recorded as
    /// `entity.removed` rather than left stale in the read model. Failures are
    /// returned to the caller, which logs them: the plan itself already
    /// landed in the session, so a store failure must not fail the tool call.
    ///
    /// # Errors
    ///
    /// Returns an error when the log cannot be appended.
    pub fn record_plan(
        &self,
        items: &[pa_core::session_engine::plan::PlanItem],
    ) -> anyhow::Result<()> {
        let log = EventLog::at(&self.paths.events);
        let previous = self.known_item_ids();
        // The snapshot is written as one durable append: several appends would
        // leave a crash-recovered log holding a mix of two snapshots, which is a
        // plan that never existed and would be restored as if it had.
        let mut batch: Vec<pa_project::event::Event> = Vec::new();
        for (position, item) in items.iter().enumerate() {
            batch.push(self.event(
                "entity.upserted",
                json!({
                    // The row key carries the session: two sessions in one
                    // project both mint `P1`…, and a bare id made them one row,
                    // so whichever wrote last owned the restored plan (ledger
                    // #24b). The `data.id` keeps the plan's own id for restore.
                    "id": self.row_id(&item.id),
                    "entityType": "plan_item",
                    "sessionId": self.session_id.clone(),
                    // The plan is an ordered list, so the order is part of the
                    // record: a projection that kept only the items would come
                    // back in whatever order the database returned.
                    "data": {
                        "position": position,
                        "text": item.text,
                        "status": item.status.as_str(),
                        "verify": item.verify,
                        "evidence": item.evidence,
                        "evidenceRevision": item.evidence_revision,
                    },
                }),
            ));
        }
        for id in previous {
            if !items.iter().any(|item| item.id == id) {
                batch.push(self.event("entity.removed", json!({ "id": self.row_id(&id) })));
            }
        }
        log.append_all(&batch)?;
        let ids = items.iter().map(|item| item.id.clone()).collect();
        *self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ids);
        Ok(())
    }

    /// The project's plan, in list order.
    ///
    /// Read through the log, not from the read model alone: the log is
    /// canonical, and a session that crashed between its last append and
    /// `sync` leaves the model behind that write. Projecting first costs one
    /// incremental pass and makes the answer the log's, not the model's.
    ///
    /// `None` when nothing was ever recorded, or when the log cannot be read —
    /// "nothing to resume", not an error the caller must distinguish.
    #[must_use]
    pub fn load_plan(&self) -> Option<Vec<crate::plan_restore::StoredPlanItem>> {
        // A failed sync still leaves the model as a usable (if older) answer.
        let _ = self.sync();
        let model = pa_project::model::ReadModel::open(&self.paths.model).ok()?;
        // Rows are keyed per session, so a project can hold several plans. The
        // one to resume is the newest session's: its plan is the work in flight,
        // and an older session's plan is a record of work already replaced.
        let newest = self.newest_session_with_plan(&model)?;
        let rows = model
            .entities_of_type(&self.paths.project_id, "plan_item")
            .ok()?;
        let mut items: Vec<_> = rows
            .into_iter()
            .filter(|(row_id, _)| row_id.starts_with(&format!("{newest}:")))
            .filter_map(|(row_id, data)| {
                // The plan's own id lives in the row's data, not in the key.
                let item_id = row_id.split_once(':').map_or(row_id.as_str(), |(_, id)| id);
                crate::plan_restore::StoredPlanItem::from_row(item_id, &data)
            })
            .collect();
        if items.is_empty() {
            return None;
        }
        items.sort_by_key(|item| item.position);
        Some(items)
    }

    /// The session that wrote the newest plan event in this project.
    ///
    /// Read from the log, not the model: the log is canonical and carries the
    /// order, so the newest writer is knowable even when the model is behind.
    fn newest_session_with_plan(&self, _model: &pa_project::model::ReadModel) -> Option<String> {
        let events = EventLog::at(&self.paths.events).read_all().ok()?;
        events
            .iter()
            .rev()
            .filter(|event| {
                event.payload.get("entityType").and_then(Value::as_str) == Some("plan_item")
            })
            .map(|event| Self::scope_key(event.session_id.as_deref()))
            .next()
    }

    /// The row-key scope for a session: its id, or `-` when it has none.
    ///
    /// One definition shared by the writer and the reader, so a recorder without
    /// a session id still reads back the plan it wrote.
    fn scope_key(session_id: Option<&str>) -> String {
        session_id.unwrap_or("-").to_string()
    }

    /// Bring the read model up to date with the log.
    ///
    /// The log is canonical and the model derived, so this is safe to call
    /// after every write and after any gap: the projector skips what already
    /// landed. A failure is the caller's to log — the log itself is already
    /// correct, so the model being behind is recoverable, not lost.
    ///
    /// # Errors
    ///
    /// Returns an error when the model cannot be opened or written.
    pub fn sync(&self) -> anyhow::Result<()> {
        let log = EventLog::at(&self.paths.events);
        if log.len_bytes()? == 0 {
            return Ok(());
        }
        if let Some(parent) = self.paths.model.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut model = pa_project::model::ReadModel::open(&self.paths.model)?;
        // Resume from the byte the model was projected through. Reading the
        // whole log per update made one session's cost quadratic in its own
        // length: measured 16 ms per sync at 100 events, 42 ms at 1 000, and
        // 8.3 s of pure replay over 200 plan updates. The offset lives in the
        // model, so a dropped model resumes from the start rather than from a
        // checkpoint that outlived its data.
        let offset = model.log_offset()?.unwrap_or(0);
        let (events, end) = log.read_from(offset)?;
        if events.is_empty() {
            return Ok(());
        }
        match pa_project::project::project(&mut model, &self.paths.project_id, &events) {
            Ok(_) => {
                model.set_log_offset(end)?;
                Ok(())
            }
            // One unappliable event must not pin the offset forever: replay the
            // whole log once (the ledger skips what landed) and record the
            // failure if it still refuses.
            Err(error) => {
                let (all, end) = log.read_from(0)?;
                pa_project::project::project(&mut model, &self.paths.project_id, &all)
                    .map_err(|_| error)?;
                model.set_log_offset(end)?;
                Ok(())
            }
        }
    }

    /// The plan item ids the log already recorded for this project.
    ///
    /// A snapshot replaces the plan, so the writer has to know which ids the
    /// previous snapshot held to record the ones that disappeared. Reading the
    /// log is the honest source: the read model may be behind.
    fn known_item_ids(&self) -> Vec<String> {
        let mut cached = self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cached.is_none() {
            *cached = Some(self.read_known_item_ids());
        }
        cached.clone().unwrap_or_default()
    }

    fn read_known_item_ids(&self) -> Vec<String> {
        let Ok(events) = EventLog::at(&self.paths.events).read_all() else {
            return Vec::new();
        };
        let mut ids: Vec<String> = Vec::new();
        for event in events {
            // Only this session's own writes. A project's log is shared, and a
            // snapshot is a whole plan: treating another session's items as
            // "previous" made this recorder emit `entity.removed` for steps it
            // never wrote, deleting the other session's work from the record
            // (measured: session B removed session A's `P2`).
            if event.session_id != self.session_id {
                continue;
            }
            let Some(row_id) = event.payload.get("id").and_then(Value::as_str) else {
                continue;
            };
            // Rows are keyed `session:item`; the caller compares plan item ids.
            let id = row_id.split_once(':').map_or(row_id, |(_, item)| item);
            match event.event_type.as_str() {
                "entity.upserted"
                    if event.payload.get("entityType").and_then(Value::as_str)
                        == Some("plan_item") =>
                {
                    if !ids.iter().any(|known| known == id) {
                        ids.push(id.to_string());
                    }
                }
                "entity.removed" => ids.retain(|known| known != id),
                _ => {}
            }
        }
        ids
    }

    /// The row key for one of this session's plan items.
    ///
    /// `session:item`, so two sessions in one project keep separate rows instead
    /// of overwriting one another. A session without an id falls back to `-`,
    /// which keeps the single-session case exactly as it was.
    fn row_id(&self, item_id: &str) -> String {
        format!("{}:{item_id}", Self::scope_key(self.session_id.as_deref()))
    }

    fn event(&self, event_type: &str, payload: Value) -> pa_project::event::Event {
        pa_project::event::Event {
            event_id: uuid::Uuid::now_v7().to_string(),
            event_type: event_type.to_string(),
            schema_version: SCHEMA_VERSION,
            project_id: self.paths.project_id.clone(),
            session_id: self.session_id.clone(),
            occurred_at: crate::util::now_iso(),
            actor: Actor {
                kind: "agent".to_string(),
                id: None,
            },
            payload,
        }
    }
}

/// The stable key a repository shares across its worktrees.
///
/// `remote.origin.url` is the identity that survives a clone; the git common
/// dir covers a repository with no remote, and a worktree reports the same
/// common dir as its main checkout.
fn git_identity(cwd: &Path) -> Option<String> {
    for args in [
        &["remote", "get-url", "origin"][..],
        &["rev-parse", "--git-common-dir"][..],
    ] {
        let output = std::process::Command::new("git")
            .arg("--no-optional-locks")
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

/// The session's recorder, when a project store is configured.
pub type ProjectRecorderHandle = Arc<ProjectRecorder>;

#[cfg(test)]
mod tests {
    use pa_core::session_engine::plan::{PlanItem, PlanItemStatus};

    use super::*;

    fn paths(dir: &Path) -> ProjectStorePaths {
        ProjectStorePaths::under(dir, "proj")
    }

    fn item(id: &str, status: PlanItemStatus) -> PlanItem {
        PlanItem {
            id: id.to_string(),
            text: format!("do {id}"),
            status,
            verify: None,
            evidence: None,
            evidence_revision: None,
        }
    }

    fn recorder(dir: &Path) -> ProjectRecorder {
        ProjectRecorder::new(paths(dir), Some("session-1".to_string()))
    }

    #[test]
    fn a_plan_snapshot_reaches_the_read_model() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        let items = vec![
            item("P1", PlanItemStatus::InProgress),
            item("P2", PlanItemStatus::Pending),
        ];

        recorder.record_plan(&items).unwrap();

        let events = EventLog::at(&recorder.paths().events).read_all().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.project_id == "proj"));
        assert_eq!(events[0].session_id.as_deref(), Some("session-1"));
        // The row key carries the session; the plan's own id is in `data`.
        assert_eq!(events[0].payload["id"], "session-1:P1");
        assert_eq!(events[0].payload["data"]["status"], "in_progress");

        let mut model = pa_project::model::ReadModel::open(&recorder.paths().model).unwrap();
        let report = pa_project::project::project(&mut model, "proj", &events).unwrap();
        assert_eq!(report.applied, 2);
        assert_eq!(model.count_entities("proj", "plan_item").unwrap(), 2);
    }

    #[test]
    fn an_item_dropped_from_the_plan_is_recorded_as_removed() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());

        recorder
            .record_plan(&[
                item("P1", PlanItemStatus::InProgress),
                item("P2", PlanItemStatus::Pending),
            ])
            .unwrap();
        recorder
            .record_plan(&[item("P1", PlanItemStatus::InProgress)])
            .unwrap();

        let events = EventLog::at(&recorder.paths().events).read_all().unwrap();
        let removals: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "entity.removed")
            .collect();
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].payload["id"], "session-1:P2");

        let mut model = pa_project::model::ReadModel::open(&recorder.paths().model).unwrap();
        pa_project::project::project(&mut model, "proj", &events).unwrap();
        assert_eq!(model.count_entities("proj", "plan_item").unwrap(), 1);
    }

    #[test]
    fn a_restored_snapshot_matches_a_rebuilt_model() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder
            .record_plan(&[item("P1", PlanItemStatus::Completed)])
            .unwrap();

        let events = EventLog::at(&recorder.paths().events).read_all().unwrap();
        let mut fresh = pa_project::model::ReadModel::open(dir.path().join("second.db")).unwrap();
        pa_project::project::project(&mut fresh, "proj", &events).unwrap();

        // The recorder keys rows per session; a rebuild from the log must
        // produce the same key, which is what makes this comparison a check of
        // the log rather than of the first model.
        let restored = fresh.entity_json("session-1:P1").unwrap().unwrap();
        assert_eq!(restored["status"], serde_json::json!("completed"));
    }

    #[test]
    fn a_second_recorder_over_the_same_log_does_not_repeat_the_first_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let items = vec![
            item("P1", PlanItemStatus::InProgress),
            item("P2", PlanItemStatus::Pending),
        ];
        recorder(dir.path()).record_plan(&items).unwrap();
        // A new recorder (a second session, or the same one after a restart)
        // writes the same snapshot: nothing was dropped, so nothing is removed.
        recorder(dir.path()).record_plan(&items[..1]).unwrap();

        let events = EventLog::at(&paths(dir.path()).events).read_all().unwrap();
        let removals = events
            .iter()
            .filter(|event| event.event_type == "entity.removed")
            .count();
        assert_eq!(removals, 1, "only P2 is new to the removal set");
    }

    #[test]
    fn a_worktree_shares_its_repository_identity() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "https://example.com/one.git"]);

        let main_id = ProjectStorePaths::project_id_for(&repo);
        let worktree = dir.path().join("wt");
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "-b",
            "side",
        ]);

        assert_eq!(main_id, ProjectStorePaths::project_id_for(&worktree));
    }

    #[test]
    fn two_different_repositories_do_not_share_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one");
        let two = dir.path().join("two");
        for (path, remote) in [
            (&one, "https://example.com/one.git"),
            (&two, "https://example.com/two.git"),
        ] {
            std::fs::create_dir_all(path).unwrap();
            assert!(std::process::Command::new("git")
                .arg("init")
                .arg("-q")
                .current_dir(path)
                .status()
                .unwrap()
                .success());
            assert!(std::process::Command::new("git")
                .args(["remote", "add", "origin", remote])
                .current_dir(path)
                .status()
                .unwrap()
                .success());
        }

        assert_ne!(
            ProjectStorePaths::project_id_for(&one),
            ProjectStorePaths::project_id_for(&two)
        );
    }
    #[test]
    fn sync_brings_the_model_up_to_date_and_survives_a_lost_model() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder
            .record_plan(&[
                item("P1", PlanItemStatus::InProgress),
                item("P2", PlanItemStatus::Pending),
            ])
            .unwrap();
        recorder.sync().unwrap();

        let count = |path: &std::path::Path| {
            let model = pa_project::model::ReadModel::open(path).unwrap();
            model.count_entities("proj", "plan_item").unwrap()
        };
        assert_eq!(count(&recorder.paths().model), 2);

        // The model is derived, so losing it must lose nothing.
        std::fs::remove_file(&recorder.paths().model).unwrap();
        recorder.sync().unwrap();
        assert_eq!(count(&recorder.paths().model), 2);
    }

    #[test]
    fn sync_repeatedly_applies_nothing_twice() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder
            .record_plan(&[item("P1", PlanItemStatus::Pending)])
            .unwrap();
        recorder.sync().unwrap();
        recorder.sync().unwrap();

        let model = pa_project::model::ReadModel::open(&recorder.paths().model).unwrap();
        assert_eq!(model.count_entities("proj", "plan_item").unwrap(), 1);
    }

    #[test]
    fn sync_on_an_empty_log_creates_no_model() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder.sync().unwrap();

        assert!(!recorder.paths().model.exists());
    }
    /// The checkpoint is a performance property, not a correctness one: the
    /// ledger makes a read from zero produce the same model, which is why this
    /// test passes with the checkpoint disabled. It guards the *contract*
    /// (a checkpoint exists and advances); `sync_cost` in `pa-project` is what
    /// shows the difference, and it is a measurement, not a gate.
    #[test]
    fn a_fresh_recorder_resumes_from_the_models_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        recorder(dir.path())
            .record_plan(&[item("P1", PlanItemStatus::InProgress)])
            .unwrap();
        recorder(dir.path()).sync().unwrap();
        let offset = {
            let model =
                pa_project::model::ReadModel::open(&recorder(dir.path()).paths().model).unwrap();
            model.log_offset().unwrap().expect("a checkpoint")
        };
        assert!(offset > 0);

        // A second session's recorder (or the same one after a restart) starts
        // with no state in memory and must not re-read the log from zero.
        recorder(dir.path())
            .record_plan(&[item("P2", PlanItemStatus::Pending)])
            .unwrap();
        recorder(dir.path()).sync().unwrap();

        let model =
            pa_project::model::ReadModel::open(&recorder(dir.path()).paths().model).unwrap();
        // Both recorders carry the same session id, so the second snapshot
        // replaces the first rather than adding to it: one row, and the
        // checkpoint still advanced across the second sync.
        assert_eq!(model.count_entities("proj", "plan_item").unwrap(), 1);
        assert!(model.log_offset().unwrap().unwrap() > offset);
        let events = EventLog::at(&recorder(dir.path()).paths().events)
            .read_all()
            .unwrap();
        eprintln!(
            "EV: {:?}",
            events
                .iter()
                .map(|e| (
                    e.event_type.as_str(),
                    e.payload["id"].as_str().unwrap_or(""),
                    e.session_id.as_deref().unwrap_or("")
                ))
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn the_stored_plan_round_trips_through_the_model_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder
            .record_plan(&[
                item("P1", PlanItemStatus::InProgress),
                item("P2", PlanItemStatus::Completed),
            ])
            .unwrap();
        recorder.sync().unwrap();

        let plan = recorder.load_plan().expect("a stored plan");
        assert_eq!(
            plan.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            vec!["P1", "P2"]
        );
        assert_eq!(plan[1].status, PlanItemStatus::Completed);
    }

    #[test]
    fn a_dropped_model_is_rebuilt_from_the_log_by_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        assert!(recorder.load_plan().is_none(), "nothing recorded yet");

        recorder
            .record_plan(&[item("P1", PlanItemStatus::Pending)])
            .unwrap();
        recorder.sync().unwrap();
        std::fs::remove_file(&recorder.paths().model).unwrap();

        // The model is derived, so losing it must lose nothing: the read
        // projects the log rather than reporting an empty project.
        let plan = recorder.load_plan().expect("rebuilt from the log");
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].id, "P1");
    }
    #[test]
    fn a_plan_written_but_never_synced_is_still_restored() {
        let dir = tempfile::tempdir().unwrap();
        // The previous session appended and died before its sync: the model
        // does not exist, and the log is the only record.
        recorder(dir.path())
            .record_plan(&[item("P1", PlanItemStatus::InProgress)])
            .unwrap();
        assert!(!recorder(dir.path()).paths().model.exists());

        let plan = recorder(dir.path()).load_plan().expect("the log holds it");

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].id, "P1");
        assert_eq!(plan[0].status, PlanItemStatus::InProgress);
    }
    /// The snapshot must reach the log as one write: a crash between per-item
    /// appends leaves a mix of two plans, and the restore path would adopt it.
    /// The observable property is that one plan write appends exactly one line
    /// per event and the log reads back as the same snapshot.
    #[test]
    fn a_plan_snapshot_is_one_contiguous_batch() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder
            .record_plan(&[
                item("P1", PlanItemStatus::InProgress),
                item("P2", PlanItemStatus::Pending),
                item("P3", PlanItemStatus::Pending),
            ])
            .unwrap();

        let events = EventLog::at(&recorder.paths().events).read_all().unwrap();
        assert_eq!(events.len(), 3);
        // Positions came from one snapshot, so they are 0,1,2 with no gap: a
        // mixed batch shows as a repeated or missing position.
        let positions: Vec<u64> = events
            .iter()
            .map(|event| event.payload["data"]["position"].as_u64().unwrap())
            .collect();
        assert_eq!(positions, vec![0, 1, 2]);
        assert_eq!(recorder.load_plan().unwrap().len(), 3);
    }
    /// The tool call writes the log and projects it. Both are blocking file and
    /// SQLite work, so the measurement exists to decide whether the call needs
    /// to leave the async runtime.
    #[test]
    #[ignore = "measurement"]
    fn record_and_sync_cost() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        let items: Vec<_> = (0..5)
            .map(|index| item(&format!("P{index}"), PlanItemStatus::Pending))
            .collect();
        let rounds = 50;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            recorder.record_plan(&items).unwrap();
            recorder.sync().unwrap();
        }
        eprintln!("record+sync: {:?}/call", start.elapsed() / rounds);
    }

    /// A project's log is shared between sessions, and a snapshot is a whole
    /// plan. Treating another session's items as "previous" made this recorder
    /// emit `entity.removed` for steps it never wrote: measured, session B
    /// erased session A's `P2` from the log.
    #[test]
    fn a_snapshot_removes_only_this_sessions_own_items() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let first = ProjectRecorder::new(paths.clone(), Some("session-a".to_string()));
        let second = ProjectRecorder::new(paths, Some("session-b".to_string()));

        first
            .record_plan(&[
                item("P1", PlanItemStatus::InProgress),
                item("P2", PlanItemStatus::Pending),
            ])
            .unwrap();
        second
            .record_plan(&[item("P1", PlanItemStatus::Completed)])
            .unwrap();

        let events = EventLog::at(&first.paths().events).read_all().unwrap();
        let removals: Vec<&str> = events
            .iter()
            .filter(|event| event.event_type == "entity.removed")
            .filter_map(|event| event.payload["id"].as_str())
            .map(|id| id.split_once(':').map_or(id, |(_, item)| item))
            .collect();
        assert!(
            removals.is_empty(),
            "a session must not remove another session's rows: {removals:?}"
        );
    }

    /// The same session replacing its own snapshot still removes the dropped
    /// step: the rule narrowed to "own items", it did not disappear.
    #[test]
    fn a_session_still_removes_a_step_it_dropped_itself() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder
            .record_plan(&[
                item("P1", PlanItemStatus::InProgress),
                item("P2", PlanItemStatus::Pending),
            ])
            .unwrap();
        recorder
            .record_plan(&[item("P1", PlanItemStatus::InProgress)])
            .unwrap();

        let events = EventLog::at(&recorder.paths().events).read_all().unwrap();
        let removals: Vec<&str> = events
            .iter()
            .filter(|event| event.event_type == "entity.removed")
            .filter_map(|event| event.payload["id"].as_str())
            .map(|id| id.split_once(':').map_or(id, |(_, item)| item))
            .collect();
        assert_eq!(removals, vec!["P2"]);
    }

    /// Two sessions in one project both mint `P1`, `P2`, so a bare item id made
    /// them one row: whichever session wrote last owned the restored plan
    /// (ledger #24b). The row key carries the session, so each plan survives.
    #[test]
    fn two_sessions_in_one_project_keep_separate_plans() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let first = ProjectRecorder::new(paths.clone(), Some("session-a".to_string()));
        let second = ProjectRecorder::new(paths, Some("session-b".to_string()));

        first
            .record_plan(&[
                item("P1", PlanItemStatus::InProgress),
                item("P2", PlanItemStatus::Pending),
            ])
            .unwrap();
        second
            .record_plan(&[
                item("P1", PlanItemStatus::Completed),
                item("P2", PlanItemStatus::Pending),
                item("P3", PlanItemStatus::Pending),
            ])
            .unwrap();
        first.sync().unwrap();

        let model = pa_project::model::ReadModel::open(&first.paths().model).unwrap();
        assert_eq!(
            model.count_entities("proj", "plan_item").unwrap(),
            5,
            "two sessions, two plans: five rows"
        );

        // Restoring gives the newest session's plan, not a mixture of both.
        let restored = first.load_plan().expect("a plan");
        let ids: Vec<&str> = restored.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, vec!["P1", "P2", "P3"]);
        assert_eq!(restored[0].status, PlanItemStatus::Completed);
    }

    /// The same session writing twice still replaces its own plan, so the key
    /// did not turn every update into an append-only plan.
    #[test]
    fn one_session_still_replaces_its_own_plan() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        recorder
            .record_plan(&[
                item("P1", PlanItemStatus::InProgress),
                item("P2", PlanItemStatus::Pending),
            ])
            .unwrap();
        recorder
            .record_plan(&[item("P1", PlanItemStatus::Completed)])
            .unwrap();

        let restored = recorder.load_plan().expect("a plan");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, "P1");
    }
}
