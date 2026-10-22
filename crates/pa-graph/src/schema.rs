//! SQLite schema for the on-disk code graph.
//!
//! Mirrors the `codebase-memory-mcp` store shape (nodes/edges with GENERATED
//! columns, a `projects` registry, `file_hashes`, `index_coverage`) with the
//! FTS5 symbol index that [`crate::search`] queries. All DDL is idempotent so
//! [`apply`] can run on every open.

/// Every DDL statement, in dependency order.
pub const SCHEMA: &[&str] = &[
    // Project registry: one row per indexed root.
    r"CREATE TABLE IF NOT EXISTS projects (
        name       TEXT PRIMARY KEY,
        root_path  TEXT NOT NULL,
        indexed_at TEXT NOT NULL
    )",
    // Key/value store metadata (schema version, fingerprints).
    r"CREATE TABLE IF NOT EXISTS store_meta (
        k TEXT PRIMARY KEY,
        v TEXT NOT NULL
    )",
    // Per-file content hashes for incremental classification.
    r"CREATE TABLE IF NOT EXISTS file_hashes (
        project  TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        rel_path TEXT NOT NULL,
        sha256   TEXT NOT NULL,
        blake3   TEXT NOT NULL DEFAULT '',
        mtime_ns INTEGER NOT NULL DEFAULT 0,
        size     INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (project, rel_path)
    )",
    // Graph nodes: symbols, files, folders.
    r"CREATE TABLE IF NOT EXISTS nodes (
        id             INTEGER PRIMARY KEY AUTOINCREMENT,
        project        TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        label          TEXT NOT NULL,
        name           TEXT NOT NULL,
        qualified_name TEXT NOT NULL,
        file_path      TEXT NOT NULL DEFAULT '',
        start_line     INTEGER NOT NULL DEFAULT 0,
        end_line       INTEGER NOT NULL DEFAULT 0,
        props_json     TEXT NOT NULL DEFAULT '{}',
        name_lower     TEXT GENERATED ALWAYS AS (lower(name)) VIRTUAL,
        qn_lower       TEXT GENERATED ALWAYS AS (lower(qualified_name)) VIRTUAL,
        UNIQUE (project, qualified_name)
    )",
    // Graph edges: CALLS / IMPORTS / CONTAINS / ...
    r"CREATE TABLE IF NOT EXISTS edges (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        project    TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        source     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
        target     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
        edge_type  TEXT NOT NULL,
        props_json TEXT NOT NULL DEFAULT '{}',
        -- IMPORTS edges dedupe per (source, target, type, local_name); NULL
        -- never conflicts in a UNIQUE index, so an absent name collapses to ''.
        local_name_gen TEXT GENERATED ALWAYS AS (
            coalesce(json_extract(props_json, '$.local_name'), '')
        ) VIRTUAL,
        UNIQUE (source, target, edge_type, local_name_gen)
    )",
    // Per-file surface hashes (G7 closure-repair). `surface_sha` hashes the
    // file's signatures only (bodies excluded), so a body-only edit leaves it
    // unchanged and a depth-1 closure needs no fixpoint iteration.
    r"CREATE TABLE IF NOT EXISTS file_surfaces (
        project     TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        rel_path    TEXT NOT NULL,
        surface_sha TEXT NOT NULL,
        body_sha    TEXT NOT NULL DEFAULT '',
        symbols_json TEXT NOT NULL DEFAULT '[]',
        updated_at  TEXT NOT NULL DEFAULT '',
        PRIMARY KEY (project, rel_path)
    )",
    // LSP-surface cache (L11): the consumed surface of a file (defs + refs),
    // invalidated when `surface_sha` changes.
    r"CREATE TABLE IF NOT EXISTS lsp_surface (
        project     TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        rel_path    TEXT NOT NULL,
        defs_json   TEXT NOT NULL DEFAULT '[]',
        surface_sha TEXT NOT NULL,
        ref_bloom   TEXT NOT NULL DEFAULT '',
        updated_at  TEXT NOT NULL DEFAULT '',
        PRIMARY KEY (project, rel_path)
    )",
    // Index coverage: honest accounting of what was skipped or partial.
    // Hash-linked receipts of index runs (the `witness` chain). One row per
    // completed run: `input_hash` is the sorted (rel_path, sha256) digest, so
    // two runs over an unchanged tree agree and an edited tree does not.
    r"CREATE TABLE IF NOT EXISTS index_receipts (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        project    TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        hash       TEXT NOT NULL,
        prev_hash  TEXT NOT NULL,
        input_hash TEXT NOT NULL,
        at         INTEGER NOT NULL
    )",
    r"CREATE INDEX IF NOT EXISTS idx_receipts_project ON index_receipts(project)",
    r"CREATE TABLE IF NOT EXISTS index_coverage (
        project  TEXT NOT NULL,
        rel_path TEXT NOT NULL,
        kind     TEXT NOT NULL,
        detail   TEXT NOT NULL DEFAULT '',
        PRIMARY KEY (project, rel_path, kind)
    )",
    // Contentless FTS5 over symbol names/qualified-names/labels/bodies plus a
    // dedicated split-token column fed by `crate::tokenize::split_for_fts`.
    // contentless_delete=1 lets a row be removed with a plain
    // `DELETE FROM symbols_fts WHERE rowid = ?` (a contentless table otherwise
    // needs the special 'delete' command).
    r"CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
        name, qn, tokens, body, label,
        content='',
        contentless_delete=1,
        tokenize='unicode61 remove_diacritics 2'
    )",
    // Indexes.
    r"CREATE INDEX IF NOT EXISTS idx_nodes_label ON nodes(project, label)",
    r"CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(project, name)",
    r"CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(project, file_path)",
    r"CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(project, source, edge_type)",
    r"CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(project, target, edge_type)",
    r"CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(project, edge_type)",
];

