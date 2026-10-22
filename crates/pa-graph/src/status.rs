//! The graph's own status: what it holds, how fresh it is, and what it cannot see.
//!
//! An agent that asks a code question needs to know three things beyond the
//! answer: whether the answer is current, whether it is complete, and what to do
//! next. Without this the tool fails open — a store that is 21 hours stale
//! answers as confidently as a fresh one. That is the measured incident this
//! module exists to prevent: 203 of 2,034 indexed files already differed from
//! their stored hash and no action said so.
//!
//! The probe is cheap enough to run on demand: `stat` every indexed file
//! (measured 15 ms for 2,034 files) and content-hash only the ones whose
//! `(mtime_ns, size)` changed (measured 29 ms for 211 files).

use std::path::Path;

use crate::error::Result;
use crate::indexer::Depth;
use crate::misses::MissReason;
use crate::store::GraphStore;

/// How current the store is with respect to the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// No indexed file changed on disk.
    Fresh,
    /// `changed` indexed files were edited since the run that stored them.
    Stale {
        /// Number of files whose content no longer matches the stored hash.
        changed: usize,
    },
    /// The store was built at an older commit. `commits` is how many commits
    /// behind HEAD it is.
    ///
    /// This is the drift neither a stat sweep nor `git status` can see: the
    /// working tree is clean, every changed file is committed, and the store
    /// simply still holds the older content. Measured on this repository while
    /// the graph was 5+ commits behind: `git status` reported 0 entries and 213
    /// of 2,034 indexed files differed from their stored hashes.
    BehindHead {
        /// Number of commits between the indexed commit and HEAD.
        commits: usize,
    },
    /// Stored paths were renamed on disk while their content stayed the same.
    Renamed {
        /// Number of paired renames.
        count: usize,
    },
    /// The project has never been indexed.
    Unindexed,
    /// The working tree could not be read (deleted root, permissions).
    Unknown {
        /// Why the probe could not run.
        reason: String,
    },
}

impl Freshness {
    /// The fragment used in the status line.
    #[must_use]
    pub fn line_suffix(&self) -> String {
        match self {
            Self::Stale { changed } => format!("stale {changed}"),
            Self::BehindHead { commits } => format!("behind {commits}"),
            Self::Renamed { count } => format!("{count} renamed"),
            other => other.as_str().to_string(),
        }
    }

    /// Stable lowercase wire name.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale { .. } => "stale",
            Self::BehindHead { .. } => "behind",
            Self::Renamed { .. } => "renamed",
            Self::Unindexed => "unindexed",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// The one concrete thing the caller should do next, if anything.
    #[must_use]
    pub fn next_step(&self) -> Option<String> {
        match self {
            Self::Fresh => None,
            Self::Stale { changed } => Some(format!("reindex ({changed} file(s) changed)")),
            Self::BehindHead { commits } => Some(format!("reindex ({commits} commit(s) behind)")),
            Self::Renamed { count } => Some(format!("reindex ({count} rename(s) to reattach)")),
            Self::Unindexed => Some("index".to_string()),
            Self::Unknown { .. } => {
                Some("run `status` after checking the project path".to_string())
            }
        }
    }
}

/// What the store holds and what it could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// Indexed files.
    pub files: usize,
    /// Files the parser reported as partially understood.
    pub partial: usize,
    /// Files the parser could not use at all.
    pub unusable: usize,
    /// Files skipped before parsing (unsupported language, oversized).
    pub skipped: usize,
}

impl Coverage {
    /// Whether every indexed file parsed cleanly.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.partial == 0 && self.unusable == 0 && self.skipped == 0
    }
}

/// Unresolved call sites, counted per reason.
///
/// The store keeps one `USAGE` edge per miss with its reason, so a count is a
/// query away; the split is the point. A single "18,206 unresolved" number told
/// a reader nothing about whether the resolver was broken or the code called
/// `clone`, and that ambiguity is why the misses were classified in the first
/// place. Only non-zero reasons are rendered: the line rides every graph
/// answer, so it must not grow a fixed tail of zeros.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Misses {
    /// The callee is a well-known std/trait method or a known macro.
    pub external_or_std: usize,
    /// A bare lowercase name whose receiver type extraction does not record.
    pub type_unknown: usize,
    /// Reserved bucket: extraction missed a definition that should exist.
    pub not_extracted: usize,
    /// The name exists but the call shape is not modelled.
    pub shape_not_modelled: usize,
    /// The fallback: the name looks genuinely absent.
    pub no_definition_with_that_name: usize,
}

