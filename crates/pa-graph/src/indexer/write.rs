//! Writing one file's contribution: its nodes, FTS rows, CONTAINS/CALLS/USAGE
//! edges, coverage notes, and hash.

use serde_json::json;

use crate::coverage::{classify_source, ParseHealth, KIND_SKIPPED};
use crate::error::Result;
use crate::extract::{CallSite, ExtractedFile};
use crate::store::GraphStore;
use crate::tokenize::split_for_fts;
use crate::types::{CallSiteVerdict, CallStrategy, EdgeType, IndexReport, Label, Node};

use super::options::Depth;
use super::resolver::Resolver;
use super::walk::{module_of, FileWork};

impl GraphStore {
    /// # Errors
    /// Propagates SQLite errors.
    pub(crate) fn account_parse_health(
        &self,
        project: &str,
        work: &FileWork,
        lang: pa_ast::SupportLang,
        extracted: &ExtractedFile,
        report: &mut IndexReport,
    ) -> Result<()> {
        let rel = work.rel.as_str();
        let health = classify_source(&work.source, lang);
        if let Some(kind) = health.coverage_kind() {
            self.record_coverage(project, rel, kind, &health.detail())?;
            match health {
                ParseHealth::Partial { .. } => report.partial += 1,
                ParseHealth::Unusable { .. } => report.unusable += 1,
                ParseHealth::Unavailable => report.skipped += 1,
                ParseHealth::Clean => {}
            }
        }
        // Clean AND empty: the extension is indexable, the grammar is compiled
        // in, and nothing came out. Gated on `Clean` so a missing grammar (which
        // is `skipped`) is not counted twice under two reasons.
        if health == ParseHealth::Clean
            && extracted.symbols.is_empty()
            && extracted.calls.is_empty()
            && extracted.imports.is_empty()
        {
            self.record_coverage(
                project,
                &work.rel,
                crate::coverage::KIND_NO_SYMBOLS,
                "parsed but yielded no symbols, calls or imports",
            )?;
        }
        Ok(())
    }

