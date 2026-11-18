//! The git-context concern: the quiet git probes and the header's git-context capture (TS
//! `captureGitContext`).

use std::process::Stdio;

use super::{GitContext, Path};

/// Capture git context for the header (best effort; None outside a repo).
///
/// Contract (TS `captureGitContext`): every field is independently optional;
/// the context exists when at least one probe succeeds. `branch` is
/// `--show-current`, so a detached HEAD yields no branch. The remote URL is
/// normalized through the git-source parser when it parses, else kept
/// verbatim. The three probes run concurrently.
#[must_use]
pub fn capture_git_context(cwd: &Path) -> Option<GitContext> {
    let probes: [&[&str]; 3] = [
        &["rev-parse", "HEAD"],
        &["branch", "--show-current"],
        &["remote", "get-url", "origin"],
    ];
    // Spawn every probe before waiting on any.
    let children = probes.map(|args| {
        std::process::Command::new("git")
            .arg("--no-optional-locks")
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    });
    let [commit, branch, remote] = children.map(|child| {
        child?
            .wait_with_output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|stdout| stdout.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    if commit.is_none() && branch.is_none() && remote.is_none() {
        return None;
    }
    Some(GitContext {
        repo_url: remote.map(|url| {
            crate::packages::parse_git_url(&url)
                .map(|source| source.repo)
                .unwrap_or(url)
        }),
        commit,
        branch,
    })
}

/// The repository's `.git` directory for `cwd`, walking up like `git` does.
///
/// A session frequently starts in a subdirectory of a repository; `git rev-parse`
/// answers from any depth, so a reader that only looked at `cwd/.git` returned
/// `None` there and silently disabled every caller's revision check. Stops at the
/// filesystem root, and refuses a `.git` that is neither a directory nor a
/// `gitdir:` pointer.
fn find_git_dir(cwd: &Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    for root in cwd.ancestors() {
        let dot_git = root.join(".git");
        if dot_git.is_dir() {
            return Some((dot_git, root.to_path_buf()));
        }
        if let Ok(pointer) = std::fs::read_to_string(&dot_git) {
            let path = pointer.trim().strip_prefix("gitdir:")?.trim().to_string();
            let resolved = Path::new(&path);
            let git_dir = if resolved.is_absolute() {
                resolved.to_path_buf()
            } else {
                root.join(resolved)
            };
            return Some((git_dir, root.to_path_buf()));
        }
    }
    None
}

/// Read the current commit without spawning `git`.
///
/// [`capture_git_context`] runs three probes because the header wants the
/// branch and the remote too. A render path needs only the commit, and it must
/// not fork: `/session` redraws on every keypress, and three process spawns per
/// frame is disk and scheduler work on a paint path.
///
/// Reads `.git/HEAD` and, for a symbolic ref, the loose ref file or its line in
/// `packed-refs` (`git gc` moves refs there, and the default branch is usually
/// packed). A worktree or submodule has a `.git` FILE holding `gitdir: <path>`,
/// so both layouts resolve. A ref with no entry in either place answers `None`
/// rather than a wrong commit, and the caller treats an unknown revision the
/// same as no repository.
#[must_use]
pub fn read_head_commit(cwd: &Path) -> Option<String> {
    let (git_dir, _repo_root) = find_git_dir(cwd)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref: ") else {
        // A detached HEAD holds the commit directly.
        return (!head.is_empty()).then(|| head.to_string());
    };
    let reference = reference.trim();
    if let Ok(value) = std::fs::read_to_string(git_dir.join(reference)) {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    // `git gc` packs refs; the default branch is usually packed on a real
    // checkout, so a loose-only reader silently answered `None` there and the
    // stale-evidence check never fired.
    let packed = std::fs::read_to_string(git_dir.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        // Comments and the `^<peeled>` lines that follow an annotated tag.
        if line.starts_with('#') || line.starts_with('^') {
            return None;
        }
        let (commit, name) = line.split_once(' ')?;
        (name.trim() == reference).then(|| commit.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo_with_head(head: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git").join("refs").join("heads")).unwrap();
        fs::write(dir.path().join(".git").join("HEAD"), head).unwrap();
        dir
    }

    #[test]
    fn a_symbolic_head_resolves_through_its_ref_file() {
        let dir = repo_with_head("ref: refs/heads/main\n");
        fs::write(
            dir.path().join(".git/refs/heads/main"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();

        assert_eq!(
            read_head_commit(dir.path()).as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn a_detached_head_is_the_commit_itself() {
        let dir = repo_with_head("0123456789abcdef0123456789abcdef01234567\n");
        assert_eq!(
            read_head_commit(dir.path()).as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn a_worktree_git_file_points_at_the_real_directory() {
        let dir = repo_with_head("");
        let worktree = dir.path().join("wt");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", dir.path().join(".git").display()),
        )
        .unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/side\n").unwrap();
        fs::write(
            dir.path().join(".git/refs/heads/side"),
            "fedcba9876543210fedcba9876543210fedcba98\n",
        )
        .unwrap();

        assert_eq!(
            read_head_commit(&worktree).as_deref(),
            Some("fedcba9876543210fedcba9876543210fedcba98")
        );
    }

    /// A ref with no loose file and no packed entry answers nothing rather than
    /// guessing a commit.
    #[test]
    fn an_unresolvable_ref_answers_none_rather_than_a_guess() {
        let dir = repo_with_head("ref: refs/heads/absent\n");
        assert_eq!(read_head_commit(dir.path()), None);
    }

    /// `git gc` packs refs, and the default branch is packed on a real
    /// checkout. A loose-only reader answered `None` there, which silently
    /// disabled the stale-evidence check on exactly the branches people use.
    #[test]
    fn a_packed_ref_resolves_without_a_loose_file() {
        let dir = repo_with_head("ref: refs/heads/main\n");
        fs::write(
            dir.path().join(".git/packed-refs"),
            "# pack-refs with: peeled fully-peeled sorted \n\
             17aa6077f5f4c9b3ab74c9fa2c8653a998a591bc refs/heads/main\n\
             367fabe5b05990446fac1d4b42e2293dfb77d600 refs/remotes/origin/x\n",
        )
        .unwrap();

        assert_eq!(
            read_head_commit(dir.path()).as_deref(),
            Some("17aa6077f5f4c9b3ab74c9fa2c8653a998a591bc")
        );
    }

    /// A loose ref wins over a stale packed entry, which is what `git` does.
    #[test]
    fn a_loose_ref_wins_over_the_packed_one() {
        let dir = repo_with_head("ref: refs/heads/main\n");
        fs::write(
            dir.path().join(".git/packed-refs"),
            "1111111111111111111111111111111111111111 refs/heads/main\n",
        )
        .unwrap();
        fs::write(
            dir.path().join(".git/refs/heads/main"),
            "2222222222222222222222222222222222222222\n",
        )
        .unwrap();

        assert_eq!(
            read_head_commit(dir.path()).as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
    }

    #[test]
    fn a_directory_outside_a_repository_answers_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_head_commit(dir.path()), None);
    }
    /// A session often starts in a subdirectory; `git` answers from any depth,
    /// so a reader that only looked at `cwd/.git` answered `None` there and
    /// silently disabled the stale-evidence check.
    #[test]
    fn a_subdirectory_of_a_repository_resolves_its_commit() {
        let dir = repo_with_head("ref: refs/heads/main\n");
        fs::write(
            dir.path().join(".git/refs/heads/main"),
            "abcdefabcdefabcdefabcdefabcdefabcdefabcd\n",
        )
        .unwrap();
        let deep = dir.path().join("crates").join("nested").join("src");
        fs::create_dir_all(&deep).unwrap();

        assert_eq!(
            read_head_commit(&deep).as_deref(),
            Some("abcdefabcdefabcdefabcdefabcdefabcdefabcd")
        );
    }
}
