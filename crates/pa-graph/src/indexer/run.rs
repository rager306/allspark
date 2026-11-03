//! The ordered indexing pass: lease, batches, removal sweep, edge resolution,
//! and the close-out receipt.
//!
//! One pass, in four numbered phases plus the close-out. Each phase is a method
//! on [`GraphStore`]; the order is load-bearing and stated at the code.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{GraphError, Result};
use crate::extract::{CallSite, ImportDef};
use crate::lease::IndexLease;
use crate::store::GraphStore;
use crate::types::{EdgeType, FileChange, IndexReport};

use super::options::IndexOptions;
use super::resolver::Resolver;

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
        let mut batches = Self::read_batches(&root, opts, &stored)?;
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

                // ONE file, ONE transaction. A re-parse first clears the file's
                // FTS rows and outgoing edges and only then writes the new
                // revision, so a failure in between used to leave the file with
                // no symbols at all — and the pass then aborted, so nothing ever
                // repaired it. The clear and the rewrite are one unit of work:
                // either the new revision lands whole or the previous one stays.
                //
                // Per file, not per batch: the batch exists to bound memory, and
                // a batch-wide transaction would hold ~8 MiB of source worth of
                // row locks for a group of files that are independent of each
                // other.
                let writes = self.with_transaction(|store| {
                    store.rewrite_file(&opts.project, w, opts.depth, &mut report)
                })?;
                if let Some(id) = writes.file_id {
                    file_node_ids.insert(w.rel.clone(), id);
                }
                // Resolution runs after every batch, so a file's call sites must
                // outlive its write. They are collected from the successful
                // commit, never from a revision that was rolled back.
                for call in writes.calls {
                    pending_calls.push((w.rel.clone(), call.caller_qn.clone(), call));
                }
                for import in writes.imports {
                    pending_imports.push((w.rel.clone(), import));
                }
            }
        }

        // 3. Removed files: stored but not seen in ANY batch. A property of the pass,
        // not of a batch — inside the loop it ran once per batch, which double-counted
        // every removal (measured: 4 reported for 2 files).
        for rel in stored.keys() {
            if !seen.contains(rel) {
                self.clear_file(&opts.project, rel)?;
                // `call_sites` keys on the file, and `clear_file` only walks the
                // node/edge/FTS tables — its rows would outlive the file and keep
                // answering impact questions about call sites that no longer exist.
                self.clear_call_sites(&opts.project, rel)?;
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
        // The resolved edges of the whole run are ONE unit of work: an edge is
        // only meaningful against the definition set it was resolved with, so a
        // pass that fails half way through must not leave a graph in which some
        // call sites were resolved against the new definitions and some against
        // the old ones. It is also where the per-statement commit cost lives —
        // one transaction instead of one commit per edge (measured on 96 files
        // of `crates/pa-graph/src`: 4.78 s -> 1.18 s for this phase).
        self.with_transaction(|store| {
            for (rel, caller_qn, call) in &pending_calls {
                store.record_call_edge(&resolver, &opts.project, rel, caller_qn, call)?;
            }
            for (rel, import) in &pending_imports {
                let Some(&file_id) = file_node_ids.get(rel) else {
                    continue;
                };
                if let Some((target, props)) = resolver.resolve_import(import) {
                    store.insert_edge(&opts.project, file_id, target, EdgeType::Imports, &props)?;
                }
            }
            Ok(())
        })?;

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

#[cfg(test)]
mod tests {
    use super::super::options::IndexOptions;
    use crate::store::GraphStore;
    use crate::types::EdgeType;

    fn write(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write");
    }

    /// `(functions, FTS rows, CONTAINS edges)` for the one file in the fixture.
    ///
    /// The three are what a reader observes about a file, and a transaction bug
    /// shows up as the three disagreeing: the node rows survive a clear (that is
    /// the design — stable ids keep incoming edges), so a count of `nodes` alone
    /// cannot see a stub file.
    fn file_state(store: &GraphStore) -> (i64, i64, i64) {
        let conn = store.connection();
        let functions = symbol_count(store);
        let fts: i64 = conn
            .query_row("SELECT count(*) FROM symbols_fts", [], |r| r.get(0))
            .expect("count");
        let contains: i64 = conn
            .query_row(
                "SELECT count(*) FROM edges WHERE project = 'p' AND edge_type = ?1",
                [EdgeType::Contains.as_str()],
                |r| r.get(0),
            )
            .expect("count");
        (functions, fts, contains)
    }

    /// `CALLS` edges in the project: the aggregate resolution writes.
    fn call_edges(store: &GraphStore) -> i64 {
        store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE project = 'p' AND edge_type = ?1",
                [EdgeType::Calls.as_str()],
                |r| r.get(0),
            )
            .expect("count")
    }

    fn symbol_count(store: &GraphStore) -> i64 {
        store
            .connection()
            .query_row(
                "SELECT count(*) FROM nodes WHERE project = 'p' AND label = 'Function'",
                [],
                |r| r.get(0),
            )
            .expect("count")
    }

    /// A file's re-parse is ONE transaction: the clear and the rewrite commit
    /// together, so a failure in the middle leaves the file's previous revision
    /// intact instead of a stub with no symbols.
    ///
    /// The failure is injected through the schema, not a test hook in the write
    /// path: a trigger rejects one insert of the NEW revision, i.e. after the
    /// clear has already deleted the old one and while the rewrite is half
    /// written. A test-only branch in the indexer would prove that the branch
    /// works, not that the clear and the rewrite are one unit.
    #[test]
    fn a_failed_rewrite_leaves_the_previous_revision_intact() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "lib.rs",
            "pub fn parse_config(path: &str) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 }\n",
        );
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first index");
        assert_eq!(symbol_count(&store), 2, "fixture must be indexed whole");

        // The new revision is LONGER than the old one: on the old writer, the
        // four symbols written before the failure would already differ from the
        // two that were there, so "the count still matches" cannot be luck.
        write(
            dir.path(),
            "lib.rs",
            "pub fn renamed_one() {}\npub fn renamed_two() {}\npub fn renamed_three() {}\npub fn renamed_four() {}\npub fn renamed_five() {}\n",
        );
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER refuse_the_last_symbol BEFORE INSERT ON nodes
                 WHEN NEW.name = 'renamed_five'
                 BEGIN SELECT RAISE(ABORT, 'injected: the rewrite fails mid-file'); END",
            )
            .expect("trigger");

        let failed = store.index_project(dir.path(), &IndexOptions::new("p"));
        assert!(
            failed.is_err(),
            "the injected failure must abort the pass, or the test proves nothing: {failed:?}"
        );

        assert_eq!(
            file_state(&store),
            (2, 3, 2),
            "the file is half written: its previous revision lost its FTS rows \
             and CONTAINS edges, and the new one only landed in part. \
             (functions, FTS rows, CONTAINS edges) must be the old revision whole"
        );
        assert!(
            store
                .node_by_qn("p", "lib::parse_config")
                .expect("qn")
                .is_some(),
            "the old symbol must still be there"
        );
        assert!(
            store
                .node_by_qn("p", "lib::renamed_one")
                .expect("qn")
                .is_none(),
            "a half-written revision leaked into the graph"
        );
    }

    /// A successful rewrite is unchanged by the transaction: the file ends up
    /// with exactly the new revision's symbols, its FTS rows, and its CONTAINS
    /// edges — no duplicates from the clear/rewrite pair, no leftovers.
    #[test]
    fn a_successful_rewrite_replaces_the_file_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "lib.rs",
            "pub fn old_one() {}\npub fn old_two() {}\n",
        );
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first index");
        assert_eq!(symbol_count(&store), 2);

        write(dir.path(), "lib.rs", "pub fn fresh() {}\n");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second index");

        assert_eq!(symbol_count(&store), 1, "old symbols must be gone");
        assert!(store.node_by_qn("p", "lib::fresh").expect("qn").is_some());
        assert!(store.node_by_qn("p", "lib::old_one").expect("qn").is_none());
        let contains: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE project = 'p' AND edge_type = ?1",
                [EdgeType::Contains.as_str()],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(contains, 1, "the File node's one CONTAINS edge");
        let fts: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM symbols_fts", [], |r| r.get(0))
            .expect("count");
        assert_eq!(fts, 2, "FTS rows must match the surviving nodes exactly");
    }

    /// The resolved edges of a run are ONE transaction, so a run that fails
    /// part way through resolution leaves none of them: a graph in which file A
    /// is resolved against the new definition set and file B against the old one
    /// is worse than a graph with no call edges at all.
    ///
    /// The failure is injected through the schema, at the FOURTH file's call
    /// site, so three files' edges are already written when it fires.
    #[test]
    fn a_failed_resolution_leaves_no_half_resolved_edges() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..4 {
            write(
                dir.path(),
                &format!("m{i}.rs"),
                &format!("pub fn f{i}() {{ target() }}\n"),
            );
        }
        write(
            dir.path(),
            "target.rs",
            "pub fn target() {}\npub fn other() {}\n",
        );
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first index");
        assert_eq!(call_edges(&store), 4, "one resolved call per file");

        // Every file changes its callee, so every file is re-parsed and its old
        // call edges are dropped before resolution runs again.
        for i in 0..4 {
            write(
                dir.path(),
                &format!("m{i}.rs"),
                &format!("pub fn f{i}() {{ other() }}\n"),
            );
        }
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER refuse_the_last_file BEFORE INSERT ON call_sites
                 WHEN NEW.caller_qn = 'm3::f3'
                 BEGIN SELECT RAISE(ABORT, 'injected: resolution fails mid-run'); END",
            )
            .expect("trigger");

        let failed = store.index_project(dir.path(), &IndexOptions::new("p"));
        assert!(
            failed.is_err(),
            "the injected failure must abort the pass: {failed:?}"
        );
        assert_eq!(
            call_edges(&store),
            0,
            "resolution committed a prefix: the files are resolved against \
             different definition sets"
        );
    }
}
