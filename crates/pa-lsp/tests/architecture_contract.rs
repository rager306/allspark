//! Architecture contract tests for `pa-lsp`.
//!
//! FORK-ONLY (allspark). Two invariants the crate must keep, checked against
//! its own sources rather than by convention:
//!
//! 1. No `unsafe` anywhere in `src/` (the workspace lints `forbid(unsafe_code)`;
//!    this test guards the crate against a future `#[allow]` or a `#![allow]`
//!    slip that the crate-level `#![forbid]` would otherwise be the only witness
//!    to).
//! 2. `pa-lsp` is a leaf: it must not depend on `pa-core` (or any other
//!    session-engine crate), so the tool stays importable without dragging the
//!    whole product graph in.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// The crate root (`<workspace>/crates/pa-lsp`).
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `src/`.
fn src_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rs(&crate_dir().join("src"), &mut files);
    files.sort();
    files
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The `unsafe` keyword is forbidden. The only place the string may appear is
/// inside a `forbid`/`deny`/`allow` attribute *name* (`unsafe_code`), which is
/// not the `unsafe` keyword — so a bare `unsafe` token anywhere else is a
/// violation.
#[test]
fn src_contains_no_unsafe_code() {
    let mut offenders = Vec::new();
    for file in src_files() {
        let text = std::fs::read_to_string(&file).expect("read source");
        for (line_no, line) in text.lines().enumerate() {
            if contains_unsafe_keyword(line) {
                offenders.push(format!("{}:{}", file.display(), line_no + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`unsafe` must not appear in pa-lsp sources: {offenders:?}"
    );
}

/// Whether a line uses the `unsafe` keyword (as opposed to the `unsafe_code`
/// lint name in an attribute). `unsafe_code` never matches because it is not
/// followed by a keyword boundary.
fn contains_unsafe_keyword(line: &str) -> bool {
    // Strip a trailing comment so prose mentioning `unsafe` does not trip the
    // check; the keyword lives in code, not documentation.
    let code = line.split("//").next().unwrap_or("");
    code.match_indices("unsafe").any(|(idx, _)| {
        let after = &code[idx + "unsafe".len()..];
        // `unsafe_code` is an attribute/lint identifier, not the keyword.
        !after.starts_with('_')
    })
}

/// `pa-lsp` must stay a leaf crate: no dependency on `pa-core` (the session
/// engine) or the product crates that would pull it in.
#[test]
fn does_not_depend_on_pa_core() {
    let manifest =
        std::fs::read_to_string(crate_dir().join("Cargo.toml")).expect("read pa-lsp Cargo.toml");

    // Only the `[dependencies]`/`[dev-dependencies]`/`[build-dependencies]`
    // tables matter; a `pa-core` mention in a comment or description does not.
    let mut in_deps_table = false;
    let mut offenders = Vec::new();
    for raw_line in manifest.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_deps_table = line.ends_with("dependencies]");
            continue;
        }
        if in_deps_table && (line.starts_with("pa-core") || line.contains("pa-core =")) {
            offenders.push(line.to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "pa-lsp must not depend on pa-core: {offenders:?}"
    );
}

/// The crate is registered as a workspace member (the one-line `Cargo.toml`
/// change this phase adds).
#[test]
fn is_a_workspace_member() {
    let workspace = crate_dir()
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("Cargo.toml");
    let manifest = std::fs::read_to_string(workspace).expect("read workspace Cargo.toml");
    assert!(
        manifest.contains("\"crates/pa-lsp\""),
        "pa-lsp must be listed in the workspace members"
    );
}
