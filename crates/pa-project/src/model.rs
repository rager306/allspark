//! The derived read model: one SQLite file per project.
//!
//! Every row here is a projection of the event log, so the whole database is
//! rebuildable. Nothing canonical lives in it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// The schema version this build writes and reads.
pub const READ_MODEL_VERSION: i64 = 1;

/// The DDL, in dependency order. Every statement is idempotent: the model is
/// opened on each attach, and a missing table must be created while an
/// existing one is left alone.
const SCHEMA: &[&str] = &[
    // One row per entity, whatever its type. `data_json` holds the whole
    // entity so a new field or entity type is a metadata change, not a
    // migration.
    r"CREATE TABLE IF NOT EXISTS entities (
        id         TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        type       TEXT NOT NULL,
        data_json  TEXT NOT NULL,
        version    INTEGER NOT NULL DEFAULT 1,
        deleted    INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    r"CREATE INDEX IF NOT EXISTS entities_by_type ON entities(project_id, type)",
    // One row per relation instance between two entities.
    r"CREATE TABLE IF NOT EXISTS relations (
        id         TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        from_id    TEXT NOT NULL,
        predicate  TEXT NOT NULL,
        to_id      TEXT NOT NULL,
        scope      TEXT NOT NULL,
        data_json  TEXT NOT NULL DEFAULT '{}'
    )",
    r"CREATE INDEX IF NOT EXISTS relations_from ON relations(from_id, predicate)",
    r"CREATE INDEX IF NOT EXISTS relations_to ON relations(to_id, predicate)",
    // A reference into another database (the code graph, an artifact). The
    // target is a stable key, never another database's row id: a reindex
    // changes row ids, and a link that followed one would rot silently.
    r"CREATE TABLE IF NOT EXISTS entity_refs (
        id          TEXT PRIMARY KEY,
        project_id  TEXT NOT NULL,
        from_id     TEXT NOT NULL,
        predicate   TEXT NOT NULL,
        target_db   TEXT NOT NULL,
        target_type TEXT NOT NULL,
        target_key  TEXT NOT NULL,
        status      TEXT NOT NULL DEFAULT 'pending',
        resolver    TEXT NOT NULL,
        data_json   TEXT NOT NULL DEFAULT '{}'
    )",
    r"CREATE INDEX IF NOT EXISTS entity_refs_from ON entity_refs(from_id, predicate)",
    // One row per applied event: the idempotency ledger. An event id that is
    // already here is skipped, so a replay after a crash cannot apply twice.
    r"CREATE TABLE IF NOT EXISTS applied_events (
        event_id     TEXT PRIMARY KEY,
        seq          INTEGER NOT NULL,
        event_type   TEXT NOT NULL,
        schema_version INTEGER NOT NULL,
        occurred_at  TEXT NOT NULL
    )",
    // The projector's checkpoint: the last event applied, and how many.
    r"CREATE TABLE IF NOT EXISTS projection_state (
        project_id    TEXT PRIMARY KEY,
        last_event_id TEXT,
        last_seq      INTEGER NOT NULL DEFAULT 0,
        schema_version INTEGER NOT NULL,
        updated_at    TEXT NOT NULL
    )",
    // The model's own schema version, so a future format is refused rather
    // than misread.
    r"CREATE TABLE IF NOT EXISTS read_model_meta (
        k TEXT PRIMARY KEY,
        v TEXT NOT NULL
    )",
];

/// One stored cross-database reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRef {
    /// The reference id.
    pub id: String,
    /// The entity that holds the claim.
    pub from_id: String,
    /// The stable key in the target database.
    pub target_key: String,
    /// `pending`, `resolved`, or `dangling`.
    pub status: String,
    /// The target's type name, meaningful to its resolver.
    pub target_type: String,
    /// The target database name (`graph`, `knowledge`, ...).
    pub target_db: String,
}

/// An open project read model.
///
/// `Debug` is derived so a failed open can be inspected in a test; the
/// connection itself carries no useful debug output.
#[derive(Debug)]
pub struct ReadModel {
    conn: Connection,
    path: PathBuf,
}

