//! Error type for `pa-graph`.

/// Errors returned by the code-graph store, its indexer, and its queries.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// SQLite returned an error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A filesystem operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// (De)serializing node/edge properties failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// The `ignore` directory walker failed.
    #[error("walk error: {0}")]
    Walk(#[from] ignore::Error),
    /// A source file could not be parsed into a usable tree.
    #[error("parse error: {0}")]
    Parse(String),
    /// The requested project is not registered.
    #[error("project not registered: {0}")]
    ProjectNotFound(String),
    /// The project name belongs to a DIFFERENT root than the one asked for.
    ///
    /// The store keys a project by name, and the tool derives that name from the
    /// root's last path component, so `/work/a/repo` and `/work/b/repo` collide.
    /// Serving the second from the first's index answers confidently about the
    /// wrong tree, so the mismatch is refused instead — naming both roots, because
    /// the caller has to know which one the store actually holds.
    #[error(
        "project {project} in this store is rooted at {stored}, not {requested}: \
         two roots with the same directory name share one project name; \
         index the second under its own graph.dbPath"
    )]
    ProjectRootMismatch {
        project: String,
        stored: String,
        requested: String,
    },
    /// No node matched the requested symbol.
    #[error("symbol not found: {0}")]
    SymbolNotFound(String),
    /// A payload-filter expression is malformed.
    #[error("invalid filter expression: {0}")]
    InvalidFilter(String),
    /// A search or trace option is invalid.
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    /// A git subprocess failed or produced unparseable output.
    #[error("git error: {0}")]
    Git(String),
    /// A source file is not in a language `pa-ast` can parse.
    #[error("unsupported language for: {0}")]
    UnsupportedLanguage(String),
    /// The store on disk was written by a different schema version.
    ///
    /// The row is deliberately left as found: overwriting it with this build's
    /// version is what turns a format mismatch into a healthy-looking store.
    #[error(
        "schema version mismatch at {store}: the store records {found:?}, this build reads \
         {expected}; reindex into a fresh store"
    )]
    SchemaVersionMismatch {
        /// Path of the store that carries the foreign version.
        store: String,
        /// The version as `store_meta` records it (not necessarily a number).
        found: String,
        /// The version this build's schema declares.
        expected: i64,
    },
}

/// Convenience alias for [`GraphError`].
pub type Result<T> = std::result::Result<T, GraphError>;
