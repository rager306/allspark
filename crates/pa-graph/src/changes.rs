//! Git-diff → affected symbols (G5).
//!
//! [`detect_changes`] answers "what symbols did this range of revisions
//! touch?" without re-indexing. It runs `git diff` between two revisions,
//! matches the changed paths against the graph's `File` nodes, and — for
//! modified files — narrows to the symbols whose line ranges intersect the
//! changed hunks, so a one-line edit inside `foo` does not report every symbol
//! in the file.
//!
//! Three provenance shapes are written back into the graph so later queries
//! (and the watcher) can see the revision context:
//!
//! - a `Branch` node carrying `{branch, head_sha, base_sha, is_git}`,
//! - `HAS_BRANCH` edges from that node to every changed file node,
//! - `FILE_CHANGES_WITH` edges between files that changed together in a commit.
//!
//! Everything shells out through [`std::process::Command`]; the crate stays
//! `#![forbid(unsafe_code)]`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{GraphError, Result};
use crate::store::GraphStore;
use crate::types::{EdgeType, Label, Node};

/// How a path changed between the two revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeStatus {
    /// The path did not exist at `a`.
    Added,
    /// The path existed at both revisions with different content.
    Modified,
    /// The path is gone at `b`.
    Deleted,
    /// The path moved.
    Renamed,
    /// The path was copied.
    Copied,
    /// The file type changed (e.g. regular → symlink).
    TypeChanged,
    /// The path has an unresolved merge conflict.
    Unmerged,
    /// Any git status letter this crate does not model explicitly.
    Unknown,
}

impl ChangeStatus {
    /// Parse a git `--name-status` letter (the first character of e.g. `R100`).
    #[must_use]
    pub fn from_git(code: char) -> Self {
        match code {
            'A' => Self::Added,
            'M' => Self::Modified,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            'U' => Self::Unmerged,
            _ => Self::Unknown,
        }
    }

    /// Stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChanged => "type_changed",
            Self::Unmerged => "unmerged",
            Self::Unknown => "unknown",
        }
    }
}

/// One `@@` hunk from a `-U0` diff, in old- and new-file line coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HunkRange {
    /// 1-based first line in the old file (0 for a pure insertion).
    pub old_start: u32,
    /// Number of old-file lines the hunk covers.
    pub old_lines: u32,
    /// 1-based first line in the new file (0 for a pure deletion).
    pub new_start: u32,
    /// Number of new-file lines the hunk covers.
    pub new_lines: u32,
}

impl HunkRange {
    /// The inclusive new-file line range this hunk touches. A zero-length
    /// insertion reports the line it lands after, so a symbol spanning that
    /// boundary still matches.
    #[must_use]
    pub fn new_span(self) -> (u32, u32) {
        if self.new_lines == 0 {
            let at = self.new_start.max(1);
            (at, at)
        } else {
            (self.new_start.max(1), self.new_start + self.new_lines - 1)
        }
    }
}

/// A path that changed between the two revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// Repo-relative path at the newer revision (the old path for deletions).
    pub path: String,
    /// For renames/copies, the previous path.
    pub old_path: Option<String>,
    /// How the path changed.
    pub status: ChangeStatus,
    /// `-U0` hunks (empty for added/deleted files, which are wholly new/gone).
    pub hunks: Vec<HunkRange>,
}

impl ChangedFile {
    /// The new-file line ranges this file changed.
    #[must_use]
    pub fn changed_spans(&self) -> Vec<(u32, u32)> {
        self.hunks.iter().map(|h| h.new_span()).collect()
    }
}

/// A graph symbol the change range touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedSymbol {
    /// Graph node id.
    pub id: i64,
    /// Short name.
    pub name: String,
    /// Qualified name.
    pub qualified_name: String,
    /// Defining file.
    pub file_path: String,
    /// Node kind.
    pub label: String,
    /// 1-based inclusive start line.
    pub start_line: u32,
    /// 1-based inclusive end line.
    pub end_line: u32,
    /// How the owning file changed.
    pub change: ChangeStatus,
    /// The changed new-file line ranges that intersect this symbol.
    pub touched_lines: Vec<(u32, u32)>,
}

