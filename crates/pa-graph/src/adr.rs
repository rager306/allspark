//! G10 — ADR memory: Architecture Decision Records in SQLite.
//!
//! An Architecture Decision Record captures one decision: its context, the
//! decision itself, the consequences, and which earlier ADR (if any) it
//! supersedes. [`AdrStore`] persists them in a dedicated `adr` table inside the
//! same graph database and exposes record / query / supersession lookups.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::Result;

/// The lifecycle status of an ADR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdrStatus {
    /// Proposed, not yet accepted.
    Proposed,
    /// Accepted and in force.
    Accepted,
    /// Rejected.
    Rejected,
    /// Superseded by a later ADR.
    Superseded,
    /// No longer relevant.
    Deprecated,
}

impl AdrStatus {
    /// Stable lowercase wire name stored in the `status` column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
            Self::Deprecated => "deprecated",
        }
    }

    /// Parse a status; unknown names map to [`AdrStatus::Proposed`].
    #[must_use]
    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            "superseded" => Self::Superseded,
            "deprecated" => Self::Deprecated,
            _ => Self::Proposed,
        }
    }
}

/// One Architecture Decision Record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adr {
    /// Numeric id (assigned by the store on insert).
    pub id: i64,
    /// Short title.
    pub title: String,
    /// Lifecycle status.
    pub status: AdrStatus,
    /// Why the decision was needed.
    pub context: String,
    /// What was decided.
    pub decision: String,
    /// What follows from it.
    pub consequences: String,
    /// Id of the ADR this one supersedes, if any.
    pub supersedes: Option<i64>,
}

impl Adr {
    /// A new ADR with status [`AdrStatus::Proposed`] and no supersession.
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        context: impl Into<String>,
        decision: impl Into<String>,
        consequences: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            title: title.into(),
            status: AdrStatus::Proposed,
            context: context.into(),
            decision: decision.into(),
            consequences: consequences.into(),
            supersedes: None,
        }
    }

    /// Set the status.
    #[must_use]
    pub fn with_status(mut self, status: AdrStatus) -> Self {
        self.status = status;
        self
    }

    /// Set the superseded ADR id.
    #[must_use]
    pub fn superseding(mut self, id: i64) -> Self {
        self.supersedes = Some(id);
        self
    }
}

/// A query filter over the ADR table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdrQuery {
    /// Restrict to this status.
    pub status: Option<AdrStatus>,
    /// Case-insensitive substring match against title/context/decision.
    pub text: Option<String>,
}

impl AdrQuery {
    /// An unfiltered query.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict to a status.
    #[must_use]
    pub fn with_status(mut self, status: AdrStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Restrict by a case-insensitive substring.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
}

/// The `adr` DDL, idempotent.
const ADR_SCHEMA: &str = r"CREATE TABLE IF NOT EXISTS adr (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT NOT NULL,
    status       TEXT NOT NULL,
    context      TEXT NOT NULL DEFAULT '',
    decision     TEXT NOT NULL DEFAULT '',
    consequences TEXT NOT NULL DEFAULT '',
    supersedes   INTEGER REFERENCES adr(id) ON DELETE SET NULL
)";

/// A SQLite-backed ADR store.
pub struct AdrStore<'a> {
    conn: &'a Connection,
}

impl<'a> AdrStore<'a> {
    /// Wrap an existing connection and ensure the `adr` table exists.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn new(conn: &'a Connection) -> Result<Self> {
        conn.execute_batch(ADR_SCHEMA)?;
        Ok(Self { conn })
    }

    /// Record `adr` and return its assigned id.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn record_adr(&self, adr: &Adr) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO adr(title, status, context, decision, consequences, supersedes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                adr.title,
                adr.status.as_str(),
                adr.context,
                adr.decision,
                adr.consequences,
                adr.supersedes
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Fetch one ADR by id.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn get_adr(&self, id: i64) -> Result<Option<Adr>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, title, status, context, decision, consequences, supersedes
                 FROM adr WHERE id = ?1",
                params![id],
                Self::map_row,
            )
            .optional()?;
        Ok(row)
    }

    /// Query ADRs, newest first.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn query_adr(&self, query: &AdrQuery) -> Result<Vec<Adr>> {
        let mut sql = String::from(
            "SELECT id, title, status, context, decision, consequences, supersedes FROM adr",
        );
        let mut clauses: Vec<String> = Vec::new();
        let mut bindings: Vec<String> = Vec::new();
        if let Some(status) = query.status {
            bindings.push(status.as_str().to_string());
            clauses.push(format!("status = ?{}", bindings.len()));
        }
        if let Some(text) = &query.text {
            bindings.push(format!("%{}%", text.to_lowercase()));
            let n = bindings.len();
            clauses.push(format!(
                "(lower(title) LIKE ?{n} OR lower(context) LIKE ?{n} OR lower(decision) LIKE ?{n})"
            ));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bindings.iter()), Self::map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The id of the ADR that supersedes `id`, if any.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn superseded_by(&self, id: i64) -> Result<Option<i64>> {
        let row = self
            .conn
            .query_row(
                "SELECT id FROM adr WHERE supersedes = ?1 ORDER BY id LIMIT 1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row)
    }

    fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Adr> {
        let status: String = row.get(2)?;
        Ok(Adr {
            id: row.get(0)?,
            title: row.get(1)?,
            status: AdrStatus::from_str_or_default(&status),
            context: row.get(3)?,
            decision: row.get(4)?,
            consequences: row.get(5)?,
            supersedes: row.get(6)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_get_round_trip() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        let store = AdrStore::new(&conn).expect("schema");
        let id = store
            .record_adr(
                &Adr::new("Use SQLite", "need a store", "embed SQLite", "no server")
                    .with_status(AdrStatus::Accepted),
            )
            .expect("record");
        let adr = store.get_adr(id).expect("get").expect("present");
        assert_eq!(adr.title, "Use SQLite");
        assert_eq!(adr.status, AdrStatus::Accepted);
        assert_eq!(adr.decision, "embed SQLite");
    }

    #[test]
    fn query_filters_by_status_and_text() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        let store = AdrStore::new(&conn).expect("schema");
        store
            .record_adr(&Adr::new("A", "", "alpha decision", "").with_status(AdrStatus::Accepted))
            .expect("a");
        store
            .record_adr(&Adr::new("B", "", "beta decision", "").with_status(AdrStatus::Proposed))
            .expect("b");
        let accepted = store
            .query_adr(&AdrQuery::new().with_status(AdrStatus::Accepted))
            .expect("query");
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].title, "A");
        let alpha = store
            .query_adr(&AdrQuery::new().with_text("ALPHA"))
            .expect("query");
        assert_eq!(alpha.len(), 1);
        assert_eq!(alpha[0].title, "A");
    }

    #[test]
    fn supersession_is_queryable_both_ways() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        let store = AdrStore::new(&conn).expect("schema");
        let old = store.record_adr(&Adr::new("Old", "", "", "")).expect("old");
        let new = store
            .record_adr(&Adr::new("New", "", "", "").superseding(old))
            .expect("new");
        assert_eq!(store.superseded_by(old).expect("query"), Some(new));
        let record = store.get_adr(new).expect("get").expect("present");
        assert_eq!(record.supersedes, Some(old));
    }

    #[test]
    fn schema_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        AdrStore::new(&conn).expect("first");
        AdrStore::new(&conn).expect("second");
    }
}
