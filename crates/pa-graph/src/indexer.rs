//! Project indexing: walk, parse, extract, resolve calls, write.
//!
//! The indexer is incremental. Each file's SHA-256 is compared against the
//! stored `file_hashes` row; unchanged files are skipped entirely, modified
//! files have their nodes/edges and FTS rows replaced, and files gone from
//! disk are removed. Call edges are resolved with a confidence ladder
//! (import map → same module → unique name → suffix) so an ambiguous call
//! degrades gracefully instead of dropping.
//!
//! The pass was one 2 206-line file; it is now one module per responsibility:
//!
//! | module | holds |
//! |---|---|
//! | [`options`] | [`Depth`] and the [`IndexOptions`] builder |
//! | [`walk`] | discovery, the per-file read, `is_indexable`, path helpers |
//! | [`batch`] | the byte-bounded batch the pass reads one group at a time |
//! | [`resolver`] | the call-edge confidence ladder and its miss reasons |
//! | [`write`] | writing one file's nodes, edges, FTS rows, and coverage |
//! | [`run`] | the ordered pass: lease, batches, sweep, resolution, close-out |
//! | [`entry`] | the free-function entry points |
//! | [`tests`] | the tests, grouped by the contract each one pins |
//!
//! The public surface is re-exported here, so `crate::indexer::IndexOptions`,
//! `crate::indexer::Depth`, and `crate::indexer::Resolver` keep their paths and
//! `pa_graph::indexer::Depth` keeps working for `pa-core`.

mod batch;
mod entry;
mod options;
mod resolver;
mod run;
mod walk;
mod write;

#[cfg(test)]
mod tests;

pub use entry::{extract_symbols, index_project};
pub use options::{Depth, IndexOptions};
pub use walk::is_indexable;

// Crate-internal surface. `incremental.rs` reindexes through the same writer and
// resolver as the full pass, so these four names are shared rather than copied.
pub use batch::SOURCE_BATCH_BYTES;
pub(crate) use resolver::Resolver;
pub(crate) use walk::{discover, module_of, rel_path};
