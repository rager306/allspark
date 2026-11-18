//! The project store writer: records plan and evidence events for a project.
//!
//! The event log is canonical and the SQLite read model is derived, so this
//! writer only appends. It is opt-in: a session without a configured project
//! store records nothing, which keeps every existing session byte-identical.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pa_project::event::{Actor, Event, EventLog, SCHEMA_VERSION};
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
        for item in items {
            self.append(
                &log,
                "entity.upserted",
                json!({
                    "id": item.id,
                    "entityType": "plan_item",
                    "data": {
                        "text": item.text,
                        "status": item.status.as_str(),
                        "verify": item.verify,
                        "evidence": item.evidence,
                        "evidenceRevision": item.evidence_revision,
                    },
                }),
            )?;
        }
        for id in previous {
            if !items.iter().any(|item| item.id == id) {
                self.append(&log, "entity.removed", json!({ "id": id }))?;
            }
        }
        let ids = items.iter().map(|item| item.id.clone()).collect();
        *self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ids);
        Ok(())
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
        let events = EventLog::at(&self.paths.events).read_all()?;
        if events.is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.paths.model.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut model = pa_project::model::ReadModel::open(&self.paths.model)?;
        pa_project::project::project(&mut model, &self.paths.project_id, &events)?;
        Ok(())
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
            let Some(id) = event.payload.get("id").and_then(Value::as_str) else {
                continue;
            };
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

    fn append(&self, log: &EventLog, event_type: &str, payload: Value) -> anyhow::Result<()> {
        let event = Event {
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
        };
        log.append(&event)
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
        assert_eq!(events[0].payload["id"], "P1");
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
        assert_eq!(removals[0].payload["id"], "P2");

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

        let restored = fresh.entity_json("P1").unwrap().unwrap();
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
}
