//! R1 — scoped context index: *physically* isolated per-scope shards.
//!
//! Filtering after a global lookup is not a scope boundary: the data of every
//! scope still sits in one structure, so a bug in the filter leaks across
//! scopes. [`ScopedContextIndex`] instead gives each scope its own SQLite shard
//! file under one root, and a search opens *only* the shard it was asked for.
//! There is no global table to post-filter.
//!
//! The index is fail-closed. When a shard cannot be opened it is moved to
//! `quarantine/` and marked; every later operation that names that scope
//! returns [`ContextIndexError::ScopeQuarantined`] rather than a silently
//! incomplete answer. [`ScopedContextIndex::recover`] is the route back: it
//! re-opens each quarantined shard and restores the ones that load.
//!
//! ```no_run
//! use pa_graph::ScopedContextIndex;
//!
//! let index = ScopedContextIndex::open("/tmp/ctx")?;
//! let shard = index.scope("agent-a")?;
//! shard.insert("note-1", "the parser rejects tabs")?;
//! let hits = shard.search("parser tabs", 5)?;
//! # Ok::<(), pa_graph::ContextIndexError>(())
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use rusqlite::{params, Connection, OptionalExtension};

/// Suffix of a scope's shard file under the index root.
const SHARD_SUFFIX: &str = ".ctx.db";
/// Suffix of a quarantine mark file under `quarantine/`.
const MARK_SUFFIX: &str = ".mark.json";
/// Longest accepted scope name, in bytes.
const MAX_SCOPE_BYTES: usize = 128;
/// Directory holding quarantined shard files and their marks.
const QUARANTINE_DIR: &str = "quarantine";

/// Errors from the scoped context index.
#[derive(Debug, thiserror::Error)]
pub enum ContextIndexError {
    /// A scope name is empty, too long, or contains unsafe bytes.
    #[error("invalid scope name: {0}")]
    InvalidScope(String),
    /// The named scope's shard is quarantined; the reason is included.
    #[error("context scope {0} is quarantined: {1}")]
    ScopeQuarantined(String, String),
    /// A shard file could not be opened or fails its manifest check.
    #[error("context shard error: {0}")]
    Shard(String),
    /// SQLite returned an error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A filesystem operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// (De)serializing a quarantine mark failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convenience alias for [`ContextIndexError`].
pub type ContextIndexResult<T> = std::result::Result<T, ContextIndexError>;

/// One recorded quarantine: which scope, why, and when it happened.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuarantineMark {
    /// The scope whose shard was quarantined.
    pub scope: String,
    /// Human-readable reason the shard was set aside.
    pub reason: String,
    /// Unix seconds when the quarantine was recorded.
    pub quarantined_at: u64,
}

/// The outcome of a [`ScopedContextIndex::recover`] pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Scopes whose shards loaded and were restored to the index.
    pub recovered: Vec<String>,
    /// Scopes that still fail to load, with the reason.
    pub still_quarantined: Vec<(String, String)>,
}

/// One ranked match from a single scope's shard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextHit {
    /// Identifier of the matching entry.
    pub id: String,
    /// Number of distinct query terms matched (higher ranks first).
    pub score: usize,
    /// The stored text of the entry.
    pub text: String,
}

/// One scope's physically isolated shard, backed by its own SQLite file.
///
/// A shard never sees another scope's rows: there is exactly one shard file per
/// scope and its connection only ever touches that file.
pub struct Shard {
    scope: String,
    path: PathBuf,
    conn: Connection,
}

