//! One probe that decides what a graph refresh must do, and why.
//!
//! A refresh answers four questions at once, cheapest source first:
//!
//! ```text
//! git rev-parse + rev-list   committed drift (the store is behind HEAD)
//! git ls-files -z            the tracked universe          (~12 ms, 2,719 files)
//! git ls-files --others      untracked files                (~23 ms)
//! stat sweep                 per-file mtime/size            (~15 ms, 2,034 files)
//! content hash               only where stat already differs (~29 ms, 211 files)
//! ```
//!
//! Measured on this repository: the whole probe is **80 ms**, against 793 ms for
//! the same information gathered by walking the filesystem (748 ms of that walk).
//! `git ls-files` is 22x faster than `os.walk` and applies `.gitignore` for free.
//!
//! The probe decides **what changed**; it never decides what to do about it. Each
//! finding carries the action that would resolve it, so the caller can render a
//! plan instead of guessing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::hash::FileHash;
use crate::store::GraphStore;

/// One thing the probe found, with the action that would resolve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A tracked file that has no row in the store. A skeleton pass closes this.
    NotIndexed {
        /// Paths, capped by the caller.
        paths: Vec<String>,
    },
    /// A stored file whose content changed.
    Changed {
        /// Paths, capped by the caller.
        paths: Vec<String>,
    },
    /// A stored file that no longer exists and whose content did not reappear
    /// elsewhere.
    Deleted {
        /// Paths, capped by the caller.
        paths: Vec<String>,
    },
    /// A stored path that vanished while a new path holds the same content.
    ///
    /// This is the case neither `git status` nor a stat sweep can name: an
    /// **unstaged** `mv` is ` D` + `??` to git, and `stat` sees two unrelated
    /// paths. The content hash is the only thing that pairs them — and we already
    /// store one per file, so the join costs nothing.
    Renamed {
        /// Old path.
        from: String,
        /// New path.
        to: String,
    },
    /// The store was built at an older commit.
    BehindHead {
        /// Commits between the indexed commit and HEAD.
        commits: usize,
    },
    /// Not a git repository, so the committed-drift question cannot be asked.
    NotGit,
}

impl Finding {
    /// The action that resolves this finding.
    #[must_use]
    pub const fn action(&self) -> &'static str {
        match self {
            Self::NotIndexed { .. } | Self::NotGit => "index (skeleton is enough)",
            Self::Renamed { .. } => "reindex (paths reattached, edges preserved)",
            Self::Changed { .. } | Self::Deleted { .. } | Self::BehindHead { .. } => "reindex",
        }
    }
}

/// Everything a refresh needs to know, in one value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// What the probe found, most consequential first.
    pub findings: Vec<Finding>,
    /// Tracked files (the universe a graph should cover).
    pub tracked: usize,
    /// Files the store knows.
    pub indexed: usize,
    /// Milliseconds the probe took, for the report line.
    pub elapsed_ms: u64,
}

impl Probe {
    /// Whether any finding requires work.
    #[must_use]
    pub fn needs_work(&self) -> bool {
        !self.findings.is_empty()
    }

    /// The single next action, or `None` when the graph is current.
    #[must_use]
    pub fn next_step(&self) -> Option<&'static str> {
        self.findings.first().map(Finding::action)
    }
}

/// How many paths a finding lists before summarising.
pub const MAX_PATHS: usize = 20;