    /// Delete symbols that disappeared from `rel`; their edges cascade, while
    /// survivors keep their ids so incoming edges stay attached.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub(crate) fn drop_stale_symbols(
        &self,
        project: &str,
        rel: &str,
        extracted: &ExtractedFile,
    ) -> Result<()> {
        let mut keep: Vec<String> = Vec::with_capacity(extracted.symbols.len() + 1);
        keep.push(format!("{project}::{rel}::__file__"));
        keep.extend(extracted.symbols.iter().map(|s| s.qualified_name.clone()));
        self.delete_file_nodes_not_in(project, rel, &keep)?;
        Ok(())
    }

    /// Write one call site's edge: a CALLS edge when it resolved, a USAGE edge
    /// carrying the reason when it did not.
    ///
    /// A miss is a fact, and discarding it is why the tool cannot answer "how many
    /// call sites in this file failed, and why". `Uses` was declared and never
    /// written until this; storing the miss is the `unresolved` class that
    /// codebase-memory-mcp publishes.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub(crate) fn record_call_edge(
        &self,
        resolver: &Resolver,
        project: &str,
        rel_path: &str,
        caller_qn: &str,
        call: &CallSite,
    ) -> Result<()> {
        if let Some((source, target, props)) = resolver.resolve_call(caller_qn, rel_path, call) {
            // A self-call resolves to the caller: the target is known, but the
            // verdict says WHY the edge is a self-loop.
            let verdict = if props.get("strategy").and_then(serde_json::Value::as_str)
                == Some(CallStrategy::Recursive.as_str())
            {
                CallSiteVerdict::Recursive
            } else {
                CallSiteVerdict::Resolved
            };
            self.record_call_site(project, rel_path, caller_qn, call, &verdict, &props)?;
            return self.insert_edge(project, source, target, EdgeType::Calls, &props);
        }
        // A path rooted at a workspace dependency names a definition that exists
        // outside the indexed tree. Recording it as a CALLS edge to the caller with
        // the dependency strategy is the difference between "we have no idea" and
        // "we know exactly where this goes, and it is not in this repo".
        if let (Some(crate_name), Some(source)) = (
            crate::misses::dependency_root(call, resolver.deps()),
            resolver.caller_id(caller_qn),
        ) {
            let props = json!({
                "callee": call.callee,
                "callee_path": call.callee_path,
                "crate": crate_name,
                "line": call.line,
                "confidence": CallStrategy::Dependency.base_confidence(),
                "strategy": CallStrategy::Dependency.as_str(),
            });
            self.record_call_site(
                project,
                rel_path,
                caller_qn,
                call,
                &CallSiteVerdict::Dependency,
                &props,
            )?;
            return self.insert_edge(project, source, source, EdgeType::Calls, &props);
        }
        let Some(source) = resolver.caller_id(caller_qn) else {
            // The caller itself is unknown: that is an indexing bug, not a miss.
            return Ok(());
        };
        let reason = resolver.miss_reason(call).to_string();
        let props = json!({
            "callee": call.callee,
            "line": call.line,
            "reason": reason,
            "confidence": 0.0,
        });
        self.record_call_site(
            project,
            rel_path,
            caller_qn,
            call,
            &CallSiteVerdict::Unresolved { reason },
            &props,
        )?;
        self.insert_edge(project, source, source, EdgeType::Uses, &props)
    }

    /// Write one file's whole contribution to the graph, as ONE unit of work.
    ///
    /// The caller runs this inside [`GraphStore::with_transaction`], so the
    /// clear of the previous revision and the write of the new one commit or
    /// roll back together. Nothing here may write a fact the caller would have
    /// to undo if the commit fails: the returned [`FileWrite`] carries what the
    /// pass must remember (node id, call sites, imports), and the caller records
    /// that only after the commit succeeds.
    ///
    /// `report` is the exception: a rolled-back attempt leaves its counts behind.
    /// That is deliberate — the report says what the pass DID, and a file whose
    /// rewrite failed did not contribute nodes.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub(crate) fn rewrite_file(
        &self,
        project: &str,
        work: &FileWork,
        depth: Depth,
        report: &mut IndexReport,
    ) -> Result<FileWrite> {
        let rel = work.rel.as_str();
        // Keep this file's nodes (stable ids) so CALLS edges from unchanged
        // files survive the re-parse; only its FTS rows and outgoing edges are
        // dropped. Stale symbols go once extraction has run (see
        // `delete_file_nodes_not_in` below).
        self.clear_file_for_reparse(project, rel)?;
        // Re-record this file's coverage from scratch so a file that now parses
        // cleanly stops being reported as `parse_partial`.
        self.clear_file_coverage(project, rel)?;
        let Some(lang) = pa_ast::SupportLang::from_path(&work.abs) else {
            self.record_coverage(project, rel, KIND_SKIPPED, "unsupported language")?;
            report.skipped += 1;
            return Ok(FileWrite::default());
        };
        if depth == Depth::Skeleton {
            report.nodes += 1;
            self.write_skeleton_file(project, work)?;
            return Ok(FileWrite::default());
        }
        let module = module_of(rel);
        let extracted = crate::extract::extract(&work.source, lang, &module);
        if !depth.resolves_edges() {
            let (nodes, file_id) = self.write_file(project, rel, &extracted)?;
            report.nodes += nodes;
            self.put_file_hash(project, rel, &work.hash)?;
            return Ok(FileWrite {
                file_id,
                ..FileWrite::default()
            });
        }

        self.account_parse_health(project, work, lang, &extracted, report)?;
        let (nodes, file_id) = self.write_file(project, rel, &extracted)?;
        self.drop_stale_symbols(project, rel, &extracted)?;
        let surface = crate::surface::surface_of(&work.source, lang, &module);
        self.put_file_surface(
            project,
            rel,
            &surface.surface_sha,
            &surface.body_sha,
            &surface.surface_json,
        )?;
        report.nodes += nodes;
        self.put_file_hash(project, rel, &work.hash)?;
        Ok(FileWrite {
            file_id,
            calls: extracted.calls,
            imports: extracted.imports,
        })
    }

    /// Write one file node without parsing: the skeleton depth's whole job.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    fn write_skeleton_file(&self, project: &str, work: &FileWork) -> Result<()> {
        let file_node = Node {
            id: 0,
            project: project.to_string(),
            label: Label::File,
            name: work.rel.clone(),
            qualified_name: format!("{project}::{}::__file__", work.rel),
            file_path: work.rel.clone(),
            start_line: 0,
            end_line: 0,
            props: json!({ "kind": "file", "size": work.source.len() }),
        };
        self.upsert_node(&file_node)?;
        self.put_file_hash(project, &work.rel, &work.hash)?;
        Ok(())
    }

    /// Write one file's File node, symbol nodes, FTS rows, and CONTAINS edges.
    ///
    /// Returns `(node_count, file_node_id)`.
    pub(crate) fn write_file(
        &self,
        project: &str,
        rel: &str,
        extracted: &ExtractedFile,
    ) -> Result<(usize, Option<i64>)> {
        let file_node = Node {
            id: 0,
            project: project.to_string(),
            label: Label::File,
            name: rel.to_string(),
            qualified_name: format!("{project}::{rel}::__file__"),
            file_path: rel.to_string(),
            start_line: 0,
            end_line: 0,
            props: json!({ "kind": "file" }),
        };
        // One statement for the whole file: the file node and every symbol it
        // defines. The per-symbol `upsert_node` was the write path's dominant
        // cost, 223 ms against 105 ms for the same rows batched (paired harness,
        // see `measurement::batched_write_against_per_symbol_write`). FTS rows
        // and CONTAINS edges stay per symbol: measured, batching them into one
        // `json_each` insert changed nothing, so they keep the simpler form.
        let file_qn = file_node.qualified_name.clone();
        let mut rows: Vec<Node> = Vec::with_capacity(extracted.symbols.len() + 1);
        rows.push(file_node);
        rows.extend(extracted.symbols.iter().map(|sym| Node {
            id: 0,
            project: project.to_string(),
            label: sym.label,
            name: sym.name.clone(),
            qualified_name: sym.qualified_name.clone(),
            file_path: rel.to_string(),
            start_line: sym.start_line,
            end_line: sym.end_line,
            props: json!({ "doc": sym.doc }),
        }));
        let ids = self.upsert_nodes(project, &rows)?;
        let file_id = ids[file_qn.as_str()];
        self.index_fts(file_id, rel, &file_qn, &split_for_fts(rel), "", "File")?;

        let mut nodes = 1;
        for sym in &extracted.symbols {
            let id = ids[sym.qualified_name.as_str()];
            let tokens = format!(
                "{} {}",
                split_for_fts(&sym.name),
                split_for_fts(&sym.qualified_name)
            );
            self.index_fts(
                id,
                &sym.name,
                &sym.qualified_name,
                &tokens,
                &sym.doc,
                sym.label.as_str(),
            )?;
            self.insert_edge(project, file_id, id, EdgeType::Contains, &json!({}))?;
            nodes += 1;
        }
        Ok((nodes, Some(file_id)))
    }
}

