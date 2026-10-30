//! Call-edge resolution: the confidence ladder and the miss reasons.

use std::collections::HashMap;

use serde_json::json;

use crate::error::Result;
use crate::extract::{CallSite, ImportDef};
use crate::misses::MissReason;
use crate::store::GraphStore;
use crate::types::CallStrategy;

/// Call-edge resolver implementing the confidence ladder.
pub(crate) struct Resolver {
    by_qn: HashMap<String, i64>,
    by_name: HashMap<String, Vec<String>>,
    /// External crates the workspace depends on, so a miss can say "this is a
    /// dependency call" instead of guessing at the leaf name. Built once per run.
    deps: crate::deps::DependencyCrates,
    /// Per-file import aliases: file path → (local name → definition name).
    ///
    /// A call through an alias (`use a::helper as h; h()`) names `h`, and `by_name` holds
    /// definition names only, so without this table every aliased call was recorded
    /// unresolved while the import beside it resolved to an edge. Keyed by the CALLER's
    /// file: an alias is file-local and must not wire an unrelated file's call.
    alias_targets: HashMap<String, HashMap<String, String>>,
}

impl Resolver {
    /// A resolver that knows no dependency crates: every miss falls back to the
    /// std/trait corpora.
    ///
    /// Both WRITE paths use [`Self::with_deps`] — the full index and the
    /// incremental reindex alike — because a dependency-blind resolver makes the
    /// two disagree about the same tree (audit R1). This constructor is for
    /// callers with no root to read manifests from, and for tests.
    #[cfg(test)]
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
            alias_targets: HashMap::new(),
            deps,
        }
    }

    /// Record one file's import aliases so `resolve_call` can see through them.
    ///
    /// Takes the imports that belong to `rel_path` — the file the caller lives in, which is
    /// a fact the indexer already holds. Deriving a module path from the qualified name was
    /// a guess that held only for extensionless stems — and keeps only the imports that
    /// actually alias something: `use a::b;` gives `("b", "b")`, which is not an alias and
    /// would add nothing to look up.
    pub(crate) fn note_imports(&mut self, rel_path: &str, imports: &[crate::extract::ImportDef]) {
        let aliases: HashMap<String, String> = imports
            .iter()
            .filter(|import| import.local_name != import.target_name)
            .map(|import| (import.local_name.clone(), import.target_name.clone()))
            .collect();
        if !aliases.is_empty() {
            self.alias_targets
                .entry(rel_path.to_string())
                .or_default()
                .extend(aliases);
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
        // No "the name exists but we refused the shape" arm here: `resolve_call`
        // returns `Some` for every callee that has a candidate — including a
        // recursive one, which now resolves to `Recursive` instead of falling
        // through to a miss — so a known name never reaches this function. The
        // arm that used to sit here was reachable only through that recursion
        // defect, and `ShapeNotModelled` is still produced below for a callee this
        // resolver cannot name at all.
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
        rel_path: &str,
        call: &CallSite,
    ) -> Option<(i64, i64, serde_json::Value)> {
        let source = *self.by_qn.get(caller_qn)?;
        // An alias is a LOCAL name: `use crate::b::helper as renamed` means `renamed()`
        // names `helper`, and `by_name` holds definition names only. Without this, every call
        // through an alias was recorded unresolved even though the import above it bound the
        // name — the import resolved to an IMPORTS edge while the call beside it did not.
        //
        // Scoped to the caller's OWN file: an alias does not leak into another module, and
        // resolving it globally would wire `renamed()` in an unrelated file to this import.
        // `alias_targets` holds only imports whose callers already passed both lookups, so a
        // file that is still pending contributes nothing rather than a guess.
        let name = self
            .alias_targets
            .get(rel_path)
            .and_then(|aliases| aliases.get(&call.callee))
            .cloned()
            .unwrap_or_else(|| call.callee.clone());
        let candidates = self.by_name.get(&name)?;
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
        // A self-call has a fully known target: it is the caller. Returning
        // `None` here sent every direct recursion down the MISS path, where it
        // was recorded as `unresolved / shape_not_modelled` — indistinguishable
        // from a name the resolver could not handle. The edge is still a
        // self-loop (that is what a recursive target is); only the strategy and
        // the confidence change, so a reader can tell the three cases apart:
        // `recursive` (known, here), `dependency` (known, outside the tree),
        // unresolved (no idea).
        let (strategy, confidence) = if target == source {
            (
                CallStrategy::Recursive,
                CallStrategy::Recursive.base_confidence(),
            )
        } else {
            (strategy, CallStrategy::base_confidence(strategy))
        };
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
        // Several definitions share the imported name: the module path the import
        // actually names is the only discriminator available here, so prefer a
        // candidate that sits in it. Without this the FIRST candidate by qualified
        // name won — `import foo from b` could wire to `a::foo` — while still
        // claiming ImportMap's 0.95, i.e. an unmeasured guess dressed as the
        // strongest evidence the resolver emits.
        //
        // When the module cannot separate them (no candidate's qname contains it)
        // the choice stays a guess, so it must not keep 0.95: the confidence drops
        // to `Suffix`'s level and the strategy says `ambiguous`, which is what a
        // caller filtering by confidence needs to see.
        let in_module = |qn: &&String| qn.contains(&import.module);
        let (target_qn, confident) = match candidates.iter().find(in_module) {
            Some(found) => (found, true),
            None => (candidates.first()?, candidates.len() == 1),
        };
        let target = *self.by_qn.get(target_qn)?;
        let strategy = if confident {
            CallStrategy::ImportMap
        } else {
            CallStrategy::Suffix
        };
        Some((
            target,
            json!({
                "local_name": import.local_name,
                "module": import.module,
                "confidence": strategy.base_confidence(),
                "strategy": strategy.as_str(),
                "ambiguous_candidates": if confident { 0 } else { candidates.len() },
            }),
        ))
    }
}

impl GraphStore {
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
