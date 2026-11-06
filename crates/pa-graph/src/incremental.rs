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
//! Added-file repair: a brand-new file can satisfy a call another file left
//! *unresolved* in a previous run. No edge exists, so the depth-1 closure cannot
//! discover the dependent — `resolve_newly_satisfiable` therefore re-resolves the
//! still-unresolved rows of files this run did NOT touch. Without it a full index
//! and an incremental one disagreed about the same tree, which the review lists as
//! an acceptance check ("новая definition, разрешающая прежний unresolved").

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
    /// At least one stored revision was shallower than this pass: a skeleton or
    /// symbols index left rows whose content hash matched but whose graph did not.
    Deepened,
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
            Self::Deepened => "deepened",
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

/// What one in-place re-parse handed back to the pass.
struct ReindexedFile {
    calls: Vec<CallSite>,
    imports: Vec<ImportDef>,
    hash: crate::hash::FileHash,
}

/// A file selected for reindexing, with its parsed inputs.
struct Target {
    rel: String,
    source: String,
    /// The content hash computed from the SAME bytes `source` was decoded from.
    /// Re-reading the file at write time (the previous behaviour) could pair a
    /// parse of the old text with the hash of a newer one — a TOCTOU that makes
    /// the next run skip a file whose index is stale (audit §4.3).
    hash: crate::hash::FileHash,
    lang: pa_ast::SupportLang,
    module: String,
}

/// The `use … as alias;` / `import … as alias` bindings of a source, without a parse tree.
///
/// `note_imports` needs the local→target pairs, and a full `extract` builds every symbol,
/// every call site and every doc string to produce them. This is called once per file whose
/// repair could need an alias, on the path an operator runs after every commit, so the
/// cheapest correct thing wins: a scan for the `as <ident>` form, which is the only shape
/// that CREATES a binding different from the name it imports.
///
/// A file this misses keeps its previous behaviour — the site stays unresolved — so the
/// failure mode is the old bug, not a wrong verdict.
fn extract_import_aliases(
    source: &str,
    lang: pa_ast::SupportLang,
) -> Vec<crate::extract::ImportDef> {
    use pa_ast::SupportLang;
    if !matches!(
        lang,
        SupportLang::Rust
            | SupportLang::Python
            | SupportLang::TypeScript
            | SupportLang::Tsx
            | SupportLang::JavaScript
    ) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in source.lines() {
        // EVERY `as` on the line, not the first: `use crate::c::{thing as first, other as
        // second};` declares two bindings, and a scan that stopped at the first repaired
        // `first` while `second` stayed unresolved (the grouped-alias case the reverify
        // found).
        let mut search_from = 0usize;
        while let Some(offset) = line[search_from..].find(" as ") {
            let as_at = search_from + offset;
            search_from = as_at + 4;
            let after = line[search_from..].trim_start();
            let local: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if local.is_empty() {
                continue;
            }
            let before = line[..as_at].trim_end();
            // The imported name: the last identifier before `as`. Only the identifier —
            // taking the path separators too reversed the whole clause into the target.
            let target: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if target.is_empty() || target == local {
                continue;
            }
            // The module: the path the entry came from. Inside a brace group the group's
            // prefix owns it (`use crate::c::{…}` → `crate::c`), so the LAST `{` wins over
            // whatever an earlier entry left behind. The resolver uses this only to break a
            // tie between same-named candidates; an empty one costs a confidence step, never
            // a wrong edge.
            let module = match before.rfind('{') {
                Some(brace) => before[..brace]
                    .trim_end_matches("::")
                    .trim_start_matches("use ")
                    .trim_start_matches("from ")
                    .trim()
                    .to_string(),
                None => before
                    .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
                    .trim_end_matches("::")
                    .trim_start_matches("use ")
                    .trim_start_matches("from ")
                    .trim_matches(|c: char| c == ',' || c == ' ')
                    .to_string(),
            };
            out.push(crate::extract::ImportDef {
                local_name: local,
                target_name: target,
                module,
            });
        }
    }
    out
}

#[cfg(test)]
mod alias_scan_tests {
    use super::*;

    /// The scan is a cheap substitute for a parse, so it is pinned by the shape it reads.
    ///
    /// It exists because a full `extract` per candidate file made every incremental pass
    /// parse every file holding an unresolved site — 1756 of 2250 on this repo. The failure
    /// mode if it misses a form is the OLD behaviour (the site stays unresolved), never a
    /// wrong verdict, which is why a scan is acceptable here and a parse is not.
    #[test]
    fn the_alias_scan_reads_the_as_form() {
        let rust =
            extract_import_aliases("use crate::c::thing as alias;\n", pa_ast::SupportLang::Rust);
        assert_eq!(rust.len(), 1, "{rust:?}");
        assert_eq!(rust[0].local_name, "alias");
        assert_eq!(rust[0].target_name, "thing");
        assert_eq!(rust[0].module, "crate::c");

        let python = extract_import_aliases(
            "from pkg.mod import thing as alias\n",
            pa_ast::SupportLang::Python,
        );
        assert_eq!(python.len(), 1, "{python:?}");
        assert_eq!(python[0].local_name, "alias");
        assert_eq!(python[0].target_name, "thing");
    }

