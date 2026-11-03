//! The byte-bounded source batch the pass reads one file group at a time.
//!
//! A batch is bounded by BYTES, not by file count: 3 000 tiny files and 30 large
//! ones need the same memory ceiling. The caller writes each batch before asking
//! for the next, so peak resident source text is one batch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::coverage::KIND_SKIPPED;
use crate::error::Result;
use crate::hash::FileHash;
use crate::store::GraphStore;
use crate::types::IndexReport;

use super::options::IndexOptions;
use super::walk::{discover, read_work, rel_path, skip_reason, FileWork};

/// Source text one batch may hold, in bytes.
///
/// The lightweight-graph review budgets 8–16 MiB resident at once. 8 MiB is the low end:
/// a batch competes with the extractor's trees and the SQLite cache in the same process.
pub const SOURCE_BATCH_BYTES: usize = 8 * 1024 * 1024;

/// The ceiling the batch loop reads, so a test can force a boundary through a small tree.
fn source_batch_bytes() -> usize {
    #[cfg(test)]
    return TEST_SOURCE_BATCH_BYTES
        .with(std::cell::Cell::get)
        .unwrap_or(SOURCE_BATCH_BYTES);
    #[cfg(not(test))]
    SOURCE_BATCH_BYTES
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_SOURCE_BATCH_BYTES: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// Batches produced while a test overrides the ceiling — the evidence that the loop ran
/// more than once, which is what makes the parity test a batching test.
#[cfg(test)]
pub(crate) static TEST_BATCHES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// The tree still to be read, and the cursor [`GraphStore::next_batch`] advances.
pub(crate) struct SourceBatches<'a> {
    root: PathBuf,
    opts: &'a IndexOptions,
    stored: &'a HashMap<String, FileHash>,
    files: Vec<PathBuf>,
    cursor: usize,
}

impl GraphStore {
    /// Discover, read, and hash every candidate file, recording a coverage
    /// note for each file that is skipped before parsing (oversized/unreadable)
    /// and returning the work items for the rest.
    /// Discover + hash, then hand back the files ONE BYTE-BOUNDED BATCH at a time.
    ///
    /// The read used to be a single `par_iter().map(read_work).collect()` over the whole
    /// tree: 2 113 files and **43.2 MiB** resident at once on this repo, three times the
    /// 8-16 MiB the lightweight-graph review budgets ("не собирать весь workspace в
    /// `Vec<FileWork>`").
    ///
    /// A batch is bounded by BYTES: 3 000 tiny files and 30 large ones need the same
    /// memory ceiling. A single file over the ceiling still gets its own batch — capping
    /// one file is `max_file_bytes`'s job.
    ///
    /// The caller writes each batch before asking for the next, so peak resident source
    /// text is one batch. Coverage notes for skipped files are recorded here, because a
    /// skip is a property of the pass.
    ///
    /// # Errors
    /// Propagates a walker error. The walk runs here, before the first batch, so a
    /// directory the walker could not read aborts the pass instead of looking like
    /// a tree with nothing left in it.
    pub(super) fn read_batches<'a>(
        root: &Path,
        opts: &'a IndexOptions,
        stored: &'a HashMap<String, FileHash>,
    ) -> Result<SourceBatches<'a>> {
        Ok(SourceBatches {
            root: root.to_path_buf(),
            opts,
            stored,
            files: discover(root, opts)?,
            cursor: 0,
        })
    }

    /// The next batch, or empty at the end of the tree.
    ///
    /// # Errors
    /// Propagates SQLite errors from the coverage notes.
    pub(super) fn next_batch(
        &self,
        batches: &mut SourceBatches<'_>,
        report: &mut IndexReport,
    ) -> Result<Vec<FileWork>> {
        let mut work: Vec<FileWork> = Vec::new();
        let mut bytes = 0usize;
        while batches.cursor < batches.files.len() {
            let abs = &batches.files[batches.cursor];
            batches.cursor += 1;
            let Some(w) = read_work(&batches.root, abs, batches.stored, batches.opts) else {
                self.record_coverage(
                    &batches.opts.project,
                    &rel_path(&batches.root, abs),
                    KIND_SKIPPED,
                    &skip_reason(abs, batches.opts),
                )?;
                report.skipped += 1;
                continue;
            };
            bytes = bytes.saturating_add(w.source.len());
            work.push(w);
            if bytes >= source_batch_bytes() {
                #[cfg(test)]
                TEST_BATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                break;
            }
        }
        Ok(work)
    }
}
