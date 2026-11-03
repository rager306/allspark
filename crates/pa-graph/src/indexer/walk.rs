//! Which files a pass reads: the walk, the per-file read, and the skip reasons.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::error::Result;
use crate::hash::{hash_bytes, FileHash};
use crate::types::FileChange;

use super::options::IndexOptions;

/// The per-repository graph ignore file (gitignore syntax, nearest wins). Owned
/// by the graph, not git: it expresses "tracked but not code" and "ignored but
/// graph-visible", which `.gitignore` cannot (issue #11).
pub(crate) const GRAPHIGNORE_FILE: &str = ".graphignore";

/// One file's read+hash work item.
///
/// `pub(crate)` rather than private because the batch loop and the write path are
/// separate modules: the pass reads a work item in one and spends it in another.
pub(crate) struct FileWork {
    pub(crate) rel: String,
    pub(crate) abs: PathBuf,
    pub(crate) change: FileChange,
    pub(crate) hash: FileHash,
    pub(crate) source: String,
}

/// Read, hash, and classify one file; `None` when it is skipped.
pub(super) fn read_work(
    root: &Path,
    abs: &Path,
    stored: &HashMap<String, FileHash>,
    opts: &IndexOptions,
) -> Option<FileWork> {
    let rel = rel_path(root, abs);
    let meta = std::fs::metadata(abs).ok()?;
    if meta.len() > opts.max_file_bytes {
        return None;
    }
    let bytes = std::fs::read(abs).ok()?;
    let mut hash = hash_bytes(&bytes, &meta);
    // The row this work item will write says how much of the file reached the
    // graph, and that is the pass's depth — not the hasher's business.
    hash.depth = opts.depth;
    let change = if opts.force {
        FileChange::Modified
    } else {
        match stored.get(&rel) {
            None => FileChange::Added,
            // Same bytes, shallower revision: the stored row claims nothing about
            // the symbols below the depth it was written at, so a deeper pass must
            // read the file. Skipping this made `index_depth=skeleton` followed by
            // a full pass report success over a graph that had no symbols.
            Some(old) if old.sha256 == hash.sha256 && old.depth.satisfies(opts.depth) => {
                FileChange::Unchanged
            }
            Some(_) => FileChange::Modified,
        }
    };
    let source = String::from_utf8_lossy(&bytes).into_owned();
    Some(FileWork {
        rel,
        abs: abs.to_path_buf(),
        change,
        hash,
        source,
    })
}

/// Why a discovered file produced no [`FileWork`]: sized/read checks mirror
/// [`read_work`] so the recorded skip reason matches what actually happened.
pub(super) fn skip_reason(abs: &Path, opts: &IndexOptions) -> String {
    match std::fs::metadata(abs) {
        Ok(meta) if meta.len() > opts.max_file_bytes => format!("oversized: {} bytes", meta.len()),
        // A file that passed the size gate but produced no work was either
        // unreadable or vanished between the walk and the read.
        _ => "unreadable".to_string(),
    }
}

/// Whether `path` is a file the graph would parse.
///
/// The single owner of "is this indexable": the walk calls it, and a watcher
/// that only wants to know whether an event is worth a reindex calls it too, so
/// a new grammar reaches both without a second extension list to drift.
#[must_use]
pub fn is_indexable(path: &Path) -> bool {
    if path.components().any(|c| c.as_os_str() == ".git") {
        return false;
    }
    // The rule file is not a symbol source: it would otherwise surface in
    // `search` and `file_tree` like any other parsed file.
    if path
        .file_name()
        .is_some_and(|name| name == GRAPHIGNORE_FILE)
    {
        return false;
    }
    pa_ast::SupportLang::from_path(path).is_some()
}

/// Discover candidate source files under `root`.
///
/// Honours `.graphignore` (gitignore syntax, one per directory, nearest wins)
/// independently of `respect_gitignore`: the file is the repository's own
/// statement about what is *not code*, which gitignore cannot express for trees
/// the repository tracks (see issue #11). The two mechanisms are AND-ed — a file
/// is a candidate only if neither excludes it — and a `!path` re-include in
/// `.graphignore` wins over gitignore, because custom ignores take precedence
/// in the `ignore` crate.
///
/// # Errors
/// Propagates a walker error. An unreadable directory is not an empty one: the
/// entries behind it are simply unknown, and the pass removing every stored file
/// it did not see would turn "we could not look" into a deleted index.
pub(crate) fn discover(root: &Path, opts: &IndexOptions) -> Result<Vec<PathBuf>> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .parents(opts.respect_gitignore)
        .add_custom_ignore_filename(GRAPHIGNORE_FILE);
    let mut out = Vec::new();
    for entry in builder.build() {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if !is_indexable(path) {
            continue;
        }
        if !opts.extensions.is_empty() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !opts.extensions.iter().any(|want| want == ext) {
                continue;
            }
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    Ok(out)
}

pub(crate) fn rel_path(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .unwrap_or(abs)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Module prefix for a file: the path without extension, `::`-joined.
pub(crate) fn module_of(rel: &str) -> String {
    let without_ext = rel.rsplit_once('.').map_or(rel, |(stem, _)| stem);
    without_ext.replace('/', "::")
}