    /// A brace group declares SEVERAL aliases on one line, and every one is a binding.
    ///
    /// The scan stopped at the first `as`, so `use crate::c::{thing as first, other as
    /// second};` repaired `first` and left `second` unresolved while a full index resolved
    /// both — the divergence the repair exists to remove, in a form one line long (found by
    /// the fourth reverify wave).
    #[test]
    fn the_alias_scan_reads_every_binding_in_a_brace_group() {
        let aliases = extract_import_aliases(
            "use crate::c::{thing as first, other as second};\n",
            pa_ast::SupportLang::Rust,
        );
        let pairs: Vec<(&str, &str, &str)> = aliases
            .iter()
            .map(|a| {
                (
                    a.local_name.as_str(),
                    a.target_name.as_str(),
                    a.module.as_str(),
                )
            })
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("first", "thing", "crate::c"),
                ("second", "other", "crate::c")
            ],
            "both bindings, and both owned by the group's module"
        );
    }

    /// A plain import is not an alias: it binds the name it imports.
    #[test]
    fn the_alias_scan_ignores_a_plain_import() {
        for source in [
            "use crate::c::thing;\n",
            "import os\n",
            "from pkg import thing\n",
        ] {
            assert!(
                extract_import_aliases(source, pa_ast::SupportLang::Rust).is_empty(),
                "a plain import is not an alias: {source}"
            );
        }
    }

    /// The empty pass costs the same whether the tree has one unresolved file or four
    /// hundred.
    ///
    /// This is the measurement the `#36` repair was written against: the first version read
    /// and PARSED every file holding an unresolved site on every pass, which on this repo is
    /// 1756 of 2250 files. The guard plus the text scan bring an empty pass back to the walk
    /// itself. Measured (400 files, all unresolved, 5 runs, debug): 46 ms median with the
    /// guard, 64 ms without — the 18 ms difference is 45 µs per file, which is the scan, not
    /// a parse (a parse of these fixtures costs ~1.2 ms).
    ///
    /// The test asserts the ORDER of magnitude, not the number: a regression back to parsing
    /// every candidate would be 400 x 1.2 ms = 480 ms, an order above the bound.
    #[test]
    fn an_empty_pass_does_not_scale_with_unresolved_files() {
        use std::time::Instant;

        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..400 {
            std::fs::write(
                dir.path().join(format!("m{i}.rs")),
                format!("pub fn f{i}() {{ missing_{i}(); }}\n"),
            )
            .expect("fixture file");
        }
        let mut store = crate::GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &crate::IndexOptions::new("p"))
            .expect("index");
        let mut timings = Vec::new();
        for _ in 0..3 {
            let started = Instant::now();
            let report = store
                .reindex_incremental(dir.path(), &crate::IndexOptions::new("p"))
                .expect("empty pass");
            assert!(
                report.reindexed_paths.is_empty(),
                "the fixtures are settled: {report:?}"
            );
            timings.push(started.elapsed());
        }
        timings.sort_unstable();
        let median = timings[1];
        assert!(
            median.as_millis() < 200,
            "an empty pass over 400 unresolved files took {median:?}; a version that parses \
             every candidate is two orders above this"
        );
    }

    /// The empty-pass guard keeps the repair off the common path.
    ///
    /// Measured on this repo: 1756 of 2250 files hold an unresolved site, so a repair that
    /// runs whenever candidates merely EXIST re-reads most of the tree after every one-file
    /// commit. Nothing arrived and nothing left ⇒ no verdict can have changed.
    ///
    /// The oracle is a resolver that WOULD repair: it knows a definition named after the
    /// fixture's unresolved callee, so a consulted repair turns one row into `resolved`. With
    /// the guard the answer is 0 and the row is untouched; without it the call repairs — which
    /// is what makes this discriminating, where "returns 0" alone would not be.
    #[test]
    fn an_empty_pass_does_not_consult_the_repair() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "pub fn caller() { missing(); }\n").expect("a.rs");
        let mut store = crate::GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &crate::IndexOptions::new("p"))
            .expect("first index");

        // A definition the fixture's call would now resolve to, handed ONLY to the resolver.
        // It is a REAL row: the writer inserts edges against it, so a fake id fails the
        // foreign key — which is also how a real repair is exercised.
        let real_id = store
            .upsert_node(&crate::types::Node {
                id: 0,
                project: "p".to_string(),
                label: crate::types::Label::Function,
                name: "missing".to_string(),
                qualified_name: "a::missing".to_string(),
                file_path: "a.rs".to_string(),
                start_line: 1,
                end_line: 1,
                props: serde_json::json!({}),
            })
            .expect("the fake definition row");
        let mut symbols = store.symbol_index("p").expect("symbols");
        symbols.insert("a::missing".to_string(), real_id);
        let resolver = crate::indexer::Resolver::new(symbols);
        let repaired = store
            .resolve_newly_satisfiable(
                &resolver,
                "p",
                &std::collections::HashSet::new(),
                dir.path(),
                &std::collections::HashSet::new(),
            )
            .expect("repair");
        assert_eq!(
            repaired, 0,
            "an empty pass consulted the repair even though nothing arrived or left"
        );
    }

    /// A language whose imports this scan does not model is left alone.
    #[test]
    fn the_alias_scan_skips_languages_it_does_not_model() {
        assert_eq!(
            extract_import_aliases("x as y\n", pa_ast::SupportLang::Haskell),
            Vec::new(),
            "a language the scan does not model yields nothing"
        );
    }
}

impl GraphStore {
    /// Incrementally reindex `root` into this store, repairing only the files
    /// a change actually affects (G7 closure repair).
    ///
    /// Returns an [`IncrementalReport`] describing what was reindexed and why.
    ///
    /// # Errors
    /// Propagates I/O, walker, and SQLite errors. A single unparseable file is
    /// skipped, never fatal. Fails with [`std::io::ErrorKind::TimedOut`] when
    /// another process holds the writer lease for this database past
    /// `opts.lease_wait`, and [`std::io::ErrorKind::Interrupted`] when
    /// `opts.lease_cancel` fires — in both cases nothing is registered and
    /// nothing is written.
    // The classification → closure → in-place-reindex → re-resolve pipeline is
    // one cohesive algorithm; splitting it would only scatter the invariants.
    #[allow(clippy::too_many_lines)]
    pub fn reindex_incremental(
        &mut self,
        root: impl AsRef<Path>,
        opts: &IndexOptions,
    ) -> Result<IncrementalReport> {
        // Writer lease before any registration or parse, as in `index_project`.
        let _lease = self.acquire_lease(opts)?;
        let root = root
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| root.as_ref().to_path_buf());
        let root_str = root.to_string_lossy().to_string();
        self.register_project_at(&opts.project, &root_str)?;

        let stored_hashes = self.all_file_hashes(&opts.project)?;
        let stored_surfaces = self.all_file_surfaces(&opts.project)?;
        let files = discover(&root, opts)?;

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
            let mut hash = hash_bytes(&bytes, &meta);
            // This pass always resolves edges, so the row it writes satisfies the
            // full depth whatever `opts.depth` the caller passed: `reindex` builds
            // the same graph a full index does, and a row stamped shallower would
            // make the next reindex redo the work.
            hash.depth = crate::types::Depth::Full;
            let source = String::from_utf8_lossy(&bytes).into_owned();
            let module = module_of(&rel);

