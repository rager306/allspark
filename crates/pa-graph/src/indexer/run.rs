//! The ordered indexing pass: lease, batches, removal sweep, edge resolution,
//! and the close-out receipt.
//!
//! One pass, in four numbered phases plus the close-out. Each phase is a method
//! on [`GraphStore`]; the order is load-bearing and stated at the code.

use std::collections::HashMap;
use std::path::Path;

use crate::coverage::KIND_SKIPPED;
use crate::error::{GraphError, Result};
use crate::extract::{CallSite, ImportDef};
use crate::lease::IndexLease;
use crate::store::GraphStore;
use crate::types::{EdgeType, FileChange, IndexReport};

use super::options::{Depth, IndexOptions};
use super::resolver::Resolver;
use super::walk::module_of;

impl GraphStore {
    /// Index `root` into this store, honouring [`IndexOptions`].
    ///
    /// # Errors
    /// Propagates SQLite, I/O, and walker errors. A single unparseable file is
    /// recorded as coverage and skipped, never fatal. Fails with
    /// [`std::io::ErrorKind::TimedOut`] when another process holds the writer
    /// lease for this database past `opts.lease_wait`, and
    /// [`std::io::ErrorKind::Interrupted`] when `opts.lease_cancel` fires —
    /// in both cases nothing is registered and nothing is written.
    #[allow(clippy::too_many_lines)] // one ordered pass; see the numbered comment
    pub fn index_project(
        &mut self,
        root: impl AsRef<Path>,
        opts: &IndexOptions,
    ) -> Result<IndexReport> {
        // The writer lease comes first: a second session must not register a
        // project (a write) or start walking before it owns the right to write.
        let _lease = self.acquire_lease(opts)?;
        let root = root
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| root.as_ref().to_path_buf());
        let root_str = root.to_string_lossy().to_string();
        self.register_project_at(&opts.project, &root_str)?;

        let mut report = IndexReport {
            project: opts.project.clone(),
            ..IndexReport::default()
        };

        // 1. Discover + hash, then read + write ONE BYTE-BOUNDED BATCH at a time.
        let stored = self.all_file_hashes(&opts.project)?;

        // 2. Write nodes/FTS per batch; call sites accumulate for resolution below.
        let mut seen: Vec<String> = Vec::new();
        let mut pending_calls: Vec<(String, String, CallSite)> = Vec::new();
        let mut pending_imports: Vec<(String, ImportDef)> = Vec::new();
        let mut file_node_ids: HashMap<String, i64> = HashMap::new();
        let mut batches = Self::read_batches(&root, opts, &stored);
        loop {
            let batch = self.next_batch(&mut batches, &mut report)?;
            if batch.is_empty() {
                break;
            }
            for w in &batch {
                seen.push(w.rel.clone());
                match w.change {
                    FileChange::Unchanged => {
                        report.unchanged += 1;
                        continue;
                    }
                    FileChange::Added => report.added += 1,
                    FileChange::Modified => report.modified += 1,
                }
                report.parsed += 1;

                // Keep this file's nodes (stable ids) so CALLS edges from
                // unchanged files survive the re-parse; only its FTS rows and
                // outgoing edges are dropped. Stale symbols go once extraction
                // has run (see `delete_file_nodes_not_in` below).
                self.clear_file_for_reparse(&opts.project, &w.rel)?;
                // Re-record this file's coverage from scratch so a file that now
                // parses cleanly stops being reported as `parse_partial`.
                self.clear_file_coverage(&opts.project, &w.rel)?;
                let Some(lang) = pa_ast::SupportLang::from_path(&w.abs) else {
                    self.record_coverage(
                        &opts.project,
                        &w.rel,
                        KIND_SKIPPED,
                        "unsupported language",
                    )?;
                    report.skipped += 1;
                    continue;
                };
                let module = module_of(&w.rel);
                if opts.depth == Depth::Skeleton {
                    self.write_skeleton_file(&opts.project, w, &mut report)?;
                    continue;
                }
                let extracted = crate::extract::extract(&w.source, lang, &module);
                if !opts.depth.resolves_edges() {
                    self.write_symbols_only(
                        &opts.project,
                        w,
                        &extracted,
                        &mut report,
                        &mut file_node_ids,
                    )?;
                    continue;
                }

                self.account_parse_health(&opts.project, w, lang, &extracted, &mut report)?;
                let (nodes, file_id) = self.write_file(&opts.project, &w.rel, &extracted)?;
                self.drop_stale_symbols(&opts.project, &w.rel, &extracted)?;
                let surface = crate::surface::surface_of(&w.source, lang, &module);
                self.put_file_surface(
                    &opts.project,
                    &w.rel,
                    &surface.surface_sha,
                    &surface.body_sha,
                    &surface.surface_json,
                )?;
                report.nodes += nodes;
                if let Some(id) = file_id {
                    file_node_ids.insert(w.rel.clone(), id);
                }
                for call in &extracted.calls {
                    pending_calls.push((w.rel.clone(), call.caller_qn.clone(), call.clone()));
                }
                for import in &extracted.imports {
                    pending_imports.push((w.rel.clone(), import.clone()));
                }
                self.put_file_hash(&opts.project, &w.rel, &w.hash)?;
            }
        }

