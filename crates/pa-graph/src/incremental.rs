//! G7 — incremental reindexing with depth-1 closure repair.
//!
//! [`GraphStore::index_project`] re-parses every file whose *content* hash
//! changed and then re-resolves calls project-wide. That is correct but coarse:
//! a one-line edit inside a function body forces the file's nodes to be deleted
//! and re-created, which cascades away the CALLS edges other files hold into it
//! until they are re-resolved.
//!
//! This module splits the file hash into a **body** hash (raw bytes) and a
//! **surface** hash (the signatures of the file's definitions, bodies
//! excluded — see [`crate::surface`]). Because the surface is a pure function
//! of the file bytes, an untouched dependent can never change its own surface,
//! so the reverse-dependency closure is *structurally* depth 1 — no fixpoint.
//!
//! The repair rules:
//!
//! | Change | Action |
//! |---|---|
//! | content unchanged | skip |
//! | body changed, surface identical | reindex **only this file** |
//! | surface changed | reindex this file **plus its depth-1 dependents** |
//! | file added | reindex it |
//! | file removed | drop its nodes/edges/hashes |
//!
//! Reindexing is **in place**: node ids are preserved (upsert by qualified
//! name, delete only the symbols that disappeared), so edges from unchanged
//! files into a body-edited file survive. Declining to be clever is never
//! wrong — when the surface of a file is unknown (no stored row) the file is
//! treated as a surface change.
//!
//! Added-file caveat: a brand-new file can satisfy a call another file left
//! *unresolved* in a previous run (no edge exists yet, so no dependent is
//! discoverable). This matches the coarse [`GraphStore::index_project`]
//! behaviour, which likewise only re-resolves calls from re-parsed files;
//! callers that need that edge should run with [`IndexOptions::with_force`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::extract::{CallSite, ImportDef};
use crate::hash::hash_bytes;
use crate::indexer::{discover, module_of, rel_path, IndexOptions, Resolver};
use crate::store::GraphStore;
use crate::surface::surface_of;
use crate::types::EdgeType;

/// Why a file (or its dependents) was reindexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum ReindexReason {
    /// Nothing changed; no file was reindexed.
    #[default]
    Unchanged,
    /// At least one new file appeared.
    Added,
    /// At least one file disappeared.
    Removed,
    /// Only function bodies changed (no surface change) — single-file repairs.
    BodyOnly,
    /// At least one surface changed, pulling in dependents (closure depth 1).
    SurfaceChange,
    /// `force` was set.
    Forced,
    /// More than one of the above applied in a single run.
    Mixed,
}

impl ReindexReason {
    /// Stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Added => "added",
            Self::Removed => "removed",
            Self::BodyOnly => "body_only",
            Self::SurfaceChange => "surface_change",
            Self::Forced => "forced",
            Self::Mixed => "mixed",
        }
    }
}