impl std::fmt::Debug for Shard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shard")
            .field("scope", &self.scope)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Shard {
    /// Open (creating if absent) the shard for `scope` under `root`.
    ///
    /// # Errors
    /// Returns [`ContextIndexError::Shard`] when an existing file is not a
    /// valid shard for `scope` (its stored manifest disagrees), and propagates
    /// SQLite/I/O failures.
    pub fn open(root: &Path, scope: &str) -> ContextIndexResult<Self> {
        validate_scope(scope)?;
        let path = shard_path(root, scope);
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS context_entries (
                 id         TEXT PRIMARY KEY,
                 text       TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        let stored: Option<String> = conn
            .query_row("SELECT v FROM meta WHERE k = 'scope'", [], |r| r.get(0))
            .optional()?;
        match stored {
            Some(found) if found == scope => {}
            Some(found) => {
                return Err(ContextIndexError::Shard(format!(
                    "shard manifest names scope {found:?}, expected {scope:?}"
                )));
            }
            None => {
                conn.execute(
                    "INSERT INTO meta(k, v) VALUES ('scope', ?1)",
                    params![scope],
                )?;
            }
        }
        Ok(Self {
            scope: scope.to_string(),
            path,
            conn,
        })
    }

    /// The scope this shard belongs to.
    #[must_use]
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// Path of the backing shard file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Insert or replace the entry `id`, returning `true` when it is new.
    ///
    /// # Errors
    /// Propagates SQLite failures.
    pub fn insert(&self, id: &str, text: &str) -> ContextIndexResult<bool> {
        let existed: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM context_entries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        self.conn.execute(
            "INSERT INTO context_entries(id, text) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET text = excluded.text",
            params![id, text],
        )?;
        Ok(existed.is_none())
    }

    /// Fetch the text stored under `id`, if any.
    ///
    /// # Errors
    /// Propagates SQLite failures.
    pub fn get(&self, id: &str) -> ContextIndexResult<Option<String>> {
        let text = self
            .conn
            .query_row(
                "SELECT text FROM context_entries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(text)
    }

    /// Remove `id`, reporting whether it was present.
    ///
    /// # Errors
    /// Propagates SQLite failures.
    pub fn remove(&self, id: &str) -> ContextIndexResult<bool> {
        let n = self
            .conn
            .execute("DELETE FROM context_entries WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Number of entries in this shard.
    ///
    /// # Errors
    /// Propagates SQLite failures.
    pub fn len(&self) -> ContextIndexResult<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM context_entries", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Whether this shard holds no entries.
    ///
    /// # Errors
    /// Propagates SQLite failures.
    pub fn is_empty(&self) -> ContextIndexResult<bool> {
        Ok(self.len()? == 0)
    }

    /// Rank this shard's entries against `query`, returning at most `limit`.
    ///
    /// The scan touches only this shard's file. Scoring is the number of
    /// distinct whitespace-separated query terms present in the entry text
    /// (case-insensitive); ties break on ascending id so the order is stable.
    ///
    /// # Errors
    /// Propagates SQLite failures.
    pub fn search(&self, query: &str, limit: usize) -> ContextIndexResult<Vec<ContextHit>> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|t| !t.is_empty())
            .collect();
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, text FROM context_entries")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut hits = Vec::new();
        for row in rows {
            let (id, text) = row?;
            let lower = text.to_lowercase();
            let score = terms.iter().filter(|t| lower.contains(t.as_str())).count();
            if score > 0 {
                hits.push(ContextHit { id, score, text });
            }
        }
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        hits.truncate(limit);
        Ok(hits)
    }
}

/// A root directory holding one physical SQLite shard per context scope.
pub struct ScopedContextIndex {
    root: PathBuf,
    quarantine_dir: PathBuf,
    marks: RwLock<BTreeMap<String, QuarantineMark>>,
}

impl std::fmt::Debug for ScopedContextIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedContextIndex")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl ScopedContextIndex {
    /// Open an index root, quarantining any shard that fails to load.
    ///
    /// The root and its `quarantine/` subdirectory are created if absent.
    /// Every `*.ctx.db` file already present is opened and validated; a file
    /// that fails is moved into `quarantine/` and marked, so one bad shard
    /// never denies the whole index. Its scope then fails closed on access.
    ///
    /// # Errors
    /// Propagates I/O failures creating the root, and errors reading the
    /// quarantine directory.
    pub fn open(root: impl AsRef<Path>) -> ContextIndexResult<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let quarantine_dir = root.join(QUARANTINE_DIR);
        std::fs::create_dir_all(&quarantine_dir)?;
        let marks = load_marks(&quarantine_dir)?;
        let index = Self {
            root,
            quarantine_dir,
            marks: RwLock::new(marks),
        };
        index.sweep_unloadable_shards()?;
        Ok(index)
    }

    /// The index root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The quarantine directory.
    #[must_use]
    pub fn quarantine_dir(&self) -> &Path {
        &self.quarantine_dir
    }

    /// Open (creating if absent) the shard for `scope`.
    ///
    /// # Errors
    /// Returns [`ContextIndexError::ScopeQuarantined`] when the scope's shard
    /// is quarantined, [`ContextIndexError::InvalidScope`] for a bad name, and
    /// propagates shard-open failures.
    pub fn scope(&self, scope: &str) -> ContextIndexResult<Shard> {
        validate_scope(scope)?;
        if let Some(mark) = self.mark(scope)? {
            return Err(ContextIndexError::ScopeQuarantined(
                scope.to_string(),
                mark.reason,
            ));
        }
        Shard::open(&self.root, scope)
    }

    /// Move `scope`'s shard into `quarantine/` and mark it.
    ///
    /// Returns `true` when a shard file was moved, `false` when there was
    /// nothing to quarantine (no file, or already quarantined). After this,
    /// [`ScopedContextIndex::scope`] fails closed for `scope`.
    ///
    /// # Errors
    /// Returns [`ContextIndexError::InvalidScope`] for a bad name and
    /// propagates I/O failures moving the file or writing the mark.
    pub fn quarantine(&self, scope: &str) -> ContextIndexResult<bool> {
        validate_scope(scope)?;
        if self.mark(scope)?.is_some() {
            return Ok(false);
        }
        let source = shard_path(&self.root, scope);
        if !source.exists() {
            return Ok(false);
        }
        let reason = "manually quarantined".to_string();
        self.set_aside(scope, &source, &reason)?;
        Ok(true)
    }

    /// Re-open every quarantined shard, restoring the ones that load.
    ///
    /// Each marked shard is moved back from `quarantine/`, opened, and
    /// validated. A shard that loads has its mark cleared; one that still
    /// fails is moved back into `quarantine/` and reported in
    /// [`RecoveryReport::still_quarantined`].
    ///
    /// # Errors
    /// Propagates I/O failures moving files or clearing marks.
    pub fn recover(&self) -> ContextIndexResult<RecoveryReport> {
        let scopes: Vec<String> = self.marks.read().map_err(lock_poisoned)?.keys().cloned().collect();
        let mut report = RecoveryReport::default();
        for scope in scopes {
            let held = self.quarantine_dir.join(shard_filename(&scope));
            let restored = self.root.join(shard_filename(&scope));
            if held.exists() {
                std::fs::rename(&held, &restored)?;
            }
            match Shard::open(&self.root, &scope) {
                Ok(shard) => {
                    drop(shard);
                    self.clear_mark(&scope)?;
                    report.recovered.push(scope);
                }
                Err(error) => {
                    let detail = error.to_string();
                    if restored.exists() {
                        self.set_aside(&scope, &restored, &detail)?;
                    }
                    report.still_quarantined.push((scope, detail));
                }
            }
        }
        Ok(report)
    }

    /// Every scope name currently backed by a shard file under the root.
    ///
    /// # Errors
    /// Propagates errors reading the root directory.
    pub fn scope_names(&self) -> ContextIndexResult<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if let Some(scope) = scope_from_filename(&entry.file_name().to_string_lossy()) {
                if validate_scope(&scope).is_ok() {
                    names.push(scope);
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// The quarantine marks currently recorded, sorted by scope.
    ///
    /// # Errors
    /// Propagates a poisoned lock.
    pub fn quarantined(&self) -> ContextIndexResult<Vec<QuarantineMark>> {
        let marks = self.marks.read().map_err(lock_poisoned)?;
        Ok(marks.values().cloned().collect())
    }

    // ── internals ───────────────────────────────────────────────────────

    fn mark(&self, scope: &str) -> ContextIndexResult<Option<QuarantineMark>> {
        let marks = self.marks.read().map_err(lock_poisoned)?;
        Ok(marks.get(scope).cloned())
    }

    fn set_aside(
        &self,
        scope: &str,
        source: &Path,
        reason: &str,
    ) -> ContextIndexResult<()> {
        let target = self.quarantine_dir.join(shard_filename(scope));
        if target.exists() {
            let _ = std::fs::remove_file(&target);
        }
        std::fs::rename(source, &target)?;
        let mark = QuarantineMark {
            scope: scope.to_string(),
            reason: reason.to_string(),
            quarantined_at: now_secs(),
        };
        write_mark(&self.quarantine_dir, &mark)?;
        self.marks
            .write()
            .map_err(lock_poisoned)?
            .insert(scope.to_string(), mark);
        Ok(())
    }

    fn clear_mark(&self, scope: &str) -> ContextIndexResult<()> {
        let path = self.quarantine_dir.join(mark_filename(scope));
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.marks.write().map_err(lock_poisoned)?.remove(scope);
        Ok(())
    }

    fn sweep_unloadable_shards(&self) -> ContextIndexResult<()> {
        let mut broken = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(scope) = scope_from_filename(&name) else {
                continue;
            };
            if validate_scope(&scope).is_err() {
                continue;
            }
            match Shard::open(&self.root, &scope) {
                Ok(shard) => drop(shard),
                Err(error) => broken.push((scope, error.to_string())),
            }
        }
        for (scope, reason) in broken {
            let source = shard_path(&self.root, &scope);
            self.set_aside(&scope, &source, &reason)?;
        }
        Ok(())
    }
}

fn lock_poisoned<T>(_: std::sync::PoisonError<T>) -> ContextIndexError {
    ContextIndexError::Shard("context index lock poisoned".to_string())
}

fn validate_scope(scope: &str) -> ContextIndexResult<()> {
    let ok = !scope.is_empty()
        && scope.len() <= MAX_SCOPE_BYTES
        && scope
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    if ok {
        Ok(())
    } else {
        Err(ContextIndexError::InvalidScope(scope.to_string()))
    }
}

fn shard_filename(scope: &str) -> String {
    format!("{scope}{SHARD_SUFFIX}")
}

fn mark_filename(scope: &str) -> String {
    format!("{scope}{MARK_SUFFIX}")
}

fn shard_path(root: &Path, scope: &str) -> PathBuf {
    root.join(shard_filename(scope))
}

fn scope_from_filename(name: &str) -> Option<String> {
    name.strip_suffix(SHARD_SUFFIX).map(str::to_string)
}

fn write_mark(dir: &Path, mark: &QuarantineMark) -> ContextIndexResult<()> {
    let json = serde_json::to_string_pretty(mark)?;
    std::fs::write(dir.join(mark_filename(&mark.scope)), json)?;
    Ok(())
}

fn load_marks(dir: &Path) -> ContextIndexResult<BTreeMap<String, QuarantineMark>> {
    let mut marks = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(MARK_SUFFIX) {
            continue;
        }
        let raw = std::fs::read_to_string(entry.path())?;
        if let Ok(mark) = serde_json::from_str::<QuarantineMark>(&raw) {
            marks.insert(mark.scope.clone(), mark);
        }
    }
    Ok(marks)
}