/// Apply every DDL statement to `conn`.
///
/// This runs the DDL only. The schema *version* is checked by
/// [`crate::GraphStore::apply_schema`], which owns the store path the mismatch
/// error has to name; a reader that finds a foreign version there must not get
/// as far as letting `IF NOT EXISTS` paper over it.
///
/// # Errors
/// Propagates any SQLite error from `execute_batch`.
pub fn apply(conn: &rusqlite::Connection) -> crate::Result<()> {
    let batch = SCHEMA.join(";\n");
    conn.execute_batch(&batch)?;
    Ok(())
}

/// The format version of everything [`SCHEMA`] declares.
///
/// This is a check, not a stamp: the store compares it with
/// `store_meta.schema_version` on every open. Equal versions open; an absent
/// key is adopted (a brand-new store, or one written before the key existed —
/// the only migration this crate can honour, since it has no migration engine);
/// a different version fails the open and is left untouched, so the mismatch
/// stays visible instead of being overwritten by the reader.
pub const SCHEMA_VERSION: i64 = 1;

/// `store_meta` key holding the store's recorded [`SCHEMA_VERSION`], as text.
pub const SCHEMA_VERSION_KEY: &str = "schema_version";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_is_idempotent_and_creates_core_tables() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        apply(&conn).expect("first apply");
        apply(&conn).expect("second apply is a no-op");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type IN ('table','index')",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert!(count >= 8, "expected core objects, got {count}");
    }

    #[test]
    fn fts5_is_available() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        apply(&conn).expect("apply");
        let v: String = conn
            .query_row("SELECT sqlite_version()", [], |r| r.get(0))
            .expect("version");
        // FTS5 must be compiled in (the `bundled` feature enables it).
        assert_ne!(v, String::new(), "sqlite_version() is nonempty");
    }

    #[test]
    fn generated_columns_are_queryable() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        apply(&conn).expect("apply");
        conn.execute(
            "INSERT INTO projects(name, root_path, indexed_at) VALUES ('p','/tmp','now')",
            [],
        )
        .expect("project");
        conn.execute(
            "INSERT INTO nodes(project,label,name,qualified_name,file_path,start_line,end_line)
             VALUES ('p','Function','ParseConfig','p::ParseConfig','a.rs',1,9)",
            [],
        )
        .expect("node");
        let lower: String = conn
            .query_row(
                "SELECT name_lower FROM nodes WHERE name='ParseConfig'",
                [],
                |r| r.get(0),
            )
            .expect("name_lower");
        assert_eq!(lower, "parseconfig");
    }

    /// Read `store_meta.schema_version` from the file itself, so a test sees
    /// the row an outside reader would rather than a cached value.
    fn stored_version(path: &std::path::Path) -> Option<String> {
        use rusqlite::OptionalExtension as _;
        let conn = rusqlite::Connection::open(path).expect("open raw");
        conn.query_row(
            "SELECT v FROM store_meta WHERE k = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .expect("read the recorded version")
    }

    #[test]
    fn a_fresh_store_records_the_current_schema_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        crate::GraphStore::open(&db).expect("open a fresh store");
        assert_eq!(stored_version(&db), Some(SCHEMA_VERSION.to_string()));
    }

    #[test]
    fn reopening_a_current_store_does_not_rewrite_the_version_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        crate::GraphStore::open(&db).expect("create the store");
        // `01` parses to the current version and renders differently, so a
        // rewritten row is visible: an open that adopted or re-stamped the
        // version would normalise it to `1`.
        let planted = format!("0{SCHEMA_VERSION}");
        {
            let conn = rusqlite::Connection::open(&db).expect("open raw");
            conn.execute(
                "UPDATE store_meta SET v = ?1 WHERE k = 'schema_version'",
                [&planted],
            )
            .expect("plant an equal version in another spelling");
        }
        crate::GraphStore::open(&db).expect("reopen a current store");
        assert_eq!(stored_version(&db), Some(planted));
    }

    #[test]
    fn a_store_from_another_version_fails_to_open_and_keeps_its_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        crate::GraphStore::open(&db).expect("create the store");
        let foreign = SCHEMA_VERSION + 1;
        {
            let conn = rusqlite::Connection::open(&db).expect("open raw");
            conn.execute(
                "UPDATE store_meta SET v = ?1 WHERE k = 'schema_version'",
                [foreign.to_string()],
            )
            .expect("plant a foreign version");
        }
        let err = crate::GraphStore::open(&db).expect_err("a foreign version must not open");
        let message = err.to_string();
        let crate::GraphError::SchemaVersionMismatch {
            store,
            found,
            expected,
        } = err
        else {
            panic!("expected a schema version mismatch, got {message}");
        };
        let expected_fields = (
            db.display().to_string(),
            foreign.to_string(),
            SCHEMA_VERSION,
        );
        assert_eq!((store, found, expected), expected_fields);
        // Both versions and the store the reader has to replace are in the text.
        assert!(
            message.contains(&foreign.to_string())
                && message.contains(&SCHEMA_VERSION.to_string())
                && message.contains(&db.display().to_string()),
            "the message names both versions and the store: {message}"
        );
        assert_eq!(stored_version(&db), Some(foreign.to_string()));
    }

    #[test]
    fn a_store_with_an_unreadable_version_fails_to_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        crate::GraphStore::open(&db).expect("create the store");
        {
            let conn = rusqlite::Connection::open(&db).expect("open raw");
            conn.execute(
                "UPDATE store_meta SET v = '' WHERE k = 'schema_version'",
                [],
            )
            .expect("blank the version");
        }
        // An empty value cannot be shown equal to this build's version, so it is
        // a foreign store: fail closed and leave the row as found.
        let err = crate::GraphStore::open(&db).expect_err("an unreadable version must not open");
        assert!(
            matches!(
                &err,
                crate::GraphError::SchemaVersionMismatch { found, .. } if found.is_empty()
            ),
            "{err}"
        );
        assert_eq!(stored_version(&db), Some(String::new()));
    }

    #[test]
    fn a_store_with_no_version_row_adopts_the_current_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("graph.db");
        crate::GraphStore::open(&db).expect("create the store");
        {
            let conn = rusqlite::Connection::open(&db).expect("open raw");
            conn.execute("DELETE FROM store_meta WHERE k = 'schema_version'", [])
                .expect("drop the version row, as a pre-key store has none");
        }
        crate::GraphStore::open(&db).expect("a store without the key adopts the current version");
        assert_eq!(stored_version(&db), Some(SCHEMA_VERSION.to_string()));
    }
}
