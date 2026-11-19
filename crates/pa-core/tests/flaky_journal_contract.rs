//! `docs/flaky.md` is a ledger with two claims a machine can check.
//!
//! 1. "Every known-flaky or environment-gated test is listed here". The checkable
//!    part: where the tree marks a unit `#[ignore]`, the reason must be stated at
//!    the attribute (not only here), so a reader of the test sees why it does not
//!    run without opening a second file.
//! 2. Every unit the journal names must still exist in the tree. A row for a
//!    renamed or deleted test is worse than no row: it reports coverage that
//!    cannot fire.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/pa-core is two levels below the root")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Every `#[ignore]` attribute in the workspace, as `(file, line, attribute)`.
fn ignore_attributes(root: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for entry in walk(root) {
        let text = read(&entry);
        for (index, line) in text.lines().enumerate() {
            // An attribute, not a mention: the line must start with it and be
            // followed by `]` or `=`. This file's own doc comments quote
            // `#[ignore]` and must not count.
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("#[ignore") {
                let rest = rest.trim_start();
                if rest.starts_with(']') || rest.starts_with('=') {
                    found.push((
                        entry.display().to_string(),
                        format!("{}:{}", index + 1, trimmed),
                    ));
                }
            }
        }
    }
    found
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn a_disabled_test_states_its_reason_at_the_attribute() {
    let root = repo_root().join("crates");
    let attributes = ignore_attributes(&root);
    assert!(
        !attributes.is_empty(),
        "the walk found no `#[ignore]` at all, which means it did not reach the tree"
    );
    let bare: Vec<_> = attributes
        .iter()
        .filter(|(_, attribute)| !attribute.contains('='))
        .collect();
    assert!(
        bare.is_empty(),
        "a disabled test without a stated reason looks like an accident: {bare:#?}"
    );
}

#[test]
fn every_journal_row_names_a_unit_that_still_exists() {
    let root = repo_root();
    let journal = read(&root.join("docs/flaky.md"));
    let crates: Vec<String> = std::fs::read_dir(root.join("crates"))
        .expect("crates/")
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    let mut checked = 0;
    for row in journal.lines().filter(|line| line.starts_with("| `")) {
        let Some(unit) = row.split('|').nth(2) else {
            continue;
        };
        // Every backticked token that could name a unit is checked. The row also
        // names its crate ("`pa-daemon`") and quotes flags ("`--lib`"); a crate
        // name is not a unit and a flag is not a name, so both are skipped by
        // shape rather than by a list.
        for name in backticked(unit) {
            let is_unit_name = !name.starts_with('-')
                && !name.contains(' ')
                && !crates.contains(&name)
                // A commit hash is not a unit name. Nine hex characters is the
                // repo's short-hash shape; a Rust identifier with no underscore
                // would look the same, and no test name is nine lowercase hex.
                && !(name.len() == 9
                    && name
                        .chars()
                        .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()))
                && name
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphanumeric());
            if !is_unit_name {
                continue;
            }
            // The journal names tests, but also a `const` it introduced, so the
            // check is "this identifier exists", not "a `fn` has this name".
            // Word boundaries keep a rename visible: `a_foo` must not match
            // `a_foo_bar`.
            let needle = name.to_string();
            let exists_as_identifier = walk(&root.join("crates")).into_iter().any(|file| {
                let text = read(&file);
                text.match_indices(&needle).any(|(index, _)| {
                    let before = index.checked_sub(1).map(|i| text.as_bytes()[i]);
                    let after = text.as_bytes().get(index + needle.len()).copied();
                    let is_boundary = |byte: Option<u8>| {
                        byte.is_none_or(|byte| !byte.is_ascii_alphanumeric() && byte != b'_')
                    };
                    is_boundary(before) && is_boundary(after)
                })
            });
            let hit = walk(&root.join("crates"))
                .into_iter()
                .any(|file| read(&file).contains(&needle));
            assert!(
                hit,
                "`docs/flaky.md` names `{name}`, which the tree does not define"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no journal row was checked");
}

fn backticked(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            break;
        };
        names.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    names
}
