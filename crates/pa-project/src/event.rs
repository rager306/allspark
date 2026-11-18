//! The append-only event log: the canonical record of what a project wrote.
//!
//! One JSON object per line. The log is canonical; the SQLite read model is a
//! projection over it, so a damaged or dropped read model is rebuilt from the
//! log rather than lost.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The schema version this crate writes.
pub const SCHEMA_VERSION: u32 = 1;

/// Who produced an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    /// `agent`, `user`, or `system`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// One durable event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Stable identity; replaying the same id twice is a no-op.
    pub event_id: String,
    /// Dotted event type, e.g. `plan.item.updated`.
    #[serde(rename = "type")]
    pub event_type: String,
    pub schema_version: u32,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// RFC 3339 UTC.
    pub occurred_at: String,
    pub actor: Actor,
    /// The event body. Never carries a prompt or raw tool output.
    #[serde(default)]
    pub payload: Value,
}

impl Event {
    /// Check the invariants every event must keep.
    ///
    /// # Errors
    ///
    /// Returns an error naming the offending field.
    pub fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("eventId", self.event_id.as_str()),
            ("type", self.event_type.as_str()),
            ("projectId", self.project_id.as_str()),
            ("occurredAt", self.occurred_at.as_str()),
            ("actor.kind", self.actor.kind.as_str()),
        ] {
            if value.trim().is_empty() {
                bail!("event field `{field}` must be non-empty");
            }
        }
        if self.schema_version == 0 {
            bail!("event schemaVersion must be at least 1");
        }
        if self.schema_version > SCHEMA_VERSION {
            bail!(
                "event schemaVersion {} is newer than this build's {SCHEMA_VERSION}; \
                 refusing to read a record it may misread",
                self.schema_version
            );
        }
        if !self.payload.is_object() {
            bail!("event payload must be a JSON object");
        }
        Ok(())
    }

    /// One JSONL line, without the trailing newline.
    ///
    /// # Errors
    ///
    /// Returns an error when the event cannot be serialized.
    pub fn to_line(&self) -> Result<String> {
        serde_json::to_string(self).context("serialize event")
    }
}

/// An append-only event log for one project.
#[derive(Debug, Clone)]
pub struct EventLog {
    path: PathBuf,
}

impl EventLog {
    /// A log at `path`. Nothing is created until the first append.
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The log's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one event as a line and flush it.
    ///
    /// # Errors
    ///
    /// Returns an error when the event is invalid or the append fails.
    pub fn append(&self, event: &Event) -> Result<()> {
        event.validate()?;
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let line = event.to_line()?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("open {}", self.path.display()))?;
        writeln!(file, "{line}").with_context(|| format!("append to {}", self.path.display()))?;
        file.flush()
            .with_context(|| format!("flush {}", self.path.display()))
    }

    /// Every event in file order.
    ///
    /// A malformed line is an error with its line number: skipping it would
    /// silently drop a durable record, and replaying it as a default would
    /// invent one.
    ///
    /// # Errors
    ///
    /// Returns an error naming the line when it cannot be read or parsed.
    pub fn read_all(&self) -> Result<Vec<Event>> {
        let file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| format!("open {}", self.path.display()));
            }
        };
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for (index, line) in reader.lines().enumerate() {
            let line = line.with_context(|| format!("read {}", self.path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            let event: Event = serde_json::from_str(&line)
                .with_context(|| format!("parse {} line {}", self.path.display(), index + 1))?;
            event
                .validate()
                .with_context(|| format!("validate {} line {}", self.path.display(), index + 1))?;
            events.push(event);
        }
        Ok(events)
    }

    /// The last event id recorded, if the log holds any.
    ///
    /// # Errors
    ///
    /// Propagates a read or parse failure.
    pub fn last_event_id(&self) -> Result<Option<String>> {
        Ok(self.read_all()?.last().map(|event| event.event_id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(id: &str) -> Event {
        Event {
            event_id: id.to_string(),
            event_type: "plan.item.updated".to_string(),
            schema_version: SCHEMA_VERSION,
            project_id: "sha256:abc".to_string(),
            session_id: Some("01J".to_string()),
            occurred_at: "2026-11-18T12:00:00Z".to_string(),
            actor: Actor {
                kind: "agent".to_string(),
                id: Some("root".to_string()),
            },
            payload: json!({ "item_id": "P1", "status": "completed" }),
        }
    }

    #[test]
    fn an_event_round_trips_through_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::at(dir.path().join("events").join("s1.jsonl"));
        log.append(&event("e1")).unwrap();
        log.append(&event("e2")).unwrap();

        let events = log.read_all().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], event("e1"));
        assert_eq!(log.last_event_id().unwrap().as_deref(), Some("e2"));
    }

    #[test]
    fn a_missing_log_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::at(dir.path().join("absent.jsonl"));
        assert_eq!(log.read_all().unwrap(), Vec::<Event>::new());
        assert_eq!(log.last_event_id().unwrap(), None);
    }

    #[test]
    fn a_malformed_line_names_its_line_number() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let good = event("e1").to_line().unwrap();
        std::fs::write(&path, format!("{good}\nnot json\n")).unwrap();

        let error = EventLog::at(&path).read_all().unwrap_err().to_string();
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn an_empty_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let good = event("e1").to_line().unwrap();
        std::fs::write(&path, format!("{good}\n\n{good}\n")).unwrap();

        assert_eq!(EventLog::at(&path).read_all().unwrap().len(), 2);
    }

    #[test]
    fn an_empty_identity_is_refused() {
        let mut bad = event("  ");
        let error = bad.validate().unwrap_err().to_string();
        assert!(error.contains("eventId"), "{error}");

        bad = event("e1");
        bad.event_type.clear();
        let error = bad.validate().unwrap_err().to_string();
        assert!(error.contains("type"), "{error}");
    }

    #[test]
    fn a_newer_schema_version_is_refused_rather_than_guessed() {
        let mut future = event("e1");
        future.schema_version = SCHEMA_VERSION + 1;

        let error = future.validate().unwrap_err().to_string();
        assert!(error.contains("newer than this build"), "{error}");
    }

    #[test]
    fn a_zero_schema_version_is_refused() {
        let mut bad = event("e1");
        bad.schema_version = 0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn a_non_object_payload_is_refused() {
        let mut bad = event("e1");
        bad.payload = json!(["not", "an", "object"]);
        let error = bad.validate().unwrap_err().to_string();
        assert!(error.contains("payload"), "{error}");
    }

    #[test]
    fn an_invalid_event_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::at(dir.path().join("events.jsonl"));
        let mut bad = event("e1");
        bad.schema_version = 0;

        assert!(log.append(&bad).is_err());
        assert_eq!(log.read_all().unwrap(), Vec::<Event>::new());
    }
}