impl ReadModel {
    /// Open (creating if absent) the read model at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot be opened, when its schema
    /// version is not this build's, or when the DDL fails.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
        // WAL keeps readers unblocked while the projector writes, and
        // `busy_timeout` bounds a contended writer instead of failing it.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let model = Self { conn, path };
        model.apply_schema()?;
        Ok(model)
    }

    /// The model's file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The connection, for reads.
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// The connection, for a transaction that needs `&mut`.
    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Count the live entities of `type_name`.
    ///
    /// # Errors
    ///
    /// Propagates a query failure.
    pub fn count_entities(&self, project_id: &str, type_name: &str) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT count(*) FROM entities WHERE project_id = ?1 AND type = ?2 AND deleted = 0",
            rusqlite::params![project_id, type_name],
            |row| row.get(0),
        )?;
        Ok(count.cast_unsigned())
    }

    /// The last event id the projector applied for `project_id`.
    ///
    /// # Errors
    ///
    /// Propagates a query failure.
    pub fn last_event_id(&self, project_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_event_id FROM projection_state WHERE project_id = ?1",
                rusqlite::params![project_id],
                |row| row.get(0),
            )
            .ok()
            .flatten())
    }

    /// The stored data of one entity, if it is live.
    ///
    /// # Errors
    ///
    /// Propagates a query failure or an unparsable stored body.
    pub fn entity_json(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT data_json FROM entities WHERE id = ?1 AND deleted = 0",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .ok();
        match stored {
            Some(text) => Ok(Some(serde_json::from_str(&text)?)),
            None => Ok(None),
        }
    }

    /// Count the relation instances of `predicate`.
    ///
    /// # Errors
    ///
    /// Propagates a query failure.
    pub fn relation_count(&self, predicate: &str) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT count(*) FROM relations WHERE predicate = ?1",
            rusqlite::params![predicate],
            |row| row.get(0),
        )?;
        Ok(count.cast_unsigned())
    }

    /// Every reference `from_id` holds, as `(target_key, status)`.
    ///
    /// # Errors
    ///
    /// Propagates a query failure.
    pub fn refs_from(&self, from_id: &str) -> Result<Vec<(String, String)>> {
        Ok(self
            .pending_refs(None)?
            .into_iter()
            .filter(|reference| reference.from_id == from_id)
            .map(|reference| (reference.target_key, reference.status))
            .collect())
    }

    /// Every reference whose status is `status`, or all of them when `None`.
    ///
    /// The checker walks this: a reference is a claim about another database
    /// that nothing verifies at write time, so the set that needs resolving is
    /// discovered from the model, not tracked in memory.
    ///
    /// # Errors
    ///
    /// Propagates a query failure.
    pub fn pending_refs(&self, status: Option<&str>) -> Result<Vec<PendingRef>> {
        let sql = match status {
            Some(_) => {
                "SELECT id, from_id, target_key, status, target_type, target_db \
                 FROM entity_refs WHERE status = ?1 ORDER BY id"
            }
            None => {
                "SELECT id, from_id, target_key, status, target_type, target_db \
                     FROM entity_refs ORDER BY id"
            }
        };
        let mut statement = self.conn.prepare(sql)?;
        let map = |row: &rusqlite::Row<'_>| {
            Ok(PendingRef {
                id: row.get(0)?,
                from_id: row.get(1)?,
                target_key: row.get(2)?,
                status: row.get(3)?,
                target_type: row.get(4)?,
                target_db: row.get(5)?,
            })
        };
        let rows = match status {
            Some(status) => statement.query_map(rusqlite::params![status], map)?,
            None => statement.query_map([], map)?,
        };
        let mut refs = Vec::new();
        for row in rows {
            refs.push(row?);
        }
        Ok(refs)
    }

    /// Set a reference's resolution status.
    ///
    /// # Errors
    ///
    /// Propagates a query failure.
    pub fn set_ref_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE entity_refs SET status = ?1 WHERE id = ?2",
            rusqlite::params![status, id],
        )?;
        Ok(())
    }

    fn apply_schema(&self) -> Result<()> {
        let batch = SCHEMA.join(";\n");
        self.conn.execute_batch(&batch)?;
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT v FROM read_model_meta WHERE k = 'version'",
                [],
                |row| row.get(0),
            )
            .ok();
        match stored {
            None => {
                self.conn.execute(
                    "INSERT INTO read_model_meta(k, v) VALUES ('version', ?1)",
                    [READ_MODEL_VERSION.to_string()],
                )?;
                Ok(())
            }
            Some(version) if version == READ_MODEL_VERSION.to_string() => Ok(()),
            Some(version) => anyhow::bail!(
                "{} holds read-model version {version}, but this build writes \
                 {READ_MODEL_VERSION}; rebuild the model from the event log",
                self.path.display()
            ),
        }
    }
}
