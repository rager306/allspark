use std::path::Path;
use std::process::Command;

fn git_stdout(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn main() {
    // Rebuild when the commit the stamp names could have changed: HEAD, its
    // reflog, and the resolved branch ref (the HEAD file only says `ref: ...`).
    println!("cargo:rerun-if-env-changed=ALLSPARK_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=ALLSPARK_BUILD_DIRTY");
    for path in [
        git_stdout(&["rev-parse", "--git-path", "HEAD"]),
        git_stdout(&["rev-parse", "--git-path", "logs/HEAD"]),
    ]
    .into_iter()
    .flatten()
    {
        // The path exists whenever the checkout has a git dir; a missing file
        // is not an error, the watch simply does not apply.
        let path = Path::new(&path);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    if let Some(branch) = git_stdout(&["symbolic-ref", "-q", "--short", "HEAD"]) {
        if let Some(ref_path) =
            git_stdout(&["rev-parse", "--git-path", &format!("refs/heads/{branch}")])
        {
            let ref_path = Path::new(&ref_path);
            if ref_path.exists() {
                println!("cargo:rerun-if-changed={}", ref_path.display());
            }
        }
    }

    let commit = git_stdout(&["rev-parse", "HEAD"])
        .map(|sha| sha.chars().take(12).collect::<String>())
        .filter(|sha| sha.len() == 12)
        .unwrap_or_else(|| "unknown".to_string());
    // An unclean tree is a *different build*: the same commit with local edits
    // must not compare equal to a clean sibling. CI sets
    // ALLSPARK_BUILD_COMMIT explicitly (like ALLSPARK_FORK_BUILD) to pin a
    // release; a local build stamps whatever the working tree is.
    let dirty = std::env::var("ALLSPARK_BUILD_COMMIT").is_err()
        && git_stdout(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty());
    let stamp = if commit == "unknown" {
        String::new()
    } else if dirty {
        format!("{commit}.dirty")
    } else {
        commit.clone()
    };
    println!("cargo:rustc-env=ALLSPARK_BUILD_HASH={stamp}");
    println!("cargo:rustc-env=ALLSPARK_BUILD_COMMIT={commit}");
}