/// The git revision context captured for a [`ChangeReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitState {
    /// Current branch (`HEAD` when detached).
    pub branch: String,
    /// Commit SHA of `b`.
    pub head_sha: String,
    /// Commit SHA of `a`.
    pub base_sha: String,
    /// Merge base of the two revisions, when one exists.
    pub merge_base: Option<String>,
    /// Whether the working tree was dirty at report time.
    pub dirty: bool,
}

/// Result of [`detect_changes`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeReport {
    /// Project name.
    pub project: String,
    /// `true` when `project_root` is inside a git work tree.
    pub is_git: bool,
    /// Revision context; `None` when the root is not a git repository.
    pub git: Option<GitState>,
    /// Changed paths, sorted by path.
    pub files: Vec<ChangedFile>,
    /// Symbols touched by the changed hunks, sorted by path then line.
    pub symbols: Vec<AffectedSymbol>,
    /// Pairs of paths that changed together in a commit in this range.
    pub co_changes: Vec<(String, String)>,
    /// Changed files the graph has no nodes for.
    ///
    /// Their symbols cannot be attributed to hunks, so they contribute nothing
    /// to `symbols`. Without this count a report over a stale index looks
    /// complete while silently missing files (gitnexus marks the same case
    /// `partial: true`). `0` means every changed file was known to the index.
    #[serde(default)]
    pub unindexed_files: usize,
}

impl ChangeReport {
    /// Number of changed files.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Number of affected symbols.
    #[must_use]
    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }
}

/// Detect the symbols changed between `git_ref_a` and `git_ref_b` for
/// `project`, then write the branch/co-change provenance back into the graph.
///
/// The project's registered root is the working directory every git command
/// runs in, with `--relative` so reported paths line up with the graph's
/// repo-relative `file_path` values.
///
/// # Errors
/// [`GraphError::ProjectNotFound`] when `project` is unregistered, and
/// [`GraphError::Git`] when a git invocation fails on a work tree.
pub fn detect_changes(
    store: &GraphStore,
    project: &str,
    git_ref_a: &str,
    git_ref_b: &str,
) -> Result<ChangeReport> {
    let root = store
        .project_root(project)?
        .ok_or_else(|| GraphError::ProjectNotFound(project.to_string()))?;
    let root = Path::new(&root);

    let mut report = ChangeReport {
        project: project.to_string(),
        is_git: false,
        git: None,
        files: Vec::new(),
        symbols: Vec::new(),
        co_changes: Vec::new(),
        unindexed_files: 0,
    };

    if !is_work_tree(root) {
        return Ok(report);
    }
    report.is_git = true;
    report.git = Some(read_git_state(root, git_ref_a, git_ref_b)?);

    let statuses = diff_name_status(root, git_ref_a, git_ref_b)?;
    let file_nodes = store.file_node_ids(project)?;

    for (status, path, old_path) in statuses {
        let hunks = if matches!(status, ChangeStatus::Added | ChangeStatus::Deleted) {
            Vec::new()
        } else {
            diff_hunks(root, git_ref_a, git_ref_b, &path)?
        };
        let file = ChangedFile {
            path,
            old_path,
            status,
            hunks,
        };
        collect_symbols(store, project, &file, &file_nodes, &mut report.symbols)?;
        // A changed file the index never saw contributes no symbols; count it
        // so the report cannot read as complete over a stale index.
        let indexed_path = if file.status == ChangeStatus::Deleted {
            file.old_path.as_deref().unwrap_or(&file.path)
        } else {
            &file.path
        };
        if !file_nodes.contains_key(indexed_path) {
            report.unindexed_files += 1;
        }
        report.files.push(file);
    }
    report.files.sort_by(|a, b| a.path.cmp(&b.path));
    report.symbols.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.start_line.cmp(&b.start_line))
    });

    report.co_changes = co_change_pairs(root, git_ref_a, git_ref_b)?;

    write_provenance(store, project, &report)?;
    Ok(report)
}

