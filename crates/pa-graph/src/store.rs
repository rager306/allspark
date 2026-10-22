//! The SQLite-backed graph store: connections, node/edge writes, and reads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::Result;
use crate::hash::FileHash;
use crate::schema;
use crate::types::{Edge, EdgeType, FileChange, Label, Node};

/// An open code-graph database.
///
/// The store owns a single [`Connection`]; it is not `Sync` and callers that
/// need concurrency should open one store per thread (SQLite WAL allows
/// concurrent readers).
pub struct GraphStore {
    conn: Connection,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for GraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The rusqlite `Connection` is not `Debug`; report the path only.
        f.debug_struct("GraphStore")
            .field("path", &self.path)
            .field("conn", &"<sqlite connection>")
            .finish()
    }
}

impl GraphStore {
    /// Open (creating if absent) the database at `path` and apply the schema.
    ///
    /// # Errors
    /// Propagates SQLite errors and directory-creation failures.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        Self::configure(&conn)?;
        let store = Self {
            conn,
            path: Some(path),
        };
        store.apply_schema()?;
        Ok(store)
    }

    /// Open a private in-memory database (tests, ephemeral queries).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn)?;
        let store = Self { conn, path: None };
        store.apply_schema()?;
        Ok(store)
    }

    fn configure(conn: &Connection) -> Result<()> {
        // WAL keeps readers non-blocking while an index run writes; foreign
        // keys make the ON DELETE CASCADE in the schema effective.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }

    fn apply_schema(&self) -> Result<()> {
        schema::apply(&self.conn)?;
        self.conn.execute(
            "INSERT INTO store_meta(k, v) VALUES ('schema_version', ?1)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![schema::SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    /// Path of the backing database, or `None` for in-memory stores.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Borrow the underlying connection (advanced use, tests).
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    // ── projects ────────────────────────────────────────────────────────

    /// Register a project (idempotent), updating its root path and timestamp.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn register_project(&self, name: &str, root: &str) -> Result<()> {
        let now = now_rfc3339();
        self.conn.execute(
            "INSERT INTO projects(name, root_path, indexed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET root_path = excluded.root_path",
            params![name, root, now],
        )?;
        Ok(())
    }

    /// Mark a project's index as freshly written.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn touch_project(&self, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET indexed_at = ?1 WHERE name = ?2",
            params![now_rfc3339(), name],
        )?;
        Ok(())
    }

    /// Look up a project's root path.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn project_root(&self, name: &str) -> Result<Option<String>> {
        let root = self
            .conn
            .query_row(
                "SELECT root_path FROM projects WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .optional()?;
        Ok(root)
    }

    /// List every registered project name.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn list_projects(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM projects ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Delete a project and, by cascade, its nodes/edges/hashes.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn delete_project(&self, name: &str) -> Result<usize> {
        let n = self
            .conn
            .execute("DELETE FROM projects WHERE name = ?1", params![name])?;
        Ok(n)
    }

    // ── nodes & edges ───────────────────────────────────────────────────

    /// Insert or replace a node keyed by `(project, qualified_name)`.
    ///
    /// Returns the node's row id.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn upsert_node(&self, node: &Node) -> Result<i64> {
        let props = serde_json::to_string(&node.props)?;
        self.conn.execute(
            "INSERT INTO nodes(project,label,name,qualified_name,file_path,start_line,end_line,props_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(project, qualified_name) DO UPDATE SET
                label = excluded.label,
                name = excluded.name,
                file_path = excluded.file_path,
                start_line = excluded.start_line,
                end_line = excluded.end_line,
                props_json = excluded.props_json",
            params![
                node.project,
                node.label.as_str(),
                node.name,
                node.qualified_name,
                node.file_path,
                node.start_line,
                node.end_line,
                props
            ],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM nodes WHERE project = ?1 AND qualified_name = ?2",
            params![node.project, node.qualified_name],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// Insert an edge, ignoring duplicates (same `source`/`target`/`type`/`local_name`).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn insert_edge(
        &self,
        project: &str,
        source: i64,
        target: i64,
        edge_type: EdgeType,
        props: &serde_json::Value,
    ) -> Result<()> {
        let props = serde_json::to_string(props)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO edges(project,source,target,edge_type,props_json)
             VALUES (?1,?2,?3,?4,?5)",
            params![project, source, target, edge_type.as_str(), props],
        )?;
        Ok(())
    }

    /// Fetch a node by row id.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn node_by_id(&self, id: i64) -> Result<Option<Node>> {
        let node = self
            .conn
            .query_row(
                "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,props_json
                 FROM nodes WHERE id = ?1",
                params![id],
                row_to_node,
            )
            .optional()?;
        Ok(node)
    }

    /// Fetch a node by qualified name within a project.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn node_by_qn(&self, project: &str, qn: &str) -> Result<Option<Node>> {
        let node = self
            .conn
            .query_row(
                "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,props_json
                 FROM nodes WHERE project = ?1 AND qualified_name = ?2",
                params![project, qn],
                row_to_node,
            )
            .optional()?;
        Ok(node)
    }

    /// In-degree and out-degree of a node, counted across all edge types.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn degree(&self, node_id: i64) -> Result<(usize, usize)> {
        let out: i64 = self.conn.query_row(
            "SELECT count(*) FROM edges WHERE source = ?1",
            params![node_id],
            |r| r.get(0),
        )?;
        let inc: i64 = self.conn.query_row(
            "SELECT count(*) FROM edges WHERE target = ?1",
            params![node_id],
            |r| r.get(0),
        )?;
        Ok((inc as usize, out as usize))
    }

    /// All outgoing edges from `node_id`, optionally filtered by type.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn out_edges(&self, node_id: i64, edge_type: Option<EdgeType>) -> Result<Vec<Edge>> {
        self.edges_for("source", node_id, edge_type)
    }

    /// All incoming edges to `node_id`, optionally filtered by type.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn in_edges(&self, node_id: i64, edge_type: Option<EdgeType>) -> Result<Vec<Edge>> {
        self.edges_for("target", node_id, edge_type)
    }

    fn edges_for(
        &self,
        column: &str,
        node_id: i64,
        edge_type: Option<EdgeType>,
    ) -> Result<Vec<Edge>> {
        // `column` is one of two compile-time constants, never user input.
        let sql = match edge_type {
            Some(_) => format!(
                "SELECT id,project,source,target,edge_type,props_json FROM edges
                 WHERE {column} = ?1 AND edge_type = ?2"
            ),
            None => format!(
                "SELECT id,project,source,target,edge_type,props_json FROM edges WHERE {column} = ?1"
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| row_to_edge(r);
        let rows = match edge_type {
            Some(t) => stmt
                .query_map(params![node_id, t.as_str()], map)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            None => stmt
                .query_map(params![node_id], map)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
    }

    /// Number of nodes in a project.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn node_count(&self, project: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM nodes WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Refresh SQLite's query-planner statistics for the whole database.
    ///
    /// Without statistics the planner estimates from the schema alone and makes
    /// the wrong index choice on the hottest query: `trace` looked up a target
    /// symbol by name and then scanned `idx_edges_type` — all 53,862 edges —
    /// instead of seeking `idx_edges_target`. Measured on a 36k-node store:
    ///
    /// ```text
    /// trace   median 52.30 ms -> 0.08 ms   (472x over 40 symbols)
    /// impact  median 34.58 ms -> 0.04 ms   (628x)
    /// ```
    ///
    /// `PRAGMA optimize` is NOT a substitute: on a store that has never been
    /// analysed it is a no-op (it only refreshes statistics it considers stale),
    /// and the query stayed at 58 ms. Only a full `ANALYZE` fixes it.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn analyze(&self) -> Result<()> {
        self.conn.execute_batch("ANALYZE")?;
        Ok(())
    }

    /// Number of edges in a project.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn edge_count(&self, project: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM edges WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Delete every node and edge of a project (used before a full reindex).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn clear_project_graph(&self, project: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM edges WHERE project = ?1", params![project])?;
        self.conn
            .execute("DELETE FROM nodes WHERE project = ?1", params![project])?;
        Ok(())
    }

    /// Delete all nodes/edges/FTS rows originating from one file.
    ///
    /// Used before an incremental re-parse. The FTS rows are keyed by node id
    /// and the table is contentless (no cascade), so they are removed first —
    /// otherwise every reindex would leak one orphaned FTS row per old symbol.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn clear_file(&self, project: &str, rel_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM symbols_fts WHERE rowid IN
                (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2)",
            params![project, rel_path],
        )?;
        self.conn.execute(
            "DELETE FROM edges WHERE project = ?1 AND source IN
                (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2)",
            params![project, rel_path],
        )?;
        self.conn.execute(
            "DELETE FROM nodes WHERE project = ?1 AND file_path = ?2",
            params![project, rel_path],
        )?;
        Ok(())
    }

    /// Clear a file's FTS rows and the edges **originating** from its nodes,
    /// keeping the nodes themselves so a re-parse upserts onto stable ids and
    /// other files' incoming edges survive the edit.
    ///
    /// Used by the re-parse path of [`GraphStore::index_project`]. Symbols
    /// that disappeared from the new content are removed afterwards by
    /// [`GraphStore::delete_file_nodes_not_in`], once the fresh symbol set is
    /// known — their edges then cascade, which is the correct outcome. A file
    /// gone from disk uses [`GraphStore::clear_file`] instead.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn clear_file_for_reparse(&self, project: &str, rel_path: &str) -> Result<()> {
        self.clear_file_fts(project, rel_path)?;
        self.conn.execute(
            "DELETE FROM edges WHERE project = ?1 AND source IN
                (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2)",
            params![project, rel_path],
        )?;
        Ok(())
    }

    /// Delete every FTS row belonging to a file's nodes, **without** deleting
    /// the nodes. Used by the incremental in-place rewriter, which preserves
    /// node ids (so edges from other files survive) and must re-insert fresh
    /// FTS rows for the symbols it keeps.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn clear_file_fts(&self, project: &str, rel_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM symbols_fts WHERE rowid IN
                (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2)",
            params![project, rel_path],
        )?;
        Ok(())
    }

    /// Delete outgoing edges of the given `edge_types` from every node defined
    /// in `rel_path`. Used by the incremental rewriter to re-resolve a file's
    /// calls/imports without disturbing edges from other files.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn delete_outgoing_edges(
        &self,
        project: &str,
        rel_path: &str,
        edge_types: &[EdgeType],
    ) -> Result<()> {
        for et in edge_types {
            self.conn.execute(
                "DELETE FROM edges WHERE project = ?1 AND edge_type = ?2 AND source IN
                    (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?3)",
                params![project, et.as_str(), rel_path],
            )?;
        }
        Ok(())
    }

    /// Delete a file's nodes whose qualified name is **not** in `keep` (plus
    /// their FTS rows). Returns the number of nodes removed.
    ///
    /// Deleting a node cascades to the edges that reference it, which is the
    /// correct outcome for a symbol that disappeared from the file.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn delete_file_nodes_not_in(
        &self,
        project: &str,
        rel_path: &str,
        keep: &[String],
    ) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT id, qualified_name FROM nodes WHERE project = ?1 AND file_path = ?2",
        )?;
        let rows = stmt.query_map(params![project, rel_path], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut stale: Vec<i64> = Vec::new();
        for row in rows {
            let (id, qn) = row?;
            if !keep.iter().any(|k| k == &qn) {
                stale.push(id);
            }
        }
        drop(stmt);
        for id in &stale {
            self.conn
                .execute("DELETE FROM symbols_fts WHERE rowid = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM nodes WHERE id = ?1", params![id])?;
        }
        Ok(stale.len())
    }

    /// Delete a node's FTS row. The table is `contentless_delete=1`, so a
    /// plain rowid delete is accepted.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn delete_fts(&self, node_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM symbols_fts WHERE rowid = ?1", params![node_id])?;
        Ok(())
    }

    /// Write a node's searchable text into the contentless FTS5 table.
    ///
    /// `tokens` is the camelCase-split rendering of the name and qualified
    /// name (see [`crate::tokenize::split_for_fts`]).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn index_fts(
        &self,
        node_id: i64,
        name: &str,
        qn: &str,
        tokens: &str,
        body: &str,
        label: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO symbols_fts(rowid, name, qn, tokens, body, label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![node_id, name, qn, tokens, body, label],
        )?;
        Ok(())
    }

    // ── file hashes ─────────────────────────────────────────────────────

    /// Record a file's hashes and metadata.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn put_file_hash(&self, project: &str, rel_path: &str, hash: &FileHash) -> Result<()> {
        self.conn.execute(
            "INSERT INTO file_hashes(project,rel_path,sha256,blake3,mtime_ns,size)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(project, rel_path) DO UPDATE SET
                sha256 = excluded.sha256,
                blake3 = excluded.blake3,
                mtime_ns = excluded.mtime_ns,
                size = excluded.size",
            params![
                project,
                rel_path,
                hash.sha256,
                hash.blake3,
                hash.mtime_ns,
                hash.size
            ],
        )?;
        Ok(())
    }

    /// Fetch a single stored file hash.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn file_hash(&self, project: &str, rel_path: &str) -> Result<Option<FileHash>> {
        let h = self
            .conn
            .query_row(
                "SELECT sha256,blake3,mtime_ns,size FROM file_hashes
                 WHERE project = ?1 AND rel_path = ?2",
                params![project, rel_path],
                |r| {
                    Ok(FileHash {
                        sha256: r.get(0)?,
                        blake3: r.get(1)?,
                        mtime_ns: r.get(2)?,
                        size: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(h)
    }

    /// Every stored file hash for a project, keyed by relative path.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn all_file_hashes(&self, project: &str) -> Result<HashMap<String, FileHash>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path,sha256,blake3,mtime_ns,size FROM file_hashes WHERE project = ?1",
        )?;
        let rows = stmt.query_map(params![project], |r| {
            Ok((
                r.get::<_, String>(0)?,
                FileHash {
                    sha256: r.get(1)?,
                    blake3: r.get(2)?,
                    mtime_ns: r.get(3)?,
                    size: r.get(4)?,
                },
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (path, hash) = row?;
            out.insert(path, hash);
        }
        Ok(out)
    }

    /// Classify `rel_path`'s freshly computed hash against the stored row.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn classify(&self, project: &str, rel_path: &str, sha256: &str) -> Result<FileChange> {
        match self.file_hash(project, rel_path)? {
            None => Ok(FileChange::Added),
            Some(old) if old.sha256 == sha256 => Ok(FileChange::Unchanged),
            Some(_) => Ok(FileChange::Modified),
        }
    }

    /// Remove a stored file hash.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn remove_file_hash(&self, project: &str, rel_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_hashes WHERE project = ?1 AND rel_path = ?2",
            params![project, rel_path],
        )?;
        Ok(())
    }

    // ── file surfaces (G7) ──────────────────────────────────────────────

    /// Record a file's surface/body hashes and canonical surface JSON.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn put_file_surface(
        &self,
        project: &str,
        rel_path: &str,
        surface_sha: &str,
        body_sha: &str,
        symbols_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO file_surfaces(project,rel_path,surface_sha,body_sha,symbols_json,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(project, rel_path) DO UPDATE SET
                surface_sha = excluded.surface_sha,
                body_sha = excluded.body_sha,
                symbols_json = excluded.symbols_json,
                updated_at = excluded.updated_at",
            params![
                project,
                rel_path,
                surface_sha,
                body_sha,
                symbols_json,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Fetch a stored file surface hash as `(surface_sha, body_sha)`.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn file_surface(&self, project: &str, rel_path: &str) -> Result<Option<(String, String)>> {
        let row = self
            .conn
            .query_row(
                "SELECT surface_sha, body_sha FROM file_surfaces WHERE project = ?1 AND rel_path = ?2",
                params![project, rel_path],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    /// Every stored surface hash for a project, keyed by relative path, as
    /// `(surface_sha, body_sha)`.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn all_file_surfaces(&self, project: &str) -> Result<HashMap<String, (String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path, surface_sha, body_sha FROM file_surfaces WHERE project = ?1",
        )?;
        let rows = stmt.query_map(params![project], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (path, pair) = row?;
            out.insert(path, pair);
        }
        Ok(out)
    }

    /// Remove a stored file surface.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn remove_file_surface(&self, project: &str, rel_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_surfaces WHERE project = ?1 AND rel_path = ?2",
            params![project, rel_path],
        )?;
        Ok(())
    }

    // ── LSP surface cache (L11) ─────────────────────────────────────────

    /// Store (or refresh) a file's cached LSP surface.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn put_lsp_surface(
        &self,
        project: &str,
        rel_path: &str,
        defs_json: &str,
        surface_sha: &str,
        ref_bloom: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO lsp_surface(project,rel_path,defs_json,surface_sha,ref_bloom,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(project, rel_path) DO UPDATE SET
                defs_json = excluded.defs_json,
                surface_sha = excluded.surface_sha,
                ref_bloom = excluded.ref_bloom,
                updated_at = excluded.updated_at",
            params![
                project,
                rel_path,
                defs_json,
                surface_sha,
                ref_bloom,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Fetch a cached LSP surface row, if present.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn lsp_surface(
        &self,
        project: &str,
        rel_path: &str,
    ) -> Result<Option<crate::lsp_surface::LspSurfaceRow>> {
        let row = self
            .conn
            .query_row(
                "SELECT project,rel_path,defs_json,surface_sha,ref_bloom,updated_at
                 FROM lsp_surface WHERE project = ?1 AND rel_path = ?2",
                params![project, rel_path],
                |r| {
                    Ok(crate::lsp_surface::LspSurfaceRow {
                        project: r.get(0)?,
                        rel_path: r.get(1)?,
                        defs_json: r.get(2)?,
                        surface_sha: r.get(3)?,
                        ref_bloom: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Remove a cached LSP surface row.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn remove_lsp_surface(&self, project: &str, rel_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM lsp_surface WHERE project = ?1 AND rel_path = ?2",
            params![project, rel_path],
        )?;
        Ok(())
    }

    /// Number of cached LSP surface rows for a project.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn lsp_surface_count(&self, project: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM lsp_surface WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Record an indexing-coverage note (`skipped`, `parse_partial`, ...).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn record_coverage(
        &self,
        project: &str,
        rel_path: &str,
        kind: &str,
        detail: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO index_coverage(project,rel_path,kind,detail)
             VALUES (?1,?2,?3,?4)",
            params![project, rel_path, kind, detail],
        )?;
        Ok(())
    }

    /// Count of coverage notes for a project.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn coverage_count(&self, project: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM index_coverage WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Look up a symbol by (case-insensitive) name, returning its qualified
    /// names and locations. Used by the call resolver and `trace`.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn nodes_by_name(&self, project: &str, name: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,props_json
             FROM nodes WHERE project = ?1 AND name_lower = lower(?2)",
        )?;
        let rows = stmt.query_map(params![project, name], row_to_node)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ── bulk reads (architecture / changes / coverage) ──────────────────

    /// Every node in a project, in insertion order.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn all_nodes(&self, project: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,props_json
             FROM nodes WHERE project = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![project], row_to_node)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every edge in a project, in insertion order.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn all_edges(&self, project: &str) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,source,target,edge_type,props_json
             FROM edges WHERE project = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![project], row_to_edge)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every node defined inside one file (including the `File` node itself).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn nodes_in_file(&self, project: &str, rel_path: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project,label,name,qualified_name,file_path,start_line,end_line,props_json
             FROM nodes WHERE project = ?1 AND file_path = ?2 ORDER BY start_line, id",
        )?;
        let rows = stmt.query_map(params![project, rel_path], row_to_node)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Files that have an edge pointing at any node defined in `rel_path`
    /// (the depth-1 reverse-dependency set for closure repair, G7).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn incoming_source_files(&self, project: &str, rel_path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT n.file_path FROM edges e
             JOIN nodes n ON n.id = e.source
             WHERE e.project = ?1 AND e.target IN
                (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2)",
        )?;
        let rows = stmt.query_map(params![project, rel_path], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The `File`-label node ids of a project, keyed by repo-relative path.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn file_node_ids(&self, project: &str) -> Result<HashMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT file_path, id FROM nodes WHERE project = ?1 AND label = 'File'")?;
        let rows = stmt.query_map(params![project], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (path, id) = row?;
            out.insert(path, id);
        }
        Ok(out)
    }

    /// Every coverage note for a project as `(rel_path, kind, detail)` rows.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn coverage_rows(&self, project: &str) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT rel_path, kind, detail FROM index_coverage WHERE project = ?1
             ORDER BY rel_path, kind",
        )?;
        let rows = stmt.query_map(params![project], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Number of files with a stored hash (the denominator for coverage).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn file_count(&self, project: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM file_hashes WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Remove every coverage note for a project.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn clear_coverage(&self, project: &str) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM index_coverage WHERE project = ?1",
            params![project],
        )?;
        Ok(n)
    }
}

