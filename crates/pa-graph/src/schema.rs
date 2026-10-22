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
/// # Errors
/// Propagates any SQLite error from `execute_batch`.
pub fn apply(conn: &rusqlite::Connection) -> crate::Result<()> {
    let batch = SCHEMA.join(";\n");
    conn.execute_batch(&batch)?;
    Ok(())
}

/// Schema version written into `store_meta.schema_version`.
pub const SCHEMA_VERSION: i64 = 1;

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
}