impl std::fmt::Display for ReindexReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of [`GraphStore::reindex_incremental`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncrementalReport {
    /// Files re-parsed directly (added + body-only + surface-changed).
    pub files_reindexed: usize,
    /// Additional dependent files pulled in by surface changes.
    pub closure_files: usize,
    /// Dominant reason for the run.
    pub reason: ReindexReason,
    /// Files removed from the index.
    pub removed: usize,
    /// Files whose content was unchanged and skipped.
    pub unchanged: usize,
    /// Paths reindexed directly.
    pub reindexed_paths: Vec<String>,
    /// Paths reindexed as closure dependents.
    pub closure_paths: Vec<String>,
}

/// A file selected for reindexing, with its parsed inputs.
struct Target {
    rel: String,
    abs: PathBuf,
    source: String,
    lang: pa_ast::SupportLang,
    module: String,
}

impl GraphStore {
    /// Incrementally reindex `root` into this store, repairing only the files
    /// a change actually affects (G7 closure repair).
    ///
    /// Returns an [`IncrementalReport`] describing what was reindexed and why.
    ///
    /// # Errors
    /// Propagates I/O, walker, and SQLite errors. A single unparseable file is
    /// skipped, never fatal.
    // The classification → closure → in-place-reindex → re-resolve pipeline is
    // one cohesive algorithm; splitting it would only scatter the invariants.
    #[allow(clippy::too_many_lines)]
    pub fn reindex_incremental(
        &mut self,
        root: impl AsRef<Path>,
        opts: &IndexOptions,
    ) -> Result<IncrementalReport> {
        let root = root
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| root.as_ref().to_path_buf());
        let root_str = root.to_string_lossy().to_string();
        self.register_project(&opts.project, &root_str)?;
        self.touch_project(&opts.project)?;

        let stored_hashes = self.all_file_hashes(&opts.project)?;
        let stored_surfaces = self.all_file_surfaces(&opts.project)?;
        let files = discover(&root, opts);

        // ── 1. classify every discovered file ───────────────────────────
        let mut targets: Vec<Target> = Vec::new();
        let mut closure_seeds: Vec<String> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut unchanged = 0usize;
        let mut reasons: BTreeSet<ReindexReason> = BTreeSet::new();

        for abs in &files {
            let rel = rel_path(&root, abs);
            seen.insert(rel.clone());
            let Some(lang) = pa_ast::SupportLang::from_path(abs) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(abs) else {
                continue;
            };
            let Some(meta) = std::fs::metadata(abs).ok() else {
                continue;
            };
            if meta.len() > opts.max_file_bytes {
                continue;
            }
            let hash = hash_bytes(&bytes, &meta);
            let source = String::from_utf8_lossy(&bytes).into_owned();
            let module = module_of(&rel);

            let reason = if opts.force {
                ReindexReason::Forced
            } else {
                match stored_hashes.get(&rel) {
                    None => ReindexReason::Added,
                    Some(old) if old.sha256 == hash.sha256 => {
                        // Content is byte-identical. If the surface row is
                        // missing (older index) backfill it without a full
                        // re-parse, then skip.
                        if !stored_surfaces.contains_key(&rel) {
                            let surface = surface_of(&source, lang, &module);
                            self.put_file_surface(
                                &opts.project,
                                &rel,
                                &surface.surface_sha,
                                &surface.body_sha,
                                &surface.surface_json,
                            )?;
                        }
                        unchanged += 1;
                        continue;
                    }
                    Some(_) => {
                        let new_surface = surface_of(&source, lang, &module);
                        match stored_surfaces.get(&rel) {
                            Some((old_surface, _)) if old_surface == &new_surface.surface_sha => {
                                ReindexReason::BodyOnly
                            }
                            // No stored surface, or it differs: treat as a
                            // surface change (declining is never wrong).
                            _ => ReindexReason::SurfaceChange,
                        }
                    }
                }
            };
            reasons.insert(reason);
            if reason == ReindexReason::SurfaceChange {
                closure_seeds.push(rel.clone());
            }
            targets.push(Target {
                rel,
                abs: abs.clone(),
                source,
                lang,
                module,
            });
        }

        // ── 2. removed files ────────────────────────────────────────────
        let mut removed = 0usize;
        for rel in stored_hashes.keys() {
            if !seen.contains(rel) {
                self.clear_file(&opts.project, rel)?;
                self.remove_file_hash(&opts.project, rel)?;
                self.remove_file_surface(&opts.project, rel)?;
                self.remove_lsp_surface(&opts.project, rel)?;
                removed += 1;
            }
        }
        if removed > 0 {
            reasons.insert(ReindexReason::Removed);
        }

        // ── 3. closure: depth-1 reverse dependencies of surface changes ──
        // Computed *before* any node mutation, so the pre-change edge set (who
        // depended on the changed file) is what seeds the closure.
        let already: BTreeSet<String> = targets.iter().map(|t| t.rel.clone()).collect();
        let mut closure: BTreeSet<String> = BTreeSet::new();
        for seed in &closure_seeds {
            for dep in self.incoming_source_files(&opts.project, seed)? {
                if dep != *seed && !already.contains(&dep) {
                    closure.insert(dep);
                }
            }
        }
        let mut closure_targets: Vec<Target> = Vec::new();
        for rel in &closure {
            let abs: PathBuf = root.join(rel);
            let Some(lang) = pa_ast::SupportLang::from_path(&abs) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let abs = abs.clone();
            closure_targets.push(Target {
                rel: rel.clone(),
                abs,
                source,
                lang,
                module: module_of(rel),
            });
        }

        // ── 4. reindex in place, collecting call sites to re-resolve ─────
        let mut pending_calls: Vec<(String, CallSite)> = Vec::new();
        let mut pending_imports: Vec<(String, ImportDef)> = Vec::new();

        for t in targets.iter().chain(closure_targets.iter()) {
            let (calls, imports) = self.reindex_file_in_place(&opts.project, t)?;
            for call in calls {
                pending_calls.push((call.caller_qn.clone(), call));
            }
            for import in imports {
                pending_imports.push((t.rel.clone(), import));
            }
        }

        // ── 5. re-resolve cross-file edges for the touched files ────────
        let symbols = self.symbol_index(&opts.project)?;
        let resolver = Resolver::new(symbols);
        let file_nodes = self.file_node_ids(&opts.project)?;
        for (caller_qn, call) in &pending_calls {
            if let Some((source, target, props)) = resolver.resolve_call(caller_qn, call) {
                self.insert_edge(&opts.project, source, target, EdgeType::Calls, &props)?;
            }
        }
        for (rel, import) in &pending_imports {
            let Some(&file_id) = file_nodes.get(rel) else {
                continue;
            };
            if let Some((target, props)) = resolver.resolve_import(import) {
                self.insert_edge(&opts.project, file_id, target, EdgeType::Imports, &props)?;
            }
        }

        let reason = pick_reason(&reasons);
        let report = IncrementalReport {
            files_reindexed: targets.len(),
            closure_files: closure_targets.len(),
            reason,
            removed,
            unchanged,
            reindexed_paths: targets.iter().map(|t| t.rel.clone()).collect(),
            closure_paths: closure_targets.iter().map(|t| t.rel.clone()).collect(),
        };
        Ok(report)
    }

    /// Re-parse one file **in place**: preserve node ids, refresh FTS rows,
    /// drop symbols that disappeared, and remove stale resolved edges so they
    /// can be recomputed. Returns the file's call sites and imports.
    fn reindex_file_in_place(
        &self,
        project: &str,
        t: &Target,
    ) -> Result<(Vec<CallSite>, Vec<ImportDef>)> {
        let extracted = crate::extract::extract(&t.source, t.lang, &t.module);
        // 1. Drop this file's FTS rows (the table is contentless; nodes stay).
        self.clear_file_fts(project, &t.rel)?;
        // 2. Drop resolved edges originating here; they are re-resolved below.
        self.delete_outgoing_edges(project, &t.rel, &[EdgeType::Calls, EdgeType::Imports])?;
        // 3. Upsert the File node, symbol nodes, FTS rows, CONTAINS edges.
        self.write_file(project, &t.rel, &extracted)?;
        // 4. Remove symbols that no longer exist (cascades their edges).
        let mut keep: Vec<String> = Vec::with_capacity(extracted.symbols.len() + 1);
        keep.push(format!("{project}::{}::__file__", t.rel));
        keep.extend(extracted.symbols.iter().map(|s| s.qualified_name.clone()));
        self.delete_file_nodes_not_in(project, &t.rel, &keep)?;
        // 5. Record the (possibly unchanged) surface and the content hash so
        //    the next run classifies this file correctly.
        let surface = surface_of(&t.source, t.lang, &t.module);
        self.put_file_surface(
            project,
            &t.rel,
            &surface.surface_sha,
            &surface.body_sha,
            &surface.surface_json,
        )?;
        if let Ok(meta) = std::fs::metadata(&t.abs) {
            let bytes = std::fs::read(&t.abs)?;
            self.put_file_hash(project, &t.rel, &hash_bytes(&bytes, &meta))?;
        }
        Ok((extracted.calls, extracted.imports))
    }
}

