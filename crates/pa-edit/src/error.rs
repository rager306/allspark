//! Error type shared by every edit engine.
//!
//! The `Display` output of each variant is the exact model-facing message the
//! TypeScript implementation threw; models are trained on these strings, so
//! engines must reproduce them byte-for-byte.

use std::path::PathBuf;

/// Failure raised while parsing, locating, staging, or writing an edit.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// The wire payload could not be parsed. `line` is the 1-indexed payload
    /// line when the parser knows it.
    #[error("{message}")]
    Parse { message: String, line: Option<u32> },
    /// The payload parsed but could not be applied (missing file, overlap,
    /// invalid op, auto-generated target, no-op, …).
    #[error("{0}")]
    Apply(String),
    /// The target is not valid UTF-8 and the operation needs its text.
    /// Existence-only operations (delete, create-existence checks) bypass
    /// decoding and never produce this.
    #[error("{display} is not valid UTF-8 at byte {valid_up_to}; refusing to edit")]
    InvalidUtf8 { display: String, valid_up_to: usize },
    /// A search anchor could not be located in the target text.
    #[error("{0}")]
    Match(String),
    /// Plan mode rejected a working-tree write.
    #[error("{0}")]
    Plan(String),
    /// An internal URL target has no host answer yet. Raised before any
    /// write, so the host can resolve the URL, `provide` it, and retry.
    #[error("Internal URL not resolved yet: {0}")]
    UnresolvedUrl(String),
    /// Filesystem failure reading a target.
    #[error("{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The host writer callback failed; carries its message verbatim.
    #[error("{0}")]
    Writer(String),
    /// The host writer failed partway through a multi-file apply: the files
    /// listed in [`PartialWrite::landed`] are already written and stay that
    /// way, and the caller needs to know which ones before planning another
    /// edit against them.
    #[error("{0}")]
    WriterPartial(PartialWrite),
}

/// A writer failure that left earlier writes of the same apply on disk.
///
/// The writes are not rolled back. A rollback would issue compensating
/// writes through the same writer that just failed — and the usual causes
/// (a full disk, a lost permission) block the restore exactly as they
/// blocked the write — and it could overwrite a change another process made
/// after ours landed. Reporting what landed is the truthful option: the
/// caller re-reads those files instead of re-planning against stale bytes.
#[derive(Debug, Clone)]
pub struct PartialWrite {
    /// The writer's own error, verbatim, so a re-plan message survives.
    pub message: String,
    /// Display paths written before the failure, in apply order. A move
    /// reports its destination, which is where the bytes are now.
    pub landed: Vec<String>,
    /// The display path whose write failed.
    pub failed: String,
    /// Non-`Noop` files the apply intended to write.
    pub planned: usize,
}

impl std::fmt::Display for PartialWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let PartialWrite {
            message,
            landed,
            failed,
            planned,
        } = self;
        let names = landed.join(", ");
        write!(
            f,
            "{message}\n\nPartial apply: {} of {planned} file writes landed before {failed} \
             failed — {names}. They stay written and were not rolled back; re-read them before \
             planning another edit.",
            landed.len(),
        )
    }
}

impl EditError {
    /// Parse failure without a line number.
    pub fn parse(message: impl Into<String>) -> Self {
        Self::Parse {
            message: message.into(),
            line: None,
        }
    }

    /// Parse failure at a known payload line.
    pub fn parse_at(message: impl Into<String>, line: u32) -> Self {
        Self::Parse {
            message: message.into(),
            line: Some(line),
        }
    }

    /// Apply failure carrying a preformatted message.
    pub fn apply(message: impl Into<String>) -> Self {
        Self::Apply(message.into())
    }

    /// Match failure carrying a preformatted message.
    pub fn matched(message: impl Into<String>) -> Self {
        Self::Match(message.into())
    }

    /// Whether this rejects an undecodable target.
    pub const fn is_invalid_utf8(&self) -> bool {
        matches!(self, Self::InvalidUtf8 { .. })
    }

    /// Whether this is `File not found` (ENOENT) for the given path.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
    }
}

/// Convenience alias used throughout the crate.
pub type EditResult<T> = Result<T, EditError>;