fn now_secs() -> u64 {
    use std::time::UNIX_EPOCH;
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> (tempfile::TempDir, ScopedContextIndex) {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = ScopedContextIndex::open(dir.path().join("root")).expect("open");
        (dir, index)
    }

    #[test]
    fn open_creates_root_and_quarantine_dir() {
        let (_dir, index) = index();
        assert!(index.root().is_dir());
        assert!(index.quarantine_dir().is_dir());
        assert_eq!(index.scope_names().expect("names").len(), 0);
        assert_eq!(index.quarantined().expect("marks").len(), 0);
    }

    #[test]
    fn scopes_are_physically_isolated() {
        let (_dir, index) = index();
        let a = index.scope("agent-a").expect("scope a");
        let b = index.scope("agent-b").expect("scope b");
        a.insert("n1", "parser rejects tabs").expect("insert a");
        b.insert("n1", "totally different text").expect("insert b");

        // Same id, different scope: no collision, because the shards are files.
        assert_ne!(a.path(), b.path());
        assert_eq!(a.get("n1").expect("get a").as_deref(), Some("parser rejects tabs"));
        assert_eq!(b.get("n1").expect("get b").as_deref(), Some("totally different text"));

        // A search in one scope never sees the other's rows (no post-filter).
        let hits_a = a.search("parser tabs", 10).expect("search a");
        assert_eq!(hits_a.len(), 1);
        let hits_b = b.search("parser tabs", 10).expect("search b");
        assert!(hits_b.is_empty(), "scope b must not match scope a's text");
    }

    #[test]
    fn quarantine_fails_closed_then_recover_restores() {
        let (_dir, index) = index();
        {
            let shard = index.scope("agent-a").expect("scope");
            shard.insert("n1", "keep me").expect("insert");
        }
        assert!(index.quarantine("agent-a").expect("quarantine"));

        // Fail closed: naming the scope is an explicit error, not an empty read.
        let err = index.scope("agent-a").expect_err("must fail closed");
        assert!(matches!(err, ContextIndexError::ScopeQuarantined(_, _)));
        assert_eq!(index.quarantined().expect("marks").len(), 1);

        // A different scope is unaffected.
        assert!(index.scope("agent-b").is_ok());

        let report = index.recover().expect("recover");
        assert_eq!(report.recovered, vec!["agent-a".to_string()]);
        assert_eq!(report.still_quarantined.len(), 0);

        let shard = index.scope("agent-a").expect("restored");
        assert_eq!(shard.get("n1").expect("get").as_deref(), Some("keep me"));
        assert_eq!(index.quarantined().expect("marks").len(), 0);
    }

    #[test]
    fn broken_shard_is_quarantined_on_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join(shard_filename("corrupt")), b"not a sqlite file").expect("write");

        let index = ScopedContextIndex::open(&root).expect("open");
        let marks = index.quarantined().expect("marks");
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].scope, "corrupt");
        assert!(index.quarantine_dir().join(shard_filename("corrupt")).exists());
        // And the scope fails closed rather than pretending it is empty.
        assert!(matches!(
            index.scope("corrupt"),
            Err(ContextIndexError::ScopeQuarantined(_, _))
        ));
    }

    #[test]
    fn shards_survive_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        {
            let index = ScopedContextIndex::open(&root).expect("open");
            index.scope("s1").expect("scope").insert("k", "v").expect("insert");
        }
        let index = ScopedContextIndex::open(&root).expect("reopen");
        assert_eq!(index.scope_names().expect("names"), vec!["s1".to_string()]);
        let shard = index.scope("s1").expect("scope");
        assert_eq!(shard.get("k").expect("get").as_deref(), Some("v"));
    }

    #[test]
    fn invalid_scope_names_are_rejected() {
        let (_dir, index) = index();
        for bad in ["", "..", "a/b", "a.b", "a b"] {
            assert!(
                matches!(index.scope(bad), Err(ContextIndexError::InvalidScope(_))),
                "scope {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn search_ranks_more_term_matches_first() {
        let (_dir, index) = index();
        let shard = index.scope("s").expect("scope");
        shard.insert("weak", "only parser here").expect("insert");
        shard.insert("strong", "parser and tabs together").expect("insert");
        let hits = shard.search("parser tabs", 10).expect("search");
        assert_eq!(hits[0].id, "strong");
        assert_eq!(hits[0].score, 2);
        assert_eq!(hits[1].id, "weak");
    }
}
