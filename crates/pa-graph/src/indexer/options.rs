//! The builder for one indexing pass.
//!
//! [`Depth`] says how much of the graph to build; [`IndexOptions`] carries that
//! plus the walk filters, the size cap, and the writer-lease policy.

use std::time::Duration;

use crate::lease::{CancelToken, DEFAULT_LEASE_WAIT};

/// How much of the graph to build.
///
/// A skeleton is cheap and answers structural questions immediately; the symbol
/// pass costs two orders of magnitude more. Measured on this repository (2,743
/// files): `Skeleton` 1,105 ms, `Symbols` about 135,000 ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Depth {
    /// One `File` node per tracked file: names, paths, sizes. Nothing is parsed.
    Skeleton,
    /// Skeleton plus every symbol definition. Calls and imports are not resolved,
    /// so `trace` and `structure` views are empty; `search` and `architecture`
    /// workspace files work.
    Symbols,
    /// Everything: symbols, call edges, import edges, coverage accounting.
    #[default]
    Full,
}

impl Depth {
    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skeleton => "skeleton",
            Self::Symbols => "symbols",
            Self::Full => "full",
        }
    }

    /// Parse a depth from its wire name; unknown names mean `Full`, so an old
    /// caller keeps the behaviour it had.
    #[must_use]
    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "skeleton" => Self::Skeleton,
            "symbols" => Self::Symbols,
            _ => Self::Full,
        }
    }

    /// Whether this pass resolves call and import edges.
    #[must_use]
    pub const fn resolves_edges(self) -> bool {
        matches!(self, Self::Full)
    }
}

pub struct IndexOptions {
    /// Project name to register/update.
    pub project: String,
    /// Extension allowlist (empty = every language `pa-ast` recognises).
    pub extensions: Vec<String>,
    /// Re-parse every file, ignoring stored hashes (full rebuild).
    pub force: bool,
    /// Maximum file size in bytes to index.
    pub max_file_bytes: u64,
    /// Honour the project's gitignore-family rules (`.gitignore`, `.git/info/exclude`,
    /// the global gitignore, and those of parent directories) via the `ignore` crate.
    ///
    /// `false` — the default — walks what is on disk. `true` is the lean view: it
    /// hides the ignored set. On a tree whose ignored files are build output or
    /// copies of the same sources, that set dwarfs the authored one and indexing it
    /// returns every symbol two or three times — measured on this repository, the
    /// walk alone is 2,058 files in ~300 ms lean against 37,131 in ~2.3 s wide, and
    /// the wide set is dominated by `target/` and `.tmp/` payload copies. So `true`
    /// is what an implicit build (session start, a cold query) wants, and `false` is
    /// what a caller who knows it has authored code under an ignore rule wants.
    ///
    /// Two things are independent of this flag: `.git` is skipped outright, and
    /// `.ignore` files (ripgrep's own format, not git's) stay honoured in both
    /// modes.
    pub respect_gitignore: bool,
    /// How much of the graph to build.
    pub depth: Depth,
    /// How long to wait for another process's index run to release the writer
    /// lease before failing. [`Duration::ZERO`] makes one attempt and gives up.
    pub lease_wait: Duration,
    /// Asked to stop waiting for the writer lease. A shutting-down session
    /// cancels its token so a queued index run does not sit out the deadline.
    pub lease_cancel: CancelToken,
}

impl IndexOptions {
    /// Default options for `project`.
    #[must_use]
    pub fn new(project: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            extensions: Vec::new(),
            force: false,
            max_file_bytes: 2 << 20,
            respect_gitignore: false,
            depth: Depth::Full,
            lease_wait: DEFAULT_LEASE_WAIT,
            lease_cancel: CancelToken::new(),
        }
    }

    /// Build only as much of the graph as `depth` asks for.
    #[must_use]
    pub fn with_depth(mut self, depth: Depth) -> Self {
        self.depth = depth;
        self
    }

    /// Force a full rebuild.
    #[must_use]
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Cap the per-file size indexed.
    #[must_use]
    pub fn with_max_file_bytes(mut self, bytes: u64) -> Self {
        self.max_file_bytes = bytes;
        self
    }

    /// Honour the gitignore-family rules, or ignore them.
    ///
    /// See [`IndexOptions::respect_gitignore`] for what the two modes trade.
    #[must_use]
    pub fn with_respect_gitignore(mut self, respect: bool) -> Self {
        self.respect_gitignore = respect;
        self
    }

    /// Restrict indexing to these extensions.
    #[must_use]
    pub fn with_extensions(mut self, extensions: Vec<String>) -> Self {
        self.extensions = extensions;
        self
    }

    /// Wait this long for another process's index run to release the writer
    /// lease. [`Duration::ZERO`] turns a contended lease into an immediate error.
    #[must_use]
    pub fn with_lease_wait(mut self, wait: Duration) -> Self {
        self.lease_wait = wait;
        self
    }

    /// Stop waiting for the writer lease when `cancel` fires.
    #[must_use]
    pub fn with_lease_cancel(mut self, cancel: CancelToken) -> Self {
        self.lease_cancel = cancel;
        self
    }
}
