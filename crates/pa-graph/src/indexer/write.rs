//! Writing one file's contribution: its nodes, FTS rows, CONTAINS/CALLS/USAGE
//! edges, coverage notes, and hash.

use std::collections::HashMap;

use serde_json::json;

use crate::coverage::{ParseHealth, classify_source};
use crate::error::Result;
use crate::extract::{CallSite, ExtractedFile};
use crate::store::GraphStore;
use crate::tokenize::split_for_fts;
use crate::types::{CallSiteVerdict, CallStrategy, EdgeType, IndexReport, Label, Node};

use super::resolver::Resolver;
use super::walk::FileWork;

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

    /// Write a file's definitions and CONTAINS edges, resolving nothing.
    ///
    /// This is the `symbols` depth: `search` and the architecture views work,
    /// `trace` and `structure` stay empty because no CALLS or IMPORTS edge
    /// exists yet.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub(crate) fn write_symbols_only(
        &self,
        project: &str,
        work: &FileWork,
        extracted: &ExtractedFile,
        report: &mut IndexReport,
        file_node_ids: &mut HashMap<String, i64>,
    ) -> Result<()> {
        let (nodes, file_id) = self.write_file(project, &work.rel, extracted)?;
        report.nodes += nodes;
        if let Some(id) = file_id {
            file_node_ids.insert(work.rel.clone(), id);
        }
        self.put_file_hash(project, &work.rel, &work.hash)?;
        Ok(())
    }

    /// Write one file node without parsing: the skeleton depth's whole job.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub(crate) fn write_skeleton_file(
        &self,
        project: &str,
        work: &FileWork,
        report: &mut IndexReport,
    ) -> Result<()> {
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
        report.nodes += 1;
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