/// Choose the dominant reason for a run: a single reason wins; several means
/// `Mixed`; nothing but skips means `Unchanged`.
fn pick_reason(reasons: &BTreeSet<ReindexReason>) -> ReindexReason {
    match reasons.len() {
        0 => ReindexReason::Unchanged,
        1 => *reasons.iter().next().unwrap_or(&ReindexReason::Unchanged),
        _ => ReindexReason::Mixed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::GraphStore;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write");
    }

    fn index(store: &mut GraphStore, dir: &Path) {
        store
            .index_project(dir, &IndexOptions::new("p"))
            .expect("index");
    }

    #[test]
    fn first_run_indexes_everything_as_added() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");
        assert_eq!(report.files_reindexed, 1);
        assert_eq!(report.reason, ReindexReason::Added);
    }

    #[test]
    fn no_change_is_a_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");
        assert_eq!(report.files_reindexed, 0);
        assert_eq!(report.closure_files, 0);
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.reason, ReindexReason::Unchanged);
    }

    #[test]
    fn body_only_edit_reindexes_exactly_one_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        write(dir.path(), "b.rs", "pub fn caller() -> u32 { foo() }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());

        // Edit only foo's body: signature (surface) unchanged.
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 42 }\n");
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");
        assert_eq!(report.files_reindexed, 1, "only a.rs reindexed: {report:?}");
        assert_eq!(report.closure_files, 0);
        assert_eq!(report.reason, ReindexReason::BodyOnly);
        assert_eq!(report.reindexed_paths, vec!["a.rs".to_string()]);
    }

    #[test]
    fn body_only_edit_preserves_incoming_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        write(dir.path(), "b.rs", "pub fn caller() -> u32 { foo() }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        let caller = store
            .node_by_qn("p", "b::caller")
            .expect("qn")
            .expect("caller");
        let (_, out_before) = store.degree(caller.id).expect("deg");
        assert!(out_before >= 1, "b::caller must call foo before the edit");

        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 + 1 }\n");
        store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");

        // The CALLS edge b::caller -> a::foo must survive a body-only edit.
        let caller = store
            .node_by_qn("p", "b::caller")
            .expect("qn")
            .expect("caller");
        let (_, out_after) = store.degree(caller.id).expect("deg");
        assert!(
            out_after >= 1,
            "in-place body edit dropped the caller's edge (out_after={out_after})"
        );
    }

    #[test]
    fn surface_change_pulls_depth_one_closure() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        write(dir.path(), "b.rs", "pub fn caller() -> u32 { foo() }\n");
        write(dir.path(), "c.rs", "pub fn unrelated() -> u32 { 7 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());

        // Rename foo -> bar: a surface change for a.rs.
        write(dir.path(), "a.rs", "pub fn bar() -> u32 { 1 }\n");
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");
        assert_eq!(report.reason, ReindexReason::SurfaceChange);
        assert_eq!(report.files_reindexed, 1, "a.rs reindexed: {report:?}");
        assert_eq!(report.closure_files, 1, "b.rs pulled in: {report:?}");
        assert_eq!(report.closure_paths, vec!["b.rs".to_string()]);
        // The unrelated file is untouched.
        assert!(!report.reindexed_paths.contains(&"c.rs".to_string()));
        assert!(!report.closure_paths.contains(&"c.rs".to_string()));
    }

    #[test]
    fn closure_does_not_include_the_changed_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Self-recursive call: a.rs has an edge into itself.
        write(
            dir.path(),
            "a.rs",
            "pub fn fib(n: u32) -> u32 { if n < 2 { n } else { fib(n - 1) } }\n",
        );
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        write(
            dir.path(),
            "a.rs",
            "pub fn fib(n: u64) -> u64 { if n < 2 { n } else { fib(n - 1) } }\n",
        );
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");
        assert_eq!(report.closure_files, 0, "self-edge must not add closure");
    }

    #[test]
    fn removal_drops_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        write(dir.path(), "b.rs", "pub fn bar() -> u32 { 2 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        std::fs::remove_file(dir.path().join("b.rs")).expect("remove");
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");
        assert_eq!(report.removed, 1);
        assert!(store.node_by_qn("p", "b::bar").expect("qn").is_none());
        assert!(store.node_by_qn("p", "a::foo").expect("qn").is_some());
    }

    #[test]
    fn added_file_is_indexed_and_searchable() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        write(dir.path(), "b.rs", "pub fn added_thing() -> u32 { 9 }\n");
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");
        assert_eq!(report.reason, ReindexReason::Added);
        assert!(store
            .node_by_qn("p", "b::added_thing")
            .expect("qn")
            .is_some());
        let hits = store
            .search("added_thing", &crate::search::SearchOptions::new("p"))
            .expect("search");
        assert!(hits.iter().any(|h| h.name == "added_thing"), "{hits:?}");
    }

    #[test]
    fn forced_run_reindexes_everything() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p").with_force(true))
            .expect("run");
        assert_eq!(report.reason, ReindexReason::Forced);
        assert_eq!(report.files_reindexed, 1);
    }

    #[test]
    fn surface_row_backfilled_for_pre_surface_index() {
        // A store indexed before surfaces existed has hashes but no surface
        // rows; a no-change run must backfill them without a re-parse.
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        store
            .connection()
            .execute("DELETE FROM file_surfaces WHERE project = 'p'", [])
            .expect("clear surfaces");
        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("run");
        assert_eq!(report.reason, ReindexReason::Unchanged);
        assert!(store.file_surface("p", "a.rs").expect("surface").is_some());
    }
}
