//! The three depths.
//!
//! A skeleton pass writes file nodes and parses nothing; a symbols pass writes
//! definitions and resolves nothing. Both are cheap answers to structural
//! questions, and both must stay distinguishable from a full pass.

use super::super::options::{Depth, IndexOptions};
use crate::store::GraphStore;

use super::fixtures::*;

/// A skeleton pass builds file nodes only: measured 1,105 ms for 2,743 files
/// against ~135,000 ms for a full pass.
#[test]
fn a_skeleton_pass_records_files_without_parsing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    let report = store
        .index_project(
            dir.path(),
            &IndexOptions::new("p").with_depth(Depth::Skeleton),
        )
        .expect("skeleton");
    assert_eq!(report.nodes, 1, "one file node, no symbols: {report:?}");
    assert_eq!(report.edges, 0, "a skeleton resolves no edges: {report:?}");
    let files: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM nodes WHERE project = 'p' AND label = 'File'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(files, 1);
}

/// A symbols pass records definitions but no edges, so the depth is visible
/// in what the store holds.
#[test]
fn a_symbols_pass_records_definitions_but_no_edges() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    let report = store
        .index_project(
            dir.path(),
            &IndexOptions::new("p").with_depth(Depth::Symbols),
        )
        .expect("symbols");
    assert!(report.nodes >= 3, "file plus two functions: {report:?}");
    // CONTAINS edges are structural and always written; what a symbols pass
    // skips is *resolution* — no CALLS, no IMPORTS.
    let resolved: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM edges WHERE project = 'p'
             AND edge_type IN ('CALLS', 'IMPORTS')",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(resolved, 0, "a symbols pass resolves no calls: {report:?}");
}

/// A pass only skips a file whose stored revision is at least as deep as the
/// request. Classification was content-hash only, and a skeleton pass stores the
/// file hash, so "index at skeleton, then at full" skipped every file it had just
/// seen: the deeper pass reported success and the graph stayed symbolic.
#[test]
fn a_deeper_pass_does_not_skip_a_file_written_by_a_shallow_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");

    let skeleton = store
        .index_project(
            dir.path(),
            &IndexOptions::new("p").with_depth(Depth::Skeleton),
        )
        .expect("skeleton");
    assert_eq!(skeleton.nodes, 1, "file node only: {skeleton:?}");

    let full = store
        .index_project(dir.path(), &IndexOptions::new("p").with_depth(Depth::Full))
        .expect("full");
    assert_eq!(
        full.unchanged, 0,
        "a file the skeleton pass wrote is not unchanged for a full pass: {full:?}"
    );
    assert!(
        store
            .node_by_qn("p", "lib::parse_config")
            .expect("qn")
            .is_some(),
        "the deeper pass skipped the file and left the graph symbolic"
    );
    let calls: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM edges WHERE project = 'p' AND edge_type = 'CALLS'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert!(calls >= 1, "a full pass resolves the call the file spells");

    // The same depth is a no-op: the row now records what the pass built.
    let again = store
        .index_project(dir.path(), &IndexOptions::new("p").with_depth(Depth::Full))
        .expect("second full");
    assert_eq!(again.unchanged, 1, "{again:?}");
    assert_eq!(again.parsed, 0, "{again:?}");
}

/// A symbols pass stores the file hash too, so the full pass after it must
/// re-resolve rather than skip: the two depths differ in exactly the resolved
/// edges this pins.
#[test]
fn a_full_pass_after_a_symbols_pass_resolves_the_edges_it_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(
            dir.path(),
            &IndexOptions::new("p").with_depth(Depth::Symbols),
        )
        .expect("symbols");
    let resolved: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM edges WHERE project = 'p'
             AND edge_type IN ('CALLS', 'IMPORTS', 'USAGE')",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(resolved, 0, "a symbols pass resolves nothing");

    store
        .index_project(dir.path(), &IndexOptions::new("p").with_depth(Depth::Full))
        .expect("full");
    let resolved: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM edges WHERE project = 'p'
             AND edge_type IN ('CALLS', 'IMPORTS', 'USAGE')",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert!(resolved >= 1, "the full pass must resolve the call sites");
}

/// The default depth is unchanged, so an existing caller gets the old
/// behaviour.
#[test]
fn the_default_depth_is_full() {
    assert_eq!(Depth::default(), Depth::Full);
    assert!(Depth::Full.resolves_edges());
    assert!(!Depth::Symbols.resolves_edges());
    assert!(!Depth::Skeleton.resolves_edges());
    assert_eq!(Depth::from_str_or_default("skeleton"), Depth::Skeleton);
    assert_eq!(Depth::from_str_or_default("nonsense"), Depth::Full);
}
