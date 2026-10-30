//! Fixtures shared by the indexer's test subjects.

use std::path::Path;

use crate::store::GraphStore;

pub(super) fn write_tree(dir: &Path) {
    std::fs::write(
        dir.join("lib.rs"),
        "pub fn parse_config(path: &str) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 }\n",
    )
    .expect("lib.rs");
}

/// Write a tree with one visible source and two under ignore rules.
///
/// `.git` must exist as a directory: the `ignore` crate applies `.gitignore`
/// only inside a repository (`require_git` is on by default), and its own test
/// suite pins that (`gitignore_no_git`). The indexer skips `.git` in either
/// mode, so an empty one is enough.
pub(super) fn write_ignored_tree(dir: &Path) {
    std::fs::create_dir(dir.join(".git")).expect(".git");
    std::fs::write(dir.join(".gitignore"), "scratch/\ngen/\n").expect(".gitignore");
    std::fs::write(dir.join("a.rs"), "pub fn tracked() {}\n").expect("a.rs");
    std::fs::create_dir(dir.join("scratch")).expect("scratch");
    std::fs::write(dir.join("scratch/b.rs"), "pub fn scratch() {}\n").expect("b.rs");
    std::fs::create_dir(dir.join("gen")).expect("gen");
    std::fs::write(dir.join("gen/c.rs"), "pub fn generated() {}\n").expect("c.rs");
}

pub(super) fn indexed_files(store: &GraphStore, project: &str) -> Vec<String> {
    let mut files: Vec<String> = store
        .all_file_hashes(project)
        .expect("hashes")
        .into_keys()
        .collect();
    files.sort();
    files
}