impl GraphStore {
    /// See [`detect_changes`].
    ///
    /// # Errors
    /// Propagates git and SQLite errors.
    pub fn detect_changes(
        &self,
        project: &str,
        git_ref_a: &str,
        git_ref_b: &str,
    ) -> Result<ChangeReport> {
        detect_changes(self, project, git_ref_a, git_ref_b)
    }
}

/// Append every symbol of `file` whose span intersects a changed hunk.
fn collect_symbols(
    store: &GraphStore,
    project: &str,
    file: &ChangedFile,
    file_nodes: &std::collections::HashMap<String, i64>,
    out: &mut Vec<AffectedSymbol>,
) -> Result<()> {
    // An added file is wholly new: every symbol it defines is affected. A
    // deleted file's symbols are already gone from the graph, so there is
    // nothing to resolve. Renames/copies are read at the new path.
    let path = if file.status == ChangeStatus::Deleted {
        file.old_path.as_deref().unwrap_or(&file.path)
    } else {
        &file.path
    };
    if !file_nodes.contains_key(path) {
        return Ok(());
    }
    let spans = file.changed_spans();
    for node in store.nodes_in_file(project, path)? {
        if node.label == Label::File {
            continue;
        }
        let touched: Vec<(u32, u32)> = if file.status == ChangeStatus::Added {
            spans.clone()
        } else {
            spans
                .iter()
                .copied()
                .filter(|&span| spans_intersect(node.start_line, node.end_line, span))
                .collect()
        };
        if file.status != ChangeStatus::Added && touched.is_empty() {
            continue;
        }
        out.push(AffectedSymbol {
            id: node.id,
            name: node.name.clone(),
            qualified_name: node.qualified_name.clone(),
            file_path: node.file_path.clone(),
            label: node.label.as_str().to_string(),
            start_line: node.start_line,
            end_line: node.end_line,
            change: file.status,
            touched_lines: touched,
        });
    }
    Ok(())
}

/// Whether `[a_start, a_end]` and `[b_start, b_end]` share at least one line.
fn spans_intersect(a_start: u32, a_end: u32, (b_start, b_end): (u32, u32)) -> bool {
    a_start <= b_end && b_start <= a_end
}

/// Write the `Branch` node, `HAS_BRANCH` edges, and `FILE_CHANGES_WITH` edges.
fn write_provenance(store: &GraphStore, project: &str, report: &ChangeReport) -> Result<()> {
    let Some(git) = &report.git else {
        return Ok(());
    };
    let branch_qn = format!("{project}::__branch__::{}", git.branch);
    let branch_node = Node {
        id: 0,
        project: project.to_string(),
        label: Label::Branch,
        name: git.branch.clone(),
        qualified_name: branch_qn,
        file_path: String::new(),
        start_line: 0,
        end_line: 0,
        props: serde_json::json!({
            "branch": git.branch,
            "head_sha": git.head_sha,
            "base_sha": git.base_sha,
            "merge_base": git.merge_base,
            "is_git": true,
            "dirty": git.dirty,
        }),
    };
    let branch_id = store.upsert_node(&branch_node)?;

    let file_nodes = store.file_node_ids(project)?;
    for file in &report.files {
        let path = if file.status == ChangeStatus::Deleted {
            file.old_path.as_deref().unwrap_or(&file.path)
        } else {
            &file.path
        };
        if let Some(&file_id) = file_nodes.get(path) {
            store.insert_edge(
                project,
                branch_id,
                file_id,
                EdgeType::HasBranch,
                &serde_json::json!({
                    "head_sha": git.head_sha,
                    "base_sha": git.base_sha,
                    "status": file.status.as_str(),
                }),
            )?;
        }
    }

    for (a, b) in &report.co_changes {
        let (Some(&a_id), Some(&b_id)) = (file_nodes.get(a), file_nodes.get(b)) else {
            continue;
        };
        store.insert_edge(
            project,
            a_id,
            b_id,
            EdgeType::FileChangesWith,
            &serde_json::json!({
                "range": format!("{}..{}", git.base_sha, git.head_sha),
            }),
        )?;
    }
    Ok(())
}

