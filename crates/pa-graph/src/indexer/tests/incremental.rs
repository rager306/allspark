//! A second pass: what it reuses, what it redoes, what it removes.
//!
//! The indexer is incremental by file hash. These tests pin the four
//! consequences: an unchanged file costs nothing, a modified one is re-parsed
//! without losing incoming edges, a removed one leaves no orphan rows, and a
//! forced pass re-parses everything.

use super::super::options::IndexOptions;
use crate::store::GraphStore;
use crate::types::EdgeType;

use super::super::batch::{TEST_BATCHES, TEST_SOURCE_BATCH_BYTES};
use super::fixtures::*;

#[test]
fn second_index_is_incremental() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    let first = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    assert_eq!(first.added, 1);
    let second = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    assert_eq!(second.unchanged, 1);
    assert_eq!(second.parsed, 0);
    assert_eq!(second.added, 0);
}

#[test]
fn modification_is_detected_and_reindexed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    std::fs::write(dir.path().join("lib.rs"), "pub fn only_one() {}\n").expect("rewrite");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    assert_eq!(report.modified, 1);
    // The old symbols are gone; the new one is present.
    assert!(
        store
            .node_by_qn("p", "lib::only_one")
            .expect("qn")
            .is_some()
    );
    assert!(
        store
            .node_by_qn("p", "lib::parse_config")
            .expect("qn")
            .is_none()
    );
}

#[test]
fn reindex_removes_stale_fts_rows() {
    // Regression: a contentless FTS table has no cascade, so a re-parsed
    // file must delete its old rows or a renamed symbol stays searchable.
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    std::fs::write(dir.path().join("lib.rs"), "pub fn renamed_only() {}\n").expect("rewrite");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    let stale = crate::search::search(
        store.connection(),
        "parse_config",
        &crate::search::SearchOptions::new("p"),
    )
    .expect("search");
    assert!(
        stale.is_empty(),
        "stale FTS rows survived reindex: {stale:?}"
    );
    let fresh = crate::search::search(
        store.connection(),
        "renamed_only",
        &crate::search::SearchOptions::new("p"),
    )
    .expect("search");
    assert!(fresh.iter().any(|h| h.name == "renamed_only"), "{fresh:?}");
}

/// Batching bounds MEMORY and must not change a single fact. A batching bug shows up
/// as a file silently missing from the graph — the class `coverage` exists to make
/// visible, and the one a green index run would not report.
#[test]
fn many_small_batches_produce_the_same_graph_as_one_large_batch() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    for i in 0..12 {
        std::fs::write(
            dir.path().join(format!("m{i}.rs")),
            format!("pub fn m{i}_one() {{}}\npub fn m{i}_two() {{ m{i}_one() }}\n"),
        )
        .expect("module");
    }

    let mut single = GraphStore::open_in_memory().expect("store");
    single
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("one batch");
    let expected = (
        single.node_count("p").expect("nodes"),
        single.edge_count("p").expect("edges"),
        single.all_file_hashes("p").expect("hashes"),
    );

    let mut many = GraphStore::open_in_memory().expect("store");
    TEST_SOURCE_BATCH_BYTES.with(|slot| slot.set(Some(60)));
    TEST_BATCHES.store(0, std::sync::atomic::Ordering::Relaxed);
    let result = many.index_project(dir.path(), &IndexOptions::new("p"));
    TEST_SOURCE_BATCH_BYTES.with(|slot| slot.set(None));
    let result = result.expect("many batches");
    assert!(
        TEST_BATCHES.load(std::sync::atomic::Ordering::Relaxed) > 1,
        "the fixture must force several batches, or it proves nothing: {result:?}"
    );
    assert_eq!(
        (
            many.node_count("p").expect("nodes"),
            many.edge_count("p").expect("edges"),
            many.all_file_hashes("p").expect("hashes"),
        ),
        expected,
        "the graph must be identical however the files were grouped"
    );

    // A removal is still detected once, not once per batch.
    std::fs::remove_file(dir.path().join("m3.rs")).expect("remove");
    let report = many
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("removal");
    assert_eq!(report.removed, 1, "removal is a pass property: {report:?}");
}

#[test]
fn removal_is_detected() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    std::fs::remove_file(dir.path().join("lib.rs")).expect("remove");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    assert_eq!(report.removed, 1);
}

#[test]
fn force_reparses_everything() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p").with_force(true))
        .expect("forced");
    assert_eq!(report.parsed, 1);
    assert_eq!(report.unchanged, 0);
}

#[test]
fn broken_file_is_indexed_and_flagged_partial() {
    // A file with an ERROR region still yields the symbols it does contain,
    // but the run must record honest coverage for it.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("broken.rs"),
        "pub fn good() {}\nfn bad( {\n    let x = );\n",
    )
    .expect("write");
    let mut store = GraphStore::open_in_memory().expect("store");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    assert_eq!(report.parsed, 1);
    assert!(
        report.partial + report.unusable >= 1,
        "broken file must be flagged: {report:?}"
    );
    let cov = store.coverage("p").expect("coverage");
    assert!(!cov.complete);
    assert!(cov.partial + cov.unusable >= 1, "{cov:?}");
}

#[test]
fn coverage_note_clears_when_file_is_fixed() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("m.rs"), "fn bad( {\n").expect("write");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    assert!(!store.coverage("p").expect("cov").complete);
    std::fs::write(dir.path().join("m.rs"), "fn good() {}\n").expect("fix");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    let cov = store.coverage("p").expect("cov");
    assert!(cov.complete, "fixed file must read clean: {cov:?}");
    assert_eq!(cov.partial, 0);
    assert_eq!(cov.unusable, 0);
}

#[test]
fn report_edges_matches_the_store_after_indexing() {
    // R07: `report.edges` used to count insert ATTEMPTS, but the store
    // dedupes (`INSERT OR IGNORE` on a UNIQUE key), so the report
    // overstated the graph by a quarter on a real project. The
    // user-visible number must be the store's truth.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn caller() -> u32 { helper() + helper() }\npub fn helper() -> u32 { 1 }\n",
    )
    .expect("lib.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    // Two call sites of the same callee collapse into one stored edge;
    // the report must agree with what the store actually holds.
    assert_eq!(
        report.edges,
        store.edge_count("p").expect("count"),
        "report counts attempts, store dedupes"
    );
}

#[test]
fn modified_file_keeps_incoming_call_edges() {
    // R08: re-parsing a modified file used to delete its nodes, and the
    // schema's ON DELETE CASCADE silently dropped every CALLS edge from
    // unchanged files into it. Each edit permanently thinned the graph.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn foo() -> u32 { 1 }\n").expect("a.rs");
    std::fs::write(
        dir.path().join("b.rs"),
        "pub fn caller() -> u32 { foo() }\n",
    )
    .expect("b.rs");
    std::fs::write(dir.path().join("c.rs"), "pub fn other() -> u32 { foo() }\n").expect("c.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    let before = store.edge_count("p").expect("count");

    // Body-only edit of a.rs; b.rs and c.rs are untouched.
    std::fs::write(dir.path().join("a.rs"), "pub fn foo() -> u32 { 1 + 1 }\n").expect("edit");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");

    assert_eq!(
        store.edge_count("p").expect("count"),
        before,
        "a body edit must not drop incoming CALLS edges"
    );
    let foo = store
        .node_by_qn("p", "a::foo")
        .expect("qn")
        .expect("foo survived");
    let calls = store
        .in_edges(foo.id, Some(EdgeType::Calls))
        .expect("in edges");
    assert_eq!(calls.len(), 2, "b::caller and c::other still call a::foo");
}