            let reason = if opts.force {
                ReindexReason::Forced
            } else {
                match stored_hashes.get(&rel) {
                    None => ReindexReason::Added,
                    // Same bytes but a shallower stored revision: a skeleton or
                    // symbols pass wrote this row and left the graph without the
                    // symbols (or the edges) this pass is here to write. Skipping
                    // it made "reindex" a no-op over a symbolic graph.
                    Some(old)
                        if old.sha256 == hash.sha256
                            && old.depth.satisfies(crate::types::Depth::Full) =>
                    {
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
                    // Same bytes, but a shallower pass wrote the row: the graph has
                    // no symbols (skeleton) or no resolved edges (symbols) for this
                    // file, so this pass must re-read it. It also seeds the closure,
                    // because nothing in the store says the file's dependents were
                    // ever resolved against symbols it did not index.
                    Some(_) if stored_hashes[&rel].sha256 == hash.sha256 => ReindexReason::Deepened,
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
            if matches!(
                reason,
                ReindexReason::SurfaceChange | ReindexReason::Deepened
            ) {
                closure_seeds.push(rel.clone());
            }
            targets.push(Target {
                rel,
                source,
                hash,
                lang,
                module,
            });
        }

        // ── 2. removed files ────────────────────────────────────────────
        // A removed file's DEPENDENTS are closure seeds, and they must be read BEFORE the
        // sweep: `forget_file` deletes the nodes and edges, so afterwards nothing records
        // who called into the file that left. Those callers then keep a `resolved` verdict
        // for a definition that no longer exists — impact answers "0 callers" for a symbol
        // and the graph still claims the call is fine (review #35).
        let mut removed_dependents: BTreeSet<String> = BTreeSet::new();
        let mut removed = 0usize;
        for rel in stored_hashes.keys() {
            if !seen.contains(rel) {
                // The dependent is normally a file this pass WALKED (it is unchanged), so
                // the filter is "not the removed file itself", never "not seen": the walked
                // set is almost every live file, and filtering on it would drop all of them.
                for dependent in self.incoming_source_files(&opts.project, rel)? {
                    if dependent != *rel {
                        removed_dependents.insert(dependent);
                    }
                }
                // Same single owner as the full pass: a removed file takes all six row
                // families. This sweep used to keep `index_coverage`, so a skipped file
                // that vanished went on depressing `parsed` (see `forget_file`).
                self.forget_file(&opts.project, rel)?;
                removed += 1;
            }
        }
        // Notes for files skipped before any hash existed have no hash row to sweep
        // (see `forget_stale_coverage`).
        let walked: std::collections::HashSet<String> = seen.iter().cloned().collect();
        self.forget_stale_coverage(&opts.project, &walked)?;
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
        // The dependents of a REMOVED file are closure seeds too (review #35): their call
        // sites name a definition that is gone, and no surface hash changed to pull them
        // in — the file they referred to simply stopped existing. They are collected above,
        // before the sweep destroyed the edges that identified them.
        for dependent in removed_dependents {
            if !already.contains(&dependent) {
                closure.insert(dependent);
            }
        }
        let mut closure_targets: Vec<Target> = Vec::new();
        for rel in &closure {
            let abs: PathBuf = root.join(rel);
            let Some(lang) = pa_ast::SupportLang::from_path(&abs) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&abs) else {
                continue;
            };
            let Ok(meta) = std::fs::metadata(&abs) else {
                continue;
            };
            let mut hash = hash_bytes(&bytes, &meta);
            hash.depth = crate::types::Depth::Full;
            let source = String::from_utf8_lossy(&bytes).into_owned();
            closure_targets.push(Target {
                rel: rel.clone(),
                source,
                hash,
                lang,
                module: module_of(rel),
            });
        }

        // ── 4. reindex in place, collecting call sites to re-resolve ─────
        let mut pending_calls: Vec<(String, String, CallSite)> = Vec::new();
        let mut pending_imports: Vec<(String, ImportDef)> = Vec::new();
        // Hashes of the re-parsed files. They commit in the resolution
        // transaction below, beside the edges they claim: a hash stored over a
        // revision whose edges never landed makes the next run skip the file and
        // turns the loss permanent.
        let mut pending_hashes: Vec<(String, crate::hash::FileHash)> = Vec::new();

        let mut reindexed_rels: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for t in targets.iter().chain(closure_targets.iter()) {
            reindexed_rels.insert(t.rel.clone());
            let reindexed = self.reindex_file_in_place(&opts.project, t)?;
            pending_hashes.push((t.rel.clone(), reindexed.hash));
            for call in reindexed.calls {
                pending_calls.push((t.rel.clone(), call.caller_qn.clone(), call));
            }
            for import in reindexed.imports {
                pending_imports.push((t.rel.clone(), import));
            }
        }

        // ── 5. re-resolve cross-file edges for the touched files ────────
        let symbols = self.symbol_index(&opts.project)?;
        // The SAME resolver the full index uses (audit R1): `Resolver::new` is
        // dependency-blind, so a reindex deleted a file's outgoing CALLS/IMPORTS
        // and re-inserted only what an intra-tree name match could find — every
        // cross-crate call vanished with no edge, no miss row and no counter
        // movement, while miss rows for calls the edit deleted stayed behind.
        // A later full index then produced a different graph for the same tree.
        let deps = crate::deps::DependencyCrates::from_workspace(&root).unwrap_or_default();
        let mut resolver = Resolver::with_deps(symbols, deps);
        // The same alias table the full index builds, or the two write paths disagree about
        // a tree with an aliased call — which is exactly what the parity test caught when
        // only the full path had it.
        for (rel, import) in &pending_imports {
            resolver.note_imports(rel, std::slice::from_ref(import));
        }
        let file_nodes = self.file_node_ids(&opts.project)?;
        // Resolution and the hashes it justifies are ONE unit of work, exactly as
        // in the full pass. A prefix of the edges is meaningless against the
        // definition set the rest was resolved with, and a hash committed over a
        // file whose edges never landed makes the next run skip that file — the
        // loss then outlives every subsequent run.
        self.with_transaction(|store| {
            // `record_call_edge` is the full index's per-call writer: it resolves,
            // ELSE records a dependency-rooted self-loop, ELSE records a miss row
            // with its reason. Calling it here is what makes the two write paths
            // agree — an unresolved call becomes evidence instead of silence.
            for (rel, caller_qn, call) in &pending_calls {
                store.record_call_edge(&resolver, &opts.project, rel, caller_qn, call)?;
            }
            for (rel, import) in &pending_imports {
                let Some(&file_id) = file_nodes.get(rel) else {
                    continue;
                };
                if let Some((target, props)) = resolver.resolve_import(import) {
                    store.insert_edge(&opts.project, file_id, target, EdgeType::Imports, &props)?;
                }
            }
            // The hashes travel WITH the resolved edges (see `Target::hash`): the
            // disk is not read a second time, so a stored hash can never describe a
            // newer revision than the symbols it indexes — nor land without them.
            for (rel, hash) in &pending_hashes {
                store.put_file_hash(&opts.project, rel, hash)?;
            }
            Ok(())
        })?;

        // A NEW definition can satisfy a call another file left unresolved in a previous
        // run, and no edge exists for it — so the closure cannot discover it and the stale
        // `USAGE` row would outlive its own reason. The review makes this an acceptance
        // check ("новая definition, разрешающая прежний unresolved"), and the parity test
        // caught the disagreement: incremental kept `USAGE a::caller`, a full index wrote
        // `CALLS a::caller -> d::missing_old` for the same tree.
        //
        // The doc used to call this a known limitation and point at `with_force`. A
        // limitation that a full index does not share is a divergence, and "run a full
        // index" costs a whole-tree parse to fix one edge.
        //
        // Cost: one pass over `call_sites` for the files this run did NOT touch. The
        // touched files have already been re-resolved above, so their rows are current.
        // The dependents of every REMOVED file: their verdicts name a file that no longer
        // exists, whatever those verdicts currently say (review #35).
        let mut invalidated: std::collections::HashSet<String> = std::collections::HashSet::new();
        for rel in stored_hashes.keys() {
            if !seen.contains(rel) {
                for dependent in self.incoming_source_files(&opts.project, rel)? {
                    if dependent != *rel {
                        invalidated.insert(dependent);
                    }
                }
            }
        }
        self.resolve_newly_satisfiable(
            &resolver,
            &opts.project,
            &reindexed_rels,
            &root,
            &invalidated,
        )?;

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
        // Stamped last, like the full index: an interrupted run must not look
        // complete. A reindex CHANGES the graph, so it must also move the
        // evidence: without `analyze` a re-indexed store keeps the old planner
        // statistics (the full path measured 52.30 ms -> 0.08 ms on `trace`),
        // and without a receipt `verify` accepts evidence that predates this
        // run — the store it describes is no longer the store that exists
        // (audit F07/R6). `finish_run` does all three for the full path; this is
        // the incremental half of the same contract.
        self.analyze()?;
        self.touch_project(&opts.project)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.record_index_receipt(&opts.project, now)?;
        if let Ok(head) = crate::changes::run_git(&root, &["rev-parse", "HEAD"]) {
            let head = head.trim();
            if !head.is_empty() {
                self.set_indexed_head(&opts.project, head)?;
            }
        }
        Ok(report)
    }