// ── git plumbing ────────────────────────────────────────────────────────

/// Run `git -C root <args>` and return stdout as lossy UTF-8.
fn run_git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| GraphError::Git(format!("spawning git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GraphError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run `git -C root <args>` and return raw stdout bytes.
fn run_git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| GraphError::Git(format!("spawning git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GraphError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(output.stdout)
}

/// Whether `root` is inside a git work tree.
fn is_work_tree(root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
}

fn read_git_state(root: &Path, a: &str, b: &str) -> Result<GitState> {
    let branch = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let head_sha = run_git(root, &["rev-parse", b])?.trim().to_string();
    let base_sha = run_git(root, &["rev-parse", a])?.trim().to_string();
    let merge_base = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge-base", a, b])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    let dirty = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty());
    Ok(GitState {
        branch,
        head_sha,
        base_sha,
        merge_base,
        dirty,
    })
}

/// Parse `git diff --name-status -z --relative` into `(status, path, old_path)`.
fn diff_name_status(
    root: &Path,
    a: &str,
    b: &str,
) -> Result<Vec<(ChangeStatus, String, Option<String>)>> {
    let bytes = run_git_bytes(
        root,
        &["diff", "--name-status", "-z", "--relative", a, b, "--"],
    )?;
    let mut out = Vec::new();
    let mut tokens = bytes.split(|&c| c == 0).map(|t| {
        String::from_utf8_lossy(t)
            .trim_end_matches('\n')
            .to_string()
    });
    while let Some(status) = tokens.next() {
        if status.is_empty() {
            continue;
        }
        let code = status.chars().next().unwrap_or('?');
        let status = ChangeStatus::from_git(code);
        let Some(first) = tokens.next() else { break };
        if first.is_empty() {
            continue;
        }
        if matches!(status, ChangeStatus::Renamed | ChangeStatus::Copied) {
            let Some(second) = tokens.next() else { break };
            out.push((status, second, Some(first)));
        } else {
            out.push((status, first, None));
        }
    }
    Ok(out)
}

/// Parse the `-U0` hunks for one path between two revisions.
fn diff_hunks(root: &Path, a: &str, b: &str, path: &str) -> Result<Vec<HunkRange>> {
    let text = run_git(
        root,
        &[
            "diff",
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            "--relative",
            a,
            b,
            "--",
            path,
        ],
    )?;
    let mut hunks = Vec::new();
    for line in text.lines() {
        if let Some(hunk) = parse_hunk_header(line) {
            hunks.push(hunk);
        }
    }
    Ok(hunks)
}

/// Parse one `@@ -a,b +c,d @@` header (the trailing `@@` is optional in
/// `-U0` output when the range has no function context).
fn parse_hunk_header(line: &str) -> Option<HunkRange> {
    let rest = line.strip_prefix("@@ ")?;
    let end = rest.find(" @@")?;
    let body = &rest[..end];
    let mut parts = body.split(' ');
    let old = parts.next()?;
    let new = parts.next()?;
    let (old_start, old_lines) = parse_range(old.strip_prefix('-')?)?;
    let (new_start, new_lines) = parse_range(new.strip_prefix('+')?)?;
    Some(HunkRange {
        old_start,
        old_lines,
        new_start,
        new_lines,
    })
}

/// Parse `start` or `start,count` into `(start, count)`, defaulting count to 1.
fn parse_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

/// Unordered pairs of paths that changed together in at least one commit in
/// `a..b`, each pair reported once with paths sorted.
fn co_change_pairs(root: &Path, a: &str, b: &str) -> Result<Vec<(String, String)>> {
    let range = format!("{a}..{b}");
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "log",
            "--name-only",
            "--no-renames",
            "--relative",
            "--pretty=format:\x01%H",
            &range,
        ])
        .output()
        .map_err(|e| GraphError::Git(format!("spawning git: {e}")))?;
    if !output.status.success() {
        // An unrelated-history range (no merge base) has no commits to walk;
        // co-change is simply empty, not an error.
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pairs: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut commit_files: BTreeSet<String> = BTreeSet::new();
    let flush = |files: &mut BTreeSet<String>, pairs: &mut BTreeMap<(String, String), usize>| {
        let list: Vec<&String> = files.iter().collect();
        for i in 0..list.len() {
            for j in (i + 1)..list.len() {
                *pairs.entry((list[i].clone(), list[j].clone())).or_insert(0) += 1;
            }
        }
        files.clear();
    };
    for line in text.lines() {
        if let Some(sha) = line.strip_prefix('\x01') {
            flush(&mut commit_files, &mut pairs);
            let _ = sha;
        } else if !line.trim().is_empty() {
            commit_files.insert(line.to_string());
        }
    }
    flush(&mut commit_files, &mut pairs);
    Ok(pairs.into_keys().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunk_headers() {
        let h = parse_hunk_header("@@ -1,3 +1,4 @@ fn foo()").expect("hunk");
        assert_eq!(
            h,
            HunkRange {
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 4
            }
        );
        // A zero-length new range (pure deletion) still reports a line.
        let d = parse_hunk_header("@@ -5,2 +4,0 @@").expect("hunk");
        assert_eq!(d.new_span(), (4, 4));
        assert!(parse_hunk_header("not a hunk").is_none());
    }

    #[test]
    fn hunk_new_span_is_inclusive() {
        let h = HunkRange {
            old_start: 1,
            old_lines: 2,
            new_start: 10,
            new_lines: 3,
        };
        assert_eq!(h.new_span(), (10, 12));
    }

    #[test]
    fn spans_intersect_boundaries() {
        assert!(spans_intersect(5, 10, (10, 12)));
        assert!(spans_intersect(5, 10, (1, 5)));
        assert!(!spans_intersect(5, 10, (11, 20)));
        assert!(!spans_intersect(5, 10, (1, 4)));
    }

    #[test]
    fn change_status_from_git_letters() {
        assert_eq!(ChangeStatus::from_git('A'), ChangeStatus::Added);
        assert_eq!(ChangeStatus::from_git('M'), ChangeStatus::Modified);
        assert_eq!(ChangeStatus::from_git('D'), ChangeStatus::Deleted);
        assert_eq!(ChangeStatus::from_git('R'), ChangeStatus::Renamed);
        assert_eq!(ChangeStatus::from_git('?'), ChangeStatus::Unknown);
    }

    /// A changed file that is not in the index must be reported, not dropped.
    ///
    /// `collect_symbols` returns early for a path the graph does not know, so a
    /// file added since the last index silently contributes no affected
    /// symbols and the report reads as complete. gitnexus marks this case
    /// `partial: true`; the report must at least carry the count.
    #[test]
    fn changed_files_outside_the_index_are_counted() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        // A real git repo with one commit.
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        std::fs::write(root.join("known.rs"), "pub fn known() {}\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "one"]);

        // The store indexes `known.rs` only.
        let store = GraphStore::open_in_memory().expect("store");
        store
            .register_project("p", root.to_str().unwrap())
            .expect("project");
        store
            .upsert_node(&Node {
                id: 0,
                project: "p".into(),
                label: Label::File,
                name: "known.rs".into(),
                qualified_name: "known.rs".into(),
                file_path: "known.rs".into(),
                start_line: 1,
                end_line: 1,
                props: serde_json::json!({}),
            })
            .expect("file node");

        // A second file lands in the commit but never entered the graph.
        std::fs::write(root.join("unindexed.rs"), "pub fn fresh() {}\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "two"]);

        let report = detect_changes(&store, "p", "HEAD~1", "HEAD").expect("changes");
        assert!(
            report.files.iter().any(|f| f.path.contains("unindexed.rs")),
            "the changed file is listed: {:#?}",
            report.files
        );
        assert_eq!(
            report.unindexed_files, 1,
            "a changed file the index never saw must be counted, not silently dropped"
        );
    }
}
