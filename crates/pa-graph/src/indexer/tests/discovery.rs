//! Which files a pass discovers.
//!
//! `discover` has three inputs that can each add or drop a file: the two ignore
//! modes, ripgrep's `.ignore`, and the extension allowlist. Each test below
//! pins one of them, and each pair is checked against the other so flipping one
//! cannot smuggle a file through the other.

use super::super::options::IndexOptions;
use crate::store::GraphStore;

use super::fixtures::*;

/// The ignore mode decides the discovered set, and both modes are worth
/// reaching: `false` (the default) indexes what is on disk, `true` hides the
/// ignored files. A lean run over a wide store drops what it can no longer
/// see and reports it as `removed` rather than leaving orphans behind.
#[test]
fn the_ignore_mode_decides_the_discovered_file_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_ignored_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");

    let wide = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("wide index");
    assert_eq!(
        wide.added, 3,
        "the default mode walks what is on disk: {wide:?}"
    );
    assert_eq!(
        indexed_files(&store, "p"),
        vec!["a.rs", "gen/c.rs", "scratch/b.rs"],
    );

    let lean = store
        .index_project(
            dir.path(),
            &IndexOptions::new("p").with_respect_gitignore(true),
        )
        .expect("lean index");
    assert_eq!(lean.parsed, 0, "a.rs itself did not change: {lean:?}");
    assert_eq!(
        lean.removed, 2,
        "the ignored files leave the store: {lean:?}"
    );
    assert_eq!(indexed_files(&store, "p"), vec!["a.rs"]);
    // A lean pass must not leave the dropped files' symbols searchable.
    let hits = crate::search::search(
        store.connection(),
        "generated",
        &crate::search::SearchOptions::new("p"),
    )
    .expect("search");
    assert!(
        hits.is_empty(),
        "pruned symbols stayed searchable: {hits:?}"
    );
}

/// The extension allowlist is orthogonal to the ignore mode: a file the
/// options exclude stays out in both, so flipping the mode cannot smuggle
/// something past the filter.
/// `.ignore` is ripgrep's own ignore format, not git's, and the mode only
/// speaks about git's rules. It stays honoured in both: a caller flipping
/// `respect_gitignore` must not also start indexing a directory the project
/// marked with `.ignore`.
#[test]
fn the_ripgrep_ignore_file_is_honoured_in_both_modes() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(".ignore"), "vendor/\n").expect(".ignore");
    std::fs::write(dir.path().join("a.rs"), "pub fn tracked() {}\n").expect("a.rs");
    std::fs::create_dir(dir.path().join("vendor")).expect("vendor");
    std::fs::write(dir.path().join("vendor/v.rs"), "pub fn vendored() {}\n").expect("v.rs");
    for respect in [false, true] {
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(
                dir.path(),
                &IndexOptions::new("p").with_respect_gitignore(respect),
            )
            .expect("index");
        assert_eq!(
            indexed_files(&store, "p"),
            vec!["a.rs"],
            "respect_gitignore={respect} dropped the `.ignore` rule",
        );
    }
}

/// A walk that FAILS is not a walk that found nothing.
///
/// The removal sweep deletes every stored file the pass did not see, so a
/// directory the walker could not traverse used to become silent data loss: the
/// pass reported success and the store lost the files it could not reach. The
/// error belongs to the pass, which must abort before it removes anything.
#[test]
fn a_failed_walk_aborts_the_pass_instead_of_emptying_the_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first index");
    assert_eq!(indexed_files(&store, "p"), vec!["lib.rs"]);

    // The root is gone, so the walker reports an error for it and every stored
    // path is "not seen". Both write paths must refuse to act on that.
    let root = dir.path().to_path_buf();
    std::fs::remove_dir_all(&root).expect("remove the tree");
    let full = store.index_project(&root, &IndexOptions::new("p"));
    assert!(full.is_err(), "a failed walk must abort the pass: {full:?}");
    let incremental = store.reindex_incremental(&root, &IndexOptions::new("p"));
    assert!(
        incremental.is_err(),
        "a failed walk must abort the incremental pass: {incremental:?}"
    );

    assert_eq!(
        indexed_files(&store, "p"),
        vec!["lib.rs"],
        "the sweep removed files a failed walk simply did not see"
    );
    assert!(
        store
            .node_by_qn("p", "lib::parse_config")
            .expect("qn")
            .is_some(),
        "the symbols are gone with the file rows"
    );
}

#[test]
fn the_extension_filter_applies_in_both_ignore_modes() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_ignored_tree(dir.path());
    std::fs::write(dir.path().join("visible.py"), "def tracked():\n    pass\n").expect("py");
    std::fs::write(dir.path().join("gen/c.py"), "def generated():\n    pass\n").expect("py");
    let rust_only = || IndexOptions::new("p").with_extensions(vec!["rs".to_string()]);
    for respect in [false, true] {
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &rust_only().with_respect_gitignore(respect))
            .expect("index");
        let files = indexed_files(&store, "p");
        assert!(
            !files
                .iter()
                .any(|file| std::path::Path::new(file).extension() == Some("py".as_ref())),
            "respect_gitignore={respect} let a filtered extension through: {files:?}"
        );
    }
}
