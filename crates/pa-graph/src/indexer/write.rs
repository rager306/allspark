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
        let file_id = self.upsert_node(&file_node)?;
        self.index_fts(
            file_id,
            rel,
            &file_node.qualified_name,
            &split_for_fts(rel),
            "",
            "File",
        )?;

        let mut nodes = 1;
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
                props: json!({ "doc": sym.doc }),
            };
            let id = self.upsert_node(&node)?;
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
mod measurement {
    //! The before/after numbers in the commit that added the write transaction.
    //!
    //! Ignored by default: it indexes a real tree from disk, so it is a harness,
    //! not a test. Run it with
    //! `PA_GRAPH_BENCH_ROOT=<tree> cargo test -p pa-graph --lib \
    //!  indexer::write::measurement -- --ignored --nocapture`.

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
