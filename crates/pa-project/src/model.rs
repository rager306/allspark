//! The derived read model: one SQLite file per project.
//!
//! Every row here is a projection of the event log, so the whole database is
//! rebuildable. Nothing canonical lives in it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

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
    /// Returns an error when the database cannot be opened or the DDL fails. A
    /// schema version that is not this build's is *not* an error: the file is
    /// derived, so it is dropped and rebuilt, exactly as a deleted model is.
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
        let mut statement = self
            .conn
            .prepare("SELECT target_key, status FROM entity_refs WHERE from_id = ?1 ORDER BY id")?;
        let rows = statement.query_map(rusqlite::params![from_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        let mut refs = Vec::new();
        for row in rows {
            refs.push(row?);
        }
        Ok(refs)
    }

    /// Every live entity of `type_name` for `project_id`, as `(id, data)`.
    ///
    /// The id is a column, not part of `data_json`, so it is returned beside
    /// the data rather than injected into it: one row, one place its id lives.
    ///
    /// Ordered by row id, which follows first-insertion order. A caller that
    /// needs a different order sorts the result; the read model does not
    /// impose one, because the order of a plan is a property of the plan, not
    /// of the table.
    ///
    /// # Errors
    ///
    /// Propagates a query failure.
    pub fn entities_of_type(
        &self,
        project_id: &str,
        type_name: &str,
    ) -> Result<Vec<(String, serde_json::Value)>> {
        let mut statement = self.conn.prepare(
            "SELECT id, data_json FROM entities
             WHERE project_id = ?1 AND type = ?2 AND deleted = 0 ORDER BY id",
        )?;
        let rows = statement.query_map(rusqlite::params![project_id, type_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut values = Vec::new();
        for row in rows {
            let (id, text) = row?;
            values.push((
                id,
                serde_json::from_str(&text).context("parse entity data_json")?,
            ));
        }
        Ok(values)
    }

    /// The log byte this model is projected through, when a producer recorded
    /// one.
    ///
    /// Kept in the model rather than in the writer's memory so it cannot
    /// outlive the data it describes: a deleted or replaced model has no
    /// checkpoint, and the next pass reads the whole log.
    ///
    /// # Errors
    ///
    /// Propagates a query failure.
    pub fn log_offset(&self) -> Result<Option<u64>> {
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT v FROM read_model_meta WHERE k = 'log_offset'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(stored.and_then(|value| value.parse().ok()))
    }

    /// Record the log byte this model is projected through.
    ///
    /// # Errors
    ///
    /// Propagates a write failure.
    pub fn set_log_offset(&self, offset: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO read_model_meta(k, v) VALUES ('log_offset', ?1)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            rusqlite::params![offset.to_string()],
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
            None => self.write_version(),
            Some(version) if version == READ_MODEL_VERSION.to_string() => Ok(()),
            Some(_) => {
                // Nothing in this file is canonical: the event log is. A format
                // change therefore rebuilds rather than refusing, which is the
                // same rule a deleted model already follows. Refusing here made
                // an upgraded build silently stop restoring a project plan,
                // because every caller treats a failed `open` as "no model".
                self.drop_tables()?;
                self.conn.execute_batch(&batch)?;
                self.write_version()
            }
        }
    }

    fn write_version(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO read_model_meta(k, v) VALUES ('version', ?1)",
            [READ_MODEL_VERSION.to_string()],
        )?;
        Ok(())
    }

    /// Drop every table this file holds, leaving the SQLite internals alone.
    ///
    /// Read from `sqlite_master` instead of a list of names: a table added to
    /// the schema must not need a second place updated, or the rebuild on a
    /// format change quietly keeps the stale one.
    fn drop_tables(&self) -> Result<()> {
        let names: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()?
        };
        for name in names {
            self.conn
                .execute_batch(&format!("DROP TABLE IF EXISTS \"{name}\""))?;
        }
        Ok(())
    }
}