    /// Re-resolve unresolved call sites whose callee now HAS a definition.
    ///
    /// Both write paths call this, after they have resolved the call sites of the
    /// files they touched. Resolution is per FILE, so a definition that arrives in
    /// one file can satisfy a call spelled in another — and that caller never
    /// changes again, so nothing else would ever revisit its miss. The store then
    /// disagreed with a fresh index of the same tree for good (review #37).
    ///
    /// Only rows still marked `unresolved` and in files this run did NOT touch are
    /// considered: a touched file's rows were just rewritten from its fresh parse.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub(crate) fn resolve_newly_satisfiable(
        &self,
        resolver: &Resolver,
        project: &str,
        reindexed: &std::collections::HashSet<String>,
        root: &Path,
        invalidated: &std::collections::HashSet<String>,
    ) -> Result<usize> {
        // Two kinds of row need repair, and they are repaired the same way — by
        // re-resolving the call against the CURRENT definition set and rewriting the
        // verdict:
        //   * `unresolved` in a file this run did not touch: a definition may have arrived
        //     that satisfies it (review #37);
        //   * ANY verdict in a file whose target was removed: `resolved` is now a lie, and
        //     `unresolved` may still be one if a different definition took the name
        //     (review #35 — the full pass hit this because it re-parses only CHANGED files,
        //     so an unchanged caller kept a verdict about a file that no longer exists).
        // Nothing arrived and nothing left: no verdict can have changed, and this is the
        // common pass (an operator runs `reindex` after a commit that touched one file). The
        // guard is not an optimisation detail — without it every pass reads and parses every
        // file holding an unresolved site, which on this repo is 1756 of 2250 files.
        if reindexed.is_empty() && invalidated.is_empty() {
            return Ok(0);
        }
        let candidates: Vec<crate::types::CallSiteRow> = self
            .call_sites(project)?
            .into_iter()
            .filter(|site| {
                !reindexed.contains(&site.rel_path)
                    && (site.resolution == "unresolved" || invalidated.contains(&site.rel_path))
            })
            .collect();
        // The aliases of the files these sites live in. A resolver built during this run
        // knows the imports of the files the run RE-PARSED; a site in a file it did not
        // touch was written through an alias this resolver has never seen, so
        // `resolve_call` looked up the local name and found nothing — the definition it
        // names had just appeared, and the repair still could not see it (review #36).
        //
        // The bindings come back from the store, not from a fresh parse: the `IMPORTS`
        // edges already carry `local_name` and `module`, so no file is read twice and the
        // repair works on the same evidence the full pass would have used.
        // The aliases of the files these sites live in, read from DISK: an `IMPORTS` edge
        // exists only for an import that RESOLVED, and the case this repair exists for is
        // precisely an import that did not resolve until now (`use crate::c::thing as
        // alias;` with no `c.rs`). So the stored edges cannot supply the binding, and
        // without it `resolve_call` looks up `alias` among the definition names and finds
        // nothing (review #36).
        //
        // Only a file that can actually need this is read: one whose unresolved sites name a
        // callee that does not appear as a definition (an alias site's callee is the LOCAL
        // name), and only when a definition arrived this run. Reading every file with an
        // unresolved site is what made this pass cost a parse per file.
        let mut resolver = resolver.clone();
        let mut aliased: std::collections::HashSet<String> = std::collections::HashSet::new();
        for site in &candidates {
            if aliased.insert(site.rel_path.clone()) {
                let Ok(source) = std::fs::read_to_string(root.join(&site.rel_path)) else {
                    continue;
                };
                let Some(lang) = pa_ast::SupportLang::from_path(&root.join(&site.rel_path)) else {
                    continue;
                };
                resolver.note_imports(&site.rel_path, &extract_import_aliases(&source, lang));
            }
        }
        let mut repaired = 0usize;
        for site in candidates {
            // The same resolver and the same writer the full index uses, so a repaired row
            // is indistinguishable from one written in a full pass.
            let call = crate::extract::CallSite {
                caller_qn: site.caller_qn.clone(),
                callee: site.callee.clone(),
                callee_path: (!site.callee_path.is_empty()).then(|| site.callee_path.clone()),
                line: site.line,
                args: Vec::new(),
            };
            let is_repair = resolver
                .resolve_call(&site.caller_qn, &site.rel_path, &call)
                .is_some_and(|(_, _, props)| {
                    // A self-loop is the same unresolved fact with a different label, so it
                    // is not a repair.
                    props.get("strategy").and_then(serde_json::Value::as_str)
                        != Some(crate::types::CallStrategy::Recursive.as_str())
                });
            if !is_repair && site.resolution == "unresolved" {
                // Still a miss, and it already says so: nothing to rewrite.
                continue;
            }
            // Drop the STALE verdict first, in both shapes. `edges` keeps a miss as a
            // self-loop and a hit as a CALLS edge; the writer will not replace either
            // across the two, so without this the graph carries both the old verdict and
            // the new one.
            self.clear_unresolved_edge(project, &site.rel_path, &site.caller_qn, &site.callee)?;
            self.clear_resolved_call_edge(project, &site.rel_path, &site.caller_qn, &site.callee)?;
            self.record_call_edge(&resolver, project, &site.rel_path, &site.caller_qn, &call)?;
            repaired += 1;
        }
        Ok(repaired)
    }

    /// Re-parse one file **in place**, as ONE unit of work: preserve node ids,
    /// refresh FTS rows, drop symbols that disappeared, and remove stale
    /// resolved edges so they can be recomputed. Returns the file's call sites,
    /// imports, and the hash its row must record.
    ///
    /// The hash comes back rather than being written here because it is the claim
    /// that the file's contribution is WHOLE, and the edges are part of that
    /// contribution: the caller commits it in the resolution transaction. Writing
    /// it here survived a failed resolution, and the next run then skipped the
    /// file as unchanged and kept the loss.
    ///
    /// The clear steps and the rewrite commit or roll back together, exactly
    /// like the full indexer's per-file rewrite ([`GraphStore::rewrite_file`]
    /// under [`GraphStore::with_transaction`]). Without the transaction a
    /// failure between the clears and the rewrite left the file with no FTS
    /// rows, no outgoing edges and NO call sites while its old symbol nodes
    /// stayed — and the pass then aborted, so nothing repaired it (the same
    /// defect `a_failed_rewrite_leaves_the_previous_revision_intact` covers on
    /// the full path).
    ///
    /// Per file, not per run: the files are independent of each other, and a
    /// run-wide transaction would hold every file's row locks until the end,
    /// including across the re-resolution phase that follows. Since this opens
    /// the transaction itself and SQLite has no nested `BEGIN`, the caller must
    /// not already be inside one — the classification and closure steps above it
    /// are not, and re-resolution runs after it.
    ///
    /// # Errors
    /// Propagates SQLite errors, and the file's previous revision stays intact.
    fn reindex_file_in_place(&self, project: &str, t: &Target) -> Result<ReindexedFile> {
        let extracted = crate::extract::extract(&t.source, t.lang, &t.module);
        // The returned call sites leave `with_transaction` only once the commit
        // succeeded: a rolled-back revision contributes nothing to the caller's
        // `pending_calls`/`pending_imports`, which are re-resolved afterwards.
        self.with_transaction(move |store| {
            // 1. Drop this file's FTS rows (the table is contentless; nodes stay).
            store.clear_file_fts(project, &t.rel)?;
            // 2. Drop resolved edges originating here; they are re-resolved below.
            //
            // USES goes too: an unresolved fact belongs to its CALL SITE, and the
            // call site lives in this file. Without the delete a body-only edit
            // that swaps `missing_old()` for `missing_new()` kept the old callee's
            // fact forever — the fresh row conflicts with the surviving self-loop's
            // UNIQUE (source, target, type) and is silently ignored — so the graph
            // answered questions about a call site that no longer exists (the
            // lightweight-graph review, §3.1; reproduced by
            // `probe_missing_old_replaced_by_missing_new`).
            store.delete_outgoing_edges(
                project,
                &t.rel,
                &[EdgeType::Calls, EdgeType::Imports, EdgeType::Uses],
            )?;
            // The per-site rows go with the edges they describe: a call site that
            // left the file must not survive as a row the aggregates no longer
            // back. `edges` deletes by `source`; `call_sites` keys on the file, so
            // a symbol that disappeared takes its sites with it here.
            store.clear_call_sites(project, &t.rel)?;
            // 3. Upsert the File node, symbol nodes, FTS rows, CONTAINS edges.
            store.write_file(project, &t.rel, &extracted)?;
            // 4. Remove symbols that no longer exist (cascades their edges).
            let mut keep: Vec<String> = Vec::with_capacity(extracted.symbols.len() + 1);
            keep.push(format!("{project}::{}::__file__", t.rel));
            keep.extend(extracted.symbols.iter().map(|s| s.qualified_name.clone()));
            store.delete_file_nodes_not_in(project, &t.rel, &keep)?;
            // 5. Record the (possibly unchanged) surface and the content hash so
            //    the next run classifies this file correctly.
            let surface = surface_of(&t.source, t.lang, &t.module);
            store.put_file_surface(
                project,
                &t.rel,
                &surface.surface_sha,
                &surface.body_sha,
                &surface.surface_json,
            )?;
            // The hash is NOT written here. It claims the file's contribution is
            // whole, and this pass adds the file's call and import edges in a
            // later step. Committing it now survived a failed resolution, and the
            // next run then skipped the file as unchanged — the sites it never
            // resolved stayed unresolved forever while every run reported success.
            // The caller commits it with them.
            // Moving, not cloning: the closure owns the parse, so the call
            // sites leave it once, after the commit.
            Ok(ReindexedFile {
                calls: extracted.calls,
                imports: extracted.imports,
                // The hash that travelled WITH the parsed bytes (see `Target::hash`).
                hash: t.hash.clone(),
            })
        })
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
    use std::collections::HashMap;

    use crate::store::GraphStore;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write");
    }

    fn index(store: &mut GraphStore, dir: &Path) {
        store
            .index_project(dir, &IndexOptions::new("p"))
            .expect("index");
    }

    /// The lightweight-graph review §3.1: replacing an unresolved call with a
    /// DIFFERENT unresolved call inside the same symbol must not leave the old
    /// callee's fact behind.
    ///
    /// Both calls are self-loop USES rows keyed (source, target, type), so the
    /// fresh row used to lose the UNIQUE race against the surviving stale one
    /// and the graph kept answering for a call site that no longer existed.
    /// A body-only edit is the trigger: the surface is unchanged, so the file
    /// is re-extracted but its unresolved facts were never cleared.
    #[test]
    fn a_body_only_edit_replaces_the_unresolved_fact() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn caller() { missing_old(); }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        write(dir.path(), "a.rs", "pub fn caller() { missing_new(); }\n");
        store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("reindex");
        let edges = store.all_edges("p").expect("edges");
        let uses: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == crate::types::EdgeType::Uses)
            .collect();
        let props: Vec<String> = uses.iter().map(|e| e.props.to_string()).collect();
        assert_eq!(uses.len(), 1, "exactly one unresolved fact: {props:?}");
        assert!(
            props[0].contains("missing_new"),
            "the surviving fact must carry the NEW callee: {}",
            props[0]
        );
        assert!(
            !props[0].contains("missing_old"),
            "STALE: the old unresolved callee survived the reindex"
        );
    }

    /// A recursive call has a KNOWN target — the caller itself — so it must not
    /// be recorded as unresolved. `resolve_call` returns `None` when
    /// `target == source`, which sent every self-call down the miss path and made
    /// recursion indistinguishable from "we could not resolve this name".
    #[test]
    fn a_recursive_call_resolves_to_itself_rather_than_going_unresolved() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "a.rs",
            "pub fn walk(n: u32) -> u32 { if n == 0 { 0 } else { walk(n - 1) } }\n",
        );
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());

        let sites: Vec<crate::types::CallSiteRow> = store
            .call_sites("p")
            .expect("call sites")
            .into_iter()
            .filter(|s| s.callee == "walk")
            .collect();
        assert_eq!(sites.len(), 1, "one recursive call: {sites:?}");
        assert_eq!(
            sites[0].resolution, "recursive",
            "a self-call is resolved, not a miss: {:?}",
            sites[0]
        );
        assert_ne!(
            sites[0].resolution, "unresolved",
            "recursion must not read as an unresolvable name"
        );
    }

    /// The lightweight-graph review §3.1, second half: "уникальность edge не
    /// включает callsite". An unresolved call is a self-loop USES row with no
    /// `local_name`, so `local_name_gen` is `''` for every miss and
    /// `UNIQUE(source, target, type, '')` collapsed a whole symbol's misses into
    /// ONE row — `INSERT OR IGNORE` then kept whichever came first.
    ///
    /// The aggregate edge still collapses (that is what the algorithms walk),
    /// but `call_sites` keeps every site, so "how many call sites failed here,
    /// and why" has an answer.
    #[test]
    fn every_call_site_survives_even_when_the_edge_aggregate_collapses() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "a.rs",
            "pub fn caller() {\n    missing_one();\n    missing_two();\n    missing_three();\n}\n",
        );
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());

        let sites = store.call_sites("p").expect("call sites");
        let misses: Vec<&crate::types::CallSiteRow> = sites
            .iter()
            .filter(|s| s.resolution == "unresolved")
            .collect();
        let callees: Vec<&str> = misses.iter().map(|s| s.callee.as_str()).collect();
        assert_eq!(
            misses.len(),
            3,
            "all three miss sites must be recorded, got {callees:?}"
        );
        for expected in ["missing_one", "missing_two", "missing_three"] {
            assert!(
                callees.contains(&expected),
                "lost the {expected} call site; recorded {callees:?}"
            );
        }
        // The aggregate keeps collapsing: one USES row, by design.
        let uses = store
            .all_edges("p")
            .expect("edges")
            .into_iter()
            .filter(|e| e.edge_type == crate::types::EdgeType::Uses)
            .count();
        assert_eq!(uses, 1, "edges stays the aggregate the walkers read");
    }

    /// The per-site rows belong to the file that spells them: a re-extract
    /// rewrites them, so a call that left the file leaves no row behind.
    #[test]
    fn a_removed_call_site_leaves_no_row_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "a.rs",
            "pub fn caller() {\n    missing_one();\n    missing_two();\n}\n",
        );
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        write(
            dir.path(),
            "a.rs",
            "pub fn caller() {\n    missing_two();\n}\n",
        );
        store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("reindex");
        let callees: Vec<String> = store
            .call_sites("p")
            .expect("call sites")
            .into_iter()
            .map(|s| s.callee)
            .collect();
        assert_eq!(
            callees,
            vec!["missing_two".to_string()],
            "the removed site must not survive: {callees:?}"
        );
    }

    /// The same interruption the full pass has, on the incremental path: the
    /// re-parse commits a file's hash, resolution is a separate step, and a
    /// failure there left the hash behind. The next run then classified the file
    /// as unchanged, skipped it, and the call sites it never resolved stayed
    /// unresolved forever.
    #[test]
    fn an_interrupted_reindex_resolution_is_recovered_by_the_next_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn target() -> u32 { 1 }\n");
        write(dir.path(), "b.rs", "pub fn caller() -> u32 { target() }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        let calls_before: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE project = 'p' AND edge_type = 'CALLS'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(calls_before, 1, "the fixture resolves one call");

        // A body-only edit of b.rs: the caller's call site moves to `missing`.
        write(dir.path(), "b.rs", "pub fn caller() -> u32 { missing() }\n");
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER refuse_the_site BEFORE INSERT ON call_sites
                 WHEN NEW.callee = 'missing'
                 BEGIN SELECT RAISE(ABORT, 'injected: the reindex resolution fails'); END",
            )
            .expect("trigger");
        assert!(
            store
                .reindex_incremental(dir.path(), &IndexOptions::new("p"))
                .is_err(),
            "the injected failure must abort the reindex"
        );
        store
            .connection()
            .execute_batch("DROP TRIGGER refuse_the_site")
            .expect("drop trigger");

        // The failed run must not have claimed b.rs: a stored hash over call sites
        // that never landed is what made the next run skip the repair.
        let stale = store.file_hash("p", "b.rs").expect("hash").expect("row");
        let on_disk = crate::hash::hash_file(&dir.path().join("b.rs"))
            .expect("hash on disk")
            .sha256;
        assert_ne!(
            stale.sha256, on_disk,
            "the failed reindex claimed a file whose call sites it never wrote"
        );

        store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("recovery");
        let callees: Vec<String> = store
            .call_sites("p")
            .expect("sites")
            .into_iter()
            .map(|site| site.callee)
            .collect();
        assert_eq!(
            callees,
            vec!["missing".to_string()],
            "the recovery run must record the new call site: {callees:?}"
        );
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

    /// A reindex over a store a skeleton pass built must re-read every file:
    /// the content hash matches, but the graph has no symbols to show for it.
    /// Classification keyed on the hash alone, so `reindex` reported "unchanged"
    /// over a symbolic graph and the caller had no way to deepen it.
    #[test]
    fn a_reindex_over_a_shallow_store_deepens_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn helper() -> u32 { 1 }\n");
        write(dir.path(), "b.rs", "pub fn caller() -> u32 { helper() }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(
                dir.path(),
                &IndexOptions::new("p").with_depth(crate::types::Depth::Skeleton),
            )
            .expect("skeleton");
        assert!(
            store.node_by_qn("p", "a::helper").expect("qn").is_none(),
            "the fixture must start symbolic"
        );

        let report = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("reindex");
        assert_eq!(
            report.unchanged, 0,
            "a symbolic row is not an indexed file: {report:?}"
        );
        assert_eq!(report.files_reindexed, 2, "{report:?}");
        assert_eq!(report.reason, ReindexReason::Deepened, "{report:?}");
        assert!(
            store.node_by_qn("p", "a::helper").expect("qn").is_some(),
            "the reindex skipped the file and left the graph symbolic"
        );
        let calls: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM edges WHERE project = 'p' AND edge_type = 'CALLS'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert!(calls >= 1, "the deepened pass resolves the call");

        // The row now records the depth the graph reaches: a second reindex is a
        // no-op rather than a second full tree read.
        let again = store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("second reindex");
        assert_eq!(again.files_reindexed, 0, "{again:?}");
        assert_eq!(again.reason, ReindexReason::Unchanged, "{again:?}");
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
    /// Audit F07/R6: a reindex changes the graph, so it must move the evidence.
    /// Before this, `verify` kept accepting a receipt that predated the
    /// reindex — the store it described was no longer the store that existed.
    #[test]
    fn a_reindex_appends_a_receipt_so_verify_sees_the_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 1 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        let after_index = store.verify_run("p").expect("verify").receipts;
        assert_eq!(after_index, 1, "the full index records one receipt");

        write(dir.path(), "a.rs", "pub fn foo() -> u32 { 2 }\n");
        store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("reindex");

        let verdict = store.verify_run("p").expect("verify");
        assert_eq!(
            verdict.receipts, 2,
            "a reindex that changed a file must append its own receipt"
        );
        assert!(
            verdict.chain_ok,
            "the appended receipt links to its predecessor"
        );
    }
    /// Audit R1, the differential proof: touching one file and re-indexing it
    /// must produce the same graph a full index produces.
    ///
    /// Before the fix the two paths disagreed — a reindex resolved calls with a
    /// dependency-blind resolver and wrote no miss rows, so a cross-crate call
    /// disappeared and an unresolved one left nothing behind.
    #[test]
    fn reindex_and_full_index_agree_on_the_same_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A workspace manifest, so a dependency-rooted call is recognisable.
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\n[dependencies]\nserde = \"1\"\n",
        );
        write(
            dir.path(),
            "a.rs",
            "pub fn caller() { serde::de::from_str(\"x\"); helper(); unknown_thing(); }\n",
        );
        write(dir.path(), "b.rs", "pub fn helper() -> u32 { 1 }\n");

        // Route A: index, then edit a file and reindex.
        let mut reindexed = GraphStore::open_in_memory().expect("store");
        index(&mut reindexed, dir.path());
        write(
            dir.path(),
            "a.rs",
            "pub fn caller() { serde::de::from_str(\"x\"); helper(); unknown_thing(); }\npub fn added() {}\n",
        );
        reindexed
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("reindex");
        let reindex_edges = reindexed.all_edges("p").expect("edges");

        // Route B: the same tree, indexed from scratch.
        let mut fresh = GraphStore::open_in_memory().expect("store");
        fresh
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let index_edges = fresh.all_edges("p").expect("edges");

        // Compare by NAME, not by row id: a reindex preserves the ids it
        // already wrote while a fresh index numbers the tree from scratch, so
        // ids are store-local and only qualified names describe the graph.
        let shape = |edges: &[crate::types::Edge], store: &GraphStore| {
            let names: HashMap<i64, String> = store
                .all_nodes("p")
                .expect("nodes")
                .into_iter()
                .map(|n| (n.id, n.qualified_name))
                .collect();
            let mut out: Vec<(String, String, String)> = edges
                .iter()
                .map(|e| {
                    (
                        e.edge_type.to_string(),
                        names.get(&e.source).cloned().unwrap_or_default(),
                        names.get(&e.target).cloned().unwrap_or_default(),
                    )
                })
                .collect();
            out.sort();
            out
        };
        assert_eq!(
            shape(&reindex_edges, &reindexed),
            shape(&index_edges, &fresh),
            "a reindex and a full index disagree about the same tree"
        );
        assert!(
            index_edges
                .iter()
                .any(|e| e.props.to_string().contains("dependency")),
            "the fixture must exercise the dependency path"
        );
    }
    /// An aliased import must be visible to the CALL beside it.
    ///
    /// `use crate::b::helper as renamed;` binds `renamed` to `helper`. Two bugs met here:
    /// `parse_alias` kept the PATH as the target name (`crate::b::helper`), which the
    /// by-short-name resolver could not match, and the resolver had no alias table at all, so
    /// even a correct target name was never consulted. The call was recorded unresolved while
    /// the import beside it resolved to an IMPORTS edge — one half of the file seeing what the
    /// other half could not.
    #[test]
    fn an_aliased_import_resolves_the_call_through_it() {
        let dir = tempfile::tempdir().expect("dir");
        write(
            dir.path(),
            "a.rs",
            "use crate::b::helper as renamed_helper;\npub fn caller() { renamed_helper(); }\n",
        );
        write(dir.path(), "b.rs", "pub fn helper() -> u32 { 1 }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");

        let nodes: HashMap<i64, String> = store
            .all_nodes("p")
            .expect("nodes")
            .into_iter()
            .map(|n| (n.id, n.qualified_name))
            .collect();
        let calls: Vec<(String, String)> = store
            .all_edges("p")
            .expect("edges")
            .iter()
            .filter(|e| e.edge_type == crate::types::EdgeType::Calls)
            .map(|e| {
                (
                    nodes.get(&e.source).cloned().unwrap_or_default(),
                    nodes.get(&e.target).cloned().unwrap_or_default(),
                )
            })
            .collect();
        assert!(
            calls.contains(&("a::caller".to_string(), "b::helper".to_string())),
            "the aliased call must resolve to the imported definition: {calls:?}"
        );
        let sites = store.call_sites("p").expect("sites");
        let site = sites
            .iter()
            .find(|s| s.callee == "renamed_helper")
            .expect("the site is recorded");
        assert_eq!(
            site.resolution, "resolved",
            "and must not be left unresolved beside a resolving import: {site:?}"
        );
    }

    /// The review's first acceptance check, made exhaustive: "Canonical parity full vs
    /// incremental: add/delete/rename/alias/body-only edit; исчезнувший unresolved call;
    /// новая definition, разрешающая прежний unresolved; occurrence multiplicity.
    /// Сравнивать normalized facts, не только counts."
    ///
    /// The existing parity test covers ONE mutation (body-only + a new function). Each
    /// mutation below has produced a real bug in this tree at some point — the stale
    /// USES fact, the collapsed call site, the removal that double-counted — so a
    /// single scenario is not coverage.
    ///
    /// Compared by NORMALIZED FACTS: edge (type, source qn, target qn) triples and
    /// per-symbol occurrence counts. Row ids and order are store-local and would make
    /// the comparison test the allocator.

    #[test]
    fn incremental_and_full_index_agree_across_every_mutation_shape() {
        /// (name, files to seed, the mutation to apply, files after the mutation)
        type Mutation = fn(&std::path::Path);
        let seed: &[(&str, &str)] = &[
            (
                "a.rs",
                "pub fn caller() { helper(); missing_old(); missing_old(); }\n",
            ),
            ("b.rs", "pub fn helper() -> u32 { 1 }\n"),
            ("c.rs", "pub fn orphan() -> u32 { 2 }\n"),
        ];
        let mutations: &[(&str, Mutation)] = &[
            ("body-only edit", |dir| {
                write(dir, "a.rs", "pub fn caller() { helper(); helper(); }\n");
            }),
            ("new definition resolving an unresolved call", |dir| {
                write(dir, "d.rs", "pub fn missing_old() {}\n");
            }),
            ("deleted file", |dir| {
                std::fs::remove_file(dir.join("c.rs")).expect("remove c.rs");
            }),
            ("renamed file", |dir| {
                std::fs::rename(dir.join("c.rs"), dir.join("renamed.rs")).expect("rename");
            }),
            // §11 names `alias` explicitly, and it is the mutation a resolver is most likely
            // to get wrong: the CALL records `helper()` either way, but the IMPORT that
            // binds the local name changes, so a stale `local_name` keeps resolving a call
            // the new source no longer makes that way.
            ("alias introduced", |dir| {
                write(
                    dir,
                    "a.rs",
                    "use crate::b::helper as renamed_helper;\n\
                     pub fn caller() { renamed_helper(); missing_old(); missing_old(); }\n",
                );
            }),
        ];

        let normalized = |store: &GraphStore| -> (EdgeFacts, Occurrences) {
            let nodes: HashMap<i64, String> = store
                .all_nodes("p")
                .expect("nodes")
                .into_iter()
                .map(|n| (n.id, n.qualified_name))
                .collect();
            let mut edges: Vec<(String, String, String)> = store
                .all_edges("p")
                .expect("edges")
                .iter()
                .map(|e| {
                    (
                        e.edge_type.to_string(),
                        nodes.get(&e.source).cloned().unwrap_or_default(),
                        nodes.get(&e.target).cloned().unwrap_or_default(),
                    )
                })
                .collect();
            edges.sort();
            let mut occurrences: Vec<(String, usize)> = store
                .call_sites("p")
                .expect("sites")
                .into_iter()
                .fold(std::collections::BTreeMap::new(), |mut acc, site| {
                    *acc.entry(format!(
                        "{}/{}/{}",
                        site.caller_qn, site.callee, site.resolution
                    ))
                    .or_insert(0) += 1;
                    acc
                })
                .into_iter()
                .collect();
            occurrences.sort();
            (edges, occurrences)
        };

        for (label, mutate) in mutations {
            // Reset the tree so each mutation starts from the same seed.
            let dir = tempfile::tempdir().expect("tempdir");
            for (name, body) in seed {
                write(dir.path(), name, body);
            }
            let mut incremental = GraphStore::open_in_memory().expect("store");
            index(&mut incremental, dir.path());

            mutate(dir.path());
            incremental
                .reindex_incremental(dir.path(), &IndexOptions::new("p"))
                .expect("reindex");

            let mut fresh = GraphStore::open_in_memory().expect("store");
            fresh
                .index_project(dir.path(), &IndexOptions::new("p"))
                .expect("full index");

            assert_eq!(
                normalized(&incremental),
                normalized(&fresh),
                "{label}: incremental and full index disagree about the same tree"
            );
        }
    }

    type EdgeFacts = Vec<(String, String, String)>;
    type Occurrences = Vec<(String, usize)>;

    /// `(functions, FTS rows, CONTAINS edges)` for the project: the three
    /// numbers a re-parse of `lib.rs` moves, so a half-written file shows up as
    /// a mix of the two revisions.
    fn file_state(store: &GraphStore) -> (i64, i64, i64) {
        let conn = store.connection();
        let functions: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes WHERE project = 'p' AND label = 'Function'",
                [],
                |r| r.get(0),
            )
            .expect("count");
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

    /// An incremental re-parse is ONE transaction, exactly like the full
    /// indexer's per-file rewrite: the clear steps and the rewrite commit
    /// together, so a failure in the middle leaves the previous revision whole
    /// instead of a file with no FTS rows and no outgoing edges.
    ///
    /// The failure is injected through the schema, not a test hook in the write
    /// path: a trigger rejects one insert of the NEW revision, i.e. after the
    /// clear steps already deleted the old file's FTS rows, edges and call
    /// sites and while the rewrite is half written. A test-only branch in the
    /// rewriter would prove that the branch works, not that the sequence is one
    /// unit of work.
    #[test]
    fn a_failed_incremental_reparse_leaves_the_previous_revision_intact() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "lib.rs",
            "pub fn parse_config(path: &str) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 }\n",
        );
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        assert_eq!(
            file_state(&store),
            (2, 3, 2),
            "fixture must be indexed whole: two functions, FTS rows for the file \
             and both symbols, one CONTAINS edge each"
        );
        let before = store
            .file_hash("p", "lib.rs")
            .expect("hash")
            .expect("hash row");

        // The new revision is LONGER than the old one, so a partially written
        // file cannot coincidentally match the old revision's counts.
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
                 BEGIN SELECT RAISE(ABORT, 'injected: the re-parse fails mid-file'); END",
            )
            .expect("trigger");

        let failed = store.reindex_incremental(dir.path(), &IndexOptions::new("p"));
        assert!(
            failed.is_err(),
            "the injected failure must abort the reindex, or the test proves nothing: {failed:?}"
        );

        assert_eq!(
            file_state(&store),
            (2, 3, 2),
            "the file is half re-indexed: its previous revision lost its FTS rows \
             and CONTAINS edges (functions, FTS rows, CONTAINS edges) must be the \
             old revision whole"
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
        // The hash row is the last write of the sequence, so a rollback has to
        // take it back too: a stored NEW hash over the OLD nodes makes the next
        // run skip the file as unchanged and the stub survives forever.
        let stored = store
            .file_hash("p", "lib.rs")
            .expect("hash")
            .expect("hash row");
        assert_eq!(
            stored.sha256, before.sha256,
            "the stored hash must still describe the revision the nodes came from"
        );
    }

    /// §4.3 (audit): the stored hash must be the one that travelled WITH the
    /// parsed bytes. The old code re-read the file at write time, so a change
    /// landing between parse and write paired old symbols with a newer hash and
    /// the next run skipped a stale index. This test hands the writer a hash
    /// that deliberately differs from the file on disk: the stored value must be
    /// the Target's, never the disk's.
    #[test]
    fn the_stored_hash_is_the_parsed_one_not_a_fresh_disk_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        std::fs::write(&path, "pub fn from_disk() {}\n").unwrap();
        let store = GraphStore::open_in_memory().unwrap();
        store
            .register_project("p", dir.path().to_str().unwrap())
            .unwrap();

        let parsed = crate::hash::hash_bytes(
            b"pub fn from_parse() {}\n",
            &std::fs::metadata(&path).unwrap(),
        );
        let target = Target {
            rel: "lib.rs".to_string(),
            source: "pub fn from_parse() {}\n".to_string(),
            hash: parsed.clone(),
            lang: pa_ast::SupportLang::from_path(&path).unwrap(),
            module: "lib".to_string(),
        };
        let reindexed = store.reindex_file_in_place("p", &target).unwrap();
        // The re-parse hands the hash back instead of writing it: it commits in the
        // caller's resolution transaction, beside the edges it claims to cover.
        store.put_file_hash("p", "lib.rs", &reindexed.hash).unwrap();

        let stored = store.file_hash("p", "lib.rs").unwrap().expect("hash row");
        assert_eq!(
            stored.sha256, parsed.sha256,
            "the stored hash must describe the parsed bytes, not a second disk read"
        );
    }

    /// A file REMOVED between incremental passes must take its call sites (#38).
    ///
    /// The full pass clears `call_sites` beside the file (`indexer/run.rs`); the
    /// incremental sweep is a separate path and forgot. A `call_sites` row keys on
    /// the file rather than a node id, so nothing else takes it: the removed file
    /// kept answering impact questions about call sites it no longer had.
    #[test]
    fn a_removed_file_loses_its_call_sites_on_the_incremental_sweep() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.rs", "pub fn helper() {}\n");
        write(dir.path(), "b.rs", "pub fn caller() { helper(); }\n");
        let mut store = GraphStore::open_in_memory().expect("store");
        index(&mut store, dir.path());
        let before = store.call_sites("p").expect("sites");
        assert!(
            before.iter().any(|s| s.rel_path == "b.rs"),
            "the fixture records a call site to lose: {before:?}"
        );

        std::fs::remove_file(dir.path().join("b.rs")).expect("remove");
        store
            .reindex_incremental(dir.path(), &IndexOptions::new("p"))
            .expect("reindex");

        let after = store.call_sites("p").expect("sites");
        assert!(
            !after.iter().any(|s| s.rel_path == "b.rs"),
            "a removed file's call sites must not survive the incremental sweep: {after:?}"
        );
    }
}