/// What one file's rewrite produced, handed back to the pass.
///
/// Collected inside the per-file transaction but recorded only after it commits,
/// so a rolled-back revision cannot leave a node id or a pending call site in
/// the pass's own state.
#[derive(Debug, Default)]
pub(crate) struct FileWrite {
    pub(crate) file_id: Option<i64>,
    pub(crate) calls: Vec<CallSite>,
    pub(crate) imports: Vec<crate::extract::ImportDef>,
}

#[cfg(test)]
mod batching_tests {
    //! What the batched node write must not change: one statement per file, the
    //! same ids a re-parse keeps, and the same FTS/CONTAINS targets.

    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::extract::{ExtractedFile, SymbolDef};
    use crate::store::GraphStore;
    use crate::types::{EdgeType, Label};

    static NODE_STATEMENTS: AtomicUsize = AtomicUsize::new(0);

    #[allow(clippy::needless_pass_by_value)] // `trace_v2` takes a plain `fn`
    fn count_node_writes(event: rusqlite::trace::TraceEvent<'_>) {
        if let rusqlite::trace::TraceEvent::Stmt(_, sql) = event {
            if sql.contains("INTO nodes(") {
                NODE_STATEMENTS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn symbol(name: &str) -> SymbolDef {
        SymbolDef {
            label: Label::Function,
            name: name.to_string(),
            qualified_name: format!("p::a.rs::{name}"),
            start_line: 1,
            end_line: 2,
            doc: format!("{name} doc"),
        }
    }

    fn extracted(docs: &[&str]) -> ExtractedFile {
        ExtractedFile {
            symbols: docs.iter().map(|name| symbol(name)).collect(),
            calls: Vec::new(),
            imports: Vec::new(),
        }
    }

    /// The whole file's nodes leave in ONE statement.
    ///
    /// The pass extracted N symbols and issued N+1 upserts; each one is a round
    /// trip to SQLite and a `serde_json` render of that node's props. The count
    /// is the observable the batching exists for — the rows themselves are
    /// unchanged, which the next two tests and
    /// `reindex_and_full_index_agree_on_the_same_tree` check.
    #[test]
    fn a_files_nodes_are_written_in_one_statement() {
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .register_project("p", "/tmp/batching")
            .expect("project");
        let file = extracted(&["one", "two", "three", "four", "five"]);

        NODE_STATEMENTS.store(0, Ordering::Relaxed);
        store.connection_mut().trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(count_node_writes),
        );
        let (written, file_id) = store.write_file("p", "a.rs", &file).expect("write");
        let statements = NODE_STATEMENTS.swap(0, Ordering::Relaxed);
        store
            .connection_mut()
            .trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT, None);

        assert_eq!(written, 6, "one file node plus five symbols");
        assert_eq!(
            statements, 1,
            "six nodes must leave in one statement, not one per node"
        );
        assert!(file_id.is_some());
    }

    /// A re-parse keeps every symbol's id and refreshes its row in place.
    ///
    /// Ids are what incoming CALLS edges from other files point at, so a rewrite
    /// that renumbered symbols would silently drop those edges.
    #[test]
    fn re_writing_a_file_keeps_every_node_id_and_refreshes_its_row() {
        let store = GraphStore::open_in_memory().expect("store");
        store
            .register_project("p", "/tmp/batching")
            .expect("project");
        let first = extracted(&["one", "two", "three"]);
        store.write_file("p", "a.rs", &first).expect("first write");
        let before = store.all_nodes("p").expect("nodes");

        let mut second = extracted(&["one", "two", "three"]);
        second.symbols[1].end_line = 99;
        second.symbols[1].doc = "rewritten".to_string();
        store
            .write_file("p", "a.rs", &second)
            .expect("second write");
        let after = store.all_nodes("p").expect("nodes");

        assert_eq!(before.len(), after.len(), "no duplicate rows");
        let ids = |nodes: &[crate::types::Node]| {
            nodes
                .iter()
                .map(|n| (n.qualified_name.clone(), n.id))
                .collect::<std::collections::HashMap<_, _>>()
        };
        assert_eq!(ids(&before), ids(&after), "ids must survive a rewrite");

        let two = store
            .node_by_qn("p", "p::a.rs::two")
            .expect("query")
            .expect("two");
        assert_eq!(two.end_line, 99);
        assert_eq!(two.prop_str("doc"), Some("rewritten"));
    }

    /// Each symbol's FTS row and CONTAINS edge hang off the id the batch
    /// returned for that symbol, not off a neighbour's.
    #[test]
    fn fts_rows_and_contains_edges_target_the_symbols_own_ids() {
        let store = GraphStore::open_in_memory().expect("store");
        store
            .register_project("p", "/tmp/batching")
            .expect("project");
        let file = extracted(&["alpha", "beta", "gamma"]);
        let (_, file_id) = store.write_file("p", "a.rs", &file).expect("write");
        let file_id = file_id.expect("file id");

        let edges = store
            .out_edges(file_id, Some(EdgeType::Contains))
            .expect("edges");
        assert_eq!(edges.len(), 3, "one CONTAINS edge per symbol");
        let indexed: Vec<(i64, String)> = store
            .all_nodes("p")
            .expect("nodes")
            .into_iter()
            .filter(|n| n.qualified_name != "p::a.rs::__file__")
            .map(|n| (n.id, n.qualified_name))
            .collect();
        let mut targets: Vec<i64> = edges.iter().map(|e| e.target).collect();
        targets.sort_unstable();
        let mut expected: Vec<i64> = indexed.iter().map(|(id, _)| *id).collect();
        expected.sort_unstable();
        assert_eq!(targets, expected, "every symbol is contained exactly once");

        for (id, qn) in indexed {
            let in_fts: i64 = store
                .connection()
                .query_row(
                    "SELECT count(*) FROM symbols_fts WHERE rowid = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .expect("fts count");
            assert_eq!(in_fts, 1, "{qn} has no FTS row at its own id {id}");
        }
    }
}

#[cfg(test)]
mod measurement {
    //! The before/after numbers in the commits that added the write transaction
    //! and the batched node upsert.
    //!
    //! Ignored by default: these index a real tree from disk, so they are a
    //! harness, not tests. Run with
    //! `PA_GRAPH_BENCH_ROOT=<tree> cargo test -p pa-graph --lib \
    //!  --release indexer::write::measurement -- --ignored --nocapture`.
    //!
    //! `batched_write_against_per_symbol_write` is the one to trust: both sides
    //! run in the same binary, over the same extracted symbols, inside the same
    //! transaction, so a shared and busy tree cannot move one side alone. On a
    //! frozen copy of `crates/pa-graph/src` (96 files, 2 154 symbols, release,
    //! 7 rounds) it measured 105-146 ms batched against 223-330 ms per symbol.

    use super::super::options::IndexOptions;
    use crate::store::GraphStore;
    use std::time::Instant;

    fn root() -> std::path::PathBuf {
        std::path::PathBuf::from(
            std::env::var("PA_GRAPH_BENCH_ROOT").unwrap_or_else(|_| "crates".to_string()),
        )
    }

    fn rounds() -> usize {
        std::env::var("PA_GRAPH_BENCH_ROUNDS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(3)
    }

    /// The write path as it was before batching, kept here as the comparison
    /// side of the harness — not as production code.
    ///
    /// Three statements per symbol: the node upsert, its FTS row, and the
    /// CONTAINS edge, exactly what `write_file` used to issue.
    fn write_one_symbol_at_a_time(
        store: &GraphStore,
        project: &str,
        rel: &str,
        extracted: &crate::extract::ExtractedFile,
    ) -> crate::error::Result<()> {
        use crate::tokenize::split_for_fts;
        use crate::types::{EdgeType, Label, Node};

        let file_node = Node {
            id: 0,
            project: project.to_string(),
            label: Label::File,
            name: rel.to_string(),
            qualified_name: format!("{project}::{rel}::__file__"),
            file_path: rel.to_string(),
            start_line: 0,
            end_line: 0,
            props: serde_json::json!({ "kind": "file" }),
        };
        let file_id = store.upsert_node(&file_node)?;
        store.index_fts(
            file_id,
            rel,
            &file_node.qualified_name,
            &split_for_fts(rel),
            "",
            "File",
        )?;
        for sym in &extracted.symbols {
            let node = Node {
                id: 0,
                project: project.to_string(),
                label: sym.label,
                name: sym.name.clone(),
                qualified_name: sym.qualified_name.clone(),
                file_path: rel.to_string(),
                start_line: sym.start_line,
                end_line: sym.end_line,
                props: serde_json::json!({ "doc": sym.doc }),
            };
            let id = store.upsert_node(&node)?;
            let tokens = format!(
                "{} {}",
                split_for_fts(&sym.name),
                split_for_fts(&sym.qualified_name)
            );
            store.index_fts(
                id,
                &sym.name,
                &sym.qualified_name,
                &tokens,
                &sym.doc,
                sym.label.as_str(),
            )?;
            store.insert_edge(
                project,
                file_id,
                id,
                EdgeType::Contains,
                &serde_json::json!({}),
            )?;
        }
        Ok(())
    }

    /// The batched `write_file` against the per-symbol loop it replaced, on the
    /// same extracted symbols inside the same transaction — the harness the
    /// commit quotes.
    ///
    /// Same binary, same input, only the write path differs, so a busy tree or
    /// a noisy machine cannot move one side without the other. The per-symbol
    /// side is spelled out here from the store's public API, which is what the
    /// shipped loop did: `upsert_node`, `index_fts`, `insert_edge`, three round
    /// trips per symbol.
    #[test]
    #[ignore = "measurement harness: paired write-path comparison, run with --ignored --nocapture"]
    fn batched_write_against_per_symbol_write() {
        use crate::extract::ExtractedFile;
        let root = root();
        let mut works: Vec<(String, ExtractedFile)> = Vec::new();
        for abs in super::super::walk::discover(&root, &IndexOptions::new("bench")) {
            let Some(lang) = pa_ast::SupportLang::from_path(&abs) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let rel = super::super::walk::rel_path(&root, &abs);
            let module = super::super::walk::module_of(&rel);
            works.push((rel, crate::extract::extract(&source, lang, &module)));
        }
        let symbols: usize = works.iter().map(|(_, e)| e.symbols.len()).sum();
        println!("{} files, {symbols} symbols", works.len());

        for round in 0..rounds() {
            let dir = tempfile::tempdir().expect("tempdir");
            let batched_store = GraphStore::open(dir.path().join("graph.db")).expect("store");
            batched_store
                .register_project("bench", "/tmp/bench")
                .expect("project");
            let t = Instant::now();
            batched_store
                .with_transaction(|_| {
                    for (rel, extracted) in &works {
                        let _ = batched_store.write_file("bench", rel, extracted)?;
                    }
                    Ok(())
                })
                .expect("batched");
            let batched = t.elapsed();

            let dir = tempfile::tempdir().expect("tempdir");
            let per_symbol = GraphStore::open(dir.path().join("graph.db")).expect("store");
            per_symbol
                .register_project("bench", "/tmp/bench")
                .expect("project");
            let t = Instant::now();
            per_symbol
                .with_transaction(|_| {
                    for (rel, extracted) in &works {
                        write_one_symbol_at_a_time(&per_symbol, "bench", rel, extracted)?;
                    }
                    Ok(())
                })
                .expect("per-symbol");
            let one_by_one = t.elapsed();

            println!(
                "round {round}: batched {batched:?} vs per-symbol {one_by_one:?} for {} rows",
                symbols + works.len()
            );
        }
    }

    /// One whole pass over `root`, timed, on a fresh store each round.
    #[test]
    #[ignore = "measurement harness: indexes a real tree, run with --ignored --nocapture"]
    fn a_full_pass_over_a_real_tree() {
        for round in 0..rounds() {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut store = GraphStore::open(dir.path().join("graph.db")).expect("store");
            let started = Instant::now();
            let report = store
                .index_project(
                    root(),
                    &IndexOptions::new("bench").with_respect_gitignore(true),
                )
                .expect("index");
            println!(
                "round {round}: {:?} for {} files ({} nodes, {} edges)",
                started.elapsed(),
                report.parsed,
                report.store_nodes,
                report.edges
            );
        }
    }
}
