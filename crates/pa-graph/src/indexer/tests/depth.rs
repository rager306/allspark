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
