//! The two composed builders: reverse impact and the bounded evidence pack.
//!
//! Both join a `trace` walk with the recorded call sites, so they live together.

use crate::tools::graph::reports::{
    EvidencePack, ImpactCaller, ImpactCandidate, ImpactEnvelope, ImpactReport, PackCoverage,
    PackLink, PackOmissions, PackSymbol, GUESS_CONFIDENCE,
};
use pa_graph::GraphStore;
/// Assemble an [`ImpactReport`] for `symbol`.
///
/// Two sources, because one cannot see the other: resolved callers come from a `trace`
/// walk (edges), unresolved mentions from `call_sites` (every site, resolved or not).
pub(crate) fn build_impact(
    store: &GraphStore,
    project: &str,
    symbol: &str,
    depth: usize,
) -> anyhow::Result<ImpactReport> {
    // The leaf name is what an unresolved site records as its `callee`: the resolver stores
    // the name as written, truncated to its last segment.
    let leaf = symbol.rsplit("::").next().unwrap_or(symbol);
    let traced = store
        .trace(
            &pa_graph::TraceOptions::new(project, symbol)
                .with_direction(pa_graph::Direction::In)
                .with_depth(depth),
        )
        .ok();
    let callers: Vec<ImpactCaller> = traced
        .as_ref()
        .map(|result| {
            result
                .steps
                .iter()
                .map(|step| ImpactCaller {
                    qualified_name: step.qualified_name.clone(),
                    file_path: step.file_path.clone(),
                    start_line: step.start_line,
                    depth: step.depth,
                    confidence: step.confidence,
                    strategy: step.strategy.clone(),
                    witness: step.witness.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    // One leaf name, one predicate: loading the whole table read 45 760 rows for `pa-core`
    // alone and filtered them in Rust (review #25). `project_unresolved_sites` still needs a
    // count, which SQLite answers without materializing the rows.
    let sites = store.call_sites_for_callee(project, Some(leaf))?;
    // TWO kinds of invisible caller, not one. A site the resolver could not place
    // (`unresolved`) has no edge at all; a site it placed by a NAME GUESS (`suffix`, 0.55)
    // has an edge to SOME definition with that name, which may be the wrong one — so a
    // change to THIS symbol may silently miss it. The review names both: "различить
    // exact/scoped resolution и name guess". Counting only `unresolved` would report a
    // guess-backed edge as if it were a fact.
    let unresolved_candidates: Vec<ImpactCandidate> = sites
        .iter()
        .filter(|site| site.callee == leaf)
        .filter(|site| {
            site.resolution == "unresolved"
                || (site.resolution == "resolved" && site.confidence <= GUESS_CONFIDENCE)
        })
        .map(|site| ImpactCandidate {
            caller_qn: site.caller_qn.clone(),
            rel_path: site.rel_path.clone(),
            line: site.line,
            reason: if site.resolution == "unresolved" {
                site.reason.clone()
            } else {
                format!("name guess (confidence {:.2})", site.confidence)
            },
        })
        .collect();
    let project_unresolved_sites = store.unresolved_call_site_count(project)?;

    Ok(ImpactReport {
        symbol: symbol.to_string(),
        envelope: ImpactEnvelope {
            resolved_callers: callers.len(),
            unresolved_mentions: unresolved_candidates.len(),
            edges_examined: traced.as_ref().map_or(0, |result| result.edges_examined),
            truncated: traced.as_ref().is_some_and(|result| result.truncated),
            project_unresolved_sites,
        },
        callers,
        unresolved_candidates,
    })
}

/// Assemble an [`EvidencePack`] for `query`.
///
/// Pure over the store plus the files it reads, so the pack is reproducible for a
/// given index and tree. Each stage records what it kept out, because a pack that
/// silently truncates reads as a complete answer.
pub(crate) fn build_evidence_pack(
    store: &GraphStore,
    project: &str,
    root: &std::path::Path,
    query: &str,
    budget: usize,
) -> anyhow::Result<EvidencePack> {
    // Rank a generous candidate set, then cut: `search` already fuses channels, so
    // the cut is the only place a symbol is lost, and the loss is counted.
    let candidates = budget.saturating_mul(4);
    let hits = store.search(
        query,
        &pa_graph::SearchOptions::new(project).with_limit(candidates),
    )?;
    let kept: Vec<&pa_graph::SearchHit> = hits.iter().take(budget).collect();
    let omitted_symbols = hits.len().saturating_sub(kept.len());

    let mut symbols = Vec::with_capacity(kept.len());
    let mut unreadable = 0usize;
    for (rank, hit) in kept.iter().enumerate() {
        let placement = store.node_by_id(hit.id).ok().flatten();
        let freshness = current_placement(root, &hit.file_path, &hit.name);
        // Gone or unreadable: either way the pack cannot vouch for the placement.
        let (current_lines, stale) = if let Ok(Some(current)) = freshness {
            (
                Some(current),
                current.0 != hit.start_line || current.1 != hit.end_line,
            )
        } else {
            unreadable += 1;
            (None, true)
        };
        symbols.push(PackSymbol {
            qualified_name: hit.qualified_name.clone(),
            name: hit.name.clone(),
            label: hit.label.as_str().to_string(),
            file_path: hit.file_path.clone(),
            start_line: hit.start_line,
            end_line: hit.end_line,
            current_lines,
            surface_sha: placement.and_then(|node| {
                node.props
                    .get("surface_sha")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }),
            stale,
            reason: if rank == 0 {
                "top hit".to_string()
            } else {
                format!("hit #{}", rank + 1)
            },
        });
    }

    // Links between symbols that are BOTH in the pack: a relation to something the
    // reader cannot see is noise, and counting it would inflate the pack's apparent
    // richness.
    let mut links = Vec::new();
    let mut links_outside = 0usize;
    let in_pack: std::collections::HashSet<&str> = symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    for symbol in &symbols {
        let Ok(Some(node)) = store.node_by_qn(project, &symbol.qualified_name) else {
            continue;
        };
        for edge in store.out_edges(node.id, None)? {
            let Some(target) = store.node_by_id(edge.target)? else {
                continue;
            };
            if in_pack.contains(target.qualified_name.as_str()) {
                links.push(PackLink {
                    from: symbol.qualified_name.clone(),
                    to: target.qualified_name,
                    edge_type: edge.edge_type.as_str().to_string(),
                    confidence: edge
                        .props
                        .get("confidence")
                        .and_then(serde_json::Value::as_f64)
                        .map(|c| c as f32),
                });
            } else {
                links_outside += 1;
            }
        }
    }

    let coverage = store.coverage(project)?;
    Ok(EvidencePack {
        query: query.to_string(),
        symbols,
        links,
        coverage: PackCoverage {
            sites: coverage.call_sites,
            resolved: coverage.call_sites_resolved,
            unresolved: coverage.call_sites_unresolved,
        },
        omitted: PackOmissions {
            symbols_below_cut: omitted_symbols,
            links_outside_pack: links_outside,
            unreadable_files: unreadable,
        },
    })
}

/// Where `name` is defined in `rel_path` RIGHT NOW, by scanning the file.
///
/// A cheap scan rather than a parse: the pack needs "does the index still agree
/// with the disk", and a line-number mismatch is enough to say no. `Ok(None)` when
/// the file is gone; `Err` when it exists but cannot be read.
pub(crate) fn current_placement(
    root: &std::path::Path,
    rel_path: &str,
    name: &str,
) -> Result<Option<(u32, u32)>, ()> {
    let path = root.join(rel_path);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return if path.exists() { Err(()) } else { Ok(None) };
    };
    let needle = format!("fn {name}");
    let mut start = None;
    for (index, line) in text.lines().enumerate() {
        if line.contains(&needle) || line.contains(&format!(" {name}(")) {
            start = Some(index as u32 + 1);
            break;
        }
    }
    // No match: the symbol the index claims is not in the file any more.
    Ok(start.map(|line| (line, line)))
}

/// Project name a store rooted at `root` indexes under when no collision is in
/// play (its final path component, or `"project"` when the root has none).
pub(crate) fn project_name(root: &std::path::Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "project".to_string(), str::to_string)
}

/// The project key a store rooted at `root` indexes under.
///
/// The key is the directory's basename, QUALIFIED by a short digest of the
/// canonical root **when the basename alone is ambiguous**. Two checkouts named
/// `service` (`/a/service`, `/b/service`) used to share one key: the write path
/// refused the second (`register_project_at`), which turned "index my second
/// checkout" into a hard error and left a shared-database user with only one
/// indexable tree per basename. The qualification keeps the readable name for
/// the ordinary case (one root per basename in a database) and only forks when
/// the tree actually differs — which the store can only answer by looking, so
/// the resolution takes the store.
///
/// The digest is over the CANONICAL path, so a symlinked spelling of the same
/// tree keeps one key (the same rule `register_project_at` compares by).
pub fn project_key_in(store: &GraphStore, root: &std::path::Path) -> anyhow::Result<String> {
    let base = root
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "project".to_string(), str::to_string);
    let Some(stored) = store.project_root(&base)? else {
        // Nobody has claimed the plain name: use it.
        return Ok(base);
    };
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let same = stored == root.to_string_lossy()
        || std::fs::canonicalize(&stored).is_ok_and(|theirs| theirs == canonical);
    if same {
        return Ok(base);
    }
    Ok(format!("{base}#{}", short_path_digest(&canonical)))
}

/// Eight hex digits over the canonical root path: enough to make a basename
/// collision distinct inside one database, short enough to read in a report
/// line.
///
/// It reuses the graph's own content hash rather than adding a second hasher,
/// so the digest is stable across platforms and versions the same way the file
/// hashes are.
pub(crate) fn short_path_digest(canonical: &std::path::Path) -> String {
    let bytes = canonical.to_string_lossy();
    let meta = std::fs::metadata(canonical).unwrap_or_else(|_| std::fs::metadata(".").unwrap());
    let hash = pa_graph::hash::hash_bytes(bytes.as_bytes(), &meta);
    hash.sha256.chars().take(8).collect()
}