impl Misses {
    /// Total unresolved call sites.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.external_or_std
            + self.type_unknown
            + self.not_extracted
            + self.shape_not_modelled
            + self.no_definition_with_that_name
    }

    /// `(wire name, count)` for every reason with a non-zero count, in the
    /// order [`MissReason::ALL`] fixes.
    #[must_use]
    pub fn present(&self) -> Vec<(&'static str, usize)> {
        MissReason::ALL
            .iter()
            .map(|reason| (*reason, self.count(*reason)))
            .filter(|(_, count)| *count > 0)
            .map(|(reason, count)| (reason.as_str(), count))
            .collect()
    }

    /// Add `count` to `reason`'s bucket.
    fn add(&mut self, reason: MissReason, count: usize) {
        let slot = match reason {
            MissReason::ExternalOrStd => &mut self.external_or_std,
            MissReason::TypeUnknown => &mut self.type_unknown,
            MissReason::NotExtracted => &mut self.not_extracted,
            MissReason::ShapeNotModelled => &mut self.shape_not_modelled,
            MissReason::NoDefinitionWithThatName => &mut self.no_definition_with_that_name,
        };
        *slot += count;
    }

    /// One reason's count.
    fn count(&self, reason: MissReason) -> usize {
        match reason {
            MissReason::ExternalOrStd => self.external_or_std,
            MissReason::TypeUnknown => self.type_unknown,
            MissReason::NotExtracted => self.not_extracted,
            MissReason::ShapeNotModelled => self.shape_not_modelled,
            MissReason::NoDefinitionWithThatName => self.no_definition_with_that_name,
        }
    }
}

/// A project's status, as reported to an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Project name.
    pub project: String,
    /// Freshness of the store relative to the working tree.
    pub freshness: Freshness,
    /// Number of completed index runs in the receipt chain.
    pub runs: usize,
    /// Whether that chain still verifies.
    pub chain_ok: bool,
    /// What the index could and could not read.
    pub coverage: Coverage,
    /// How deep the stored graph is, inferred from what it holds.
    pub depth: Depth,
    /// Unresolved call sites, split by reason.
    pub misses: Misses,
}

impl Report {
    /// One line an agent can read without parsing JSON.
    ///
    /// Shape: `[graph: <freshness> | <files> files, <parse honesty> | next: <step>]`.
    /// Bounded by construction: the numbers are the only variable part.
    #[must_use]
    pub fn line(&self) -> String {
        use std::fmt::Write as _;
        let mut out = format!(
            "[graph: {} | {} files",
            self.freshness.line_suffix(),
            self.coverage.files,
        );
        if !self.coverage.is_complete() {
            let Coverage {
                partial,
                unusable,
                skipped,
                ..
            } = &self.coverage;
            let _ = write!(
                out,
                ", {partial} partial, {unusable} unusable, {skipped} skipped"
            );
        }
        if self.depth != Depth::Full {
            let _ = write!(out, ", {} depth", self.depth.as_str());
        }
        // The split, not just the total: "18,206 unresolved" could not say
        // whether the resolver was broken or the code called `clone`.
        if self.misses.total() > 0 {
            let present: Vec<String> = self
                .misses
                .present()
                .into_iter()
                .map(|(reason, count)| format!("{count} {reason}"))
                .collect();
            let _ = write!(
                out,
                ", {} unresolved ({})",
                self.misses.total(),
                present.join(", "),
            );
        }
        if !self.chain_ok {
            out.push_str(", receipts BROKEN");
        }
        if let Some(next) = self.freshness.next_step() {
            let _ = write!(out, " | next: {next}");
        }
        out.push(']');
        out
    }
}