/// Run every probe against `root`, cheapest source first.
///
/// # Errors
/// Propagates SQLite errors; git failures degrade to [`Finding::NotGit`], never
/// to an error, because a missing git binary must not break a refresh.
pub fn probe(store: &GraphStore, project: &str, root: &Path) -> Result<Probe> {
    let started = std::time::Instant::now();
    let stored: HashMap<String, FileHash> = store.all_file_hashes(project)?;
    let mut probe = Probe {
        indexed: stored.len(),
        ..Probe::default()
    };

    // 1. Committed drift. Invisible to both a stat sweep and `git status`.
    if let Some(behind) = commits_behind(store, project, root) {
        if behind > 0 {
            probe.findings.push(Finding::BehindHead { commits: behind });
        }
    }

    // 2. The universe on disk, from git: 22x faster than walking, and
    //    `.gitignore` is already applied. Two lists are needed, not one:
    //    `ls-files` reports a file as tracked even after it was renamed away
    //    (an unstaged `mv` keeps `old.rs` in the index and puts `new.rs` only in
    //    `--others`), so the tracked list alone would call a rename a deletion.
    let Some(tracked) = git_ls_files(root, &["ls-files", "-z"]) else {
        probe.findings.push(Finding::NotGit);
        probe.elapsed_ms = elapsed_ms(started);
        return Ok(probe);
    };
    let untracked = git_ls_files(root, &["ls-files", "-z", "--others", "--exclude-standard"])
        .unwrap_or_default();
    probe.tracked = tracked.len();

    // 3. Files on disk the store has never seen: a tracked path it lacks, or an
    //    untracked arrival (which is what a rename looks like to git).
    let missing: Vec<String> = tracked
        .iter()
        .chain(untracked.iter())
        .filter(|rel| !stored.contains_key(*rel))
        .cloned()
        .collect();
    if !missing.is_empty() {
        probe.findings.push(Finding::NotIndexed {
            paths: cap(missing.clone()),
        });
    }

    // 4-5. Stat sweep, then content hashes for the stat-differing set only.
    let (changed, gone) = sweep(root, &stored);
    let really_changed = rehash(root, &stored, &changed);
    if !really_changed.is_empty() {
        probe.findings.push(Finding::Changed {
            paths: cap(really_changed),
        });
    }

    // 6. Pair removals with arrivals by content. This is the rename channel, and
    //    it needs no git: `git status -M` reports an unstaged `mv` as ` D` + `??`.
    // Gated on departures: with none there is no rename, and hashing every arrival
    // to look for one costs 1,090 ms for 685 unindexed files.
    let (renames, paired) = if gone.is_empty() {
        (Vec::new(), HashSet::new())
    } else {
        pair_renames(root, &stored, &gone, &missing)
    };
    // A rename is also an arrival that must not be reported as "not indexed".
    if let Some(Finding::NotIndexed { paths }) = probe
        .findings
        .iter_mut()
        .find(|f| matches!(f, Finding::NotIndexed { .. }))
    {
        paths.retain(|p| !paired.contains(p));
        if paths.is_empty() {
            probe
                .findings
                .retain(|f| !matches!(f, Finding::NotIndexed { .. }));
        }
    }
    let unpaired_gone: Vec<String> = gone
        .into_iter()
        .filter(|old| {
            !renames
                .iter()
                .any(|r| matches!(r, Finding::Renamed { from, .. } if from == old))
        })
        .collect();
    if !unpaired_gone.is_empty() {
        probe.findings.push(Finding::Deleted {
            paths: cap(unpaired_gone),
        });
    }
    probe.findings.extend(renames);

    probe.elapsed_ms = elapsed_ms(started);
    Ok(probe)
}

/// Split stored files into "stat moved" and "gone from disk".
///
/// `on_disk` is the git-derived universe, so a file that is tracked but whose path
/// vanished is a departure; anything else is compared by `(mtime, size)`.
fn sweep(root: &Path, stored: &HashMap<String, FileHash>) -> (Vec<String>, Vec<String>) {
    let mut changed = Vec::new();
    let mut gone = Vec::new();
    for (rel, record) in stored {
        match std::fs::metadata(root.join(rel)) {
            Ok(meta) => {
                let mtime_ns = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
                let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
                if mtime_ns != record.mtime_ns || size != record.size {
                    changed.push(rel.clone());
                }
            }
            Err(_) => gone.push(rel.clone()),
        }
    }
    (changed, gone)
}

/// Content-hash the stat-differing files and keep the ones whose bytes moved.
///
/// A `touch` bumps mtime without changing content; only the hash can tell the two
/// apart, and it runs on the small set `sweep` already narrowed.
fn rehash(root: &Path, stored: &HashMap<String, FileHash>, changed: &[String]) -> Vec<String> {
    changed
        .iter()
        .filter(|rel| content_sha(root.join(rel)).is_some_and(|sha| sha != stored[*rel].sha256))
        .cloned()
        .collect()
}