        // 3. Removed files: stored but not seen in ANY batch. A property of the pass,
        // not of a batch — inside the loop it ran once per batch, which double-counted
        // every removal (measured: 4 reported for 2 files).
        for rel in stored.keys() {
            if !seen.contains(rel) {
                self.clear_file(&opts.project, rel)?;
                self.remove_file_hash(&opts.project, rel)?;
                self.remove_file_surface(&opts.project, rel)?;
                report.removed += 1;
            }
        }

        // 4. Resolve cross-file edges now that every definition is known.
        let symbols = self.symbol_index(&opts.project)?;
        // Read the manifests once per run. A workspace without manifests yields an
        // empty set, which simply means no call is attributed to a dependency.
        let deps = crate::deps::DependencyCrates::from_workspace(&root).unwrap_or_default();
        let mut resolver = Resolver::with_deps(symbols, deps);
        // Aliases must be known BEFORE any call resolves: a call written through an alias
        // names the local name, and the resolver looks definitions up by name. Keyed by the
        // FILE, which the indexer already holds — deriving a module path from the file name
        // was a guess (`a.rs` -> `a`) that the next language breaks.
        for (rel, import) in &pending_imports {
            resolver.note_imports(rel, std::slice::from_ref(import));
        }
        for (rel, caller_qn, call) in &pending_calls {
            self.record_call_edge(&resolver, &opts.project, rel, caller_qn, call)?;
        }
        for (rel, import) in &pending_imports {
            let Some(&file_id) = file_node_ids.get(rel) else {
                continue;
            };
            if let Some((target, props)) = resolver.resolve_import(import) {
                self.insert_edge(&opts.project, file_id, target, EdgeType::Imports, &props)?;
            }
        }

        // The store dedupes insert attempts (`INSERT OR IGNORE`), so counting
        // attempts overstates the graph; report the store's actual edge count.
        // `store_nodes` is set in the same breath so the two size figures always
        // describe the same instant: `nodes` above counts what THIS run wrote,
        // and printing it beside the store's edge total read as a graph that
        // shrank by 25% after a one-file edit (issue #2.1).
        report.edges = self.edge_count(&opts.project)?;
        report.store_nodes = self.node_count(&opts.project)?;
        self.finish_run(&opts.project, &root)?;

        Ok(report)
    }

    /// Take this database's writer lease, waiting for a live index run as
    /// `opts` allows.
    ///
    /// The returned guard is dropped by the caller when it returns, so every
    /// path out of an index run — including `?` on an error — releases the lock.
    ///
    /// # Errors
    /// The lease failure folded into [`GraphError::Io`], which keeps the timeout
    /// and cancellation error kinds distinct.
    pub(crate) fn acquire_lease(&self, opts: &IndexOptions) -> Result<IndexLease> {
        let Some(path) = self.path() else {
            // An in-memory store has no other writer to serialise against.
            return Ok(IndexLease::unshared());
        };
        IndexLease::acquire(path, opts.lease_wait, &opts.lease_cancel)
            .map_err(|err| GraphError::Io(err.into()))
    }

    /// Close out a completed index run.
    ///
    /// Three steps, in order, and the order is load-bearing:
    /// 1. `ANALYZE` — later queries pick their indexes from these statistics;
    ///    without them `trace` walks the whole edge table (472x slower).
    /// 2. `indexed_at` last — a run that fails or is interrupted must not look
    ///    complete. It used to be stamped before any parsing began.
    /// 3. A hash-linked receipt — the `witness` chain's first consumer. Two
    ///    runs over an unchanged tree append the same `input_hash`.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    fn finish_run(&self, project: &str, root: &Path) -> Result<()> {
        self.analyze()?;
        self.touch_project(project)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.record_index_receipt(project, now)?;
        // Record the commit this run was built at. A store behind HEAD is the
        // drift neither a stat sweep nor `git status` can see.
        if let Ok(head) = crate::changes::run_git(root, &["rev-parse", "HEAD"]) {
            let head = head.trim();
            if !head.is_empty() {
                self.set_indexed_head(project, head)?;
            }
        }
        Ok(())
    }
}