/// Whether the store is current, without touching stored hashes.
///
/// Compares `(mtime_ns, size)` only: this is the cheap probe. `changed` counts
/// files whose stat differs, which is an upper bound on real content changes.
///
/// # Errors
/// Propagates SQLite errors.
pub fn probe_freshness(store: &GraphStore, project: &str, root: &Path) -> Result<Freshness> {
    let hashes = store.all_file_hashes(project)?;
    if hashes.is_empty() {
        return Ok(Freshness::Unindexed);
    }
    // Committed drift first: it is invisible to both a stat sweep and
    // `git status`, and it is the usual shape of a stale graph.
    if let Some(behind) = commits_behind(store, project, root) {
        if behind > 0 {
            return Ok(Freshness::BehindHead { commits: behind });
        }
    }
    // A renamed file is stale in a way a stat count cannot express: the number is
    // small but the answer is wrong for every caller of the old path.
    if let Ok(probe) = crate::probe::probe(store, project, root) {
        if let Some(renames) = probe
            .findings
            .iter()
            .filter(|f| matches!(f, crate::probe::Finding::Renamed { .. }))
            .count()
            .checked_sub(0)
            .filter(|n| *n > 0)
        {
            return Ok(Freshness::Renamed { count: renames });
        }
    }
    let mut changed = 0usize;
    for (rel, stored) in &hashes {
        match std::fs::metadata(root.join(rel)) {
            Ok(meta) => {
                let mtime_ns = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
                let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
                if mtime_ns != stored.mtime_ns || size != stored.size {
                    changed += 1;
                }
            }
            Err(_) => changed += 1,
        }
    }
    if changed == 0 {
        Ok(Freshness::Fresh)
    } else {
        Ok(Freshness::Stale { changed })
    }
}

/// Infer how deep the stored graph is from what it holds.
///
/// The store records no depth of its own, and adding a column would let the two
/// disagree. Reading it back from the data cannot: a graph with no symbol nodes
/// is a skeleton, one with symbols but no edges is a symbols pass, and anything
/// else is full.
///
/// # Errors
/// Propagates SQLite errors.
pub fn infer_depth(store: &GraphStore, project: &str) -> Result<Depth> {
    let nodes = store.all_nodes(project)?;
    if !nodes.iter().any(|n| n.label != crate::types::Label::File) {
        return Ok(Depth::Skeleton);
    }
    if store.edge_count(project)? == 0 {
        return Ok(Depth::Symbols);
    }
    Ok(Depth::Full)
}

/// How many commits HEAD is ahead of the commit the store was built at.
///
/// `None` when either side is unknown (not a git repo, no recorded commit, or
/// git unavailable) — the caller then falls back to the stat sweep, never to a
/// false "fresh".
fn commits_behind(store: &GraphStore, project: &str, root: &Path) -> Option<usize> {
    let indexed = store.indexed_head(project).ok().flatten()?;
    let head = crate::changes::run_git(root, &["rev-parse", "HEAD"]).ok()?;
    let head = head.trim();
    if head.is_empty() || head == indexed {
        return Some(0);
    }
    let range = format!("{indexed}..HEAD");
    let count = crate::changes::run_git(root, &["rev-list", "--count", &range]).ok()?;
    count.trim().parse().ok()
}

/// How long a computed status is reused before the probe runs again.
///
/// A status line rides **every** graph answer, so its cost is a per-call cost.
/// Measured on this repository: the full probe is 77 ms in release and the rest of
/// the report pushes it to 255 ms, which is more than the query it annotates.
/// Within one turn the answer cannot change (the agent is not editing files and
/// re-indexing in the same millisecond), so a short memo makes the common case
/// free and keeps the honest answer one call away.
pub const STATUS_TTL_MS: u64 = 2_000;

/// `(project, ms_since_epoch, report)` — one entry is enough: a session works on
/// one project at a time, and a second project simply replaces the entry.
static LAST_REPORT: std::sync::Mutex<Option<(String, u64, Report)>> = std::sync::Mutex::new(None);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// [`report`] memoised for [`STATUS_TTL_MS`].
///
/// A poisoned lock is treated as a miss: a status line must never turn an answer
/// into an error.
///
/// # Errors
/// Propagates SQLite errors from the underlying probe.
pub fn cached_report(store: &GraphStore, project: &str, root: &Path) -> Result<Report> {
    let now = now_ms();
    if let Ok(guard) = LAST_REPORT.lock() {
        if let Some((cached_project, at, cached)) = guard.as_ref() {
            if cached_project == project && now.saturating_sub(*at) < STATUS_TTL_MS {
                return Ok(cached.clone());
            }
        }
    }
    let fresh = report(store, project, root)?;
    if let Ok(mut guard) = LAST_REPORT.lock() {
        *guard = Some((project.to_string(), now, fresh.clone()));
    }
    Ok(fresh)
}