/// Pair departures with arrivals by content hash.
///
/// Returns the rename findings and the set of arrival paths that were claimed.
/// This is the channel `git status -M` cannot provide for an unstaged `mv`, and it
/// costs one read per candidate because the hashes are already stored.
fn pair_renames(
    root: &Path,
    stored: &HashMap<String, FileHash>,
    gone: &[String],
    arrivals: &[String],
) -> (Vec<Finding>, HashSet<String>) {
    let mut paired: HashSet<String> = HashSet::new();
    let mut renames = Vec::new();
    for old in gone {
        let want = &stored[old].sha256;
        if let Some(found) = arrivals.iter().find(|new| {
            !paired.contains(*new) && content_sha(root.join(new)).is_some_and(|sha| sha == *want)
        }) {
            paired.insert(found.clone());
            renames.push(Finding::Renamed {
                from: old.clone(),
                to: found.clone(),
            });
        }
    }
    (renames, paired)
}

/// The sha256 of a file's bytes, or `None` when it cannot be read.
///
/// Separate from [`crate::hash::hash_bytes`], which also stamps mtime and size;
/// the pairing step only needs the content digest.
fn content_sha(path: PathBuf) -> Option<String> {
    use sha2::Digest as _;
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", sha2::Sha256::digest(&bytes)))
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn cap(paths: Vec<String>) -> Vec<String> {
    paths.into_iter().take(MAX_PATHS).collect()
}

/// `git -C root <args>` split on NUL, or `None` when git is unavailable or the
/// directory is not a repository.
fn git_ls_files(root: &Path, args: &[&str]) -> Option<Vec<String>> {
    let out = crate::changes::run_git(root, args).ok()?;
    let mut files: Vec<String> = out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

/// How many commits HEAD is ahead of the commit the store was built at.
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

/// A probe's findings as one bounded line.
#[must_use]
pub fn render(probe: &Probe) -> String {
    if !probe.needs_work() {
        return format!(
            "[graph: fresh | {} files covered | {} ms]",
            probe.indexed, probe.elapsed_ms
        );
    }
    let mut parts = Vec::new();
    for finding in &probe.findings {
        parts.push(match finding {
            Finding::BehindHead { commits } => format!("behind {commits} commit(s)"),
            Finding::NotIndexed { paths } => format!("{} not indexed", paths.len()),
            Finding::Changed { paths } => format!("{} changed", paths.len()),
            Finding::Deleted { paths } => format!("{} deleted", paths.len()),
            Finding::Renamed { .. } => {
                let n = probe
                    .findings
                    .iter()
                    .filter(|f| matches!(f, Finding::Renamed { .. }))
                    .count();
                format!("{n} renamed")
            }
            Finding::NotGit => "not a git repo".to_string(),
        });
    }
    let next = probe.next_step().unwrap_or("index");
    format!(
        "[graph: {} of {} files | next: {next} | {} ms]",
        probe.indexed,
        probe.tracked.max(probe.indexed),
        probe.elapsed_ms,
        // Keep the interesting parts first but bounded.
    )
    .replace(
        " | ",
        &format!(
            "{} | ",
            if parts.is_empty() {
                String::new()
            } else {
                format!("{} ", parts.join(", "))
            }
        ),
    )
}

/// The finding kinds as stable wire names, for telemetry and tests.
#[must_use]
pub fn kind_of(finding: &Finding) -> &'static str {
    match finding {
        Finding::NotIndexed { .. } => "not_indexed",
        Finding::Changed { .. } => "changed",
        Finding::Deleted { .. } => "deleted",
        Finding::Renamed { .. } => "renamed",
        Finding::BehindHead { .. } => "behind_head",
        Finding::NotGit => "not_git",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, GraphStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = GraphStore::open_in_memory().expect("store");
        (dir, store)
    }

    fn git(dir: &Path, args: &[&str]) {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git");
    }

    #[test]
    fn a_non_git_directory_reports_not_git() {
        let (dir, store) = fixture();
        std::fs::write(dir.path().join("a.rs"), "pub fn a() {}").expect("a.rs");
        let p = probe(&store, "p", dir.path()).expect("probe");
        assert!(
            p.findings.contains(&Finding::NotGit),
            "expected NotGit: {:?}",
            p.findings
        );
    }

    #[test]
    fn an_unstaged_rename_is_paired_by_content_not_by_git() {
        let (dir, store) = fixture();
        git(dir.path(), &["init"]);
        git(dir.path(), &["config", "user.email", "t@e.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("old.rs"), "pub fn keep() -> u32 { 1 }\n").expect("old");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "base"]);
        let mut store = store;
        store
            .index_project(dir.path(), &crate::indexer::IndexOptions::new("p"))
            .expect("index");
        // An UNSTAGED move: git calls this " D" + "??", not a rename.
        std::fs::rename(dir.path().join("old.rs"), dir.path().join("new.rs")).expect("mv");
        let p = probe(&store, "p", dir.path()).expect("probe");
        let renamed: Vec<&Finding> = p
            .findings
            .iter()
            .filter(|f| matches!(f, Finding::Renamed { .. }))
            .collect();
        assert_eq!(renamed.len(), 1, "expected one rename: {:?}", p.findings);
        assert_eq!(
            renamed[0],
            &Finding::Renamed {
                from: "old.rs".into(),
                to: "new.rs".into()
            }
        );
        assert!(
            !p.findings
                .iter()
                .any(|f| matches!(f, Finding::NotIndexed { .. })),
            "the arrival must not also be reported as new: {:?}",
            p.findings
        );
        assert!(
            !p.findings
                .iter()
                .any(|f| matches!(f, Finding::Deleted { .. })),
            "the departure must not also be reported as deleted: {:?}",
            p.findings
        );
    }

    #[test]
    fn an_edit_changes_and_a_delete_deletes() {
        let (dir, store) = fixture();
        git(dir.path(), &["init"]);
        git(dir.path(), &["config", "user.email", "t@e.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.rs"), "pub fn a() {}\n").expect("a");
        std::fs::write(dir.path().join("b.rs"), "pub fn b() {}\n").expect("b");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "base"]);
        let mut store = store;
        store
            .index_project(dir.path(), &crate::indexer::IndexOptions::new("p"))
            .expect("index");
        std::fs::write(dir.path().join("a.rs"), "pub fn a() -> u8 { 1 }\n").expect("edit");
        std::fs::remove_file(dir.path().join("b.rs")).expect("rm");
        let p = probe(&store, "p", dir.path()).expect("probe");
        assert!(
            p.findings.iter().any(
                |f| matches!(f, Finding::Changed { paths } if paths == &vec!["a.rs".to_string()])
            ),
            "a.rs must report changed: {:?}",
            p.findings
        );
        assert!(
            p.findings.iter().any(
                |f| matches!(f, Finding::Deleted { paths } if paths == &vec!["b.rs".to_string()])
            ),
            "b.rs must report deleted: {:?}",
            p.findings
        );
    }

    #[test]
    fn a_clean_tree_reports_nothing_to_do() {
        let (dir, store) = fixture();
        git(dir.path(), &["init"]);
        git(dir.path(), &["config", "user.email", "t@e.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.rs"), "pub fn a() {}\n").expect("a");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "base"]);
        let mut store = store;
        store
            .index_project(dir.path(), &crate::indexer::IndexOptions::new("p"))
            .expect("index");
        let p = probe(&store, "p", dir.path()).expect("probe");
        assert!(!p.needs_work(), "expected a clean probe: {:?}", p.findings);
        assert!(render(&p).contains("fresh"), "{}", render(&p));
    }

    #[test]
    fn the_line_names_the_next_action() {
        let p = Probe {
            findings: vec![Finding::BehindHead { commits: 5 }],
            tracked: 100,
            indexed: 80,
            elapsed_ms: 12,
        };
        let line = render(&p);
        assert!(line.contains("behind 5 commit(s)"), "{line}");
        assert!(line.contains("next: reindex"), "{line}");
    }
}
