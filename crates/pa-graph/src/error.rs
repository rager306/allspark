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
}

/// Convenience alias for [`GraphError`].
pub type Result<T> = std::result::Result<T, GraphError>;