/// Build a full report for `project` rooted at `root`.
///
/// # Errors
/// Propagates SQLite errors.
pub fn report(store: &GraphStore, project: &str, root: &Path) -> Result<Report> {
    let freshness = probe_freshness(store, project, root)?;
    let verdict = store.verify_run(project)?;
    let rows = store.coverage_rows(project)?;
    let mut coverage = Coverage {
        files: store.file_count(project)?,
        partial: 0,
        unusable: 0,
        skipped: 0,
    };
    for (_, kind, _) in &rows {
        match kind.as_str() {
            "parse_partial" => coverage.partial += 1,
            "parse_unusable" => coverage.unusable += 1,
            "skipped" => coverage.skipped += 1,
            _ => {}
        }
    }
    Ok(Report {
        project: project.to_string(),
        freshness,
        runs: verdict.receipts,
        chain_ok: verdict.chain_ok,
        coverage,
        depth: infer_depth(store, project)?,
        misses: misses(store, project)?,
    })
}

/// Count the unresolved call sites of `project`, split by stored reason.
///
/// One grouped query over the `USAGE` edges: the reason is already a stored
/// property, so this adds no scan of the tree. An unrecognised stored reason
/// (a store written by a newer indexer) counts toward the total under the
/// fallback rather than being dropped — a miss that happened is still a miss.
///
/// # Errors
/// Propagates SQLite errors.
pub fn misses(store: &GraphStore, project: &str) -> Result<Misses> {
    let mut stmt = store.connection().prepare(
        "SELECT coalesce(json_extract(props_json, '$.reason'), ''),
                count(*)
         FROM edges
         WHERE project = ?1 AND edge_type = 'USAGE'
         GROUP BY 1",
    )?;
    let rows = stmt.query_map([project], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut misses = Misses::default();
    for row in rows {
        let (stored, count) = row?;
        let reason = MissReason::from_str(&stored).unwrap_or(MissReason::NoDefinitionWithThatName);
        misses.add(reason, usize::try_from(count).unwrap_or(usize::MAX));
    }
    Ok(misses)
}

#[cfg(test)]
mod tests {
    use crate::indexer::IndexOptions;

    /// The drift a stat sweep and `git status` both miss: the working tree is
    /// clean, every change is committed, and the store still holds older
    /// content. Measured on this repository while the graph was 5+ commits
    /// behind: `git status` reported 0 entries, 213 of 2,034 stored hashes
    /// differed, and nothing said so.
    #[test]
    fn a_store_behind_head_reports_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .expect("git")
        };
        if !run(&["init"]).status.success() {
            return;
        }
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.rs"), "pub fn one() {}\n").expect("a.rs");
        run(&["add", "."]);
        run(&["commit", "-m", "one"]);
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        let fresh = crate::status::probe_freshness(&store, "p", dir.path()).expect("probe");
        assert_eq!(
            fresh,
            crate::status::Freshness::Fresh,
            "clean tree is fresh"
        );
        std::fs::write(dir.path().join("a.rs"), "pub fn two() {}\n").expect("edit");
        run(&["add", "."]);
        run(&["commit", "-m", "two"]);
        let behind = crate::status::probe_freshness(&store, "p", dir.path()).expect("probe");
        assert_eq!(
            behind,
            crate::status::Freshness::BehindHead { commits: 1 },
            "one commit ahead of the indexed head"
        );
        assert_eq!(
            behind.line_suffix(),
            "behind 1",
            "the line names the axis that drifted"
        );
    }

    use super::*;

    fn report_with(freshness: Freshness, coverage: Coverage) -> Report {
        Report {
            project: "p".into(),
            freshness,
            runs: 1,
            chain_ok: true,
            coverage,
            depth: Depth::Full,
            misses: Misses::default(),
        }
    }

    #[test]
    fn a_fresh_complete_store_says_nothing_more() {
        let r = report_with(
            Freshness::Fresh,
            Coverage {
                files: 12,
                partial: 0,
                unusable: 0,
                skipped: 0,
            },
        );
        assert_eq!(r.line(), "[graph: fresh | 12 files]");
    }

    #[test]
    fn a_stale_store_names_the_next_step() {
        let r = report_with(
            Freshness::Stale { changed: 3 },
            Coverage {
                files: 12,
                partial: 0,
                unusable: 0,
                skipped: 0,
            },
        );
        assert_eq!(
            r.line(),
            "[graph: stale 3 | 12 files | next: reindex (3 file(s) changed)]"
        );
    }

    #[test]
    fn an_incomplete_parse_is_reported() {
        let r = report_with(
            Freshness::Fresh,
            Coverage {
                files: 20,
                partial: 2,
                unusable: 1,
                skipped: 5,
            },
        );
        assert_eq!(
            r.line(),
            "[graph: fresh | 20 files, 2 partial, 1 unusable, 5 skipped]"
        );
    }

    #[test]
    fn a_broken_chain_is_never_silent() {
        let mut r = report_with(
            Freshness::Fresh,
            Coverage {
                files: 1,
                partial: 0,
                unusable: 0,
                skipped: 0,
            },
        );
        r.chain_ok = false;
        assert!(r.line().contains("receipts BROKEN"), "{}", r.line());
    }

    /// The line carries the split, not just the total: "18,206 unresolved" was
    /// the number that could not tell a broken resolver from ordinary `clone`
    /// calls.
    #[test]
    fn the_line_splits_unresolved_calls_by_reason() {
        let mut r = report_with(
            Freshness::Fresh,
            Coverage {
                files: 12,
                partial: 0,
                unusable: 0,
                skipped: 0,
            },
        );
        r.misses = Misses {
            external_or_std: 3_660,
            type_unknown: 9_200,
            not_extracted: 0,
            shape_not_modelled: 900,
            no_definition_with_that_name: 446,
        };
        let line = r.line();
        assert!(
            line.contains(
                "14206 unresolved (3660 external_or_std, 9200 type_unknown, \
900 shape_not_modelled, 446 no_definition_with_that_name)"
            ),
            "the split is rendered: {line}"
        );
        // Bounded: a reason with a zero count is not listed, so the tail cannot
        // grow with the number of variants.
        assert!(!line.contains("not_extracted"), "{line}");
        assert!(!line.contains(", 0 "), "{line}");
    }

    /// No misses, no section: a complete answer must not carry an empty tail.
    #[test]
    fn a_store_with_no_misses_renders_no_split() {
        let r = report_with(
            Freshness::Fresh,
            Coverage {
                files: 12,
                partial: 0,
                unusable: 0,
                skipped: 0,
            },
        );
        let line = r.line();
        assert!(!line.contains("unresolved"), "{line}");
        assert_eq!(line, "[graph: fresh | 12 files]");
    }

    /// The counts come from the stored `USAGE` edges, so a store written before
    /// the split still counts — under the fallback, never dropped.
    #[test]
    fn misses_are_counted_from_the_stored_reasons() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("a.rs"),
            "pub fn one(s: String) { let _t = s.clone(); }\n\
             pub fn two() { helper_fn(); }\n\
             pub fn three() { other_unknown(); }\n",
        )
        .expect("a.rs");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &IndexOptions::new("p"))
            .expect("index");
        // One legible old-style reason: it must land in the fallback bucket.
        store
            .connection()
            .execute(
                "UPDATE edges SET props_json = json_set(props_json, '$.reason', 'legacy_reason')
                 WHERE project = 'p' AND edge_type = 'USAGE'
                   AND json_extract(props_json, '$.callee') = 'other_unknown'",
                [],
            )
            .expect("rewrite");
        let misses = super::misses(&store, "p").expect("misses");
        assert_eq!(
            misses,
            Misses {
                external_or_std: 1,
                type_unknown: 1,
                not_extracted: 0,
                shape_not_modelled: 0,
                no_definition_with_that_name: 1,
            },
            "{misses:?}"
        );
        assert_eq!(misses.total(), 3);
        // And the report - not just the raw count - carries the split.
        let report = super::report(&store, "p", dir.path()).expect("report");
        assert_eq!(report.misses, misses);
        assert!(
            report.line().contains("3 unresolved ("),
            "{}",
            report.line()
        );
    }

    #[test]
    fn an_unindexed_project_asks_for_an_index() {
        let r = report_with(
            Freshness::Unindexed,
            Coverage {
                files: 0,
                partial: 0,
                unusable: 0,
                skipped: 0,
            },
        );
        assert_eq!(r.line(), "[graph: unindexed | 0 files | next: index]");
    }
}