fn row_to_node(r: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    let label: String = r.get(2)?;
    let props: String = r.get(8)?;
    Ok(Node {
        id: r.get(0)?,
        project: r.get(1)?,
        label: Label::from_str_or_default(&label),
        name: r.get(3)?,
        qualified_name: r.get(4)?,
        file_path: r.get(5)?,
        start_line: r.get::<_, i64>(6)? as u32,
        end_line: r.get::<_, i64>(7)? as u32,
        props: serde_json::from_str(&props)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
    })
}

fn row_to_edge(r: &rusqlite::Row<'_>) -> rusqlite::Result<Edge> {
    let edge_type: String = r.get(4)?;
    let props: String = r.get(5)?;
    Ok(Edge {
        id: r.get(0)?,
        project: r.get(1)?,
        source: r.get(2)?,
        target: r.get(3)?,
        edge_type: EdgeType::from_str_or_default(&edge_type),
        props: serde_json::from_str(&props)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
    })
}

/// Current time as an RFC 3339-ish UTC string without pulling in `chrono`.
#[must_use]
pub fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("epoch:{secs}")
}

impl GraphStore {
    /// Search this store's project for symbols matching `query`.
    ///
    /// # Errors
    /// Propagates query validation and SQLite errors.
    pub fn search(
        &self,
        query: &str,
        opts: &crate::search::SearchOptions,
    ) -> Result<Vec<crate::search::SearchHit>> {
        crate::search::search(&self.conn, query, opts)
    }

    /// Trace the call graph from `opts.symbol`.
    ///
    /// # Errors
    /// Propagates [`crate::GraphError::SymbolNotFound`] and SQLite errors.
    pub fn trace(&self, opts: &crate::trace::TraceOptions) -> Result<crate::trace::TraceResult> {
        crate::trace::trace(self, opts)
    }

    /// Argument data-flow into `symbol`.
    ///
    /// # Errors
    /// Propagates [`crate::GraphError::SymbolNotFound`] and SQLite errors.
    pub fn data_flow(
        &self,
        project: &str,
        symbol: &str,
    ) -> Result<Vec<(String, Vec<serde_json::Value>)>> {
        crate::trace::data_flow(self, project, symbol)
    }

    /// Cross-service (HTTP) edges recorded in the project.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn cross_service(&self, project: &str) -> Result<Vec<crate::trace::CrossServiceEdge>> {
        crate::trace::cross_service(self, project)
    }
}
