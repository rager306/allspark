//! Project indexing: walk, parse, extract, resolve calls, write.
//!
//! The indexer is incremental. Each file's SHA-256 is compared against the
//! stored `file_hashes` row; unchanged files are skipped entirely, modified
//! files have their nodes/edges and FTS rows replaced, and files gone from
//! disk are removed. Call edges are resolved with a confidence ladder
//! (import map → same module → unique name → suffix) so an ambiguous call
//! degrades gracefully instead of dropping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ignore::WalkBuilder;
use rayon::prelude::*;
use serde_json::json;

use crate::coverage::{classify_source, ParseHealth, KIND_SKIPPED};
use crate::error::{GraphError, Result};
use crate::extract::{CallSite, ExtractedFile, ImportDef};
use crate::hash::{hash_bytes, FileHash};
use crate::lease::{CancelToken, IndexLease, DEFAULT_LEASE_WAIT};
use crate::misses::MissReason;

use crate::store::GraphStore;
use crate::tokenize::split_for_fts;
use crate::types::{CallStrategy, EdgeType, FileChange, IndexReport, Label, Node};

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

/// One file's read+hash work item.
struct FileWork {
    rel: String,
    abs: PathBuf,
    change: FileChange,
    hash: FileHash,
    source: String,
}

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

        // 1. Discover + read + hash (parallel).
        let stored = self.all_file_hashes(&opts.project)?;
        let work = self.prepare_work(&root, opts, &stored, &mut report)?;

        // 2. Write nodes/FTS for changed files; collect call sites to resolve.
        let mut seen: Vec<String> = Vec::with_capacity(work.len());
        let mut pending_calls: Vec<(String, CallSite)> = Vec::new();
        let mut pending_imports: Vec<(String, ImportDef)> = Vec::new();
        let mut file_node_ids: HashMap<String, i64> = HashMap::new();

        for w in &work {
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
                self.record_coverage(&opts.project, &w.rel, KIND_SKIPPED, "unsupported language")?;
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

            self.account_parse_health(&opts.project, &w.rel, &w.source, lang, &mut report)?;
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
                pending_calls.push((call.caller_qn.clone(), call.clone()));
            }
            for import in &extracted.imports {
                pending_imports.push((w.rel.clone(), import.clone()));
            }
            self.put_file_hash(&opts.project, &w.rel, &w.hash)?;
        }

        // 3. Removed files: stored but not seen this run.
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
        let resolver = Resolver::with_deps(symbols, deps);
        for (caller_qn, call) in &pending_calls {
            self.record_call_edge(&resolver, &opts.project, caller_qn, call)?;
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

    /// Record a file's parse honesty and fold it into the run report.
    ///
    /// A tree with ERROR regions still yields symbols, but the reader must know
    /// it was degraded (G9).
    ///
    /// # Errors
    /// Propagates SQLite errors.
    fn account_parse_health(
        &self,
        project: &str,
        rel: &str,
        source: &str,
        lang: pa_ast::SupportLang,
        report: &mut IndexReport,
    ) -> Result<()> {
        let health = classify_source(source, lang);
        if let Some(kind) = health.coverage_kind() {
            self.record_coverage(project, rel, kind, &health.detail())?;
            match health {
                ParseHealth::Partial { .. } => report.partial += 1,
                ParseHealth::Unusable { .. } => report.unusable += 1,
                ParseHealth::Unavailable => report.skipped += 1,
                ParseHealth::Clean => {}
            }
        }
        Ok(())
    }

    /// Delete symbols that disappeared from `rel`; their edges cascade, while
    /// survivors keep their ids so incoming edges stay attached.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    fn drop_stale_symbols(
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
    fn record_call_edge(
        &self,
        resolver: &Resolver,
        project: &str,
        caller_qn: &str,
        call: &CallSite,
    ) -> Result<()> {
        if let Some((source, target, props)) = resolver.resolve_call(caller_qn, call) {
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
            return self.insert_edge(
                project,
                source,
                source,
                EdgeType::Calls,
                &json!({
                    "callee": call.callee,
                    "callee_path": call.callee_path,
                    "crate": crate_name,
                    "line": call.line,
                    "confidence": CallStrategy::Dependency.base_confidence(),
                    "strategy": CallStrategy::Dependency.as_str(),
                }),
            );
        }
        let Some(source) = resolver.caller_id(caller_qn) else {
            // The caller itself is unknown: that is an indexing bug, not a miss.
            return Ok(());
        };
        self.insert_edge(
            project,
            source,
            source,
            EdgeType::Uses,
            &json!({
                "callee": call.callee,
                "line": call.line,
                "reason": resolver.miss_reason(call),
                "confidence": 0.0,
            }),
        )
    }

    /// Write a file's definitions and CONTAINS edges, resolving nothing.
    ///
    /// This is the `symbols` depth: `search` and the architecture views work,
    /// `trace` and `structure` stay empty because no CALLS or IMPORTS edge
    /// exists yet.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    fn write_symbols_only(
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
    fn write_skeleton_file(
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

    /// Discover, read, and hash every candidate file, recording a coverage
    /// note for each file that is skipped before parsing (oversized/unreadable)
    /// and returning the work items for the rest.
    fn prepare_work(
        &self,
        root: &Path,
        opts: &IndexOptions,
        stored: &HashMap<String, FileHash>,
        report: &mut IndexReport,
    ) -> Result<Vec<FileWork>> {
        let files = discover(root, opts);
        let outcomes: Vec<Option<FileWork>> = files
            .par_iter()
            .map(|abs| read_work(root, abs, stored, opts))
            .collect();
        let mut work: Vec<FileWork> = Vec::with_capacity(outcomes.len());
        for (abs, outcome) in files.iter().zip(outcomes) {
            if let Some(w) = outcome {
                work.push(w);
            } else {
                self.record_coverage(
                    &opts.project,
                    &rel_path(root, abs),
                    KIND_SKIPPED,
                    &skip_reason(abs, opts),
                )?;
                report.skipped += 1;
            }
        }
        Ok(work)
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

    /// Snapshot of `qualified_name -> node id` for the project.
    pub(crate) fn symbol_index(&self, project: &str) -> Result<HashMap<String, i64>> {
        let conn = self.connection();
        let mut stmt = conn.prepare("SELECT qualified_name, id FROM nodes WHERE project = ?1")?;
        let rows = stmt.query_map([project], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (qn, id) = row?;
            out.insert(qn, id);
        }
        Ok(out)
    }
}

/// Read, hash, and classify one file; `None` when it is skipped.
fn read_work(
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
    let hash = hash_bytes(&bytes, &meta);
    let change = if opts.force {
        FileChange::Modified
    } else {
        match stored.get(&rel) {
            None => FileChange::Added,
            Some(old) if old.sha256 == hash.sha256 => FileChange::Unchanged,
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
fn skip_reason(abs: &Path, opts: &IndexOptions) -> String {
    match std::fs::metadata(abs) {
        Ok(meta) if meta.len() > opts.max_file_bytes => format!("oversized: {} bytes", meta.len()),
        // A file that passed the size gate but produced no work was either
        // unreadable or vanished between the walk and the read.
        _ => "unreadable".to_string(),
    }
}

/// Discover candidate source files under `root`.
pub(crate) fn discover(root: &Path, opts: &IndexOptions) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .parents(opts.respect_gitignore);
    let mut out = Vec::new();
    for entry in builder.build().flatten() {
        let path = entry.path();
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if path.components().any(|c| c.as_os_str() == ".git") {
            continue;
        }
        if pa_ast::SupportLang::from_path(path).is_none() {
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
    out
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

/// Call-edge resolver implementing the confidence ladder.
pub(crate) struct Resolver {
    by_qn: HashMap<String, i64>,
    by_name: HashMap<String, Vec<String>>,
    /// External crates the workspace depends on, so a miss can say "this is a
    /// dependency call" instead of guessing at the leaf name. Built once per run.
    deps: crate::deps::DependencyCrates,
}

impl Resolver {
    /// A resolver that knows no dependency crates: every miss falls back to the
    /// std/trait corpora. Used where a root is not available (tests, incremental
    /// paths that did not read manifests).
    pub(crate) fn new(by_qn: HashMap<String, i64>) -> Self {
        Self::with_deps(by_qn, crate::deps::DependencyCrates::default())
    }

    /// A resolver that can attribute a path-rooted call to a workspace dependency.
    pub(crate) fn with_deps(
        by_qn: HashMap<String, i64>,
        deps: crate::deps::DependencyCrates,
    ) -> Self {
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for qn in by_qn.keys() {
            let name = qn.rsplit("::").next().unwrap_or(qn).to_string();
            by_name.entry(name).or_default().push(qn.clone());
        }
        for v in by_name.values_mut() {
            v.sort();
        }
        Self {
            by_qn,
            by_name,
            deps,
        }
    }

    /// The external crates this resolver knows about.
    pub(crate) fn deps(&self) -> &crate::deps::DependencyCrates {
        &self.deps
    }

    /// The id of `caller_qn`, when it is a known definition.
    ///
    /// A call site whose caller is unknown cannot be attributed at all, so a miss
    /// there is not worth recording: the caller itself is the problem.
    pub(crate) fn caller_id(&self, caller_qn: &str) -> Option<i64> {
        self.by_qn.get(caller_qn).copied()
    }

    /// Why a call site did not resolve: see [`crate::misses`] for the split and
    /// the heuristic behind it.
    ///
    /// The order matters. A name the tree knows but cannot resolve is a shape
    /// problem, whatever it is called; only then does the external/std corpus
    /// apply, and only for a miss — a resolved name never reaches here, so the
    /// corpus cannot relabel a real CALLS edge.
    pub(crate) fn miss_reason(&self, call: &CallSite) -> &'static str {
        if self.by_name.contains_key(&call.callee) {
            // The name exists; the resolution failed for a structural reason we do
            // not model yet (a path prefix, a trait method, an operator).
            return MissReason::ShapeNotModelled.as_str();
        }
        if crate::misses::is_external_or_std(call)
            || crate::misses::dependency_root(call, &self.deps).is_some()
        {
            return MissReason::ExternalOrStd.as_str();
        }
        MissReason::classify_unmatched(&call.callee).as_str()
    }

    /// Resolve one call site to `(source_id, target_id, props)`.
    pub(crate) fn resolve_call(
        &self,
        caller_qn: &str,
        call: &CallSite,
    ) -> Option<(i64, i64, serde_json::Value)> {
        let source = *self.by_qn.get(caller_qn)?;
        let candidates = self.by_name.get(&call.callee)?;
        let (target_qn, strategy) = if candidates.len() == 1 {
            (candidates[0].clone(), CallStrategy::UniqueName)
        } else {
            let caller_module = caller_qn.rsplit_once("::").map_or("", |(m, _)| m);
            match candidates
                .iter()
                .find(|qn| qn.rsplit_once("::").map_or("", |(m, _)| m) == caller_module)
            {
                Some(qn) => (qn.clone(), CallStrategy::SameModule),
                None => (candidates[0].clone(), CallStrategy::Suffix),
            }
        };
        let target = *self.by_qn.get(&target_qn)?;
        if target == source {
            return None;
        }
        let confidence = CallStrategy::base_confidence(strategy);
        let props = json!({
            "callee": call.callee,
            "confidence": confidence,
            "strategy": strategy.as_str(),
            "candidates": candidates.len(),
            "args": call.args.iter().map(|a| json!({"i": a.i, "e": a.e})).collect::<Vec<_>>(),
            "line": call.line,
        });
        Some((source, target, props))
    }

    /// Resolve an import binding to `(target_id, props)`, if the imported
    /// symbol is defined in the project.
    pub(crate) fn resolve_import(&self, import: &ImportDef) -> Option<(i64, serde_json::Value)> {
        let candidates = self.by_name.get(&import.target_name)?;
        let target_qn = candidates.first()?;
        let target = *self.by_qn.get(target_qn)?;
        Some((
            target,
            json!({
                "local_name": import.local_name,
                "module": import.module,
                "confidence": CallStrategy::ImportMap.base_confidence(),
                "strategy": CallStrategy::ImportMap.as_str(),
            }),
        ))
    }
}

/// Extract the symbol definitions `pa-ast` yields for one source string.
///
/// Thin re-export so integration tests and the daemon can preview extraction
/// without opening a store.
#[must_use]
pub fn extract_symbols(
    source: &str,
    lang: pa_ast::SupportLang,
    module: &str,
) -> Vec<crate::extract::SymbolDef> {
    crate::extract::extract(source, lang, module).symbols
}

/// Convenience: index a project with default options.
///
/// # Errors
/// Propagates [`GraphStore::index_project`] errors.
pub fn index_project(
    store: &mut GraphStore,
    root: impl AsRef<Path>,
    project: &str,
) -> Result<IndexReport> {
    store.index_project(root, &IndexOptions::new(project))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tree(dir: &Path) {
        std::fs::write(
            dir.join("lib.rs"),
            "pub fn parse_config(path: &str) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 }\n",
        )
        .expect("lib.rs");
    }

    /// Write a tree with one visible source and two under ignore rules.
    ///
    /// `.git` must exist as a directory: the `ignore` crate applies `.gitignore`
    /// only inside a repository (`require_git` is on by default), and its own test
    /// suite pins that (`gitignore_no_git`). The indexer skips `.git` in either
    /// mode, so an empty one is enough.
    fn write_ignored_tree(dir: &Path) {
        std::fs::create_dir(dir.join(".git")).expect(".git");
        std::fs::write(dir.join(".gitignore"), "scratch/\ngen/\n").expect(".gitignore");
        std::fs::write(dir.join("a.rs"), "pub fn tracked() {}\n").expect("a.rs");
        std::fs::create_dir(dir.join("scratch")).expect("scratch");
        std::fs::write(dir.join("scratch/b.rs"), "pub fn scratch() {}\n").expect("b.rs");
        std::fs::create_dir(dir.join("gen")).expect("gen");
        std::fs::write(dir.join("gen/c.rs"), "pub fn generated() {}\n").expect("c.rs");
    }

    fn indexed_files(store: &GraphStore, project: &str) -> Vec<String> {
        let mut files: Vec<String> = store
            .all_file_hashes(project)
            .expect("hashes")
            .into_keys()
            .collect();
        files.sort();
        files
    }

    /// The ignore mode decides the discovered set, and both modes are worth
    /// reaching: `false` (the default) indexes what is on disk, `true` hides the
    /// ignored files. A lean run over a wide store drops what it can no longer
    /// see and reports it as `removed` rather than leaving orphans behind.
    #[test]
    fn the_ignore_mode_decides_the_discovered_file_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_ignored_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");

        let wide = store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("wide index");
        assert_eq!(
            wide.added, 3,
            "the default mode walks what is on disk: {wide:?}"
        );
        assert_eq!(
            indexed_files(&store, "p"),
            vec!["a.rs", "gen/c.rs", "scratch/b.rs"],
        );

        let lean = store
            .index_project(
                dir.path(),
                &IndexOptions::new("p").with_respect_gitignore(true),
            )
            .expect("lean index");
        assert_eq!(lean.parsed, 0, "a.rs itself did not change: {lean:?}");
        assert_eq!(
            lean.removed, 2,
            "the ignored files leave the store: {lean:?}"
        );
        assert_eq!(indexed_files(&store, "p"), vec!["a.rs"]);
        // A lean pass must not leave the dropped files' symbols searchable.
        let hits = crate::search::search(
            store.connection(),
            "generated",
            &crate::search::SearchOptions::new("p"),
        )
        .expect("search");
        assert!(
            hits.is_empty(),
            "pruned symbols stayed searchable: {hits:?}"
        );
    }

    /// The extension allowlist is orthogonal to the ignore mode: a file the
    /// options exclude stays out in both, so flipping the mode cannot smuggle
    /// something past the filter.
    /// `.ignore` is ripgrep's own ignore format, not git's, and the mode only
    /// speaks about git's rules. It stays honoured in both: a caller flipping
    /// `respect_gitignore` must not also start indexing a directory the project
    /// marked with `.ignore`.
    #[test]
    fn the_ripgrep_ignore_file_is_honoured_in_both_modes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".ignore"), "vendor/\n").expect(".ignore");
        std::fs::write(dir.path().join("a.rs"), "pub fn tracked() {}\n").expect("a.rs");
        std::fs::create_dir(dir.path().join("vendor")).expect("vendor");
        std::fs::write(dir.path().join("vendor/v.rs"), "pub fn vendored() {}\n").expect("v.rs");
        for respect in [false, true] {
            let mut store = GraphStore::open_in_memory().expect("store");
            store
                .index_project(
                    dir.path(),
                    &IndexOptions::new("p").with_respect_gitignore(respect),
                )
                .expect("index");
            assert_eq!(
                indexed_files(&store, "p"),
                vec!["a.rs"],
                "respect_gitignore={respect} dropped the `.ignore` rule",
            );
        }
    }

    #[test]
    fn the_extension_filter_applies_in_both_ignore_modes() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_ignored_tree(dir.path());
        std::fs::write(dir.path().join("visible.py"), "def tracked():\n    pass\n").expect("py");
        std::fs::write(dir.path().join("gen/c.py"), "def generated():\n    pass\n").expect("py");
        let rust_only = || IndexOptions::new("p").with_extensions(vec!["rs".to_string()]);
        for respect in [false, true] {
            let mut store = GraphStore::open_in_memory().expect("store");
            store
                .index_project(dir.path(), &rust_only().with_respect_gitignore(respect))
                .expect("index");
            let files = indexed_files(&store, "p");
            assert!(
                !files
                    .iter()
                    .any(|file| std::path::Path::new(file).extension() == Some("py".as_ref())),
                "respect_gitignore={respect} let a filtered extension through: {files:?}"
            );
        }
    }

    #[test]
    fn indexes_and_resolves_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        let report = store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        assert_eq!(report.added, 1);
        assert_eq!(report.parsed, 1);
        assert!(report.nodes >= 3, "expected file + 2 functions: {report:?}");
        assert!(
            report.edges >= 1,
            "expected at least one CALLS edge: {report:?}"
        );
        let hits = crate::search::search(
            store.connection(),
            "load",
            &crate::search::SearchOptions::new("p"),
        )
        .expect("search");
        assert!(hits.iter().any(|h| h.name == "load"), "{hits:?}");
    }

    /// A `use` that binds a symbol to a file must produce an IMPORTS edge. The
    /// live store on 2026-10-22 had 53,863 CALLS and **zero** IMPORTS, so every
    /// structure/layers/cycles view saw call edges only — the resolver was
    /// silently returning `None` for every binding in the repo.
    #[test]
    fn a_cross_file_use_emits_an_imports_edge() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("a.rs"),
            "pub struct Widget;\npub fn make() -> Widget { Widget }\n",
        )
        .expect("a.rs");
        std::fs::write(
            dir.path().join("b.rs"),
            "use crate::a::{Widget, make};\npub fn use_it() -> Widget { make() }\n",
        )
        .expect("b.rs");
        std::fs::write(dir.path().join("lib.rs"), "pub mod a;\npub mod b;\n").expect("lib.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let imports: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE edge_type = 'IMPORTS'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert!(
            imports >= 2,
            "a use binding of Widget and make must produce at least two IMPORTS edges, got {imports}"
        );
    }

    /// Indexing must leave planner statistics behind. Without them the planner
    /// picks `idx_edges_type` for a name-keyed trace and walks every edge in the
    /// store: measured 52.30 ms -> 0.08 ms per trace on a 36k-node store (472x).
    #[test]
    fn indexing_leaves_planner_statistics() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "pub fn caller() { callee() }\n").expect("a.rs");
        std::fs::write(dir.path().join("b.rs"), "pub fn callee() {}\n").expect("b.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let stats: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE 'sqlite_stat%'",
                [],
                |r| r.get(0),
            )
            .expect("stat tables");
        assert!(stats > 0, "indexing must leave planner statistics behind");
    }

    /// Two runs over an unchanged tree must produce the same input digest, and
    /// the chain must verify. This is the `witness` module's first consumer.
    #[test]
    fn index_runs_record_a_verifiable_receipt_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        let first: String = store
            .connection()
            .query_row(
                "SELECT input_hash FROM index_receipts WHERE project = 'p'",
                [],
                |r| r.get(0),
            )
            .expect("first receipt");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second");
        let hashes: Vec<String> = store
            .connection()
            .prepare("SELECT input_hash FROM index_receipts WHERE project = 'p' ORDER BY id")
            .expect("stmt")
            .query_map([], |r| r.get::<_, String>(0))
            .expect("rows")
            .map(|r| r.expect("row"))
            .collect();
        assert_eq!(hashes.len(), 2, "one receipt per completed run");
        assert_eq!(
            hashes[0], hashes[1],
            "an unchanged tree must digest the same on both runs"
        );
        assert_eq!(hashes[0], first);
        assert!(
            store.verify_receipt_chain("p").expect("verify"),
            "the chain must verify"
        );
    }

    /// Editing a file changes the run digest: receipts are not just counters.
    #[test]
    fn a_content_change_changes_the_run_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        std::fs::write(dir.path().join("lib.rs"), "pub fn other() -> u32 { 2 }\n").expect("edit");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second");
        let hashes: Vec<String> = store
            .connection()
            .prepare("SELECT input_hash FROM index_receipts WHERE project = 'p' ORDER BY id")
            .expect("stmt")
            .query_map([], |r| r.get::<_, String>(0))
            .expect("rows")
            .map(|r| r.expect("row"))
            .collect();
        assert_ne!(
            hashes[0], hashes[1],
            "an edited tree must digest differently"
        );
    }

    /// A rewritten receipt must be detected.
    #[test]
    fn a_tampered_receipt_breaks_verification() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        store
            .connection()
            .execute(
                "UPDATE index_receipts SET input_hash = 'forged' WHERE project = 'p'",
                [],
            )
            .expect("tamper");
        assert!(
            !store.verify_receipt_chain("p").expect("verify"),
            "a rewritten input_hash must not verify"
        );
    }

    /// The `verify` action is the join of three modules that had no consumer:
    /// `quality` builds the envelope, `witness` supplies the receipts, and
    /// `proof_gate` decides. A freshly indexed project must accept.
    #[test]
    fn verify_accepts_a_cleanly_indexed_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let verdict = store.verify_run("p").expect("verify");
        assert_eq!(verdict.receipts, 1, "one completed run, one receipt");
        assert!(verdict.chain_ok, "the chain must be intact");
        assert!(
            verdict.decision.is_accept(),
            "a clean run must accept, got {:?}",
            verdict.decision
        );
        assert_eq!(verdict.envelope.count, 1);
    }

    /// A project with no runs has nothing to verify, and the gate must say so
    /// rather than silently accepting.
    #[test]
    fn verify_rejects_a_project_with_no_runs() {
        let store = GraphStore::open_in_memory().expect("store");
        store
            .register_project("empty", "/tmp/empty")
            .expect("project");
        let verdict = store.verify_run("empty").expect("verify");
        assert_eq!(verdict.receipts, 0);
        assert!(!verdict.decision.is_accept(), "no receipts must not accept");
    }

    /// A tampered receipt must not accept, even though the envelope is healthy.
    #[test]
    fn verify_rejects_a_tampered_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        store
            .connection()
            .execute(
                "UPDATE index_receipts SET input_hash = 'forged' WHERE project = 'p'",
                [],
            )
            .expect("tamper");
        let verdict = store.verify_run("p").expect("verify");
        assert!(!verdict.chain_ok, "a rewritten receipt breaks the chain");
        assert!(!verdict.decision.is_accept());
    }

    /// A run whose files mostly failed to parse is degraded: the quality
    /// envelope must carry a flag and the gate must reject.
    #[test]
    fn verify_rejects_a_degraded_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A file whose parser reports an unusable region.
        std::fs::write(dir.path().join("broken.rs"), "fn {{{ ((( )\n").expect("broken.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let verdict = store.verify_run("p").expect("verify");
        if verdict.envelope.is_degraded() {
            assert!(
                !verdict.decision.is_accept(),
                "a flagged envelope must reject, got {:?}",
                verdict.decision
            );
        }
    }

    /// A skeleton pass builds file nodes only: measured 1,105 ms for 2,743 files
    /// against ~135,000 ms for a full pass.
    #[test]
    fn a_skeleton_pass_records_files_without_parsing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        let report = store
            .index_project(
                dir.path(),
                &IndexOptions::new("p").with_depth(Depth::Skeleton),
            )
            .expect("skeleton");
        assert_eq!(report.nodes, 1, "one file node, no symbols: {report:?}");
        assert_eq!(report.edges, 0, "a skeleton resolves no edges: {report:?}");
        let files: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM nodes WHERE project = 'p' AND label = 'File'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(files, 1);
    }

    /// A symbols pass records definitions but no edges, so the depth is visible
    /// in what the store holds.
    #[test]
    fn a_symbols_pass_records_definitions_but_no_edges() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        let report = store
            .index_project(
                dir.path(),
                &IndexOptions::new("p").with_depth(Depth::Symbols),
            )
            .expect("symbols");
        assert!(report.nodes >= 3, "file plus two functions: {report:?}");
        // CONTAINS edges are structural and always written; what a symbols pass
        // skips is *resolution* — no CALLS, no IMPORTS.
        let resolved: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE project = 'p'
                 AND edge_type IN ('CALLS', 'IMPORTS')",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(resolved, 0, "a symbols pass resolves no calls: {report:?}");
    }

    /// The default depth is unchanged, so an existing caller gets the old
    /// behaviour.
    #[test]
    fn the_default_depth_is_full() {
        assert_eq!(Depth::default(), Depth::Full);
        assert!(Depth::Full.resolves_edges());
        assert!(!Depth::Symbols.resolves_edges());
        assert!(!Depth::Skeleton.resolves_edges());
        assert_eq!(Depth::from_str_or_default("skeleton"), Depth::Skeleton);
        assert_eq!(Depth::from_str_or_default("nonsense"), Depth::Full);
    }

    /// A call site that does not resolve must leave a record with a reason.
    /// Discarding it is why the tool could not answer "how many calls here failed,
    /// and why" — the metric codebase-memory-mcp publishes as `unresolved`.
    #[test]
    fn an_unresolved_call_site_is_recorded_with_a_reason() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("a.rs"),
            "pub fn caller() { totally_unknown_fn(); }\npub fn known() {}\n",
        )
        .expect("a.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let rows: Vec<(String, String)> = store
            .connection()
            .prepare(
                "SELECT json_extract(props_json,'$.callee'), json_extract(props_json,'$.reason')
                 FROM edges WHERE project='p' AND edge_type='USAGE'",
            )
            .expect("stmt")
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .expect("rows")
            .map(|r| r.expect("row"))
            .collect();
        assert_eq!(rows.len(), 1, "one unresolved call site: {rows:?}");
        assert_eq!(rows[0].0, "totally_unknown_fn");
        // A plain lowercase identifier: it reads like a method call, and what a
        // resolver needs (the receiver's type) is not recorded, so this is
        // `type_unknown` rather than the old catch-all.
        assert_eq!(rows[0].1, "type_unknown");

        // A resolved call is NOT also recorded as a miss.
        let calls: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE project='p' AND edge_type='CALLS'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(calls, 0, "nothing in this fixture resolves");
    }

    /// The split, on the two shape groups that made it necessary: a trait
    /// method (`clone`) and a constructor (`Some`), both of which call a
    /// definition that lives outside the indexed tree.
    ///
    /// Measured on this repository before the split: 17,395 of 18,206 misses
    /// shared one `no_definition_with_that_name` label, so the count could not
    /// say whether the resolver was broken or the code called `clone`.
    #[test]
    fn a_std_method_and_a_constructor_are_classified_external() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("a.rs"),
            // One unresolved call per caller: a miss is a self-loop `USAGE`
            // edge and the store dedupes on (source, target, type), so two
            // misses from one function collapse into one row.
            "pub fn with_method(s: String) { let _t = s.clone(); }\n\
             pub fn with_ctor() -> Option<u32> { Some(1) }\n\
             pub fn with_from(s: String) -> String { String::from(s) }\n\
             pub fn with_parse(s: String) -> Result<u32, std::num::ParseIntError> { s.parse() }\n",
        )
        .expect("a.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let rows: Vec<(String, String)> = store
            .connection()
            .prepare(
                "SELECT json_extract(props_json,'$.callee'), json_extract(props_json,'$.reason')
                 FROM edges WHERE project='p' AND edge_type='USAGE' ORDER BY 1",
            )
            .expect("stmt")
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .expect("rows")
            .map(|r| r.expect("row"))
            .collect();
        let reasons: std::collections::BTreeMap<String, String> = rows.into_iter().collect();
        // `clone` is a std trait method; `Some` is a std variant; `from` is a
        // conversion trait reached through a path. All of them name definitions
        // outside the indexed tree, so all are external rather than "no
        // definition with that name".
        assert_eq!(
            reasons.get("clone").map(String::as_str),
            Some("external_or_std"),
            "{reasons:?}"
        );
        assert_eq!(
            reasons.get("Some").map(String::as_str),
            Some("external_or_std"),
            "{reasons:?}"
        );
        assert_eq!(
            reasons.get("from").map(String::as_str),
            Some("external_or_std"),
            "{reasons:?}"
        );
        assert_eq!(
            reasons.get("parse").map(String::as_str),
            Some("external_or_std"),
            "a FromStr trait method reached through a path: {reasons:?}"
        );
    }

    /// A plain unknown `helper_fn()` is `type_unknown`, not `no_definition`:
    /// a bare lowercase name cannot be told apart from `receiver.method()`
    /// without the receiver's type, so the honest label names what is missing.
    /// The fixture also shows the boundary — `Some` is external, `helper_fn` is
    /// not, and they sit in the same function.
    #[test]
    fn a_plain_unknown_name_is_type_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("a.rs"),
            "pub fn caller() { helper_fn(); }\n",
        )
        .expect("a.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let reason: String = store
            .connection()
            .query_row(
                "SELECT json_extract(props_json,'$.reason') FROM edges
                 WHERE project='p' AND edge_type='USAGE'",
                [],
                |r| r.get(0),
            )
            .expect("reason");
        assert_eq!(reason, "type_unknown");
    }

    /// A name the tree knows, but a call shape resolution refuses, stays
    /// `shape_not_modelled` — the one bucket that points back at our resolver
    /// rather than at the code or at the standard library.
    ///
    /// The fixture uses self-recursion because that is the miss `resolve_call`
    /// creates on purpose: an edge from a node to itself is not a call-graph
    /// edge, so the miss is recorded instead.
    #[test]
    fn a_known_name_with_an_unmodelled_shape_stays_a_shape_problem() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "pub fn caller() { caller(); }\n").expect("a.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let reason: String = store
            .connection()
            .query_row(
                "SELECT json_extract(props_json,'$.reason') FROM edges
                 WHERE project='p' AND edge_type='USAGE'",
                [],
                |r| r.get(0),
            )
            .expect("reason");
        assert_eq!(reason, "shape_not_modelled");
    }

    /// A miss must be distinguishable from a hit in a count, and the wire name
    /// must be the one the schema writes (`USAGE`, not the enum variant's spelling).
    #[test]
    fn the_miss_edge_uses_the_stored_wire_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "pub fn caller() { nope_fn(); }\n").expect("a.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let usage: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE project='p' AND edge_type='USAGE'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(usage, 1, "the miss is stored under USAGE");
        let mut stmt = store
            .connection()
            .prepare("SELECT DISTINCT edge_type FROM edges WHERE project='p'")
            .expect("stmt");
        let kinds: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("rows")
            .map(|r| r.expect("row"))
            .collect();
        assert!(
            !kinds.iter().any(|k| k == "USES"),
            "USES is the enum variant, not the wire name: {kinds:?}"
        );
    }

    /// An edge whose callee name had many candidates is a guess, and the line must
    /// say so. Measured worst case on a real repository: `new` with 400 candidates.
    #[test]
    fn an_ambiguous_edge_names_its_candidate_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        // `shared` is defined in b and c but not in a, so the call from a has no
        // same-module candidate and must fall to the suffix rung.
        std::fs::write(dir.path().join("a.rs"), "pub fn caller() { shared(); }\n").expect("a.rs");
        std::fs::write(dir.path().join("b.rs"), "pub fn shared() {}\n").expect("b.rs");
        std::fs::write(dir.path().join("c.rs"), "pub fn shared() {}\n").expect("c.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let rows: Vec<(String, i64)> = store
            .connection()
            .prepare(
                "SELECT json_extract(props_json,'$.strategy'),
                        json_extract(props_json,'$.candidates')
                 FROM edges WHERE project='p' AND edge_type='CALLS'",
            )
            .expect("stmt")
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .expect("rows")
            .map(|r| r.expect("row"))
            .collect();
        assert_eq!(rows.len(), 1, "one call site: {rows:?}");
        assert_eq!(rows[0].0, "suffix", "two same-named defs is a suffix guess");
        assert_eq!(rows[0].1, 2, "the edge records how many candidates existed");
    }

    /// A call whose path is rooted at a workspace dependency names a definition
    /// outside the tree. It must be a CALLS edge with the `dependency` strategy,
    /// not a miss: the reader needs to know where it goes, not that we failed.
    #[test]
    fn a_call_rooted_at_a_dependency_is_a_dependency_edge() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[dependencies]\ntempdir = \"3\"\n",
        )
        .expect("Cargo.toml");
        std::fs::write(
            dir.path().join("a.rs"),
            "pub fn caller() { tempdir::TempDir::new(); }\n",
        )
        .expect("a.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let rows: Vec<(String, String, String)> = store
            .connection()
            .prepare(
                "SELECT edge_type, json_extract(props_json,'$.strategy'),
                        json_extract(props_json,'$.crate')
                 FROM edges WHERE project='p' AND edge_type IN ('CALLS','USAGE')",
            )
            .expect("stmt")
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("rows")
            .map(|r| r.expect("row"))
            .collect();
        assert_eq!(rows.len(), 1, "one call site: {rows:?}");
        assert_eq!(rows[0].0, "CALLS", "a dependency call is not a miss");
        assert_eq!(rows[0].1, "dependency");
        assert_eq!(rows[0].2, "tempdir");
    }

    /// A `crate::`/`self::` path is in-tree by definition and must stay a miss:
    /// excusing it as a dependency would hide a real resolver gap.
    #[test]
    fn a_crate_local_path_is_not_a_dependency() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[dependencies]\ncrate = \"1\"\n",
        )
        .expect("Cargo.toml");
        std::fs::write(
            dir.path().join("a.rs"),
            "pub fn caller() { crate::elsewhere::thing(); }\n",
        )
        .expect("a.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let strategies: Vec<String> = store
            .connection()
            .prepare("SELECT json_extract(props_json,'$.strategy') FROM edges WHERE project='p'")
            .expect("stmt")
            .query_map([], |r| r.get::<_, Option<String>>(0))
            .expect("rows")
            .filter_map(|r| r.expect("row"))
            .collect();
        assert!(
            !strategies.iter().any(|s| s == "dependency"),
            "a crate-local path must not be excused as a dependency: {strategies:?}"
        );
    }

    #[test]
    fn second_index_is_incremental() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        let first = store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        assert_eq!(first.added, 1);
        let second = store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second");
        assert_eq!(second.unchanged, 1);
        assert_eq!(second.parsed, 0);
        assert_eq!(second.added, 0);
    }

    #[test]
    fn modification_is_detected_and_reindexed() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        std::fs::write(dir.path().join("lib.rs"), "pub fn only_one() {}\n").expect("rewrite");
        let report = store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second");
        assert_eq!(report.modified, 1);
        // The old symbols are gone; the new one is present.
        assert!(store
            .node_by_qn("p", "lib::only_one")
            .expect("qn")
            .is_some());
        assert!(store
            .node_by_qn("p", "lib::parse_config")
            .expect("qn")
            .is_none());
    }

    #[test]
    fn reindex_removes_stale_fts_rows() {
        // Regression: a contentless FTS table has no cascade, so a re-parsed
        // file must delete its old rows or a renamed symbol stays searchable.
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        std::fs::write(dir.path().join("lib.rs"), "pub fn renamed_only() {}\n").expect("rewrite");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second");
        let stale = crate::search::search(
            store.connection(),
            "parse_config",
            &crate::search::SearchOptions::new("p"),
        )
        .expect("search");
        assert!(
            stale.is_empty(),
            "stale FTS rows survived reindex: {stale:?}"
        );
        let fresh = crate::search::search(
            store.connection(),
            "renamed_only",
            &crate::search::SearchOptions::new("p"),
        )
        .expect("search");
        assert!(fresh.iter().any(|h| h.name == "renamed_only"), "{fresh:?}");
    }

    #[test]
    fn removal_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        std::fs::remove_file(dir.path().join("lib.rs")).expect("remove");
        let report = store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second");
        assert_eq!(report.removed, 1);
    }

    #[test]
    fn force_reparses_everything() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_tree(dir.path());
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        let report = store
            .index_project(dir.path(), &IndexOptions::new("p").with_force(true))
            .expect("forced");
        assert_eq!(report.parsed, 1);
        assert_eq!(report.unchanged, 0);
    }

    #[test]
    fn broken_file_is_indexed_and_flagged_partial() {
        // A file with an ERROR region still yields the symbols it does contain,
        // but the run must record honest coverage for it.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("broken.rs"),
            "pub fn good() {}\nfn bad( {\n    let x = );\n",
        )
        .expect("write");
        let mut store = GraphStore::open_in_memory().expect("store");
        let report = store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        assert_eq!(report.parsed, 1);
        assert!(
            report.partial + report.unusable >= 1,
            "broken file must be flagged: {report:?}"
        );
        let cov = store.coverage("p").expect("coverage");
        assert!(!cov.complete);
        assert!(cov.partial + cov.unusable >= 1, "{cov:?}");
    }

    #[test]
    fn coverage_note_clears_when_file_is_fixed() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("m.rs"), "fn bad( {\n").expect("write");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        assert!(!store.coverage("p").expect("cov").complete);
        std::fs::write(dir.path().join("m.rs"), "fn good() {}\n").expect("fix");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second");
        let cov = store.coverage("p").expect("cov");
        assert!(cov.complete, "fixed file must read clean: {cov:?}");
        assert_eq!(cov.partial, 0);
        assert_eq!(cov.unusable, 0);
    }

    #[test]
    fn report_edges_matches_the_store_after_indexing() {
        // R07: `report.edges` used to count insert ATTEMPTS, but the store
        // dedupes (`INSERT OR IGNORE` on a UNIQUE key), so the report
        // overstated the graph by a quarter on a real project. The
        // user-visible number must be the store's truth.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn caller() -> u32 { helper() + helper() }\npub fn helper() -> u32 { 1 }\n",
        )
        .expect("lib.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        let report = store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        // Two call sites of the same callee collapse into one stored edge;
        // the report must agree with what the store actually holds.
        assert_eq!(
            report.edges,
            store.edge_count("p").expect("count"),
            "report counts attempts, store dedupes"
        );
    }

    #[test]
    fn modified_file_keeps_incoming_call_edges() {
        // R08: re-parsing a modified file used to delete its nodes, and the
        // schema's ON DELETE CASCADE silently dropped every CALLS edge from
        // unchanged files into it. Each edit permanently thinned the graph.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "pub fn foo() -> u32 { 1 }\n").expect("a.rs");
        std::fs::write(
            dir.path().join("b.rs"),
            "pub fn caller() -> u32 { foo() }\n",
        )
        .expect("b.rs");
        std::fs::write(dir.path().join("c.rs"), "pub fn other() -> u32 { foo() }\n").expect("c.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("first");
        let before = store.edge_count("p").expect("count");

        // Body-only edit of a.rs; b.rs and c.rs are untouched.
        std::fs::write(dir.path().join("a.rs"), "pub fn foo() -> u32 { 1 + 1 }\n").expect("edit");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("second");

        assert_eq!(
            store.edge_count("p").expect("count"),
            before,
            "a body edit must not drop incoming CALLS edges"
        );
        let foo = store
            .node_by_qn("p", "a::foo")
            .expect("qn")
            .expect("foo survived");
        let calls = store
            .in_edges(foo.id, Some(EdgeType::Calls))
            .expect("in edges");
        assert_eq!(calls.len(), 2, "b::caller and c::other still call a::foo");
    }

    #[test]
    fn resolver_ladder_prefers_same_module() {
        let mut by_qn = HashMap::new();
        by_qn.insert("a::m::foo".to_string(), 1i64);
        by_qn.insert("b::n::foo".to_string(), 2i64);
        by_qn.insert("a::m::caller".to_string(), 3i64);
        let resolver = Resolver::new(by_qn);
        let call = CallSite {
            caller_qn: "a::m::caller".to_string(),
            callee: "foo".to_string(),
            callee_path: None,
            line: 1,
            args: Vec::new(),
        };
        let (src, tgt, props) = resolver
            .resolve_call("a::m::caller", &call)
            .expect("resolve");
        assert_eq!(src, 3);
        assert_eq!(tgt, 1, "same-module foo must win");
        assert_eq!(
            props.get("strategy").and_then(|v| v.as_str()),
            Some("same_module")
        );
    }

    #[test]
    fn resolver_unique_name_strategy() {
        let mut by_qn = HashMap::new();
        by_qn.insert("a::solo".to_string(), 5i64);
        by_qn.insert("a::caller".to_string(), 6i64);
        let resolver = Resolver::new(by_qn);
        let call = CallSite {
            caller_qn: "a::caller".to_string(),
            callee: "solo".to_string(),
            callee_path: None,
            line: 1,
            args: Vec::new(),
        };
        let (_, tgt, props) = resolver.resolve_call("a::caller", &call).expect("resolve");
        assert_eq!(tgt, 5);
        assert_eq!(
            props.get("strategy").and_then(|v| v.as_str()),
            Some("unique")
        );
    }
}
